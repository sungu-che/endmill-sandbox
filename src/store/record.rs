use super::rdb::{Metric, NewRun};
use super::series::{self, PointRow, WindowRow};
use super::vector::{self, Scalar, VecDoc};
use super::{now_ms, Store};
use crate::decision::DecisionReport;
use crate::pipeline::{purpose_key, ImageOutcome, IngestOutcome, MoldGeometryOutcome, ProcessOutcome, Workspace};
use crate::timeseries;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunContext {
    pub project_id: i64,
    pub profile_id: Option<i64>,
    pub preset_id: Option<i64>,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub tool_key: String,
}

pub fn crossing_life(t: &[f64], y: &[f64], threshold: f64, minutes_per_t: f64) -> Option<f64> {
    if t.len() < 2 || y.is_empty() || y[0] >= threshold {
        return None;
    }
    for i in 1..t.len() {
        if y[i] >= threshold {
            let span = (y[i] - y[i - 1]).max(1e-12);
            let tc = t[i - 1] + (threshold - y[i - 1]) / span * (t[i] - t[i - 1]);
            return Some((tc - t[0]) * minutes_per_t);
        }
    }
    None
}

fn finite(v: f64, cap: f64) -> f64 {
    if v.is_finite() {
        v.min(cap)
    } else {
        cap
    }
}

impl Store {
    pub fn run_context(&mut self, ws: &Workspace) -> Result<RunContext, String> {
        let endmill_id = self.index_endmill(&ws.profile.endmill_setting)?;
        let workpiece_id = self.index_workpiece(&ws.profile.workpiece_setup)?;
        let (profile_id, preset_id) = match self.rdb.profile_ids(self.project_id, &ws.profile.name)? {
            Some((id, preset)) => (Some(id), preset),
            None => (None, self.rdb.preset_id(&ws.profile.name)?),
        };
        Ok(RunContext {
            project_id: self.project_id,
            profile_id,
            preset_id,
            endmill_id,
            workpiece_id,
            tool_key: ws.tool_key(),
        })
    }

    fn new_run(rc: &RunContext, ws: &Workspace, kind: &str, note: String) -> NewRun {
        NewRun {
            project_id: rc.project_id,
            profile_id: rc.profile_id,
            preset_id: rc.preset_id,
            endmill_id: rc.endmill_id,
            workpiece_id: rc.workpiece_id,
            tool_key: rc.tool_key.clone(),
            kind: kind.into(),
            purpose: purpose_key(ws.purpose).into(),
            coolant: ws.profile.coolant_config.method.key().into(),
            conditions: ws.profile.conditions.clone(),
            note,
        }
    }

    pub fn record_process(&mut self, ws: &Workspace, out: &ProcessOutcome) -> Result<Vec<String>, String> {
        let rc = self.run_context(ws)?;
        let a = &out.analysis;
        let mut m = vec![
            Metric::new("tool_life_min", "pred", finite(a.wear.tool_life_min, 1e6), "min"),
            Metric::new("linear_life_min", "pred", finite(a.wear.linear_life_min, 1e6), "min"),
            Metric::new("vb_rate_mm_min", "pred", a.wear.vb_rate_mm_per_min, "mm/min"),
            Metric::new("spindle_power_kw", "pred", a.spindle_power_kw, "kW"),
            Metric::new("power_util", "pred", a.spindle_power_kw / a.available_power_kw.max(1e-9), ""),
            Metric::new("interface_c", "pred", a.thermal.interface_c, "°C"),
            Metric::new("wall_error_um", "pred", a.error_budget_um.total_um, "µm"),
            Metric::new("wall_util", "pred", a.error_budget_um.utilization, ""),
            Metric::new("ra_um", "pred", a.surface.wall_ra_um, "µm"),
            Metric::new("mrr_cm3_min", "pred", a.mrr_cm3_min, "cm³/min"),
            Metric::new("chatter_margin", "pred", finite(a.stability.margin, 10.0), "×"),
            Metric::new("fn_hz", "pred", a.stability.fn_hz, "Hz"),
            Metric::new("frf_measured", "pred", if a.stability.measured { 1.0 } else { 0.0 }, ""),
            Metric::new("sim_vb_end_mm", "pred", out.sim.vb_end_mm, "mm"),
            Metric::new("sim_max_wall_um", "pred", out.sim.max_wall_error_um, "µm"),
            Metric::new("sim_cut_min", "pred", out.sim.cut_time_min, "min"),
        ];
        if out.sim.min_chatter_margin > 0.0 {
            m.push(Metric::new("sim_chatter_margin", "pred", out.sim.min_chatter_margin, "×"));
        }
        if out.sim.max_workpiece_defl_um > 0.0 {
            m.push(Metric::new("workpiece_defl_um", "pred", out.sim.max_workpiece_defl_um, "µm"));
        }
        let run = Self::new_run(&rc, ws, "process", out.source.clone());
        let run_id = self.rdb.insert_run(&run, &m)?;
        let mut log = vec![format!(
            "공정 해석 #{} 저장 → 프로젝트 '{}' / 앤드밀 #{} / 공작물 #{}",
            run_id,
            self.project_label(),
            rc.endmill_id,
            rc.workpiece_id
        )];
        log.extend(self.put_regime_vector(&rc, run_id, ws, out)?);
        if let Some(n) = &out.nominal_mold {
            log.extend(self.put_mold_vector(&rc, run_id, n)?);
        }
        Ok(log)
    }

    fn put_regime_vector(&mut self, rc: &RunContext, run_id: i64, ws: &Workspace, out: &ProcessOutcome) -> Result<Vec<String>, String> {
        let a = &out.analysis;
        let tri = &a.tribology;
        let doc = super::RegimeDoc {
            run_id,
            endmill_id: rc.endmill_id,
            workpiece_id: rc.workpiece_id,
            project_id: rc.project_id,
            coating: tri.coating_family.clone(),
            chem: tri.workpiece_chem.clone(),
            coolant: ws.profile.coolant_config.method.key().into(),
            dominant: a.wear.dominant.clone(),
            vc_m_min: a.vc_effective_m_min,
            interface_c: a.thermal.interface_c,
            tool_life_min: finite(a.wear.tool_life_min, 1e6),
            summary: format!(
                "{} × {} × {} · μ {:.2} · {:.0}°C · {}",
                tri.coating_family, tri.workpiece_chem, tri.coolant_medium, tri.mu_eff, a.thermal.interface_c, a.wear.dominant
            ),
            vector: a.regime.clone(),
        };
        Ok(match self.put_regime(&doc)? {
            Some(index) => vec![format!("상호작용 레짐 벡터 → LanceDB cut_regime ({}) · {}", index.label(), doc.summary)],
            None => Vec::new(),
        })
    }

    fn put_mold_vector(&mut self, rc: &RunContext, run_id: i64, g: &MoldGeometryOutcome) -> Result<Vec<String>, String> {
        let dim = g.vector.values.len() as u32;
        let Some(v) = self.vec.as_mut() else {
            return Ok(Vec::new());
        };
        if dim == 0 {
            return Ok(Vec::new());
        }
        let spec = vector::mold_geom_spec(dim);
        let name = spec.name.clone();
        let index = v.ensure(spec)?;
        let pk = format!("r{}:{}:{}", run_id, g.role, g.mold_id);
        v.upsert(
            &name,
            &[VecDoc {
                pk: pk.clone(),
                vector: g.vector.values.clone(),
                fields: vec![
                    ("workpiece_id".into(), Scalar::Int(rc.workpiece_id)),
                    ("endmill_id".into(), Scalar::Int(rc.endmill_id)),
                    ("project_id".into(), Scalar::Int(rc.project_id)),
                    ("mold_id".into(), Scalar::Text(g.mold_id.clone())),
                    ("role".into(), Scalar::Text(g.role.clone())),
                    ("max_depth_mm".into(), Scalar::Real(g.descriptor.max_depth_mm)),
                    ("steep_ratio".into(), Scalar::Real(g.descriptor.steep_ratio)),
                ],
            }],
        )?;
        self.rdb
            .add_vector_ref(&name, &pk, "workpiece", rc.workpiece_id, Some(rc.project_id), Some(run_id))?;
        Ok(vec![format!("금형 형상 벡터 → LanceDB {} ({})", name, index.label())])
    }

    fn put_image_vector(&mut self, rc: &RunContext, run_id: i64, domain: &str, img: &ImageOutcome) -> Result<Vec<String>, String> {
        let dim = img.global.len() as u32;
        let Some(v) = self.vec.as_mut() else {
            return Ok(Vec::new());
        };
        if dim == 0 {
            return Ok(Vec::new());
        }
        let spec = vector::tool_image_spec(dim);
        let name = if domain == "tool" {
            spec.name.clone()
        } else {
            format!("mold_image_d{}", dim)
        };
        let spec = vector::CollectionSpec { name: name.clone(), ..spec };
        let index = v.ensure(spec)?;
        let state = img.comparison.as_ref().map(|c| c.state.key().to_string()).unwrap_or_else(|| "reference".into());
        let pk = format!("r{}:{}:{}", run_id, img.image.view, img.role);
        v.upsert(
            &name,
            &[VecDoc {
                pk: pk.clone(),
                vector: img.global.clone(),
                fields: vec![
                    ("endmill_id".into(), Scalar::Int(rc.endmill_id)),
                    ("workpiece_id".into(), Scalar::Int(rc.workpiece_id)),
                    ("project_id".into(), Scalar::Int(rc.project_id)),
                    ("run_id".into(), Scalar::Int(run_id)),
                    ("tool_key".into(), Scalar::Text(img.image.key.clone())),
                    ("view".into(), Scalar::Text(img.image.view.clone())),
                    ("role".into(), Scalar::Text(img.role.clone())),
                    ("state".into(), Scalar::Text(state)),
                    ("minutes".into(), Scalar::Real(img.minutes.unwrap_or(-1.0))),
                    ("radial_loss_um".into(), Scalar::Real(img.measured_radial_um.unwrap_or(-1.0))),
                ],
            }],
        )?;
        self.rdb
            .add_vector_ref(&name, &pk, "endmill", rc.endmill_id, Some(rc.project_id), Some(run_id))?;
        Ok(vec![format!("이미지 전역 임베딩 → LanceDB {} ({})", name, index.label())])
    }

    pub fn record_ingest(&mut self, ws: &Workspace, out: &IngestOutcome) -> Result<Vec<String>, String> {
        let mut log = Vec::new();
        if out.doc.applied.is_empty() {
            return Ok(log);
        }
        if !out.doc.fingerprint.is_empty() {
            self.rdb
                .mark_seen(&out.doc.fingerprint, &out.doc.kind_label, &out.doc.name, Some(self.project_id))?;
        }
        let rc = self.run_context(ws)?;
        if !out.doc.duplicate {
            let mut m: Vec<Metric> = Vec::new();
            for c in out.calibrations.iter() {
                if c.predicted.abs() > 1e-12 {
                    m.push(Metric::new(&format!("{}_ratio", c.axis), "calib", c.measured / c.predicted, ""));
                }
            }
            let mut kind = "document";
            if let Some(t) = &out.tool_image {
                kind = "tool_image";
                if let Some(r) = t.measured_radial_um {
                    m.push(Metric::new("radial_loss_um", "meas", r, "µm"));
                }
                if let Some(r) = t.predicted_radial_um {
                    m.push(Metric::new("radial_loss_um", "pred", r, "µm"));
                }
                if let Some(id) = t.identity {
                    m.push(Metric::new("image_identity", "meas", id as f64, ""));
                }
                if let Some(c) = &t.comparison {
                    m.push(Metric::new("wear_severity", "meas", c.severity_index as f64, ""));
                }
            }
            if let Some(t) = &out.mold_image {
                kind = "mold_image";
                if let Some(c) = &t.comparison {
                    m.push(Metric::new("surface_severity", "meas", c.severity_index as f64, ""));
                }
            }
            if let Some(g) = &out.mold_geometry {
                kind = "mold_geometry";
                m.push(Metric::new("mold_max_depth_mm", "meas", g.descriptor.max_depth_mm, "mm"));
                m.push(Metric::new("mold_steep_ratio", "meas", g.descriptor.steep_ratio, ""));
                if let Some(f) = &g.fit {
                    m.push(Metric::new("mold_fit_r2", "meas", f.r2, ""));
                }
            }
            if let Some(w) = &out.wall {
                kind = "wall_cmm";
                m.push(Metric::new("wall_fit_r2", "meas", w.fit.r2, ""));
                m.push(Metric::new("wall_rms_um", "meas", w.fit.rms_residual_um, "µm"));
            }
            let run = Self::new_run(&rc, ws, kind, out.doc.name.clone());
            let run_id = self.rdb.insert_run(&run, &m)?;
            log.push(format!("입력 '{}' → 실행 #{} ({}) 저장, 지표 {}개", out.doc.name, run_id, kind, m.len()));
            if let Some(t) = &out.tool_image {
                log.extend(self.put_image_vector(&rc, run_id, "tool", t)?);
            }
            if let Some(t) = &out.mold_image {
                log.extend(self.put_image_vector(&rc, run_id, "mold", t)?);
            }
            if let Some(g) = &out.mold_geometry {
                log.extend(self.put_mold_vector(&rc, run_id, g)?);
            }
            log.extend(self.persist_series(ws, &rc, run_id)?);
        } else {
            log.extend(self.persist_series(ws, &rc, 0)?);
        }
        Ok(log)
    }

    pub fn persist_series(&mut self, ws: &Workspace, rc: &RunContext, run_id: i64) -> Result<Vec<String>, String> {
        let mut log = Vec::new();
        if self.ts.is_none() || ws.series.is_empty() {
            return Ok(log);
        }
        let now = now_ms();
        let part_min = ws.sim.as_ref().map(|r| r.cut_time_min);
        let mut rows: Vec<PointRow> = Vec::new();
        let mut wins: Vec<WindowRow> = Vec::new();
        let mut lives: Vec<(String, i64, f64, String)> = Vec::new();
        for (key, s) in ws.series.iter() {
            let sid = series::series_id(rc.project_id, &rc.tool_key, key);
            for (t, y) in s.t.iter().zip(s.y.iter()) {
                rows.push(PointRow {
                    series_id: sid.clone(),
                    project_id: rc.project_id,
                    endmill_id: rc.endmill_id,
                    workpiece_id: rc.workpiece_id,
                    run_id,
                    tool_key: rc.tool_key.clone(),
                    metric: key.clone(),
                    unit: s.unit.clone(),
                    t_unit: s.t_unit.clone(),
                    t: *t,
                    y: *y,
                    source: "workspace".into(),
                    ingested_at: now,
                });
            }
            let Some(thr) = ws.threshold(key) else {
                continue;
            };
            let k = s.minutes_per_t(part_min);
            for w in series::episode_windows(s, thr, k) {
                wins.push(WindowRow {
                    series_id: sid.clone(),
                    episode: w.episode,
                    project_id: rc.project_id,
                    endmill_id: rc.endmill_id,
                    workpiece_id: rc.workpiece_id,
                    metric: key.clone(),
                    t_start: w.t_start,
                    t_end: w.t_end,
                    points: w.points as i64,
                    duration_min: w.duration_min,
                    final_ratio: w.final_ratio,
                    rate_per_hour: w.rate_per_hour,
                    vector: w.vector,
                });
            }
            if let (true, Some(k)) = (key == "vb_mm" || key == "radial_loss_um", k) {
                for (ep, (a, b)) in timeseries::episodes_of(s).into_iter().enumerate() {
                    if let Some(life) = crossing_life(&s.t[a..b], &s.y[a..b], thr, k) {
                        lives.push((sid.clone(), ep as i64, life, key.clone()));
                    }
                }
            }
        }
        if let Some(ts) = self.ts.as_mut() {
            let (ins, upd) = ts.upsert_points(&rows)?;
            let nw = ts.upsert_windows(&wins)?;
            log.push(format!(
                "시계열 {}종 → LanceDB 점 {}건 추가 / {}건 갱신, 마모 곡선 창 {}개",
                ws.series.len(),
                ins,
                upd,
                nw
            ));
        }
        for (sid, ep, life, key) in lives {
            if !self.rdb.life_marked(&sid, ep)? {
                let run = Self::new_run(rc, ws, "life", format!("{} 에피소드 {} 한계 도달", key, ep));
                let rid = self.rdb.insert_run(&run, &[Metric::new("tool_life_min", "meas", life, "min")])?;
                self.rdb.mark_life(&sid, ep, rid, life)?;
                log.push(format!("{} 에피소드 {} 가 한계에 도달 → 실측 수명 {:.1}분 기록 (실행 #{})", key, ep, life, rid));
            }
        }
        Ok(log)
    }

    pub fn restore_series(&self, ws: &mut Workspace) -> Result<usize, String> {
        let Some(ts) = self.ts.as_ref() else {
            return Ok(0);
        };
        let key = ws.tool_key();
        let mut map = BTreeMap::new();
        for meta in ts.list(Some(self.project_id), Some(&key))? {
            if let Some(s) = ts.load(&meta.series_id)? {
                map.insert(meta.metric.clone(), s);
            }
        }
        let n = map.len();
        ws.replace_series(map);
        Ok(n)
    }

    pub fn record_decision(&mut self, ws: &Workspace, rep: &DecisionReport) -> Result<Vec<String>, String> {
        let rc = self.run_context(ws)?;
        let mut m = vec![Metric::new("decision_code", "meta", rep.final_action.conservativeness() as f64, "")];
        if let Some(s) = rep.feed_scale {
            m.push(Metric::new("feed_scale", "meta", s, ""));
        }
        if let Some(s) = rep.speed_scale {
            m.push(Metric::new("speed_scale", "meta", s, ""));
        }
        for g in rep.gates.iter().filter(|g| g.id == "chatter" || g.id == "error_budget" || g.id == "image_wear") {
            m.push(Metric::new(&format!("gate_{}", g.id), "meas", g.value, &g.unit));
        }
        let run = Self::new_run(&rc, ws, "decision", rep.final_action_label.clone());
        let id = self.rdb.insert_run(&run, &m)?;
        Ok(vec![format!("판단 결과 '{}' → 실행 #{} 저장", rep.final_action_label, id)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_life_interpolates() {
        let t = [0.0, 10.0, 20.0, 30.0];
        let y = [0.0, 0.1, 0.2, 0.4];
        let l = crossing_life(&t, &y, 0.3, 1.0).unwrap();
        assert!((l - 25.0).abs() < 1e-9);
        assert!(crossing_life(&t, &y, 0.5, 1.0).is_none());
        assert!(crossing_life(&t, &[0.4, 0.5, 0.6, 0.7], 0.3, 1.0).is_none());
    }
}