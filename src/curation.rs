use crate::physics::{self, CutContext, Purpose, ThermalBody};
use crate::pipeline::coolant_type_of;
use crate::profile::{CoolantConfig, EndMillMockupSetting, MachineLimits, MachiningProfile};
use crate::store::attr::{endmill_diff, workpiece_diff, AttrDiff, EndMillAttr, WorkpieceAttr};
use crate::store::rdb::{EndMillRow, PairStats};
use crate::store::series::WindowHit;
use crate::store::Store;
use crate::workpiece_setup::WorkpieceSetup;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicsCard {
    pub rpm: u32,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub tool_life_min: f64,
    pub power_util: f64,
    pub wall_util: f64,
    pub interface_c: f64,
    pub coating_limit_c: f64,
    pub chatter_margin: f64,
    pub ra_um: f64,
    pub mrr_cm3_min: f64,
    pub feasible: bool,
    pub score: f64,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub chatter_measured: bool,
    #[serde(default)]
    pub regime: Vec<f32>,
    #[serde(default)]
    pub dominant_wear: String,
    #[serde(default)]
    pub dominant_share: f64,
    #[serde(default)]
    pub mu_eff: f64,
    #[serde(default)]
    pub compat_severity: u8,
    #[serde(default)]
    pub compat: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: String,
    pub weight: f64,
    pub score: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub endmill_id: i64,
    pub label: String,
    pub attr_key: String,
    pub aliases: Vec<String>,
    pub score: f64,
    pub confidence: f64,
    pub is_current: bool,
    pub evidence: Vec<Evidence>,
    pub physics: PhysicsCard,
    pub history: Option<PairStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeighborCard {
    pub id: i64,
    pub label: String,
    pub similarity: f64,
    pub runs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveNeighbor {
    pub series_id: String,
    pub endmill_label: String,
    pub workpiece_label: String,
    pub similarity: f64,
    pub duration_min: f64,
    pub final_ratio: f64,
    pub rate_per_hour: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurationReport {
    pub workpiece_id: i64,
    pub workpiece_label: String,
    pub purpose: String,
    pub similar_workpieces: Vec<NeighborCard>,
    pub candidates: Vec<Candidate>,
    pub curves: Vec<CurveNeighbor>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairHistory {
    pub workpiece_id: i64,
    pub workpiece_label: String,
    pub stats: PairStats,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndMillComparison {
    pub a: Candidate,
    pub b: Candidate,
    pub attribute_similarity: f64,
    pub diffs: Vec<AttrDiff>,
    pub history_a: Vec<PairHistory>,
    pub history_b: Vec<PairHistory>,
    pub head_to_head: Vec<String>,
    pub verdict: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkpieceSuggestion {
    pub workpiece_id: i64,
    pub workpiece_label: String,
    pub via_endmill_label: String,
    pub similarity: f64,
    pub relay_score: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndMillRelay {
    pub endmill_id: i64,
    pub label: String,
    pub aliases: Vec<String>,
    pub workpieces: Vec<PairHistory>,
    pub similar_endmills: Vec<NeighborCard>,
    pub suggestions: Vec<WorkpieceSuggestion>,
    pub notes: Vec<String>,
}

fn alias_names(e: &EndMillRow) -> Vec<String> {
    e.aliases
        .iter()
        .map(|a| if a.model.is_empty() { a.name.clone() } else { format!("{} ({})", a.name, a.model) })
        .collect()
}

pub fn pair_score(s: &PairStats) -> Option<(f64, f64, String)> {
    let mut parts: Vec<f64> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let mut n = 0.0;
    if let (Some(r), true) = (s.wear_ratio, s.wear_ratio_n > 0) {
        parts.push(((1.0 / r.max(0.2)).sqrt().clamp(0.0, 1.5)) / 1.5);
        text.push(format!("실측/예측 마모비 {:.2} ({}건)", r, s.wear_ratio_n));
        n += s.wear_ratio_n as f64;
    }
    if let Some(l) = s.life_meas.filter(|_| s.life_meas_n > 0) {
        let q = match s.life_pred.filter(|p| *p > 0.0) {
            Some(p) => l / p,
            None => 1.0,
        };
        parts.push((q.max(0.0).sqrt().clamp(0.0, 1.5)) / 1.5);
        text.push(format!("실측 수명 {:.0}분 ({}회)", l, s.life_meas_n));
        n += s.life_meas_n as f64;
    }
    let decisions = s.replace_count + s.inspect_count;
    if decisions > 0 {
        let rate = s.replace_count as f64 / (s.runs.max(1) as f64);
        parts.push(1.0 - (2.0 * rate).min(1.0));
        text.push(format!("교체 판단 {}회 / 점검 {}회", s.replace_count, s.inspect_count));
        n += 0.5 * decisions as f64;
    }
    if parts.is_empty() {
        return None;
    }
    Some((parts.iter().sum::<f64>() / parts.len() as f64, n, text.join(", ")))
}

fn weights(purpose: Purpose) -> (f64, f64, f64, f64) {
    match purpose {
        Purpose::Finishing => (0.25, 0.45, 0.10, 0.20),
        Purpose::Roughing => (0.35, 0.15, 0.35, 0.15),
    }
}

pub fn physics_card(setting: &EndMillMockupSetting, setup: &WorkpieceSetup, machine: &MachineLimits, coolant: &CoolantConfig, purpose: Purpose) -> PhysicsCard {
    let mut profile = MachiningProfile::from_preset(&crate::profile::MachiningPreset::default_steel_general(), "curation");
    profile.endmill_setting = setting.clone();
    profile.workpiece_setup = setup.clone();
    profile.coolant_config = coolant.clone();
    profile.machine = machine.clone();
    let ctx = CutContext::from_profile(&profile);
    let body = ThermalBody::of_setup(setup, &ctx.wp);
    let rec = physics::recommend(&ctx, setting.is_high_end, body, purpose);
    let mut conds = rec.conditions.clone();
    conds.coolant = coolant_type_of(&coolant.method);
    let a = physics::analyze_cut(&ctx, &conds, true, body);
    let power_util = a.spindle_power_kw / a.available_power_kw.max(1e-9);
    let margin = if a.stability.margin.is_finite() { a.stability.margin } else { 10.0 };
    let mut reasons = Vec::new();
    if power_util > 1.0 {
        reasons.push(format!("동력 {:.0}% 초과", power_util * 100.0));
    }
    if a.thermal.interface_c > ctx.tool.coating.max_temp_c {
        reasons.push(format!("날끝 {:.0}°C > 코팅 한계 {:.0}°C", a.thermal.interface_c, ctx.tool.coating.max_temp_c));
    }
    if margin < 0.8 && a.stability.measured {
        reasons.push(format!("채터 여유 {:.2}배 (실측 FRF)", margin));
    }
    let compat = &a.tribology.compat;
    if compat.severity >= 2 {
        for m in compat.messages.iter() {
            reasons.push(m.clone());
        }
    }
    let feasible = reasons.is_empty();
    let dominant = a.wear.mechanisms.iter().find(|m| m.key == a.wear.dominant);
    let life = a.wear.tool_life_min;
    let s_life = 1.0 - (-(life / ctx.wp.ref_life_min.max(1.0))).exp();
    let s_acc = (1.0 - a.error_budget_um.utilization / 1.5).clamp(0.0, 1.0);
    let s_ch = (margin / 1.5).min(1.0);
    let (wl, wa, _, wc) = weights(purpose);
    let base = wl * s_life + wa * s_acc + wc * s_ch;
    PhysicsCard {
        rpm: conds.spindle_rpm,
        vc_m_min: conds.cutting_speed_m_min,
        fz_mm: conds.feed_per_tooth_mm,
        ap_mm: conds.axial_doc_mm,
        ae_mm: conds.radial_doc_mm,
        tool_life_min: life,
        power_util,
        wall_util: a.error_budget_um.utilization,
        interface_c: a.thermal.interface_c,
        coating_limit_c: ctx.tool.coating.max_temp_c,
        chatter_margin: margin,
        ra_um: a.surface.wall_ra_um,
        mrr_cm3_min: a.mrr_cm3_min,
        feasible,
        score: if feasible { base } else { 0.3 * base },
        reasons,
        chatter_measured: a.stability.measured,
        regime: a.regime.clone(),
        dominant_wear: dominant.map(|m| m.label.clone()).unwrap_or_default(),
        dominant_share: dominant.map(|m| m.share).unwrap_or(0.0),
        mu_eff: a.tribology.mu_eff,
        compat_severity: compat.severity,
        compat: compat.messages.clone(),
    }
}

fn finish_scores(cands: &mut [Candidate], purpose: Purpose) {
    let mut mrr: Vec<f64> = cands.iter().map(|c| c.physics.mrr_cm3_min).filter(|v| *v > 0.0).collect();
    mrr.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = if mrr.is_empty() { 1.0 } else { mrr[mrr.len() / 2].max(1e-6) };
    let (_, _, wm, _) = weights(purpose);
    for c in cands.iter_mut() {
        let s_mrr = c.physics.mrr_cm3_min / (c.physics.mrr_cm3_min + mid);
        let phys = c.physics.score + if c.physics.feasible { wm * s_mrr } else { 0.3 * wm * s_mrr };
        c.physics.score = phys;
        let mut num = phys;
        let mut den = 1.0;
        for e in c.evidence.iter().filter(|e| e.kind != "physics") {
            num += e.weight * e.score;
            den += e.weight;
        }
        c.score = num / den;
        c.confidence = (den - 1.0) / den;
        c.evidence.insert(
            0,
            Evidence {
                kind: "physics".into(),
                weight: 1.0,
                score: phys,
                text: format!(
                    "물리 사전: 권장 S{} fz {:.3} ap {:.2} ae {:.2} → 수명 {:.0}분, 오차 사용률 {:.0}%, 채터 여유 {:.2}배{}, 계면 μ {:.2} · 지배 마모 {} {:.0}%{}",
                    c.physics.rpm,
                    c.physics.fz_mm,
                    c.physics.ap_mm,
                    c.physics.ae_mm,
                    c.physics.tool_life_min,
                    c.physics.wall_util * 100.0,
                    c.physics.chatter_margin,
                    if c.physics.chatter_measured { "(실측 FRF)" } else { "(모델 추정)" },
                    c.physics.mu_eff,
                    c.physics.dominant_wear,
                    c.physics.dominant_share * 100.0,
                    if c.physics.feasible { String::new() } else { format!(" (제약: {})", c.physics.reasons.join(", ")) }
                ),
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn curate_for_workpiece(
    store: &mut Store,
    setup: &WorkpieceSetup,
    current: Option<&EndMillMockupSetting>,
    machine: &MachineLimits,
    coolant: &CoolantConfig,
    purpose: Purpose,
    project_scope: Option<i64>,
    curve: Option<(&str, &[f32], Option<&str>)>,
    limit: usize,
) -> Result<CurationReport, String> {
    let wp_id = store.index_workpiece(setup)?;
    let wp_label = WorkpieceAttr::from_setup(setup).label(setup);
    let mut notes = Vec::new();
    let neighbors: Vec<_> = store
        .similar_workpieces(setup, 6, Some(wp_id))?
        .into_iter()
        .filter(|n| n.similarity >= 0.2)
        .collect();
    let mut similar_workpieces = Vec::new();
    let mut relay_stats: Vec<(f64, Vec<PairStats>)> = Vec::new();
    for n in neighbors.iter() {
        if let Some(w) = store.rdb.workpiece(n.id)? {
            let stats = store.rdb.pair_stats(None, Some(n.id), project_scope)?;
            similar_workpieces.push(NeighborCard {
                id: n.id,
                label: w.label.clone(),
                similarity: n.similarity,
                runs: w.runs,
            });
            relay_stats.push((n.similarity, stats));
        }
    }
    let direct: BTreeMap<i64, PairStats> = store
        .rdb
        .pair_stats(None, Some(wp_id), project_scope)?
        .into_iter()
        .map(|s| (s.endmill_id, s))
        .collect();
    let mut ids: BTreeSet<i64> = direct.keys().cloned().collect();
    for (_, stats) in relay_stats.iter() {
        for s in stats.iter().take(5) {
            ids.insert(s.endmill_id);
        }
    }
    let current_id = match current {
        Some(s) => {
            let id = store.index_endmill(s)?;
            ids.insert(id);
            for nb in store.similar_endmills(&EndMillAttr::from_setting(s), 5, Some(id), None)? {
                ids.insert(nb.id);
            }
            Some(id)
        }
        None => None,
    };
    if ids.len() < 3 {
        for e in store.rdb.list_endmills(10)? {
            ids.insert(e.id);
        }
        notes.push("이력이 적어 라이브러리의 다른 앤드밀도 물리 사전으로 함께 평가했습니다".into());
    }
    let mut cands: Vec<Candidate> = Vec::new();
    for id in ids.into_iter().take(limit.max(3)) {
        let Some(row) = store.rdb.endmill(id)? else {
            continue;
        };
        let setting = row.setting();
        let physics = physics_card(&setting, setup, machine, coolant, purpose);
        let mut evidence = Vec::new();
        let history = direct.get(&id).cloned();
        if let Some(h) = &history {
            match pair_score(h) {
                Some((s, n, text)) => evidence.push(Evidence {
                    kind: "direct".into(),
                    weight: n / (n + 2.0),
                    score: s,
                    text: format!("이 공작물 직접 이력 {}회: {}", h.runs, text),
                }),
                None => evidence.push(Evidence {
                    kind: "direct".into(),
                    weight: 0.0,
                    score: 0.0,
                    text: format!("이 공작물에서 {}회 사용 (실측 결과 없음)", h.runs),
                }),
            }
        }
        let mut num = 0.0;
        let mut den = 0.0;
        let mut nsum = 0.0;
        let mut best_text = String::new();
        for (sim, stats) in relay_stats.iter() {
            if let Some(s) = stats.iter().find(|s| s.endmill_id == id) {
                if let Some((sc, n, text)) = pair_score(s) {
                    num += sim * n * sc;
                    den += sim * n;
                    nsum += sim * n;
                    if best_text.is_empty() {
                        best_text = format!("유사도 {:.2} 공작물: {}", sim, text);
                    }
                }
            }
        }
        if den > 0.0 {
            evidence.push(Evidence {
                kind: "relay_workpiece".into(),
                weight: 0.6 * nsum / (nsum + 3.0),
                score: num / den,
                text: format!("유사 공작물 relay — {}", best_text),
            });
        }
        let attr = row.attr.clone();
        let mut num = 0.0;
        let mut den = 0.0;
        let mut first = String::new();
        for nb in store.similar_endmills(&attr, 5, Some(id), None)? {
            if let Some(s) = direct.get(&nb.id) {
                if let Some((sc, n, text)) = pair_score(s) {
                    num += nb.similarity * n * sc;
                    den += nb.similarity * n;
                    if first.is_empty() {
                        let lbl = store.rdb.endmill(nb.id)?.map(|e| e.label).unwrap_or_default();
                        first = format!("속성 유사도 {:.2} 앤드밀 {}: {}", nb.similarity, lbl, text);
                    }
                }
            }
        }
        if den > 0.0 {
            evidence.push(Evidence {
                kind: "relay_endmill".into(),
                weight: 0.4 * den / (den + 3.0),
                score: num / den,
                text: format!("유사 앤드밀 relay — {}", first),
            });
        }
        if let Some(e) = regime_evidence(store, &physics.regime, id, wp_id, project_scope)? {
            evidence.push(e);
        }
        cands.push(Candidate {
            endmill_id: id,
            label: row.label.clone(),
            attr_key: row.attr_key.clone(),
            aliases: alias_names(&row),
            score: 0.0,
            confidence: 0.0,
            is_current: Some(id) == current_id,
            evidence,
            physics,
            history,
        });
    }
    finish_scores(&mut cands, purpose);
    cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut curves = Vec::new();
    if let (Some((metric, vector, exclude)), Some(ts)) = (curve, store.ts.as_ref()) {
        let hits: Vec<WindowHit> = ts.similar_windows(metric, vector, 5, exclude)?;
        for h in hits {
            let em = store.rdb.endmill(h.row.endmill_id)?.map(|e| e.label).unwrap_or_else(|| format!("#{}", h.row.endmill_id));
            let wp = store.rdb.workpiece(h.row.workpiece_id)?.map(|w| w.label).unwrap_or_else(|| format!("#{}", h.row.workpiece_id));
            curves.push(CurveNeighbor {
                series_id: h.row.series_id.clone(),
                endmill_label: em,
                workpiece_label: wp,
                similarity: h.similarity,
                duration_min: h.row.duration_min,
                final_ratio: h.row.final_ratio,
                rate_per_hour: h.row.rate_per_hour,
            });
        }
    }
    if cands.iter().any(|c| !c.physics.chatter_measured && c.physics.chatter_margin < 0.8) {
        notes.push("탭 테스트 FRF 가 없는 앤드밀의 채터 여유는 공구·홀더 모델 추정이라 제약이 아닌 점수에만 반영했습니다".into());
    }
    if cands.iter().all(|c| c.confidence < 0.05) {
        notes.push("실측 이력이 없어 순위는 물리 사전(권장 조건 해석)만으로 정해졌습니다. 측정 로그·공구 이미지가 쌓이면 relay 근거가 더해집니다".into());
    }
    Ok(CurationReport {
        workpiece_id: wp_id,
        workpiece_label: wp_label,
        purpose: crate::pipeline::purpose_key(purpose).into(),
        similar_workpieces,
        candidates: cands,
        curves,
        notes,
    })
}

pub fn regime_evidence(store: &mut Store, regime: &[f32], endmill_id: i64, workpiece_id: i64, project_scope: Option<i64>) -> Result<Option<Evidence>, String> {
    if regime.is_empty() {
        return Ok(None);
    }
    let mut filter = format!("NOT (endmill_id = {} AND workpiece_id = {})", endmill_id, workpiece_id);
    if let Some(p) = project_scope {
        filter.push_str(&format!(" AND project_id = {}", p));
    }
    let hits = store.similar_regimes(regime, 8, Some(&filter))?;
    let mut seen: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut num = 0.0;
    let mut den = 0.0;
    let mut best = String::new();
    for h in hits.into_iter().filter(|h| h.similarity >= 0.25) {
        if !seen.insert((h.endmill_id, h.workpiece_id)) {
            continue;
        }
        let Some(stats) = store.rdb.pair_stats(Some(h.endmill_id), Some(h.workpiece_id), project_scope)?.into_iter().next() else {
            continue;
        };
        let Some((sc, n, text)) = pair_score(&stats) else {
            continue;
        };
        num += h.similarity * n * sc;
        den += h.similarity * n;
        if best.is_empty() {
            let em = store.rdb.endmill(h.endmill_id)?.map(|e| e.label).unwrap_or_else(|| format!("#{}", h.endmill_id));
            let wp = store.rdb.workpiece(h.workpiece_id)?.map(|w| w.label).unwrap_or_else(|| format!("#{}", h.workpiece_id));
            best = format!(
                "레짐 유사도 {:.2} ({} × {} · {} · 지배 {}) {} / {}: {}",
                h.similarity, h.coating, h.chem, h.coolant, h.dominant, em, wp, text
            );
        }
    }
    if den <= 0.0 {
        return Ok(None);
    }
    Ok(Some(Evidence {
        kind: "relay_regime".into(),
        weight: 0.5 * den / (den + 3.0),
        score: num / den,
        text: format!("마찰·열·마모 상호작용 레짐 relay — {}", best),
    }))
}

fn histories(store: &Store, endmill_id: i64, project_scope: Option<i64>) -> Result<Vec<PairHistory>, String> {
    let mut out = Vec::new();
    for s in store.rdb.pair_stats(Some(endmill_id), None, project_scope)? {
        let label = store.rdb.workpiece(s.workpiece_id)?.map(|w| w.label).unwrap_or_default();
        out.push(PairHistory {
            workpiece_id: s.workpiece_id,
            workpiece_label: label,
            score: pair_score(&s).map(|p| p.0),
            stats: s,
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn compare_endmills(
    store: &mut Store,
    a_id: i64,
    b_id: i64,
    setup: &WorkpieceSetup,
    machine: &MachineLimits,
    coolant: &CoolantConfig,
    purpose: Purpose,
    project_scope: Option<i64>,
) -> Result<EndMillComparison, String> {
    let ra = store.rdb.endmill(a_id)?.ok_or_else(|| format!("앤드밀 #{} 없음", a_id))?;
    let rb = store.rdb.endmill(b_id)?.ok_or_else(|| format!("앤드밀 #{} 없음", b_id))?;
    let wp_id = store.index_workpiece(setup)?;
    let mk = |row: &EndMillRow, store: &Store| -> Result<Candidate, String> {
        let physics = physics_card(&row.setting(), setup, machine, coolant, purpose);
        let history = store
            .rdb
            .pair_stats(Some(row.id), Some(wp_id), project_scope)?
            .into_iter()
            .next();
        let mut evidence = Vec::new();
        if let Some((s, n, text)) = history.as_ref().and_then(pair_score) {
            evidence.push(Evidence {
                kind: "direct".into(),
                weight: n / (n + 2.0),
                score: s,
                text,
            });
        }
        Ok(Candidate {
            endmill_id: row.id,
            label: row.label.clone(),
            attr_key: row.attr_key.clone(),
            aliases: alias_names(row),
            score: 0.0,
            confidence: 0.0,
            is_current: false,
            evidence,
            physics,
            history,
        })
    };
    let mut pair = vec![mk(&ra, store)?, mk(&rb, store)?];
    finish_scores(&mut pair, purpose);
    let b = pair.pop().unwrap();
    let a = pair.pop().unwrap();
    let diffs = endmill_diff(&ra.attr, &rb.attr);
    let sim = crate::store::attr::similarity_from_l2(crate::store::attr::l2_sq(&ra.attr.vector(), &rb.attr.vector()));
    let history_a = histories(store, a_id, project_scope)?;
    let history_b = histories(store, b_id, project_scope)?;
    let mut head = Vec::new();
    for ha in history_a.iter() {
        if let Some(hb) = history_b.iter().find(|h| h.workpiece_id == ha.workpiece_id) {
            head.push(format!(
                "{}: A {} / B {} (실행 {} vs {})",
                ha.workpiece_label,
                ha.score.map(|s| format!("{:.2}", s)).unwrap_or_else(|| "실측 없음".into()),
                hb.score.map(|s| format!("{:.2}", s)).unwrap_or_else(|| "실측 없음".into()),
                ha.stats.runs,
                hb.stats.runs
            ));
        }
    }
    let mut verdict = Vec::new();
    let (better, worse) = if a.score >= b.score { (&a, &b) } else { (&b, &a) };
    verdict.push(format!(
        "이 공작물·목적 기준 종합 점수 {} {:.2} vs {} {:.2}",
        better.label, better.score, worse.label, worse.score
    ));
    if let Some(top) = diffs.first() {
        verdict.push(format!("가장 큰 속성 차이: {} ({} → {}, 차이의 {:.0}%)", top.attribute, top.a, top.b, top.share * 100.0));
    }
    if (a.physics.tool_life_min - b.physics.tool_life_min).abs() > 0.1 * a.physics.tool_life_min.max(b.physics.tool_life_min) {
        verdict.push(format!("예측 수명 A {:.0}분 / B {:.0}분", a.physics.tool_life_min, b.physics.tool_life_min));
    }
    if (a.physics.chatter_margin - b.physics.chatter_margin).abs() > 0.2 {
        verdict.push(format!("채터 여유 A {:.2}배 / B {:.2}배", a.physics.chatter_margin, b.physics.chatter_margin));
    }
    if a.physics.dominant_wear != b.physics.dominant_wear || (a.physics.mu_eff - b.physics.mu_eff).abs() > 0.05 {
        verdict.push(format!(
            "상호작용: A μ {:.2} · {} {:.0}% / B μ {:.2} · {} {:.0}%",
            a.physics.mu_eff,
            a.physics.dominant_wear,
            a.physics.dominant_share * 100.0,
            b.physics.mu_eff,
            b.physics.dominant_wear,
            b.physics.dominant_share * 100.0
        ));
    }
    for (tag, c) in [("A", &a), ("B", &b)] {
        if c.physics.compat_severity >= 1 {
            verdict.push(format!("{} 소재 궁합: {}", tag, c.physics.compat.join(" / ")));
        }
    }
    Ok(EndMillComparison {
        a,
        b,
        attribute_similarity: sim,
        diffs,
        history_a,
        history_b,
        head_to_head: head,
        verdict,
    })
}

pub fn relay_for_endmill(store: &mut Store, endmill_id: i64, project_scope: Option<i64>) -> Result<EndMillRelay, String> {
    let row = store.rdb.endmill(endmill_id)?.ok_or_else(|| format!("앤드밀 #{} 없음", endmill_id))?;
    let workpieces = histories(store, endmill_id, project_scope)?;
    let used: BTreeSet<i64> = workpieces.iter().map(|h| h.workpiece_id).collect();
    let mut similar_endmills = Vec::new();
    let mut suggestions: Vec<WorkpieceSuggestion> = Vec::new();
    for nb in store.similar_endmills(&row.attr, 6, Some(endmill_id), None)? {
        let Some(e) = store.rdb.endmill(nb.id)? else {
            continue;
        };
        similar_endmills.push(NeighborCard {
            id: nb.id,
            label: e.label.clone(),
            similarity: nb.similarity,
            runs: e.runs,
        });
        for h in histories(store, nb.id, project_scope)? {
            if used.contains(&h.workpiece_id) {
                continue;
            }
            if let Some(sc) = h.score {
                let relay = sc * nb.similarity;
                suggestions.push(WorkpieceSuggestion {
                    workpiece_id: h.workpiece_id,
                    workpiece_label: h.workpiece_label.clone(),
                    via_endmill_label: e.label.clone(),
                    similarity: nb.similarity,
                    relay_score: relay,
                    text: format!(
                        "속성 유사도 {:.2} 의 {} 가 이 공작물에서 점수 {:.2} — 이 앤드밀도 후보로 검토할 만합니다",
                        nb.similarity, e.label, sc
                    ),
                });
            }
        }
    }
    suggestions.sort_by(|a, b| b.relay_score.partial_cmp(&a.relay_score).unwrap_or(std::cmp::Ordering::Equal));
    suggestions.dedup_by_key(|s| s.workpiece_id);
    let mut notes = Vec::new();
    if workpieces.is_empty() {
        notes.push("이 앤드밀로 기록된 실행이 아직 없습니다".into());
    }
    Ok(EndMillRelay {
        endmill_id,
        label: row.label.clone(),
        aliases: alias_names(&row),
        workpieces,
        similar_endmills,
        suggestions,
        notes,
    })
}

pub fn workpiece_comparison(a: &WorkpieceSetup, b: &WorkpieceSetup) -> Vec<AttrDiff> {
    workpiece_diff(&WorkpieceAttr::from_setup(a), a, &WorkpieceAttr::from_setup(b), b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::rdb::{Metric, NewRun};

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("endmill_cur_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn relay_evidence_lifts_proven_endmill() {
        let dir = tmp("relay");
        let mut st = Store::open(&dir).unwrap();
        let steel = WorkpieceSetup::default_steel_block();
        let mut steel_b = steel.clone();
        steel_b.width_mm = 140.0;
        let good = EndMillMockupSetting::default_10mm_4flute();
        let mut weak = good.clone();
        weak.coating_name = Some("TiN".into());
        weak.helix_angle_deg = 30.0;
        weak.model = "WEAK".into();
        let eg = st.index_endmill(&good).unwrap();
        let ew = st.index_endmill(&weak).unwrap();
        let wb = st.index_workpiece(&steel_b).unwrap();
        let p = st.project_id;
        for (em, ratio, life) in [(eg, 0.8, 70.0), (ew, 1.8, 20.0)] {
            let mut last_run = 0;
            for _ in 0..3 {
                let run = NewRun {
                    project_id: p,
                    profile_id: None,
                    preset_id: None,
                    endmill_id: em,
                    workpiece_id: wb,
                    tool_key: "T".into(),
                    kind: "measurement".into(),
                    purpose: "roughing".into(),
                    coolant: "mist".into(),
                    conditions: crate::profile::MachiningPreset::default_steel_general().conditions,
                    note: String::new(),
                };
                last_run = st
                    .rdb
                    .insert_run(
                        &run,
                        &[
                            Metric::new("wear_ratio", "calib", ratio, ""),
                            Metric::new("tool_life_min", "meas", life, "min"),
                            Metric::new("tool_life_min", "pred", 50.0, "min"),
                        ],
                    )
                    .unwrap();
            }
            let setting = st.rdb.endmill(em).unwrap().unwrap().setting();
            let card = physics_card(&setting, &steel_b, &MachineLimits::default(), &CoolantConfig::mist(), Purpose::Roughing);
            let doc = crate::store::RegimeDoc {
                run_id: last_run,
                endmill_id: em,
                workpiece_id: wb,
                project_id: p,
                coating: setting.coating_name.clone().unwrap_or_default(),
                chem: "철계".into(),
                coolant: "mist".into(),
                dominant: card.dominant_wear.clone(),
                vc_m_min: card.vc_m_min,
                interface_c: card.interface_c,
                tool_life_min: card.tool_life_min,
                summary: String::new(),
                vector: card.regime.clone(),
            };
            assert!(st.put_regime(&doc).unwrap().is_some());
        }
        let rep = curate_for_workpiece(
            &mut st,
            &steel,
            Some(&weak),
            &MachineLimits::default(),
            &CoolantConfig::mist(),
            Purpose::Roughing,
            None,
            None,
            8,
        )
        .unwrap();
        assert!(!rep.similar_workpieces.is_empty());
        let g = rep.candidates.iter().find(|c| c.endmill_id == eg).unwrap();
        let w = rep.candidates.iter().find(|c| c.endmill_id == ew).unwrap();
        assert!(g.evidence.iter().any(|e| e.kind == "relay_workpiece"));
        assert!(g.evidence.iter().any(|e| e.kind == "relay_regime"), "{:?}", g.evidence);
        assert!(g.physics.regime.len() == crate::tribology::REGIME_DIM && !g.physics.dominant_wear.is_empty());
        assert!(g.score > w.score, "{} vs {}", g.score, w.score);
        assert!(w.is_current);
        let cmp = compare_endmills(&mut st, eg, ew, &steel, &MachineLimits::default(), &CoolantConfig::mist(), Purpose::Roughing, None).unwrap();
        assert!(!cmp.diffs.is_empty());
        assert!(!cmp.verdict.is_empty());
        let relay = relay_for_endmill(&mut st, ew, None).unwrap();
        assert_eq!(relay.workpieces.len(), 1);
        drop(st);
        let _ = std::fs::remove_dir_all(&dir);
    }
}