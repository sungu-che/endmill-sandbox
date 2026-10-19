//! 공구 수명 말기(정지·교체·만료·파손)와 공구 × 피삭재 N:N 행렬 시험.
//!
//! 예전 경로 시뮬레이션은 VB 를 한계의 2배에서 자르기만 하고 경로 끝까지 깎았다 (인코넬 포켓: 한계를 넘긴 뒤 134 s · 합력 8 kN,
//! 고경도 볼 포켓: 한계를 넘긴 뒤 26 분). 여기서는 그런 일이 다시 생기지 않는지와, 공구를 다른 소재에 댔을 때의 판정이
//! 리서치 정리(문헌·제조사 자료)와 같은 방향인지를 본다. 행렬 결과는 `[MATRIX]` 줄과 CSV/JSON 으로 남긴다
//! (`ENDMILL_MATRIX_DIR` 가 있으면 그 폴더, 없으면 임시 폴더).
use endmill_model::appfit;
use endmill_model::gcode::ToolPathSegment;
use endmill_model::loadsim::WallErrorSample;
use endmill_model::pairmatrix::{self, MatrixCell};
use endmill_model::physics::{self, CutContext};
use endmill_model::profile::{EndMillMockupSetting, MachiningPreset, MachiningProfile, ToolLifePolicy};
use endmill_model::store::Store;
use endmill_model::toollife;
use endmill_model::WorkpieceMaterial;

fn preset(name: &str) -> MachiningProfile {
    let p = MachiningPreset::builtin().into_iter().find(|p| p.name.contains(name)).expect("preset");
    MachiningProfile::from_preset(&p, &p.name)
}

fn simulate(p: &MachiningProfile, pattern: &str) -> endmill_model::loadsim::LoadSimReport {
    let ctx = CutContext::from_profile(p);
    endmill_model::viewport::build_report_prog(p, pattern, &ctx, &mut |_| true).unwrap().1
}

fn with_policy(name: &str, policy: ToolLifePolicy) -> MachiningProfile {
    let mut p = preset(name);
    p.machine.tool_life_policy = policy;
    p
}

fn print_events(tag: &str, r: &endmill_model::loadsim::LoadSimReport) {
    let tl = &r.tool_life;
    println!(
        "[EOL] {} · 상태 {} · 공구 {} · 완료 {:.0}% · VB {:.3} mm · 응력 ×{:.2} · 피로 {:.3} · 치핑 {:.0}% · 부하 ×{:.2}",
        tag,
        tl.state,
        tl.tools_used,
        tl.completed_fraction() * 100.0,
        r.vb_end_mm,
        tl.peak_stress_ratio,
        tl.fatigue_damage,
        tl.chip_probability * 100.0,
        tl.load_ratio_max
    );
    for e in tl.events.iter() {
        println!("[EOL]    {} #{} t {:.1} s seg {} — {}", e.kind, e.tool_no, e.t_s, e.seg, e.message);
    }
}

#[test]
fn tertiary_wear_factor_starts_at_the_onset_and_reaches_the_multiplier_at_collapse() {
    for g in [
        endmill_model::pairref::WpGroup::Aluminum,
        endmill_model::pairref::WpGroup::CarbonSteel,
        endmill_model::pairref::WpGroup::Hardened,
        endmill_model::pairref::WpGroup::Nickel,
    ] {
        let p = toollife::eol_params(g);
        assert!((toollife::accel_factor(&p, 0.0) - 1.0).abs() < 1e-12);
        assert!((toollife::accel_factor(&p, p.vb_accel_mm) - 1.0).abs() < 1e-12);
        assert!((toollife::accel_factor(&p, p.vb_collapse_mm) - p.accel_mult).abs() < 1e-9, "{:?}", g);
        let mid = toollife::accel_factor(&p, 0.5 * (p.vb_accel_mm + p.vb_collapse_mm));
        assert!(mid > 1.0 && mid < p.accel_mult);
        assert!(p.vb_collapse_mm > p.vb_accel_mm && p.static_frac > p.fatigue_frac);
    }
}

#[test]
fn rake_corrected_radial_recession_matches_the_classic_formula() {
    let f = physics::radial_per_vb(12.0, 18.0);
    let ta = 12f64.to_radians().tan();
    let tg = 18f64.to_radians().tan();
    assert!((f - ta / (1.0 - ta * tg)).abs() < 1e-12);
    assert!(f > 1.06 * ta && f < 1.09 * ta, "positive rake recesses more than VB·tanα: {}", f / ta);
    assert!(physics::radial_per_vb(12.0, -5.0) < ta, "negative rake recesses less");
}

#[test]
fn body_breaking_force_has_the_published_scale() {
    // 리서치 추산: 2.5D 팔에서 2날 약 26·D² N, 4날 약 43·D² N (항절력 4 GPa, 코어 단면) — 이 모델은 등가 단면·정적 유효 강도라 같은 자릿수면 된다
    let p = preset("알루미늄 황삭");
    let mut tool = CutContext::from_profile(&p).tool;
    tool.stickout_mm = 15.0 + tool.loc_mm.min(15.0);
    let eol = toollife::eol_params(endmill_model::pairref::WpGroup::Aluminum);
    let s = toollife::strength_of(&tool, &eol);
    let ap = 2.0;
    let unit = toollife::body_stress(&tool, 1.0, 0.0, ap).sigma_mpa;
    let f_break = s.static_mpa() / unit;
    println!("[EOL] Ø{} {}날 돌출 {:.0} mm 정적 파손 하중 {:.0} N (정적 유효 {:.0} MPa)", tool.diameter_mm, tool.flutes, tool.stickout_mm, f_break, s.static_mpa());
    assert!(f_break > 400.0 && f_break < 3000.0, "Ø6 2-flute breaks near 1 kN, got {:.0} N", f_break);
    assert!(s.fatigue_mpa() < s.static_mpa());
    assert!(s.cycles_to_failure(0.9 * s.fatigue_mpa()).is_infinite());
    assert!(s.cycles_to_failure(1.2 * s.fatigue_mpa()) < 1.0e7);
}

#[test]
fn inconel_pocket_stops_changes_expires_or_breaks_by_policy_instead_of_cutting_on() {
    let limit = CutContext::from_profile(&preset("인코넬")).vb_limit_mm();
    // 감시 없음: 파손으로 끝난다 (붕괴·피로·과부하·치핑 중 하나), 날끝 붕괴 VB 를 넘겨 깎지 않는다
    let run = simulate(&with_policy("인코넬", ToolLifePolicy::RunToFailure), "pocket_zigzag");
    print_events("인코넬 포켓 · 감시 없음", &run);
    let tr = &run.tool_life;
    assert!(tr.failed && !tr.failure_cause.is_empty(), "an unmonitored worn tool ends in failure");
    assert!(run.vb_end_mm <= tr.params.vb_collapse_mm * toollife::VB_GUARD_FACTOR + 1e-9);
    // 정지: 수명 판정(지금 절삭을 마치고) 또는 치핑 감지(즉시)에서 멈춘다 — 한계를 크게 넘겨 깎지 않는다
    let stop = simulate(&with_policy("인코넬", ToolLifePolicy::Stop), "pocket_zigzag");
    print_events("인코넬 포켓 · 정지", &stop);
    let tl = &stop.tool_life;
    assert!(tl.events.iter().any(|e| e.kind == "life_end" || e.kind == "chip_detected"), "the pocket wears or chips the tool");
    assert!(tl.unfinished && !tl.failed && tl.completed_fraction() < 0.99, "stop leaves the rest of the path uncut");
    assert!(stop.vb_end_mm <= toollife::HARD_LIMIT_FACTOR * limit + 0.01, "no deep overrun past the limit: {:.3} vs {:.2}", stop.vb_end_mm, limit);
    assert!(stop.warnings.iter().any(|w| w.contains("[공구 수명]")));
    // 같은 공구는 감시 여부와 관계없이 같은 위험을 겪는다: 감시하는 정책이 감시 없는 공구가 부서지는 지점을 넘어 깎지 않는다
    assert!(
        stop.cut_time_min <= run.cut_time_min + 1e-9,
        "the monitored tool never cuts past the point where the same unmonitored tool fails: {:.3} vs {:.3} min",
        stop.cut_time_min,
        run.cut_time_min
    );
    assert!(tl.chip_probability < toollife::CHIP_FAILURE_P + 0.05, "a monitored tool is not left cutting with a likely chipped edge: {:.2}", tl.chip_probability);
    // 교체 후 계속: 공구 여러 개로 끝까지, 어느 공구도 판정을 크게 넘기지 않고 치핑 확률이 파손 수준에 닿으면 바로 바뀐다
    let change = simulate(&with_policy("인코넬", ToolLifePolicy::ChangeTool), "pocket_zigzag");
    print_events("인코넬 포켓 · 교체", &change);
    let tc = &change.tool_life;
    if !tc.failed {
        assert!(tc.tool_changes >= 1 && tc.tools_used == tc.tool_changes + 1);
        assert!(!tc.unfinished && tc.completed_fraction() > 0.999);
        for r in tc.retired.iter() {
            assert!(r.vb_band_mm <= toollife::HARD_LIMIT_FACTOR * limit + 0.01, "retired tool #{} overran: {:.3}", r.tool_no, r.vb_band_mm);
            assert!(r.chip_p < toollife::CHIP_FAILURE_P + 0.05, "retired tool #{} kept cutting chipped: {:.2}", r.tool_no, r.chip_p);
            assert!(r.cut_min > 0.0 && !r.field.is_empty());
            assert!(r.cause == "life_end" || r.cause == "chipping", "retired tool #{} cause {:?}", r.tool_no, r.cause);
        }
        assert_eq!(tc.retired.iter().filter(|r| r.cause == "chipping").count() as u32, tc.chip_detected);
        assert!(change.vb_end_mm < limit + 0.01);
        assert!(change.cut_time_min >= stop.cut_time_min);
        assert!(change.wear_log.iter().any(|b| b.tool_no == tc.tools_used), "bins carry the tool number");
    }
    // 공구 수명 관리(프로그램 끝까지): 만료 표시가 남고, 끝까지 가거나 그 사이 파손으로 멈춘다
    let fin = simulate(&with_policy("인코넬", ToolLifePolicy::FinishPart), "pocket_zigzag");
    print_events("인코넬 포켓 · 프로그램 끝까지", &fin);
    assert!(fin.tool_life.expired || fin.tool_life.failed);
}

#[test]
fn a_finishing_ball_cannot_rough_a_hardened_pocket() {
    // 고경도 정삭 프리셋(Ø6 롱넥 볼)으로 포켓을 판다. 예전 경로는 ap 5.4 mm 를 슬롯형으로 깎아 첫 0.1 분에 몸체 과부하로 부러졌다. 지금 경로는
    // 고경도 스퀘어 공구의 슬롯 ap 0.4·D 로 단계 절입하지만 (볼의 폭 전체 ap 는 제조사 자료가 없음), 58 HRC 를 볼로 폭 전체 깎는 결합은 채터·응력·
    // 경도 배수로 치핑이 먼저 온다: 감시하면 경로 1 % 남짓에서 치핑 감지로 멈추고, 감시 없으면 그 자리에서 치핑 파손, 예비 공구로 이어 가면
    // 공구를 수십 개 쓰고도 끝내지 못한다 — 어느 정책으로도 이 공구로는 포켓을 황삭할 수 없다
    for pol in ToolLifePolicy::ALL {
        let r = simulate(&with_policy("금형강", pol), "pocket_zigzag");
        let tl = &r.tool_life;
        print_events(&format!("고경도 포켓 · {}", pol.key()), &r);
        assert!((tl.unfinished || tl.failed) && tl.completed_fraction() < 0.5, "{:?}: the ball nose does not get through the pocket ({:.0}%)", pol, tl.completed_fraction() * 100.0);
        match pol {
            ToolLifePolicy::Stop => assert!(!tl.failed && tl.chip_detected >= 1 && tl.completed_fraction() < 0.05, "monitoring stops on chipping right away"),
            ToolLifePolicy::ChangeTool => assert!(tl.tools_used >= 10, "sister tools keep chipping: {} tools", tl.tools_used),
            _ => assert!(tl.failed && tl.failure_cause == "chipping" && tl.completed_fraction() < 0.05, "{:?}: chips at once, cause {:?}", pol, tl.failure_cause),
        }
    }
}

#[test]
fn intended_finishing_passes_do_not_break_their_tools() {
    for name in ["알루미늄 황삭", "티타늄", "탄소강", "스테인리스", "인코넬", "CFRP", "알루미늄 정삭"] {
        let r = simulate(&preset(name), "rect_profile");
        let tl = &r.tool_life;
        println!("[EOL] {} rect_profile · 상태 {} · 응력 ×{:.2} · 치핑 {:.0}% · 부하 ×{:.2}", name, tl.state, tl.peak_stress_ratio, tl.chip_probability * 100.0, tl.load_ratio_max);
        assert!(!tl.failed, "{}: {:?}", name, tl.stop_reason());
        assert!(!tl.unfinished && tl.completed_fraction() > 0.999, "{}", name);
        assert!(tl.peak_stress_ratio < 0.8, "{}: {:.2}", name, tl.peak_stress_ratio);
    }
}

#[test]
fn a_tool_that_starts_worn_out_is_not_used_silently() {
    let mut p = with_policy("탄소강", ToolLifePolicy::Stop);
    let mut ctx = CutContext::from_profile(&p);
    ctx.tool.flank_wear_mm = 1.05 * ctx.vb_limit_mm();
    let r = endmill_model::viewport::build_report_prog(&p, "rect_profile", &ctx, &mut |_| true).unwrap().1;
    assert!(r.tool_life.events.iter().any(|e| e.kind == "start_worn"));
    assert!(r.tool_life.unfinished && r.cut_time_min == 0.0 && r.removed_volume_cm3 == 0.0, "the worn tool does not start cutting");
    p.machine.tool_life_policy = ToolLifePolicy::ChangeTool;
    ctx.machine.tool_life_policy = ToolLifePolicy::ChangeTool;
    let r2 = endmill_model::viewport::build_report_prog(&p, "rect_profile", &ctx, &mut |_| true).unwrap().1;
    assert_eq!(r2.tool_life.tool_changes, 1);
    assert!(!r2.tool_life.unfinished && r2.vb_start_mm == 0.0 && r2.cut_time_min > 0.0, "a sister tool takes over before the first cut");
}

#[test]
fn unfinished_first_pass_holds_the_compensation() {
    let p = with_policy("인코넬", ToolLifePolicy::Stop);
    let (ctx, _) = endmill_model::pipeline::calibrated_context(&p, None);
    let r = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap().1;
    assert!(r.tool_life.unfinished);
    let sds = endmill_model::sds::SdsStore::open(&std::env::temp_dir().join(format!("endmill-eol-sds-{}", std::process::id())));
    let log = endmill_model::wearcomp::analyze_job(&p, &ctx, &r, None, &sds);
    let draft = endmill_model::wearcomp::evaluate(&p, &ctx, &r, &log, &endmill_model::wearcomp::CompOptions::default(), None);
    let g = draft.gates.iter().find(|g| g.id == "pass1_complete").expect("gate");
    assert_eq!(g.severity, 3);
    assert_eq!(draft.rule_action, endmill_model::wearcomp::CompAction::Hold);
    assert!(!draft.allowed.iter().any(|a| a.cuts()), "no compensation pass on an unfinished first pass: {:?}", draft.allowed);
}

/// 멈춘 점 뒤의 남은 경로(이송 이동)에서 공구 축이 벽 점 높이(z_mid) 아래로 지나며 그 점에 가장 가까이 오는 수평 거리.
/// 시험용 독립 계산이라 모든 이송 이동을 0.05 mm 현으로 나눠 잰다 (직선은 현도 같은 직선 위)
fn rest_reach(segs: &[ToolPathSegment], start: (f64, f64, f64), stop: &toollife::ToolLifeEvent, x: f64, y: f64, z_mid: f64) -> f64 {
    let reach = |a: (f64, f64, f64), b: (f64, f64, f64)| -> f64 {
        let dz = b.2 - a.2;
        let (s0, s1) = if dz.abs() < 1e-12 {
            if a.2 < z_mid {
                (0.0, 1.0)
            } else {
                return f64::INFINITY;
            }
        } else if dz > 0.0 {
            (0.0, ((z_mid - a.2) / dz).min(1.0))
        } else {
            (((z_mid - a.2) / dz).max(0.0), 1.0)
        };
        if s0 >= s1 {
            return f64::INFINITY;
        }
        let (ux, uy) = (b.0 - a.0, b.1 - a.1);
        let l2 = ux * ux + uy * uy;
        let s = if l2 > 1e-18 { (((x - a.0) * ux + (y - a.1) * uy) / l2).clamp(s0, s1) } else { s0 };
        (x - a.0 - s * ux).hypot(y - a.1 - s * uy)
    };
    let stop_pt = (stop.x, stop.y, stop.z);
    let gap = |p: &(f64, f64, f64)| (p.0 - stop_pt.0).hypot(p.1 - stop_pt.1).hypot(p.2 - stop_pt.2);
    let mut best = f64::INFINITY;
    let mut prev = start;
    for (k, seg) in segs.iter().enumerate() {
        let s = prev;
        prev = seg.end_point();
        if k < stop.seg || !seg.feed().map(|f| f > 0.0).unwrap_or(false) {
            continue;
        }
        let mut pts = vec![s];
        pts.extend(seg.polyline_from(s, 0.05));
        if k == stop.seg {
            let i = (0..pts.len()).min_by(|a, b| gap(&pts[*a]).partial_cmp(&gap(&pts[*b])).unwrap()).unwrap();
            pts = pts.split_off(i);
            pts[0] = stop_pt;
        }
        for w in pts.windows(2) {
            best = best.min(reach(w[0], w[1]));
        }
    }
    best
}

/// 1차가 멈추면 마지막에 깎인 벽 중 남은 경로가 다시 깎을 곳은 미절삭 소재의 경계다 — 부품 벽이 아니라 최종 벽에서 빠진다.
/// 인코넬 포켓(옛 지그재그 고정 경로 `legacy_zigzag`, ap 0.5·D, 정지 정책, 약 28 % 에서 멈춤)에서 마지막 줄 너머의 벽은 다음 줄이 깎아 내고, 스톡 가장자리
/// 스킨(줄 끝 피치 이동이 만든 ±X 벽)은 남는다. 최종 벽 통계·세그먼트별 벽·품질 저하 보고·감사 줄이 모두 같은 최종 벽을 쓰고, 끝까지 간 작업은 그대로다
#[test]
fn a_stopped_pocket_counts_only_part_walls_as_final() {
    let mut p = with_policy("인코넬", ToolLifePolicy::Stop);
    // 옛 경로의 첫 줄은 폭 전체 슬롯이다. 슬롯이 측면 절삭보다 뜨겁게 물리는 단속 절삭 열 축적(`physics::interrupted_factor`, 문헌 벤치마크 2라운드)을
    // 넣은 뒤로는 프리셋 ap 1·D 슬롯에서 공구가 첫 줄 17 s 만에 수명에 닿고, 그 줄을 마저 깎다 정적 과부하로 부러진다 (예전 모델은 슬롯도
    // 측면 절삭과 같은 온도로 봐 61 % 까지 갔다). 멈춘 포켓 장면은 Ni 계 슬롯 권장의 가운데값 0.5·D (Mitsubishi · Haas 0.5, Helical 0.25~0.5,
    // Seco 0.6 — `WorkpieceProps::slot_ap_ratio` 주석) 로 본다
    p.conditions.axial_doc_mm = 0.5 * p.endmill_setting.diameter_mm;
    let (ctx, r, segs) = legacy_report(&p);
    let tl = &r.tool_life;
    assert!(tl.unfinished && !tl.failed);
    let stop = tl.events.iter().rev().find(|e| e.kind == "stop").expect("stop event");
    let start = (0.0, 0.0, p.endmill_setting.loc_mm + 10.0);
    let rad = 0.5 * ctx.tool.diameter_mm;
    // 남은 경로의 공구가 벽 점을 셀 1/4 넘게 덮으면 깎여 나가는 벽 — 경계 띠(셀 5 %)는 원호 현 근사의 차이
    let (reach, band) = (rad - 0.25 * r.cell_mm, 0.05 * r.cell_mm);
    let d = |x: f64, y: f64, z: f64| rest_reach(&segs, start, stop, x, y, z);
    let finals: Vec<&WallErrorSample> = r.wall_errors.iter().filter(|w| w.final_wall).collect();
    let uncut: Vec<&WallErrorSample> = r.wall_errors.iter().filter(|w| w.uncut_boundary).collect();
    let worst = |v: &[&WallErrorSample]| v.iter().map(|w| w.total_um.abs()).fold(0.0, f64::max);
    println!(
        "[EOL] 인코넬 포켓 · 정지 seg {} t {:.1} s ({:.1}, {:.1}) · 완료 {:.0}% · 최종 벽 {} 점 · 미절삭 경계 {} 점 (보고서 표본 {}) · 최종 벽 오차 최대 {:.1} µm · 미절삭 경계 최대 {:.1} µm (표본)",
        stop.seg,
        stop.t_s,
        stop.x,
        stop.y,
        tl.completed_fraction() * 100.0,
        r.final_wall_count,
        r.uncut_boundary_count,
        uncut.len(),
        r.max_wall_error_um,
        worst(&uncut)
    );
    for w in r.warnings.iter().filter(|w| w.contains("미절삭 경계")) {
        println!("[EOL]    {}", w);
    }
    let mut by_seg: std::collections::BTreeMap<usize, (usize, f64, f64, f64, f64)> = std::collections::BTreeMap::new();
    for w in uncut.iter() {
        let e = by_seg.entry(w.seg).or_insert((0, f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY));
        *e = (e.0 + 1, e.1.min(w.x), e.2.max(w.x), e.3.min(w.y), e.4.max(w.y));
    }
    for (seg, (n, x0, x1, y0, y1)) in by_seg.iter() {
        println!("[EOL]    미절삭 경계 표본 seg {} · {} 점 · X {:.1}~{:.1} Y {:.1}~{:.1}", seg, n, x0, x1, y0, y1);
    }
    assert!(r.uncut_boundary_count > 0 && !uncut.is_empty(), "the last row leaves a boundary that the rest of the pocket cuts away");
    for w in uncut.iter() {
        let dd = d(w.x, w.y, w.z_mid);
        assert!(!w.final_wall && dd < reach + band, "uncut-boundary wall at ({:.2}, {:.2}, {:.2}) is {:.3} mm from the rest of the path (reach {:.3})", w.x, w.y, w.z_mid, dd, reach);
    }
    for w in finals.iter() {
        let dd = d(w.x, w.y, w.z_mid);
        assert!(dd > reach - band, "final wall at ({:.2}, {:.2}, {:.2}) seg {} lies {:.3} mm from the rest of the path — the rest would cut it away", w.x, w.y, w.z_mid, w.seg, dd);
    }
    // 스톡 가장자리 스킨은 부품 벽: 가장자리에서 1.5 mm 안, 벽 법선이 그 가장자리 쪽. 지그재그는 줄 끝 피치 이동이 줄의 둥근 앞이 남긴
    // 모서리로 들어가며 ±X 스킨을 만든다 (첫 줄은 슬롯이라 벽 표본이 없고, +Y 스킨은 남은 경로의 마지막 줄이 만든다)
    let (sw, sh) = (p.workpiece_setup.width_mm, p.workpiece_setup.height_mm);
    let side_of = |w: &WallErrorSample| -> Option<&'static str> {
        if w.nx > 0.7 && sw - w.x < 1.5 {
            Some("+X")
        } else if w.nx < -0.7 && w.x < 1.5 {
            Some("−X")
        } else if w.ny > 0.7 && sh - w.y < 1.5 {
            Some("+Y")
        } else if w.ny < -0.7 && w.y < 1.5 {
            Some("−Y")
        } else {
            None
        }
    };
    let skins = |s: &str| finals.iter().filter(|w| side_of(w) == Some(s)).count();
    let (n_px, n_mx, n_py, n_my) = (skins("+X"), skins("−X"), skins("+Y"), skins("−Y"));
    println!("[EOL]    스톡 가장자리 스킨 최종 벽: +X {} · −X {} · +Y {} · −Y {} 점 · 그 밖의 최종 벽 {} 점", n_px, n_mx, n_py, n_my, finals.len() - n_px - n_mx - n_py - n_my);
    assert!(n_px > 0 && n_mx > 0, "the stepover skins at both X edges stay final: +X {} −X {}", n_px, n_mx);
    assert!(!uncut.iter().any(|w| side_of(w).is_some()), "no stock-edge skin is taken for uncut stock");
    // 소비처가 모두 같은 최종 벽을 쓴다 (미절삭 경계는 다시 깎일 벽으로 센다)
    assert!(r.uncut_boundary_count <= r.transient_wall_count);
    assert_eq!(r.quality_loss.final_points, r.final_wall_count, "the quality-loss report counts the same final walls");
    assert_eq!(r.seg_walls.iter().map(|s| s.n).sum::<usize>(), r.final_wall_count, "per-segment wall stats use the same final walls");
    for q in r.quality_loss.points.iter() {
        assert!(d(q.x, q.y, q.z) > reach - band, "a degraded wall point the rest would cut away: ({:.2}, {:.2})", q.x, q.y);
    }
    if r.final_wall_count < 4800 {
        // 보고서에 최종 벽이 빠짐없이 들어 있는 크기 — 경로 벽 오차 최대·평균이 바로 그 최종 벽에서 나온다
        assert_eq!(finals.len(), r.final_wall_count);
        assert!((worst(&finals) - r.max_wall_error_um).abs() < 1e-9);
        let mean = finals.iter().map(|w| w.total_um).sum::<f64>() / finals.len() as f64;
        assert!((mean - r.mean_wall_error_um).abs() < 1e-6);
    }
    assert!(r.warnings.iter().any(|w| w.contains("미절삭 경계")), "the exclusion is reported");
    // 감사 줄: 수명 줄과 경로 미완료 플래그가 뺀 점 수를 밝힌다
    let sds = endmill_model::sds::SdsStore::open(&std::env::temp_dir().join(format!("endmill-uncut-sds-{}", std::process::id())));
    let log = endmill_model::wearcomp::analyze_job(&p, &ctx, &r, None, &sds);
    let audit = endmill_model::simaudit::build(&p, &ctx, &r, &log, None, &r.pattern);
    assert_eq!(audit.tool_life.uncut_boundary_points, r.uncut_boundary_count);
    let lines = endmill_model::simaudit::lines(&audit);
    for l in lines.iter().filter(|l| l.contains("미절삭 경계")) {
        println!("{}", l);
    }
    assert!(lines.iter().any(|l| l.starts_with("[AUDIT] 수명") && l.contains("미절삭 경계")));
    assert!(lines.iter().any(|l| l.contains("path_unfinished") && l.contains("미절삭 경계")));

    // 끝까지 간 작업은 남은 경로가 없어 그대로다
    let done = simulate(&preset("인코넬"), "rect_profile");
    assert!(!done.tool_life.unfinished);
    assert_eq!(done.uncut_boundary_count, 0);
    assert!(done.final_wall_count > 0 && done.wall_errors.iter().all(|w| !w.uncut_boundary));
    assert!(!done.warnings.iter().any(|w| w.contains("미절삭 경계")));
    assert_eq!(done.quality_loss.final_points, done.final_wall_count);
}

/// 새 기본 경로(`gcode::pocket_plan`)는 마지막 정삭 윤곽이 벽 치수를 정한다 — 그 전에 멈춘 포켓은 깎은 벽이 모두 남은 경로(남은 황삭 · 정삭 윤곽)가
/// 다시 깎을 황삭 벽이라 부품 벽이 아직 없다. 최종 벽 0점, 벽 위치 오차·측벽 Ra 통계는 0 (황삭 과도 벽을 부품 벽 오차처럼 보이지 않게), 경고 한 줄,
/// 미절삭 경계는 모두 남은 경로가 덮는 벽 (독립 계산), 품질 저하 보고·세그먼트별 벽·감사가 같은 판정을 쓴다
#[test]
fn a_pocket_stopped_before_its_finishing_contour_has_no_part_walls_yet() {
    let p = with_policy("인코넬", ToolLifePolicy::Stop);
    let ctx = CutContext::from_profile(&p);
    let (_, r, segs) = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
    let tl = &r.tool_life;
    assert!(tl.unfinished && !tl.failed, "the Inconel pocket stops on the tool-life call");
    let stop = tl.events.iter().rev().find(|e| e.kind == "stop").expect("stop event");
    let finish_at = segs.iter().rposition(|s| matches!(s, ToolPathSegment::Plunge { .. })).expect("finishing descent");
    assert!(stop.seg < finish_at, "the stop comes before the finishing contour: seg {} vs {}", stop.seg, finish_at);
    let start = (0.0, 0.0, p.endmill_setting.loc_mm + 10.0);
    let rad = 0.5 * ctx.tool.diameter_mm;
    let (reach, band) = (rad - 0.25 * r.cell_mm, 0.05 * r.cell_mm);
    let uncut: Vec<&WallErrorSample> = r.wall_errors.iter().filter(|w| w.uncut_boundary).collect();
    println!(
        "[EOL] 인코넬 포켓 (기본 경로) · 정지 seg {} / 정삭 하강 seg {} · 완료 {:.0}% · 최종 벽 {} 점 · 미절삭 경계 {} 점 (보고서 표본 {})",
        stop.seg,
        finish_at,
        tl.completed_fraction() * 100.0,
        r.final_wall_count,
        r.uncut_boundary_count,
        uncut.len()
    );
    assert_eq!(r.final_wall_count, 0, "no part wall exists before the finishing contour");
    assert!(r.uncut_boundary_count > 0 && !uncut.is_empty());
    for w in uncut.iter() {
        let dd = rest_reach(&segs, start, stop, w.x, w.y, w.z_mid);
        assert!(dd < reach + band, "uncut-boundary wall at ({:.2}, {:.2}, {:.2}) is {:.3} mm from the rest of the path (reach {:.3})", w.x, w.y, w.z_mid, dd, reach);
    }
    assert!(r.max_wall_error_um == 0.0 && r.mean_wall_error_um == 0.0 && r.wall_ra_mean_um == 0.0, "no statistics from roughing walls: {:.1} µm", r.max_wall_error_um);
    assert!(r.warnings.iter().any(|w| w.contains("최종 벽 없음")), "{:#?}", r.warnings);
    assert_eq!(r.quality_loss.final_points, 0);
    assert!(r.seg_walls.is_empty());
    let sds = endmill_model::sds::SdsStore::open(&std::env::temp_dir().join(format!("endmill-uncut-new-sds-{}", std::process::id())));
    let log = endmill_model::wearcomp::analyze_job(&p, &ctx, &r, None, &sds);
    let audit = endmill_model::simaudit::build(&p, &ctx, &r, &log, None, "pocket_zigzag");
    assert_eq!(audit.tool_life.uncut_boundary_points, r.uncut_boundary_count);
}

#[test]
fn mid_path_tool_changes_are_recorded_as_separate_tool_instances() {
    let p = with_policy("인코넬", ToolLifePolicy::ChangeTool);
    let dir = std::env::temp_dir().join(format!("endmill-eol-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = endmill_model::sds::SdsStore::open(&dir.join("sds"));
    let (mut ctx, _) = endmill_model::pipeline::calibrated_context(&p, Some(&sds));
    let (row, start, _) = store.prepare_tool(&p, &mut ctx).unwrap();
    let r = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
    let rep = r.1;
    if rep.tool_life.tool_changes == 0 || rep.tool_life.failed {
        return;
    }
    let log = endmill_model::wearcomp::analyze_job_with(&p, &ctx, &rep, None, &sds, Some(&start));
    let opts = endmill_model::wearcomp::CompOptions::default();
    let draft = endmill_model::wearcomp::evaluate(&p, &ctx, &rep, &log, &opts, None);
    let dec = endmill_model::wearcomp::consult(&draft, None, None, false);
    let comp = endmill_model::wearcomp::finalize(&p, &ctx, &rep, &r.2, &log, &draft, &dec, &opts);
    let _ = endmill_model::wearcomp::record(&mut sds, &p, &log, &comp, None);
    let mut row = row;
    let em = row.endmill_id;
    let out = endmill_model::store::jobs::record_job(
        &store.rdb,
        Some(&mut row),
        &endmill_model::store::jobs::JobInput {
            project_id: store.project_id,
            profile: &p,
            ctx: &ctx,
            pattern: "pocket_zigzag",
            stamp: "EOL1",
            parent_job_id: None,
            endmill_id: em,
            workpiece_id: None,
            report: &rep,
            log: &log,
            comp: &comp,
            replan: false,
            summary: serde_json::json!({}),
        },
    )
    .unwrap();
    for l in out.lines.iter() {
        println!("[EOL] {}", l);
    }
    let tools = store.rdb.list_tools(true, 20).unwrap();
    // 경로 중 교체(수명 판정이든 치핑 감지든)로 빠진 공구는 모두 은퇴로 남는다
    let retired = tools.iter().filter(|t| t.retired_at.is_some() && t.retire_reason.contains("예비 공구로 교체")).count();
    assert!(retired as u32 >= rep.tool_life.tool_changes, "each worn-out tool is retired: {} of {}", retired, rep.tool_life.tool_changes);
    let active = store.rdb.active_tool(&row.slot_key).unwrap().expect("a sister tool is active");
    assert!(active.vb_band_mm < CutContext::from_profile(&p).vb_limit_mm(), "the active instance holds only the last tool's wear: {:.3}", active.vb_band_mm);
    assert!((active.cut_min - rep.tool_life.last_tool_cut_min).abs() < 1e-6);
}

#[test]
fn a_diamond_coated_tool_on_steel_wears_at_the_substrate_rate_once_the_coating_is_gone() {
    // 2026-10-18 앱 로그: CFRP 용 Ø10 6날 CVD 다이아몬드(10 µm) 공구를 탄소강·Ti·58 HRC 에 대면 VB 가 1,191~33,260 µm/min 로 자랐다.
    // 철·Ti 의 탄소 용해는 코팅이 있는 동안의 이야기다 — 랜드가 코팅을 뚫으면 노출된 몫만큼 무코팅 초경 마모율로 옮겨 가야 한다
    let tool = EndMillMockupSetting::builtin_tools().into_iter().find(|t| t.coating_name.as_deref() == Some("Diamond")).expect("diamond tool");
    let uc = pairmatrix::use_cases().into_iter().find(|u| u.key == "carbon_steel").expect("carbon steel use case");
    let p = pairmatrix::cell_profile(&tool, &uc);
    let fresh = CutContext::from_profile(&p);
    let strip_mm = endmill_model::toolwear::coating_strip_um(&fresh.tool) / 1000.0;
    assert!(strip_mm > 0.02 && strip_mm < 0.1, "10 µm diamond at the tool's clearance: {:.3} mm", strip_mm);
    // 새 공구로는 이 경로(약 0.3 분)에서 코팅을 다 못 뚫는다 (흑연화 개시 700 °C 아래 계면 약 590 °C).
    // 그래서 이미 뚫린 랜드(띠 폭의 3배 — 노출 몫 2/3)에서 시작해, 같은 랜드의 무코팅 공구와 성장 속도를 견준다
    let vb0 = 3.0 * strip_mm;
    let run = |prof: &MachiningProfile| {
        let mut ctx = CutContext::from_profile(prof);
        ctx.tool.flank_wear_mm = vb0;
        endmill_model::viewport::build_report_prog(prof, "rect_profile", &ctx, &mut |_| true).unwrap().1
    };
    let r = run(&p);
    print_events("다이아몬드 공구 · 탄소강 (뚫린 랜드에서 시작)", &r);
    let mut bare = p.clone();
    bare.endmill_setting.coating_name = None;
    let rb = run(&bare);
    print_events("같은 공구 무코팅 · 탄소강", &rb);
    assert!(r.tool_life.events.iter().any(|e| e.kind == "coating_through"), "the break-through is logged");
    let growth = |x: &endmill_model::loadsim::LoadSimReport| (x.vb_end_mm - vb0) * 1000.0 / x.cut_time_min.max(1e-9);
    let (g_dia, g_bare) = (growth(&r), growth(&rb));
    let worst = r.wear_log.iter().map(|b| b.vb_rate_um_min).fold(0.0f64, f64::max);
    println!("[EOL] 코팅 관통 랜드 VB {:.3} mm 에서 평균 VB 성장 다이아몬드 {:.1} · 무코팅 {:.1} µm/min (비 {:.2}) · 최대 {:.1} µm/min", vb0, g_dia, g_bare, g_dia / g_bare.max(1e-9), worst);
    assert!(worst < 400.0, "after break-through the land grows at a carbide-in-steel rate, not mm/min: {:.0} µm/min", worst);
    // 무코팅 초경의 크기 비교: 건식 선삭 SAE 1045 · 무코팅 K10 · f 0.2 에서 125 m/min 약 11, 150 m/min 14~17 µm/min 뒤 급증 (연속 절삭,
    // Kümmel 외 2013 Adv. Tribol. 519686) — 밀링은 날이 깎는 시간 비(이 칸 ae 0.3D 에서 약 0.18)만큼 작아진다.
    // 남은 코팅 몫(1/3)이 조화 평균으로 섞여 무코팅보다 조금 빠를 뿐이다 (코팅만 남았다면 이 비는 수 배)
    assert!(g_bare > 0.0 && g_dia / g_bare > 0.8 && g_dia / g_bare < 2.0, "diamond {:.1} vs bare {:.1} µm/min", g_dia, g_bare);
}

// ---------------------------------------------------------------- N:N 행렬

fn wp(m: WorkpieceMaterial) -> physics::WorkpieceProps {
    physics::WorkpieceProps::of(&m)
}

#[test]
fn application_fit_reproduces_the_research_anchor_cases() {
    // (1) 38 HRC 강: 날카로운 날(4 µm) 대 호닝(25 µm) — 수명이 크게 차이 (문헌 약 8배)
    let p = preset("탄소강");
    let ctx = CutContext::from_profile(&p);
    let mut sharp = ctx.tool.clone();
    sharp.edge_radius_um = 4.0;
    let mut honed = ctx.tool.clone();
    honed.edge_radius_um = 25.0;
    let w38 = wp(WorkpieceMaterial::AlloySteel { hardness_hrc: 38 });
    let fs = appfit::evaluate(&sharp, &w38, false);
    let fh = appfit::evaluate(&honed, &w38, false);
    assert!(fs.wear_factor * fs.chip_factor.sqrt() > 2.0 * fh.wear_factor * fh.chip_factor.sqrt(), "sharp {:?} vs honed {:?}", (fs.wear_factor, fs.chip_factor), (fh.wear_factor, fh.chip_factor));
    // (2) +8° 경사·날카로운 날로 56~58 HRC: 첫 패스 파손급 치핑
    let mut pos = ctx.tool.clone();
    pos.rake_deg = 8.0;
    pos.edge_radius_um = 4.0;
    let f58 = appfit::evaluate(&pos, &wp(WorkpieceMaterial::AlloySteel { hardness_hrc: 58 }), false);
    // 지수식으로는 ×수천 — 상한(×100, 첫 진입에서 날이 깨지는 수준)에 걸리고 그 사실이 메모로 남는다
    assert!(f58.chip_factor >= appfit::CHIP_FACTOR_MAX - 1e-9 && f58.verdict == 2, "{}", f58.summary());
    assert!(f58.notes.iter().any(|n| n.contains("상한")), "the cap is reported: {:?}", f58.notes);
    // (3) Ti 여유각 8° 는 10° 보다 치핑·마모가 크다
    let ti = wp(WorkpieceMaterial::Titanium);
    let mut c8 = ctx.tool.clone();
    c8.clearance_deg = 8.0;
    c8.rake_deg = 10.0;
    c8.edge_radius_um = 12.0;
    let mut c10 = c8.clone();
    c10.clearance_deg = 10.0;
    let a8 = appfit::evaluate(&c8, &ti, false);
    let a10 = appfit::evaluate(&c10, &ti, false);
    assert!(a8.chip_factor > a10.chip_factor && a8.wear_factor > a10.wear_factor);
    // (4) HSS 로 58 HRC 는 파손급
    let mut hss = ctx.tool.clone();
    hss.substrate = "hss".into();
    assert_eq!(appfit::evaluate(&hss, &wp(WorkpieceMaterial::AlloySteel { hardness_hrc: 58 }), false).verdict, 2);
    // (5) 알루미늄 슬롯에 6날은 칩 막힘
    let d6 = CutContext::from_profile(&preset("CFRP")).tool;
    let al = wp(WorkpieceMaterial::Aluminum);
    assert!(appfit::evaluate_with(&d6, &al, true, 1.0).evac_risk > appfit::evaluate(&d6, &al, false).evac_risk);
}

#[test]
fn every_built_in_preset_tool_fits_its_own_material() {
    for p in MachiningPreset::builtin() {
        let prof = MachiningProfile::from_preset(&p, &p.name);
        let ctx = CutContext::from_profile(&prof);
        let fit = appfit::evaluate(&ctx.tool, &ctx.wp, false);
        println!("[MATRIX] 프리셋 {} — {}", p.name, fit.summary());
        assert!(fit.verdict <= 1, "{}: {}", p.name, fit.summary());
        assert!(fit.wear_factor < 1.2, "{}: the intended pair must not change its calibrated wear: ×{:.3}", p.name, fit.wear_factor);
    }
}

fn save_matrix(cells: &[MatrixCell], tag: &str) {
    let dir = std::env::var("ENDMILL_MATRIX_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("endmill-matrix"));
    let _ = std::fs::create_dir_all(&dir);
    let csv = dir.join(format!("pair_matrix_{}.csv", tag));
    let json = dir.join(format!("pair_matrix_{}.json", tag));
    let _ = std::fs::write(&csv, pairmatrix::csv(cells));
    let _ = std::fs::write(&json, serde_json::to_string_pretty(cells).unwrap_or_default());
    println!("[MATRIX] 저장: {} · {}", csv.display(), json.display());
}

#[test]
fn tool_material_matrix_ranks_like_practice() {
    let cells = pairmatrix::run(Some("rect_profile"));
    for l in pairmatrix::lines(&cells) {
        println!("{}", l);
    }
    save_matrix(&cells, "rect_profile");
    assert_eq!(cells.len(), 64);
    for c in cells.iter() {
        assert!(c.life_min.is_finite() && c.life_min > 0.0, "{} × {}: life {}", c.use_case, c.tool, c.life_min);
        assert!(c.interface_c.is_finite() && c.ra_um.is_finite() && c.fit_score.is_finite());
    }
    // 물리 타당성 (`pairmatrix::check`): 계산 결함 0 건. 앱에서 공구를 그대로 두고 소재만 바꿔 본 조합(2026-10-18 로그 —
    // CFRP 용 다이아몬드 공구를 58 HRC·Ti·탄소강·알루미늄에)이 모두 이 64칸 안에 있다
    let issues = pairmatrix::check(&cells);
    for l in pairmatrix::check_lines("rect_profile", &cells, &issues) {
        println!("{}", l);
    }
    let hard: Vec<_> = issues.iter().filter(|i| i.severity >= 2).collect();
    assert!(hard.is_empty(), "calculation defects in the N:N matrix: {:#?}", hard);
    let cell = |uc: &str, model: &str| cells.iter().find(|c| c.use_case == uc && c.tool_model == model).expect("cell");
    // 의도된 짝은 부적합이 아니고, 경로에서 파손되지 않는다
    for c in cells.iter().filter(|c| c.intended) {
        assert!(c.verdict <= 1, "{} × {}: {:?}", c.use_case, c.tool, c.reasons);
        assert!(c.path_failure.is_empty(), "{} × {} broke: {}", c.use_case, c.tool, c.path_failure);
    }
    // 다이아몬드 코팅은 철계·니켈계에서 화학 궁합이 나쁘다 (탄소 확산)
    for uc in ["carbon_steel", "alloy_steel_30", "stainless", "inconel", "hardened_58"] {
        assert!(cell(uc, "BUILTIN-DC10F6").compat_severity >= 2, "diamond on {}", uc);
    }
    // 알루미늄용 날카로운 고경사 공구(DLC Ø8, TiAlN Ø6)로 58 HRC 는 부적합 (치핑)
    for m in ["BUILTIN-DL8F3", "YG-1 AL-2F"] {
        let c = cell("hardened_58", m);
        assert_eq!(c.verdict, 2, "{} on 58 HRC: {:?}", m, c.reasons);
        assert!(c.fit_chip > 20.0);
    }
    // 고경도용 음의 경사 볼은 알루미늄에서 무뎌 구성인선 (적합 아님)
    let hb_al = cell("aluminum", "BUILTIN-HB6N18");
    assert!(hb_al.fit_bue > 1.5 && hb_al.verdict >= 1, "{:?}", hb_al.reasons);
    // CFRP 는 다이아몬드 코팅 공구가 가장 오래 간다
    let cf_best = cells.iter().filter(|c| c.use_case == "cfrp").max_by(|a, b| a.life_min.partial_cmp(&b.life_min).unwrap()).unwrap();
    assert_eq!(cf_best.tool_model, "BUILTIN-DC10F6", "best on CFRP: {}", cf_best.tool);
    // 고경도에서는 고경도용 볼이 알루미늄용 공구들보다 순위가 높다
    let hb = cell("hardened_58", "BUILTIN-HB6N18");
    assert!(hb.rank_in_material < cell("hardened_58", "BUILTIN-DL8F3").rank_in_material && hb.rank_in_material < cell("hardened_58", "YG-1 AL-2F").rank_in_material);
    // 인코넬에서는 내열합금용 5날이 DLC 알루미늄 공구보다 오래 간다
    assert!(cell("inconel", "BUILTIN-AT12F5").life_min > cell("inconel", "BUILTIN-DL8F3").life_min);
    // 의도된 짝은 그 소재에서 상위권 (8개 중 4위 안)
    for c in cells.iter().filter(|c| c.intended) {
        assert!(c.rank_in_material <= 4, "{} × {} ranks {}", c.use_case, c.tool, c.rank_in_material);
    }
}

// ---------------------------------------------------------------- 포켓 벽 오차 (얇은 벽 하중·탄성 복원, 슬롯형 이동)

/// 세그먼트가 Y 방향 직선 이동인가 (지그재그 포켓의 패스 끝 피치 이동)
fn y_move(segs: &[endmill_model::gcode::ToolPathSegment], seg: usize) -> bool {
    let prev = if seg > 0 { segs[seg - 1].end_point() } else { (0.0, 0.0, 0.0) };
    let e = segs[seg.min(segs.len() - 1)].end_point();
    (e.0 - prev.0).abs() < 1e-6 && (e.1 - prev.1).abs() > 1e-6 && (e.2 - prev.2).abs() < 1e-6
}

/// 2026-10-19 개편 전의 합성 지그재그 포켓 — 직선 플런지로 ap 전체, 스톡 가장자리 1 mm 스킨 (여유 D/2 + 1), 줄 끝 피치 이동 ×0.7 이송, 정삭 없음.
/// 앱 기본 경로(`gcode::pocket_plan`)는 이제 그런 스킨을 남기지 않으므로, 얇은 벽 하중 모델과 미절삭 경계 판정은 이 고정 경로가 만든 스킨으로 계속 확인한다
fn legacy_zigzag(p: &MachiningProfile) -> Vec<ToolPathSegment> {
    use ToolPathSegment as S;
    let feed = p.conditions.feed_rate_mm_min;
    let cut_z = -p.conditions.axial_doc_mm;
    let safe_z = cut_z + p.endmill_setting.loc_mm + 10.0;
    let (w, h, dia) = (p.workpiece_setup.width_mm, p.workpiece_setup.height_mm, p.endmill_setting.diameter_mm);
    let step = endmill_model::gcode::pattern_stepover(p, 0.2, 0.6);
    let margin = dia / 2.0 + 1.0;
    let (inner_w, inner_h) = (w - 2.0 * margin, h - 2.0 * margin);
    let mut segs = vec![
        S::Rapid { x: 0.0, y: 0.0, z: safe_z },
        S::Rapid { x: margin, y: margin, z: safe_z },
        S::Plunge { x: margin, y: margin, z_start: safe_z, z_end: cut_z, feed: feed * 0.4 },
    ];
    let (mut y, mut dir) = (margin, 1.0f64);
    while y < margin + inner_h {
        let x_end = if dir > 0.0 { margin + inner_w } else { margin };
        segs.push(S::Linear { x: x_end, y, z: cut_z, feed });
        y += step;
        if y < margin + inner_h {
            segs.push(S::Linear { x: x_end, y, z: cut_z, feed: feed * 0.7 });
        }
        dir = -dir;
    }
    segs.push(S::Rapid { x: 0.0, y: 0.0, z: safe_z });
    endmill_model::gcode::GCodeGenerator::insert_safe_retracts(segs, safe_z)
}

/// 옛 지그재그 고정 경로를 뷰포트와 같은 격자로 시뮬레이션
fn legacy_report(p: &MachiningProfile) -> (CutContext, endmill_model::loadsim::LoadSimReport, Vec<ToolPathSegment>) {
    let ctx = CutContext::from_profile(p);
    let segs = legacy_zigzag(p);
    let r = endmill_model::loadsim::simulate_segments_prog(p, &segs, "pocket_zigzag_legacy", &ctx, endmill_model::viewport::VIEWPORT_MAX_CELLS, &mut |_| true).unwrap();
    (ctx, r, segs)
}

#[test]
fn pocket_skin_carries_only_its_own_chip_force_and_springs_back_within_its_collapse_load() {
    use endmill_model::loadsim::WallErrorSample;
    // 2026-10-19 품질 저하 보고: 인코넬 프리셋 pocket_zigzag 의 최종 벽 오차 600~1,013 µm (정상 공구로도). 최악 점(12번째 패스 끝의 피치 이동,
    // 스톡 가장자리 0.92 mm 스킨, z −6) 1,012.7 = 공구 휨 501.2 + 얇은 벽 458.3 + 마모 45.7 + 소재 열 8.8 − 런아웃 1.3 − 공구 열 0.1.
    // 얇은 벽 458 µm 는 공구 앞 미절삭 띠를 폭 전체로 깎는 피치 이동의 법선 힘 6.9 kN 전체를 스킨에 걸어 나온 탄성 1.48 mm 를 ±500·t 로
    // 자른 값이었다 (뿌리 굽힘 12 GPa = 항복의 10배). 그 힘은 공구 앞 덩어리가 면내로 받고, 벽이 돌아오는 몫은 전소성 하중까지다.
    // 앱 기본 경로는 그 뒤 개편돼 스킨을 남기지 않으므로 그때의 경로(`legacy_zigzag`)로 본다.
    // 단속 절삭 열 축적(문헌 벤치마크 2라운드)을 넣은 뒤로는 그 ap 1·D 폭 전체 슬롯에서 공구가 첫 줄 17 s 만에 수명에 닿고 정적 과부하로 부러져
    // 스킨이 생기지 않는다 — 인코넬은 제조사 슬롯 상한(이 공구 급 Kennametal HARVI II TE 0.75·D, `WorkpieceProps::slot_ap_ratio`)으로 깎고,
    // 수명에 닿은 공구는 예비 공구로 바꿔(교체 정책) 포켓 끝까지 스킨을 만든다
    let run = |name: &str| legacy_report(&preset(name));
    let collapse_cap_ok = |ctx: &CutContext, w: &WallErrorSample, bulk_c: f64| {
        // 경로의 σy 는 그때 소재 온도에서 — 가장 찬(가장 단단한) 온도로 잰 상한을 넘지 않아야 한다
        let sy = ctx.wp.flow_stress(0.0, 1.0, ctx.env.ambient_c.min(bulk_c));
        w.workpiece_um.abs() <= 1.5 * sy / w.wall_sigma_per_n.max(1e-12) / w.wall_k_n_mm * 1000.0 + 1e-6
    };
    let inconel = {
        let mut p = with_policy("인코넬", ToolLifePolicy::ChangeTool);
        let slot = physics::WorkpieceProps::of(&p.workpiece_setup.effective_material()).slot_ap_ratio.expect("Ni alloys have vendor slot data");
        p.conditions.axial_doc_mm = slot * p.endmill_setting.diameter_mm;
        p
    };
    let (ctx, r, segs) = legacy_report(&inconel);
    assert!(!r.tool_life.unfinished && !r.tool_life.failed && r.tool_life.tools_used > 1, "spare tools finish the pocket: {}", r.tool_life.state);
    let finals: Vec<&WallErrorSample> = r.wall_errors.iter().filter(|w| w.final_wall).collect();
    let skin_steps: Vec<&&WallErrorSample> = finals.iter().filter(|w| w.wall_k_n_mm > 0.0 && y_move(&segs, w.seg)).collect();
    let worst = finals.iter().max_by(|a, b| a.total_um.abs().partial_cmp(&b.total_um.abs()).unwrap()).expect("final walls");
    println!(
        "[WALL] 인코넬 포켓 · 최종 벽 {}점 (스킨 피치 이동 {}점) · 최대 {:.1} µm (seg {} 공구 휨 {:.1} + 얇은 벽 {:.1} + 마모 {:.1}) · 얇은 벽 휨 최대 {:.1} µm",
        finals.len(),
        skin_steps.len(),
        r.max_wall_error_um,
        worst.seg,
        worst.deflection_um,
        worst.workpiece_um,
        worst.wear_um,
        r.max_workpiece_defl_um
    );
    for w in r.warnings.iter().filter(|w| w.contains("슬롯형") || w.contains("얇은 벽")) {
        println!("[WALL]    {}", w);
    }
    assert!(skin_steps.len() >= 50, "the stepover moves form the side skins: {}", skin_steps.len());
    assert!(
        skin_steps.iter().all(|w| w.workpiece_um == 0.0 && w.wall_stress_mpa == 0.0),
        "the stepover's chip force goes into the uncut stock ahead, not into the skin beside it: {:?}",
        skin_steps.iter().map(|w| (w.seg, w.workpiece_um)).filter(|x| x.1 != 0.0).take(5).collect::<Vec<_>>()
    );
    // 스킨 형상(두께·강성)은 남는다 — 그 스킨을 따라 깎는 2차 보정 예측이 쓴다
    assert!(skin_steps.iter().all(|w| w.wall_mm > 0.5 && w.wall_mm < 1.5 && w.wall_k_n_mm > 0.0));
    assert!(finals.iter().filter(|w| w.wall_k_n_mm > 0.0).all(|w| collapse_cap_ok(&ctx, w, r.bulk_c)));
    // 남는 최악 오차는 슬롯형 이동(ap 1·D 에서 법선 힘 6~7 kN 으로 휨 약 500 µm, 슬롯 상한 0.75·D 에서도 약 370 µm)의 공구 휨이다 — 공구 끝 강성
    // 13.8 N/µm 는 실측 대역이라 경로 쪽 문제였고, 그래서 기본 경로를 슬롯 ap 단계 절입 · 정삭 윤곽으로 바꿨다 (`docs/pocket-zigzag-path.md`)
    assert!(
        worst.workpiece_um.abs() < 1.0 && worst.deflection_um > 300.0 && worst.deflection_um > 0.8 * worst.total_um.abs(),
        "{:?}",
        (worst.deflection_um, worst.workpiece_um, worst.total_um)
    );
    assert!(r.max_wall_error_um < 650.0, "was 968.6 µm (Stop) with the skin term: {:.1}", r.max_wall_error_um);
    // 탄소강 프리셋: 끝까지 깎아 마지막 패스가 위쪽 스킨을 측면 절삭(벽 쪽 띠만)으로 만든다 — 그 벽은 지금처럼 힘을 받아 휘고(띠와 벽이 한 몸),
    // 피치 이동이 만든 옆 스킨은 휘지 않는다. 예전 최대 533 µm 중 324 µm 가 피치 이동의 스킨 휨이었다
    let (cs, rs, ss) = run("탄소강");
    let thin: Vec<&WallErrorSample> = rs.wall_errors.iter().filter(|w| w.final_wall && w.wall_k_n_mm > 0.0).collect();
    println!("[WALL] 탄소강 포켓 · 최종 얇은 벽 {}점 · 최대 벽 오차 {:.1} µm · 얇은 벽 휨 최대 {:.1} µm", thin.len(), rs.max_wall_error_um, rs.max_workpiece_defl_um);
    assert!(thin.iter().any(|w| !y_move(&ss, w.seg) && w.workpiece_um > 1.0), "side milling along a skin still bends it");
    assert!(thin.iter().filter(|w| y_move(&ss, w.seg)).all(|w| w.workpiece_um == 0.0));
    assert!(thin.iter().all(|w| collapse_cap_ok(&cs, w, rs.bulk_c)));
    assert!(rs.max_wall_error_um < 300.0, "{:.1}", rs.max_wall_error_um);
}
