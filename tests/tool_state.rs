use endmill_model::loadsim::LoadSimReport;
use endmill_model::physics::{self, CutContext, ThermalBody, ToolGeometry};
use endmill_model::profile::{CoolantConfig, CoolantMethod, MachiningPreset, MachiningProfile};
use endmill_model::sds::SdsStore;
use endmill_model::store::jobs::{self, JobInput, JobRecordOutcome, ToolInstanceRow};
use endmill_model::store::Store;
use endmill_model::toolwear::{self, AxialProfile, ToolStart, WearField};
use endmill_model::wearcomp::{self, CompAction, CompOptions, CompOutcome, EvalExtra};
use endmill_model::wearlog::{LifeForecast, WearLog};
use endmill_model::WorkpieceMaterial;
use std::path::PathBuf;

fn preset(name: &str) -> MachiningProfile {
    let p = MachiningPreset::builtin().into_iter().find(|p| p.name.contains(name)).expect("preset");
    MachiningProfile::from_preset(&p, &p.name)
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("endmill-toolstate-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tool_of(p: &MachiningProfile) -> ToolGeometry {
    CutContext::from_profile(p).tool
}

#[test]
fn wear_field_band_mean_tracks_vb_and_notch_peaks_at_the_depth_of_cut_line() {
    let mut f = WearField::new(20.0, 3);
    let axial = AxialProfile {
        notch: 0.4,
        notch_sigma_mm: 0.5,
        corner: 0.0,
        corner_z_mm: 0.0,
        corner_sigma_mm: 0.2,
    };
    f.deposit(6.0, 0.05, &axial, &[1.0, 0.5, 0.8]);
    assert!((f.band_vb(6.0) - 0.05).abs() < 1e-9, "{}", f.band_vb(6.0));
    assert!((f.flute_band_vb(1, 6.0) - 0.025).abs() < 1e-9);
    assert_eq!(f.worst_flute(6.0), 0);
    let k_notch = (5.9 / f.dz_mm) as usize;
    assert!(f.at(0, k_notch) > f.at(0, 2), "notch {} vs middle {}", f.at(0, k_notch), f.at(0, 2));
    assert!(f.max_vb() > f.band_vb(6.0));
    let k_above = (8.0 / f.dz_mm) as usize;
    assert_eq!(f.at(0, k_above), 0.0);
    assert!(f.band_vb(12.0) < 0.6 * f.band_vb(6.0));
    let mut g = f.clone();
    assert!(g.add(&f));
    assert!((g.band_vb(6.0) - 0.1).abs() < 1e-9);
    assert!(g.subtract(&f));
    assert!((g.band_vb(6.0) - 0.05).abs() < 1e-9);
    g.anchor_band(6.0, 0.08, &axial, &[1.0, 0.5, 0.8]);
    assert!((g.band_vb(6.0) - 0.08).abs() < 1e-9);
}

#[test]
fn partial_depth_bins_keep_the_band_mean_and_a_stable_notch_peak() {
    let axial = AxialProfile {
        notch: 0.4,
        notch_sigma_mm: 0.3,
        corner: 0.3,
        corner_z_mm: 0.0,
        corner_sigma_mm: 0.2,
    };
    let mut ratios = Vec::new();
    for ap in [0.3, 0.5, 0.9, 2.2, 6.0, 6.1, 6.25, 6.4, 13.7] {
        let mut f = WearField::new(25.0, 2);
        f.deposit(ap, 0.05, &axial, &[1.0, 1.0]);
        assert!((f.band_vb(ap) - 0.05).abs() < 1e-9, "ap {} band {}", ap, f.band_vb(ap));
        if ap >= 2.0 {
            ratios.push(f.max_vb() / f.band_vb(ap));
        }
    }
    let lo = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = ratios.iter().cloned().fold(0.0, f64::max);
    assert!(hi / lo < 1.12, "peak/band ratio depends on where ap falls in a bin: {:?}", ratios);
    let mut a = WearField::new(25.0, 3);
    a.deposit(6.0, 0.04, &axial, &[1.0, 1.0, 1.0]);
    let b = a.resampled(25.04, 3).unwrap();
    assert!(b.compatible(25.04, 3));
    assert!((b.band_vb(6.0) - a.band_vb(6.0)).abs() < 2e-3);
    let mut c = WearField::new(25.04, 3);
    assert!(!c.add(&a));
    assert!(c.add_any(&a));
    assert!(c.band_vb(6.0) > 0.03);
}

#[test]
fn flute_shares_follow_runout_and_chip_thickness() {
    let p = preset("탄소강");
    let mut t = tool_of(&p);
    t.runout_um = 0.0;
    assert!(toolwear::flute_shares(&t, 0.01, 0.25).iter().all(|s| (*s - 1.0).abs() < 1e-12));
    t.runout_um = 5.0;
    let thin = toolwear::flute_shares(&t, 0.005, 0.25);
    let thick = toolwear::flute_shares(&t, 0.08, 0.25);
    let spread = |v: &[f64]| v.iter().cloned().fold(1.0, f64::min);
    assert!(thin.iter().any(|s| (*s - 1.0).abs() < 1e-12) && thick.iter().any(|s| (*s - 1.0).abs() < 1e-12));
    assert!(spread(&thin) < spread(&thick), "{:?} vs {:?}", thin, thick);
}

#[test]
fn chatter_wear_factor_is_continuous_bounded_and_weighted_by_frf_source() {
    assert_eq!(toolwear::chatter_wear_factor(f64::INFINITY, false), 1.0);
    assert!(toolwear::chatter_wear_factor(5.0, true) < 1.0005);
    let near = toolwear::chatter_wear_factor(1.2, true);
    assert!(near > 1.0 && near < 1.15, "{}", near);
    let severe_measured = toolwear::chatter_wear_factor(0.07, true);
    let severe_estimated = toolwear::chatter_wear_factor(0.07, false);
    assert!((severe_measured - 46.0 / 15.0).abs() < 0.05, "{}", severe_measured);
    assert!(severe_estimated > 1.9 && severe_estimated < 2.1, "{}", severe_estimated);
    for measured in [true, false] {
        let a = toolwear::chatter_wear_factor(1.0001, measured);
        let b = toolwear::chatter_wear_factor(0.9999, measured);
        assert!((a - b).abs() < 2e-3, "jump at the stability limit {} vs {}", a, b);
        let p_a = toolwear::chatter_probability(1.0001, measured);
        let p_b = toolwear::chatter_probability(0.9999, measured);
        assert!((p_a - p_b).abs() < 1e-3);
    }
    assert!(toolwear::chatter_probability(1.05, true) > 0.3);
    let mut last = 1.0;
    for m in [3.0, 1.5, 1.1, 0.95, 0.8, 0.6, 0.4, 0.2, 0.1] {
        let f = toolwear::chatter_wear_factor(m, true);
        assert!(f >= last, "{} < {} at margin {}", f, last, m);
        assert!(f <= 46.0 / 15.0 + 1e-9);
        last = f;
    }
    let mild = toolwear::chatter_wear_factor(1.0 / 1.3, true);
    assert!(mild > 1.3 && mild < 1.7, "mild chatter factor {}", mild);
    let mut last_est = 1.0;
    for m in [3.0, 2.0, 1.6, 1.35, 1.2, 1.05, 1.0, 0.9, 0.6, 0.3, 0.1] {
        let est = toolwear::chatter_wear_factor(m, false);
        assert!(est >= last_est, "estimated-FRF factor must rise as the margin falls: {} < {} at {}", est, last_est, m);
        if m >= 0.9 {
            assert!(est >= toolwear::chatter_wear_factor(m, true) - 1e-12, "an estimated FRF must not look safer than a measured one at margin {}", m);
        }
        last_est = est;
    }
}

#[test]
fn a_field_from_an_older_bin_layout_is_resampled_instead_of_mixed() {
    let mut old = WearField::new(25.0, 4);
    old.nz = 40;
    old.dz_mm = 25.0 / 40.0;
    old.vb_mm = vec![0.0; 160];
    let mut inc = WearField::new(25.0, 4);
    inc.deposit(10.0, 0.05, &AxialProfile::default(), &[1.0, 0.5, 0.5, 1.0]);
    assert!(!old.compatible(25.0, 4));
    assert!(!old.add(&inc), "fields on different axial grids must not be added bin by bin");
    let mut conformed = old.resampled(25.0, 4).unwrap();
    assert!(conformed.add(&inc));
    let bands: Vec<f64> = (0..4).map(|f| conformed.flute_band_vb(f, 10.0)).collect();
    for (b, want) in bands.iter().zip([0.05, 0.025, 0.025, 0.05]) {
        assert!((b - want).abs() < 1e-9, "{:?}", bands);
    }
}

fn simulate(p: &MachiningProfile, ctx: &CutContext) -> LoadSimReport {
    endmill_model::viewport::build_report_prog(p, "pocket_zigzag", ctx, &mut |_| true).unwrap().1
}

#[test]
fn simulation_continues_from_stored_wear_without_repeating_break_in() {
    let p = preset("탄소강");
    let ctx = CutContext::from_profile(&p);
    let r1 = simulate(&p, &ctx);
    assert_eq!(r1.vb_start_mm, 0.0);
    assert!(r1.vb_end_mm > 0.0);
    let mut ctx2 = ctx.clone();
    ctx2.tool.flank_wear_mm = r1.vb_end_mm;
    let r2 = simulate(&p, &ctx2);
    assert!((r2.vb_start_mm - r1.vb_end_mm).abs() < 1e-12);
    assert!(r2.wear_age0_min > 0.0);
    assert!(r2.vb_end_mm > r1.vb_end_mm);
    let first = |r: &LoadSimReport| {
        let b = &r.wear_log;
        let k = (b.len() / 20).max(2);
        (b[k].vb_mm - r.vb_start_mm) / b[k].t_cut_s.max(1e-9)
    };
    assert!(first(&r2) < first(&r1), "worn tool must not repeat the break-in spike: {} vs {}", first(&r2), first(&r1));
    assert!(r1.warnings.iter().all(|w| !w.contains("누적 마모")));
    assert!(r2.warnings.iter().any(|w| w.contains("누적 마모")));
    assert!((r1.wear_field.band_vb(p.conditions.axial_doc_mm) - (r1.vb_end_mm - r1.vb_start_mm)).abs() < 0.02 * r1.vb_end_mm.max(1e-6) + 1e-6);
    assert!(!r1.regimes.is_empty());
    assert!(r1.regimes.iter().all(|g| g.t_cut_s > 0.0 && g.phi_ex_deg >= g.phi_st_deg));
    assert!((r1.growth_calib_mm + r1.growth_fixed_mm - (r1.vb_end_mm - r1.vb_start_mm)).abs() < 1e-9);
    let last = r1.last_cut.as_ref().unwrap();
    let firstc = r1.first_cut.as_ref().unwrap();
    assert_eq!(last.z.len(), firstc.z.len());
    assert!(last.z.iter().zip(firstc.z.iter()).all(|(l, f)| !l.is_finite() || !f.is_finite() || *l >= *f - 1e-3));
}

struct Tracked {
    ctx: CutContext,
    report: LoadSimReport,
    log: WearLog,
    comp: CompOutcome,
    start: ToolStart,
    segs: Vec<endmill_model::gcode::ToolPathSegment>,
}

fn tracked_job_with(store: &mut Store, sds: &mut SdsStore, p: &MachiningProfile, opts: &CompOptions, exclude_root: Option<i64>) -> (Tracked, ToolInstanceRow) {
    let (mut ctx, _) = endmill_model::pipeline::calibrated_context(p, Some(sds));
    let (row, start, _) = store.prepare_tool(p, &mut ctx).unwrap();
    let (_v, report, segs) = endmill_model::viewport::build_report_prog(p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
    let log = wearcomp::analyze_job_with(p, &ctx, &report, None, sds, Some(&start));
    let refs = jobs::reference_summary(&store.rdb, p, &ctx, &report.regimes, exclude_root, 8).unwrap();
    let extra = EvalExtra { tool: Some(start.clone()), refs: Some(refs), tool_replaced: false };
    let draft = wearcomp::evaluate_with(p, &ctx, &report, &log, opts, None, &extra);
    let dec = wearcomp::consult(&draft, None, None, false);
    let comp = wearcomp::finalize(p, &ctx, &report, &segs, &log, &draft, &dec, opts);
    (Tracked { ctx, report, log, comp, start, segs }, row)
}

fn tracked_job(store: &mut Store, sds: &mut SdsStore, p: &MachiningProfile, opts: &CompOptions) -> (Tracked, ToolInstanceRow) {
    tracked_job_with(store, sds, p, opts, None)
}

fn record(store: &Store, row: &mut ToolInstanceRow, p: &MachiningProfile, j: &Tracked, comp: &CompOutcome, stamp: &str, parent: Option<i64>, replan: bool) -> JobRecordOutcome {
    let em = row.endmill_id;
    jobs::record_job(
        &store.rdb,
        Some(row),
        &JobInput {
            project_id: store.project_id,
            profile: p,
            ctx: &j.ctx,
            pattern: "pocket_zigzag",
            stamp,
            parent_job_id: parent,
            endmill_id: em,
            workpiece_id: None,
            report: &j.report,
            log: &j.log,
            comp,
            replan,
            summary: serde_json::json!({ "stamp": stamp }),
        },
    )
    .unwrap()
}

fn replan_comp(p: &MachiningProfile, j: &Tracked, measured_um: f64) -> CompOutcome {
    let mut opts = CompOptions::default();
    opts.measured_radial_um = Some(measured_um);
    let extra = EvalExtra { tool: Some(j.start.clone()), refs: None, tool_replaced: false };
    let draft = wearcomp::evaluate_with(p, &j.ctx, &j.report, &j.log, &opts, None, &extra);
    let dec = wearcomp::consult(&draft, None, None, false);
    wearcomp::finalize(p, &j.ctx, &j.report, &j.segs, &j.log, &draft, &dec, &opts)
}

#[test]
fn tool_instance_accumulates_across_jobs_records_passes_and_resets() {
    let p = preset("탄소강");
    let dir = temp_dir("store");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(row.generation, 1);
    assert!(j1.start.is_fresh());
    let out1 = record(&store, &mut row, &p, &j1, &j1.comp, "T1", None, false);
    let t1 = out1.tool.clone().unwrap();
    assert_eq!(row.id, t1.id);
    assert_eq!(t1.jobs, 1);
    assert!((t1.vb_band_mm - j1.report.vb_end_mm).abs() < 0.01 * j1.report.vb_end_mm + 1e-6, "stored {} vs simulated {}", t1.vb_band_mm, j1.report.vb_end_mm);
    assert!(t1.cut_min >= j1.report.cut_time_min - 1e-9);
    assert!(out1.records > 0);
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(row2.id, t1.id);
    assert!((j2.report.vb_start_mm - t1.vb_band_mm).abs() < 1e-9, "{} vs {}", j2.report.vb_start_mm, t1.vb_band_mm);
    assert!((j2.report.wear_age0_min - t1.cut_min).abs() < 1e-9, "break-in continues from the tool's real cutting minutes: {} vs {}", j2.report.wear_age0_min, t1.cut_min);
    assert!(j2.comp.draft.refs.as_ref().map(|r| r.n == 1).unwrap_or(false), "second job must find the first job (one reference per job)");
    assert!(j2.log.life.as_ref().map(|l| l.history_n > j1.log.life.as_ref().map(|x| x.history_n).unwrap_or(0)).unwrap_or(false));
    assert!(j2.comp.draft.state_text.contains("reused"));
    let out2 = record(&store, &mut row2, &p, &j2, &j2.comp, "T2", None, false);
    let t2 = out2.tool.clone().unwrap();
    assert_eq!(t2.jobs, 2);
    assert!(t2.vb_band_mm > t1.vb_band_mm);
    assert!(t2.history.len() > t1.history.len());
    assert!(t2.life_used_max >= t1.life_used_max);
    let list = store.rdb.jobs(Some(store.project_id), None, 10).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|j| j.passes.len() == 2 && j.passes[0].pass == 1 && j.passes[1].pass == 2));
    let detail = store.rdb.job_detail(out2.job_id).unwrap().unwrap();
    assert!(!detail.records.is_empty() && detail.events.iter().any(|e| e.kind == "pass1"));
    let tan = j2.ctx.tool.clearance_deg.to_radians().tan();
    let measured = t2.vb_band_mm * 1000.0 * tan * 1.3;
    let comp = replan_comp(&p, &j2, measured);
    let mut row3 = store.rdb.active_tool(&t2.slot_key).unwrap().unwrap();
    let out3 = record(&store, &mut row3, &p, &j2, &comp, "T2R", Some(out2.job_id), true);
    let t3 = out3.tool.clone().unwrap();
    assert_eq!(t3.jobs, 2, "a replan must not count pass one twice");
    assert_eq!(t3.last_measured_um, Some(measured));
    assert_eq!(t3.anchor_um, Some(measured));
    assert_eq!(out3.root_job_id, out2.job_id);
    if !comp.decision.final_action.cuts() {
        assert!((t3.vb_band_mm - measured / 1000.0 / tan).abs() < 1e-6, "{} vs {}", t3.vb_band_mm, measured / 1000.0 / tan);
    }
    let parent_records = store.rdb.cut_records(out2.job_id).unwrap();
    assert!(parent_records.iter().filter(|r| r.pass == 1).all(|r| r.measured_ratio.is_some()));
    let replan_detail = store.rdb.job_detail(out3.job_id).unwrap().unwrap();
    assert_eq!(replan_detail.job.parent_job_id, Some(out2.job_id));
    assert_eq!(replan_detail.job.root_job_id, Some(out2.job_id));
    assert!(replan_detail.job.passes.iter().any(|p| p.pass == 1 && p.status == "reused" && !p.committed));
    let (fresh, _) = store.reset_tool_for(&p, "test reset").unwrap();
    assert_eq!(fresh.generation, 2);
    assert_eq!(fresh.vb_band_mm, 0.0);
    assert_eq!(fresh.jobs, 0);
    assert_eq!(fresh.thermal_damage, 0.0);
    assert!(store.rdb.tool_by_id(t3.id).unwrap().unwrap().retired_at.is_some());
    assert!(store.rdb.wear_events(t3.id, 50).unwrap().iter().any(|e| e.kind == "reset"));
    let (j4, _) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!(j4.start.is_fresh() && j4.report.vb_start_mm == 0.0);
    let mut stale = fresh.clone();
    stale.version += 7;
    assert!(store.rdb.save_tool(&stale).is_err(), "a stale snapshot must not overwrite the stored tool");
}

#[test]
fn second_pass_wear_is_charged_reverted_on_replan_and_chained_replans_keep_the_root() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("split");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let action = j1.comp.decision.final_action;
    assert!(action.cuts() && action != CompAction::ToolChange, "aluminium roughing preset is expected to cut a second pass, got {:?}", action);
    let out1 = record(&store, &mut row, &p, &j1, &j1.comp, "A1", None, false);
    let t1 = out1.tool.clone().unwrap();
    let p1_min = j1.report.cut_time_min;
    let p2_min = j1.comp.cut_time_s / 60.0;
    assert!((t1.cut_min - (p1_min + p2_min)).abs() < 1e-9);
    let pass2 = store.rdb.job_passes(out1.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert!(pass2.committed && pass2.status == "planned");
    let tan = j1.ctx.tool.clearance_deg.to_radians().tan();
    let m1 = j1.report.vb_end_mm * 1000.0 * tan * 1.8;
    let c1 = replan_comp(&p, &j1, m1);
    let mut r1 = store.rdb.active_tool(&t1.slot_key).unwrap().unwrap();
    let o1 = record(&store, &mut r1, &p, &j1, &c1, "A1R1", Some(out1.job_id), true);
    let a1 = o1.tool.clone().unwrap();
    let p2b = if c1.decision.final_action.cuts() && c1.decision.final_action != CompAction::ToolChange { c1.cut_time_s / 60.0 } else { 0.0 };
    assert!((a1.cut_min - (p1_min + p2b)).abs() < 1e-9, "replan must revert the old second pass: {} vs {}", a1.cut_min, p1_min + p2b);
    assert_eq!(a1.jobs, 1);
    let replaced = store.rdb.job_passes(out1.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(replaced.status, "replaced");
    assert!(!replaced.committed);
    let ratio1 = store.rdb.cut_records(out1.job_id).unwrap().iter().find(|r| r.pass == 1).and_then(|r| r.measured_ratio);
    assert!(ratio1.is_some());
    let m2 = m1 * 1.1;
    let c2 = replan_comp(&p, &j1, m2);
    let mut r2 = store.rdb.active_tool(&t1.slot_key).unwrap().unwrap();
    let o2 = record(&store, &mut r2, &p, &j1, &c2, "A1R2", Some(o1.job_id), true);
    assert_eq!(o2.root_job_id, out1.job_id, "a replan of a replan keeps the original job as root");
    let a2 = o2.tool.clone().unwrap();
    assert_eq!(a2.jobs, 1);
    let ratio2 = store.rdb.cut_records(out1.job_id).unwrap().iter().find(|r| r.pass == 1).and_then(|r| r.measured_ratio);
    assert!(ratio2.is_some() && ratio2 != ratio1, "the newest measurement updates the root records");
    let refs_self = jobs::reference_summary(&store.rdb, &p, &j1.ctx, &j1.report.regimes, Some(out1.job_id), 8).unwrap();
    assert_eq!(refs_self.n, 0, "a replan must not reference its own part");
    let refs_other = jobs::reference_summary(&store.rdb, &p, &j1.ctx, &j1.report.regimes, None, 8).unwrap();
    assert_eq!(refs_other.n, 1, "records of one part count as one reference job, got {:?}", refs_other.refs.iter().map(|r| r.job_id).collect::<Vec<_>>());
    assert_eq!(refs_other.n_measured, 1);
    assert_eq!(refs_other.conservative_factor(), 1.0, "one measured job must not tighten the plan");
}

#[test]
fn tool_change_waits_for_reset_then_charges_the_new_tool() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("change");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let _ = record(&store, &mut row, &p, &j1, &j1.comp, "C1", None, false);
    let limit = j1.ctx.vb_limit_mm();
    let mut worn = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    worn.thermal_damage = 1.05;
    worn.version = store.rdb.save_tool(&worn).unwrap();
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!((j2.start.thermal_damage - 1.05).abs() < 1e-12);
    let gate = j2.comp.draft.gates.iter().find(|g| g.id == "thermal_fatigue").expect("thermal fatigue gate");
    assert_eq!(gate.severity, 2);
    assert!(gate.blocks.contains(&CompAction::Split) && !gate.blocks.contains(&CompAction::ToolChange));
    assert_eq!(j2.comp.decision.final_action, CompAction::ToolChange, "{:?}", j2.comp.draft.reasons);
    assert!(j2.comp.vb_after_mm < 0.5 * limit, "the new tool starts its second pass fresh: {}", j2.comp.vb_after_mm);
    assert!(j2.comp.samples.first().map(|s| s.vb_mm < 1e-9).unwrap_or(false), "second-pass samples must start from a new edge");
    assert!(j2.comp.vb_after_mm < j2.comp.draft.vb_now_mm, "{} vs worn {}", j2.comp.vb_after_mm, j2.comp.draft.vb_now_mm);
    let before = row2.clone();
    let out2 = record(&store, &mut row2, &p, &j2, &j2.comp, "C2", None, false);
    let t2 = out2.tool.clone().unwrap();
    assert_eq!(t2.pending_job_id, Some(out2.job_id));
    assert!((t2.cut_min - (before.cut_min + j2.report.cut_time_min)).abs() < 1e-9, "the worn tool is charged with pass one only");
    let p2 = store.rdb.job_passes(out2.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(p2.status, "awaiting_tool");
    assert!(!p2.committed);
    let (fresh, lines) = store.reset_tool_for(&p, "새 공구 장착").unwrap();
    assert_eq!(fresh.generation, t2.generation + 1);
    assert_eq!(fresh.jobs, 1);
    assert!(fresh.thermal_damage < 0.5, "the new tool carries only its own second-pass fatigue: {}", fresh.thermal_damage);
    assert!((fresh.cut_min - j2.comp.cut_time_s / 60.0).abs() < 1e-9);
    assert!(fresh.vb_band_mm > 0.0 && fresh.vb_band_mm < 0.5 * limit);
    assert!(lines.iter().any(|l| l.contains("새 공구")));
    let p2b = store.rdb.job_passes(out2.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(p2b.status, "done_new_tool");
    assert!(p2b.committed);
    assert!(store.rdb.wear_events(fresh.id, 10).unwrap().iter().any(|e| e.kind == "pass2" && e.job_id == Some(out2.job_id)));
}

#[test]
fn presets_keep_separate_tools_and_spec_changes_retire_the_old_one() {
    let dir = temp_dir("slots");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let a = preset("탄소강");
    let mut b = a.clone();
    b.name = "두 번째 탄소강 프리셋".into();
    let ta = tool_of(&a);
    let read = store.tool_for(&a).unwrap();
    assert_eq!(read.id, 0, "reading the tool state must not create rows");
    assert!(store.rdb.list_tools(true, 50).unwrap().is_empty());
    let mut ca = CutContext::from_profile(&a);
    let mut cb = CutContext::from_profile(&b);
    let (ra, _, _) = store.prepare_tool(&a, &mut ca).unwrap();
    let (rb, _, _) = store.prepare_tool(&b, &mut cb).unwrap();
    assert_ne!(ra.id, rb.id, "two presets with the same end mill geometry are two tool slots");
    let (rb2, _) = store.reset_tool_for(&b, "b only").unwrap();
    assert_eq!(store.rdb.active_tool(&ra.slot_key).unwrap().unwrap().id, ra.id, "resetting one preset leaves the other");
    assert_eq!(rb2.generation, 2);
    let mut a_loc = a.clone();
    a_loc.endmill_setting.loc_mm = ta.loc_mm + 0.04;
    let mut c_loc = CutContext::from_profile(&a_loc);
    let (r_loc, _, note) = store.prepare_tool(&a_loc, &mut c_loc).unwrap();
    assert_eq!(r_loc.id, ra.id);
    assert!(note.is_none());
    let mut a_d = a.clone();
    a_d.endmill_setting.diameter_mm += 2.0;
    let mut c_d = CutContext::from_profile(&a_d);
    let (r_d, _, note_d) = store.prepare_tool(&a_d, &mut c_d).unwrap();
    assert_eq!(r_d.id, 0, "a spec change only stages a new tool until a job is recorded");
    assert!(note_d.is_some());
    assert_eq!(store.rdb.active_tool(&ra.slot_key).unwrap().unwrap().id, ra.id, "starting or cancelling a job with an edited spec keeps the worn tool");
    let (back, _, note_back) = store.prepare_tool(&a, &mut CutContext::from_profile(&a)).unwrap();
    assert_eq!(back.id, ra.id, "editing the spec back finds the same tool again");
    assert!(note_back.is_none());
    let mut sds = SdsStore::open(&dir.join("sds"));
    let (jd, mut row_d) = tracked_job(&mut store, &mut sds, &a_d, &CompOptions::default());
    assert_eq!(row_d.id, 0);
    let out = record(&store, &mut row_d, &a_d, &jd, &jd.comp, "D1", None, false);
    let nd = out.tool.unwrap();
    assert!(nd.id > 0 && nd.id != ra.id && nd.generation == ra.generation + 1, "{:?}", (nd.id, nd.generation));
    assert_eq!(row_d.id, nd.id);
    assert_eq!(nd.jobs, 1);
    assert!(store.rdb.tool_by_id(ra.id).unwrap().unwrap().retired_at.is_some());
    assert!(store.rdb.wear_events(ra.id, 10).unwrap().iter().any(|e| e.kind == "geometry_change"));
}

#[test]
fn a_spec_change_while_a_tool_change_is_pending_charges_that_second_pass_to_the_new_tool() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("pendingspec");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let _ = record(&store, &mut row, &p, &j1, &j1.comp, "S1", None, false);
    let mut worn = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    worn.thermal_damage = 1.05;
    worn.version = store.rdb.save_tool(&worn).unwrap();
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(j2.comp.decision.final_action, CompAction::ToolChange);
    let o2 = record(&store, &mut row2, &p, &j2, &j2.comp, "S2", None, false);
    assert_eq!(o2.tool.as_ref().unwrap().pending_job_id, Some(o2.job_id));
    let mut q = p.clone();
    q.endmill_setting.helix_angle_deg += 5.0;
    let (j3, mut row3) = tracked_job(&mut store, &mut sds, &q, &opts);
    assert_eq!(row3.id, 0);
    let o3 = record(&store, &mut row3, &q, &j3, &j3.comp, "S3", None, false);
    let n = o3.tool.unwrap();
    let p2 = store.rdb.job_passes(o2.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(p2.status, "done_new_tool", "the pending second pass must not be left waiting forever");
    assert!(p2.committed);
    assert!(store.rdb.wear_events(n.id, 20).unwrap().iter().any(|e| e.kind == "pass2" && e.job_id == Some(o2.job_id)));
    let own = j3.report.cut_time_min + if j3.comp.decision.final_action.cuts() && j3.comp.decision.final_action != CompAction::ToolChange { j3.comp.cut_time_s / 60.0 } else { 0.0 };
    assert!((n.cut_min - (j2.comp.cut_time_s / 60.0 + own)).abs() < 1e-9, "{} vs {}", n.cut_min, j2.comp.cut_time_s / 60.0 + own);
    assert!(o3.lines.iter().any(|l| l.contains("은퇴")));
}

#[test]
fn a_flute_count_change_drops_a_pending_second_pass_that_no_longer_fits() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("pendingflutes");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let _ = record(&store, &mut row, &p, &j1, &j1.comp, "F1", None, false);
    let mut worn = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    worn.thermal_damage = 1.05;
    worn.version = store.rdb.save_tool(&worn).unwrap();
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(j2.comp.decision.final_action, CompAction::ToolChange);
    let o2 = record(&store, &mut row2, &p, &j2, &j2.comp, "F2", None, false);
    let mut q = p.clone();
    q.endmill_setting.flute_count = if q.endmill_setting.flute_count == 3 { 4 } else { 3 };
    let (j3, mut row3) = tracked_job(&mut store, &mut sds, &q, &opts);
    let o3 = record(&store, &mut row3, &q, &j3, &j3.comp, "F3", None, false);
    let n = o3.tool.unwrap();
    let p2 = store.rdb.job_passes(o2.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(p2.status, "replaced", "a plan for another flute count is not charged to the new tool");
    assert!(!p2.committed);
    assert!(!store.rdb.wear_events(n.id, 20).unwrap().iter().any(|e| e.kind == "pass2" && e.job_id == Some(o2.job_id)));
    let own = j3.report.cut_time_min + if j3.comp.decision.final_action.cuts() && j3.comp.decision.final_action != CompAction::ToolChange { j3.comp.cut_time_s / 60.0 } else { 0.0 };
    assert!((n.cut_min - own).abs() < 1e-9 && n.jobs == 1, "{} vs {}", n.cut_min, own);
}

#[test]
fn a_reset_during_a_tool_change_job_hands_its_second_pass_to_the_new_tool() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("midreset");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let _ = record(&store, &mut row, &p, &j1, &j1.comp, "R1", None, false);
    let mut worn = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    worn.thermal_damage = 1.05;
    worn.version = store.rdb.save_tool(&worn).unwrap();
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(j2.comp.decision.final_action, CompAction::ToolChange);
    let (fresh, _) = store.reset_tool_for(&p, "작업 도중 새 공구 장착").unwrap();
    let o2 = record(&store, &mut row2, &p, &j2, &j2.comp, "R2", None, false);
    assert!(o2.tool.is_none(), "the retired tool is not charged");
    assert_eq!(o2.handoff.as_ref().map(|t| t.id), Some(fresh.id));
    let p2 = store.rdb.job_passes(o2.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(p2.status, "done_new_tool", "the second pass of the tool change must not wait forever");
    let n = store.rdb.tool_by_id(fresh.id).unwrap().unwrap();
    assert!((n.cut_min - j2.comp.cut_time_s / 60.0).abs() < 1e-9 && n.jobs == 1, "{}", n.cut_min);
    assert!(store.rdb.wear_events(n.id, 20).unwrap().iter().any(|e| e.kind == "pass2" && e.job_id == Some(o2.job_id)));
}

#[test]
fn replanning_after_a_reset_charges_only_the_new_second_pass_to_the_new_tool() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("afterreset");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!(j1.comp.decision.final_action.cuts() && j1.comp.decision.final_action != CompAction::ToolChange);
    let old = row.clone();
    let o1 = record(&store, &mut row, &p, &j1, &j1.comp, "Z1", None, false);
    let old_after = store.rdb.tool_by_id(old.id).unwrap().unwrap();
    let (fresh, _) = store.reset_tool_for(&p, "2차 전에 새 공구 장착").unwrap();
    let plan_fresh = |m: f64| {
        let mut o = CompOptions::default();
        o.measured_radial_um = Some(m);
        let extra = EvalExtra { tool: Some(j1.start.clone()), refs: None, tool_replaced: true };
        let d = wearcomp::evaluate_with(&p, &j1.ctx, &j1.report, &j1.log, &o, None, &extra);
        assert!(!d.allowed.iter().any(|a| matches!(a, CompAction::Execute | CompAction::ExecuteReduced | CompAction::Split)), "{:?}", d.allowed);
        let dec = wearcomp::consult(&d, None, None, false);
        assert_eq!(dec.final_action, CompAction::ToolChange, "{:?}", d.reasons);
        wearcomp::finalize(&p, &j1.ctx, &j1.report, &j1.segs, &j1.log, &d, &dec, &o)
    };
    let tan = j1.ctx.tool.clearance_deg.to_radians().tan();
    let measured = (j1.report.vb_end_mm * 1000.0 * tan * 1.2).max(4.0);
    let c1 = plan_fresh(measured);
    let mut retired = store.rdb.tool_by_id(old.id).unwrap().unwrap();
    let a1 = record(&store, &mut retired, &p, &j1, &c1, "Z1R1", Some(o1.job_id), true);
    assert!(a1.tool.is_none());
    assert_eq!(a1.handoff.as_ref().map(|t| t.id), Some(fresh.id));
    let parent2 = store.rdb.job_passes(o1.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(parent2.status, "replaced", "the old tool's second pass never ran");
    let n1 = store.rdb.tool_by_id(fresh.id).unwrap().unwrap();
    assert!((n1.cut_min - c1.cut_time_s / 60.0).abs() < 1e-9 && n1.jobs == 1, "{} {}", n1.cut_min, n1.jobs);
    assert_eq!(store.rdb.tool_by_id(old.id).unwrap().unwrap().cut_min, old_after.cut_min, "a retired tool keeps its history");
    let c2 = plan_fresh(measured * 1.1);
    let mut retired2 = store.rdb.tool_by_id(old.id).unwrap().unwrap();
    let a2 = record(&store, &mut retired2, &p, &j1, &c2, "Z1R2", Some(a1.job_id), true);
    let n2 = store.rdb.tool_by_id(fresh.id).unwrap().unwrap();
    assert!((n2.cut_min - c2.cut_time_s / 60.0).abs() < 1e-9, "a second replan replaces the first hand-off: {} vs {}", n2.cut_min, c2.cut_time_s / 60.0);
    assert_eq!(n2.jobs, 1, "the same part is counted once on the new tool");
    let first = store.rdb.job_passes(a1.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    assert_eq!(first.status, "replaced");
    assert_eq!(a2.root_job_id, o1.job_id);
}

#[test]
fn a_corrected_measurement_replaces_the_mistaken_one_but_never_undercuts_an_earlier_part() {
    let p = preset("탄소강");
    let dir = temp_dir("remeasure");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let o1 = record(&store, &mut row, &p, &j1, &j1.comp, "M1", None, false);
    let tan = j1.ctx.tool.clearance_deg.to_radians().tan();
    let sim_um = j1.report.vb_end_mm * 1000.0 * tan;
    let typo = (sim_um * 3.0).max(9.0);
    let right = (sim_um * 1.1).max(4.0);
    assert!(typo > right);
    let c1 = replan_comp(&p, &j1, typo);
    let mut r1 = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    let a1 = record(&store, &mut r1, &p, &j1, &c1, "M1R1", Some(o1.job_id), true);
    assert_eq!(a1.tool.as_ref().unwrap().anchor_um, Some(typo));
    let c2 = replan_comp(&p, &j1, right);
    let mut r2 = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    let a2 = record(&store, &mut r2, &p, &j1, &c2, "M1R2", Some(a1.job_id), true);
    let t2 = a2.tool.unwrap();
    assert_eq!(t2.anchor_um, Some(right), "re-measuring the same part replaces the mistaken value");
    assert!(a2.lines.iter().any(|l| l.contains("정정")));
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert_eq!(j2.start.anchor_um, Some(right));
    let o2 = record(&store, &mut row2, &p, &j2, &j2.comp, "M2", None, false);
    let low = right * 0.5;
    let c3 = replan_comp(&p, &j2, low);
    let mut r3 = store.rdb.active_tool(&row.slot_key).unwrap().unwrap();
    let a3 = record(&store, &mut r3, &p, &j2, &c3, "M2R1", Some(o2.job_id), true);
    assert_eq!(a3.tool.unwrap().anchor_um, Some(right), "a later part cannot measure less wear than an earlier one");
}

#[test]
fn reverting_a_second_pass_also_lowers_the_durability_peak() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("peak");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!(j1.comp.decision.final_action.cuts() && j1.comp.decision.final_action != CompAction::ToolChange);
    let o1 = record(&store, &mut row, &p, &j1, &j1.comp, "P1", None, false);
    let t1 = o1.tool.unwrap();
    let p2 = store.rdb.job_passes(o1.job_id).unwrap().into_iter().find(|p| p.pass == 2).unwrap();
    let before = p2.metrics.get("life_used_max_before").and_then(|v| v.as_f64()).expect("durability before the second pass");
    assert!(t1.life_used_max > before + 1e-6, "the second pass adds wear: {} vs {}", t1.life_used_max, before);
    let mut dec = j1.comp.decision.clone();
    dec.final_action = CompAction::Hold;
    let hold = wearcomp::finalize(&p, &j1.ctx, &j1.report, &j1.segs, &j1.log, &j1.comp.draft, &dec, &opts);
    let mut r = store.rdb.active_tool(&t1.slot_key).unwrap().unwrap();
    let o = record(&store, &mut r, &p, &j1, &hold, "P1H", Some(o1.job_id), true);
    let t = o.tool.unwrap();
    assert!((t.life_used_max - before).abs() < 1e-9, "a second pass that never ran must not keep raising the peak: {} vs {}", t.life_used_max, before);
}

#[test]
fn a_library_from_the_older_tool_layout_opens_and_retires_its_geometry_keyed_tools() {
    let dir = temp_dir("devlayout");
    {
        let _ = Store::open(&dir.join("library")).unwrap();
    }
    let path = dir.join("library").join("library.sqlite");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "DROP TABLE cut_records; DROP TABLE tool_wear_events; DROP TABLE job_passes; DROP TABLE jobs; DROP TABLE tool_instances;
             CREATE TABLE tool_instances(id INTEGER PRIMARY KEY, attr_key TEXT NOT NULL, endmill_id INTEGER, label TEXT NOT NULL, generation INTEGER NOT NULL, vb_band_mm REAL NOT NULL DEFAULT 0, vb_max_mm REAL NOT NULL DEFAULT 0, cut_min REAL NOT NULL DEFAULT 0, removed_cm3 REAL NOT NULL DEFAULT 0, jobs INTEGER NOT NULL DEFAULT 0, field_json TEXT NOT NULL DEFAULT '', history_json TEXT NOT NULL DEFAULT '[]', anchor_um REAL, pred_base_um REAL NOT NULL DEFAULT 0, last_measured_um REAL, last_ap_mm REAL NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, retired_at INTEGER, retire_reason TEXT NOT NULL DEFAULT '');
             CREATE UNIQUE INDEX ux_tool_active ON tool_instances(attr_key) WHERE retired_at IS NULL;
             CREATE TABLE jobs(id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL, name TEXT NOT NULL, profile_name TEXT NOT NULL, pattern TEXT NOT NULL, stamp TEXT NOT NULL, parent_job_id INTEGER, tool_instance_id INTEGER, endmill_id INTEGER, workpiece_id INTEGER, material_key TEXT NOT NULL, coating_family TEXT NOT NULL, coolant TEXT NOT NULL, final_action TEXT NOT NULL, decided_by TEXT NOT NULL, vb_start_mm REAL NOT NULL, vb_end_mm REAL NOT NULL, report_line TEXT NOT NULL, summary_json TEXT NOT NULL, created_at INTEGER NOT NULL);
             INSERT INTO tool_instances(attr_key, label, generation, vb_band_mm, created_at, updated_at) VALUES('d10.00:z4', 'old', 1, 0.05, 1, 1);",
        )
        .unwrap();
    }
    let mut store = Store::open(&dir.join("library")).expect("an older dev layout must still open");
    let tools = store.rdb.list_tools(true, 10).unwrap();
    assert_eq!(tools.len(), 1);
    assert!(tools[0].retired_at.is_some() && tools[0].slot_key == "d10.00:z4");
    let p = preset("탄소강");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let (j, mut row) = tracked_job(&mut store, &mut sds, &p, &CompOptions::default());
    assert!(row.id > 0 && j.start.is_fresh());
    let out = record(&store, &mut row, &p, &j, &j.comp, "L1", None, false);
    assert_eq!(out.root_job_id, out.job_id);
    assert_eq!(store.rdb.list_tools(false, 10).unwrap().len(), 1);
}

#[test]
fn stored_wear_matches_the_simulation_at_a_depth_between_bins_and_durability_never_drops() {
    let mut p = preset("탄소강");
    p.conditions.axial_doc_mm = 6.3;
    let dir = temp_dir("nonaligned");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let out1 = record(&store, &mut row, &p, &j1, &j1.comp, "N1", None, false);
    let t1 = out1.tool.unwrap();
    let simulated = if j1.comp.decision.final_action.cuts() { j1.comp.vb_after_mm } else { j1.report.vb_end_mm };
    assert!((t1.vb_band_mm - simulated).abs() < 0.02 * simulated + 1e-6, "stored {} vs simulated {}", t1.vb_band_mm, simulated);
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!((j2.report.vb_start_mm - t1.vb_band_mm).abs() < 1e-9);
    let out2 = record(&store, &mut row2, &p, &j2, &j2.comp, "N2", None, false);
    let t2 = out2.tool.unwrap();
    assert!(t2.vb_band_mm > t1.vb_band_mm);
    let tool = tool_of(&p);
    let limit = j2.ctx.vb_limit_mm();
    let mut shallow = store.rdb.active_tool(&t2.slot_key).unwrap().unwrap();
    let shares = vec![1.0; shallow.field.flutes];
    shallow.field.deposit(3.0, 0.15, &AxialProfile::default(), &shares);
    let crafted = toolwear::summarize(&shallow.field, &tool, 3.0, limit).life_used;
    shallow.life_used_max = crafted;
    shallow.version = store.rdb.save_tool(&shallow).unwrap();
    let mut deep = p.clone();
    deep.conditions.axial_doc_mm = 20.0;
    let (j3, mut row3) = tracked_job(&mut store, &mut sds, &deep, &opts);
    let out3 = record(&store, &mut row3, &deep, &j3, &j3.comp, "N3", None, false);
    let t3 = out3.tool.unwrap();
    let at_deep = toolwear::summarize(&t3.field, &tool, 20.0, limit).life_used;
    assert!(at_deep < crafted, "the scenario must lower the wear measured over the deeper band: {} vs {}", at_deep, crafted);
    assert!(t3.life_used_max >= crafted - 1e-12, "durability went back from {} to {}", crafted, t3.life_used_max);
    assert!(t3.vb_max_mm >= shallow.field.max_vb() - 1e-12);
}

#[test]
fn calibration_learns_against_the_uncalibrated_prediction() {
    let p = preset("탄소강");
    let dir = temp_dir("calib");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, None, None, &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    let mut d = r.outcome.draft.clone();
    d.calib_wear = 2.0;
    d.growth_calib_mm = 0.02;
    d.growth_fixed_mm = 0.01;
    d.vb_end1_mm = 0.03;
    d.radial_start_um = 0.0;
    d.radial_end1_um = 0.03 * 1000.0 * 0.2;
    d.radial_used_um = 4.0;
    d.tool = None;
    let (m, base) = wearcomp::calibration_pair(&d).unwrap();
    assert!((m - 4.0).abs() < 1e-12);
    assert!((base - (0.02 / 2.0 + 0.01) * 1000.0 * 0.2).abs() < 1e-9, "base {}", base);
    let mut t = ToolStart::default();
    t.anchor_um = Some(2.5);
    t.pred_base_since_anchor_um = 1.0;
    d.tool = Some(t.clone());
    d.radial_used_um = 6.5;
    let (m2, base2) = wearcomp::calibration_pair(&d).unwrap();
    assert!((m2 - 4.0).abs() < 1e-12);
    assert!((base2 - (1.0 + (0.02 / 2.0 + 0.01) * 1000.0 * 0.2)).abs() < 1e-9);
    d.radial_used_um = 3.0;
    assert!(wearcomp::calibration_pair(&d).is_none(), "growth below the measurement resolution must not train the calibration");
}

#[test]
fn measured_wear_moves_the_parts_left_forecast() {
    let lf = LifeForecast {
        method: "holt".into(),
        step_min: 0.5,
        history_n: 9,
        t_now_min: 10.0,
        vb_now_mm: 0.1,
        limit_mm: 0.2,
        future_t_min: (1..=16).map(|i| 10.0 + i as f64).collect(),
        future_p50_mm: (1..=16).map(|i| 0.1 + 0.01 * i as f64).collect(),
        future_p90_mm: (1..=16).map(|i| 0.1 + 0.015 * i as f64).collect(),
        ..Default::default()
    };
    assert_eq!(lf.cross_with_offset(0.0, false), Some(10.0));
    assert_eq!(lf.cross_with_offset(0.05, false), Some(5.0));
    assert_eq!(lf.cross_with_offset(0.12, false), Some(0.0));
    assert_eq!(lf.cross_with_offset(-0.05, false), Some(15.0));
    let p = preset("탄소강");
    let dir = temp_dir("parts");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, None, None, &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    let ctx = CutContext::from_profile(&p);
    let tan = ctx.tool.clearance_deg.to_radians().tan();
    let mut opts = CompOptions::default();
    opts.measured_radial_um = Some(1.05 * ctx.vb_limit_mm() * 1000.0 * tan);
    let draft = wearcomp::evaluate(&p, &ctx, &r.report, &r.log, &opts, None);
    let dec = wearcomp::consult(&draft, None, None, false);
    let segs = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap().2;
    let out = wearcomp::finalize(&p, &ctx, &r.report, &segs, &r.log, &draft, &dec, &opts);
    if dec.final_action != CompAction::ToolChange {
        assert_eq!(out.parts_left_p50, Some(0.0), "{}", out.parts_source);
        assert_eq!(out.parts_left_p90, Some(0.0));
    }
    let plan = |m: Option<f64>| {
        let mut o = CompOptions::default();
        o.measured_radial_um = m;
        let d = wearcomp::evaluate(&p, &ctx, &r.report, &r.log, &o, None);
        let mut dec = wearcomp::consult(&d, None, None, false);
        dec.final_action = CompAction::Hold;
        wearcomp::finalize(&p, &ctx, &r.report, &segs, &r.log, &d, &dec, &o)
    };
    let base = plan(None);
    let fast = plan(Some(2.0 * r.report.radial_loss_end_um));
    assert!((fast.draft.measured_rate_ratio - 2.0).abs() < 0.3, "{}", fast.draft.measured_rate_ratio);
    let (b50, f50) = (base.parts_left_p50.unwrap(), fast.parts_left_p50.unwrap());
    assert!(f50 <= 0.8 * b50, "a measured 2x wear rate must shorten the median forecast, not only move its start: {} vs {}", f50, b50);
    assert!(fast.parts_left_p90.unwrap() <= f50);
    let o = CompOptions::default();
    let d = wearcomp::evaluate(&p, &ctx, &r.report, &r.log, &o, None);
    let mut dec = wearcomp::consult(&d, None, None, false);
    dec.final_action = CompAction::Hold;
    let held = |log: &WearLog| wearcomp::finalize(&p, &ctx, &r.report, &segs, log, &d, &dec, &o);
    let mut no_life = r.log.clone();
    no_life.life = None;
    let physics_only = held(&no_life);
    let limit = ctx.vb_limit_mm();
    let mut optimistic = r.log.clone();
    optimistic.life = Some(LifeForecast {
        method: "holt".into(),
        step_min: 1000.0,
        history_n: 9,
        t_now_min: 0.0,
        vb_now_mm: 0.0,
        limit_mm: limit,
        future_t_min: (1..=16).map(|i| 1000.0 * i as f64).collect(),
        future_p50_mm: (1..=16).map(|i| limit * i as f64 / 15.0).collect(),
        future_p90_mm: (1..=16).map(|i| limit * i as f64 / 14.0).collect(),
        ..Default::default()
    });
    let late = held(&optimistic);
    assert!(physics_only.parts_left_p50.is_some(), "{}", physics_only.parts_source);
    assert_eq!(late.parts_left_p50, physics_only.parts_left_p50, "an optimistic history forecast must not outlast the path physics: {}", late.parts_source);
    assert_eq!(late.parts_left_p90, physics_only.parts_left_p90, "{}", late.parts_source);
    assert!(late.parts_source.contains("더 짧아 적용"), "{}", late.parts_source);
}

#[test]
fn the_path_factor_ignores_break_in_so_an_early_tool_change_forecast_holds() {
    let p = preset("탄소강");
    let dir = temp_dir("steadyk");
    let sds = SdsStore::open(&dir.join("sds"));
    let (ctx0, _) = endmill_model::pipeline::calibrated_context(&p, Some(&sds));
    let limit = ctx0.vb_limit_mm();
    let nominal = physics::analyze_cut(&ctx0, &p.conditions, true, ThermalBody::of_setup(&p.workpiece_setup, &ctx0.wp));
    let mut ctx = ctx0.clone();
    let mut ks = Vec::new();
    let mut first: Option<(LoadSimReport, Vec<endmill_model::gcode::ToolPathSegment>)> = None;
    let mut parts = 0usize;
    loop {
        let (_v, r, segs) = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
        ks.push(wearcomp::path_wear(&nominal.trajectory, &r).0.expect("steady path factor"));
        if r.vb_end_mm >= limit || parts >= 40 {
            break;
        }
        if first.is_none() {
            first = Some((r.clone(), segs));
        }
        parts += 1;
        ctx.tool.flank_wear_mm = r.vb_end_mm;
    }
    let kmin = ks.iter().cloned().fold(f64::INFINITY, f64::min);
    let kmax = ks.iter().cloned().fold(0.0, f64::max);
    assert!(kmax / kmin < 1.05, "the steady path factor must not drift as break-in fades: {:?}", ks);
    let (r0, segs0) = first.unwrap();
    let opts = CompOptions::default();
    let log = wearcomp::analyze_job(&p, &ctx0, &r0, None, &sds);
    let d = wearcomp::evaluate(&p, &ctx0, &r0, &log, &opts, None);
    let mut dec = wearcomp::consult(&d, None, None, false);
    dec.final_action = CompAction::ToolChange;
    let out = wearcomp::finalize(&p, &ctx0, &r0, &segs0, &log, &d, &dec, &opts);
    let p50 = out.parts_left_p50.unwrap();
    assert!(p50 <= parts as f64 && p50 + 2.0 >= parts as f64, "tool change after the first part: forecast {} parts, a fresh tool lasts {}", p50, parts);
    let first_growth = d.path_trajectory.vb_at(r0.cut_time_min);
    assert!((first_growth - r0.vb_end_mm).abs() < 0.1 * r0.vb_end_mm, "break-in runs in real time like the simulation: {} vs {}", first_growth, r0.vb_end_mm);
    let rule = wearcomp::finalize(&p, &ctx0, &r0, &segs0, &log, &d, &wearcomp::consult(&d, None, None, false), &opts);
    let r50 = rule.parts_left_p50.unwrap();
    assert!(r50 <= (parts - 1) as f64 && r50 + 3.0 >= (parts - 1) as f64, "same tool after its first part: forecast {}, it really lasts {} more ({})", r50, parts - 1, rule.parts_source);
}

#[test]
fn tool_change_parts_left_follows_a_fresh_tool_on_the_same_path() {
    let p = preset("티타늄");
    let dir = temp_dir("freshpath");
    let sds = SdsStore::open(&dir.join("sds"));
    let (ctx0, _) = endmill_model::pipeline::calibrated_context(&p, Some(&sds));
    let limit = ctx0.vb_limit_mm();
    let mut ctx = ctx0.clone();
    let mut runs: Vec<(CutContext, LoadSimReport, Vec<endmill_model::gcode::ToolPathSegment>)> = Vec::new();
    loop {
        let (_v, r, segs) = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
        let done = r.vb_end_mm >= limit;
        let next = r.vb_end_mm;
        runs.push((ctx.clone(), r, segs));
        if done || runs.len() > 12 {
            break;
        }
        ctx.tool.flank_wear_mm = next;
    }
    let fresh_parts = runs.iter().filter(|(_, r, _)| r.vb_end_mm < limit).count();
    let (w0, w1) = (&runs[0].1, &runs[1].1);
    assert!((w1.chatter_wear_factor_fresh - w0.chatter_wear_factor).abs() < 0.02, "a fresh edge on the same path chatters like the first part: {} vs {}", w1.chatter_wear_factor_fresh, w0.chatter_wear_factor);
    assert!(w1.chatter_wear_factor_fresh > w1.chatter_wear_factor + 0.05, "flank wear adds process damping: {} vs {}", w1.chatter_wear_factor_fresh, w1.chatter_wear_factor);
    assert!((2..12).contains(&fresh_parts), "the titanium finishing preset should wear a tool out within a few parts: {}", fresh_parts);
    let opts = CompOptions::default();
    let plan = |k: usize, force: Option<CompAction>| {
        let (c, rep, segs) = &runs[k];
        let log = wearcomp::analyze_job(&p, c, rep, None, &sds);
        let draft = wearcomp::evaluate(&p, c, rep, &log, &opts, None);
        let mut dec = wearcomp::consult(&draft, None, None, false);
        if let Some(a) = force {
            dec.final_action = a;
        }
        let out = wearcomp::finalize(&p, c, rep, segs, &log, &draft, &dec, &opts);
        (draft, out, log)
    };
    let (_, first, log0) = plan(0, None);
    let next_growth = runs[1].1.vb_end_mm - runs[1].1.vb_start_mm;
    let fused = log0.per_part_vb_p50_mm.expect("fused per-part growth");
    assert!(fused > 0.6 * next_growth && fused < 1.6 * next_growth, "the physics predictor must follow this path: fused {} mm/part vs the next part {}", fused, next_growth);
    let left = (fresh_parts - 1) as f64;
    let f50 = first.parts_left_p50.expect("parts left after the first part");
    assert!(f50 <= left && f50 + 1.0 >= left, "first part: forecast {} more parts, the same tool really lasts {} more ({})", f50, left, first.parts_source);
    let (draft, out, _) = plan(1, Some(CompAction::ToolChange));
    let wctx = &runs[1].0;
    assert!(runs[1].1.vb_start_mm > 0.0);
    assert!(draft.path_factor > 1.2, "the zigzag pocket wears faster than the nominal side cut: ×{}", draft.path_factor);
    let nominal1 = physics::analyze_cut(wctx, &p.conditions, true, ThermalBody::of_setup(&p.workpiece_setup, &wctx.wp));
    assert!(draft.fresh_life_min < 0.9 * nominal1.wear.tool_life_min);
    if runs.len() > 2 {
        let (late, _, _) = plan(2, Some(CompAction::ToolChange));
        let flat = nominal1.trajectory.for_path(late.path_factor).life_min;
        assert!(late.fresh_life_min < 0.97 * flat, "a fresh edge has less process damping than the worn one, so it wears faster early: {} vs {}", late.fresh_life_min, flat);
    }
    let p50 = out.parts_left_p50.expect("parts left for the new tool");
    let p90 = out.parts_left_p90.expect("cautious parts left for the new tool");
    assert!(p50 <= fresh_parts as f64 && p50 + 1.0 >= fresh_parts as f64, "new tool: forecast {} parts, a fresh tool on this path lasts {} ({})", p50, fresh_parts, out.parts_source);
    assert!(p90 <= p50);
    assert!(out.parts_source.contains("같은 경로"));
}

#[test]
fn a_job_that_reaches_the_wear_cap_does_not_stretch_the_path_life() {
    let p = preset("인코넬");
    let ctx = CutContext::from_profile(&p);
    let r = simulate(&p, &ctx);
    if r.vb_end_mm < 2.0 * ctx.vb_limit_mm() - 1e-9 {
        return;
    }
    assert!(r.growth_cut_min < r.cut_time_min, "{} vs {}", r.growth_cut_min, r.cut_time_min);
    let nominal = physics::analyze_cut(&ctx, &p.conditions, true, ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp));
    let (_, tr) = wearcomp::path_wear(&nominal.trajectory, &r);
    let limit = ctx.vb_limit_mm();
    let bins = &r.wear_log;
    let i = bins.iter().position(|b| b.vb_mm >= limit).expect("the simulated tool passes the limit");
    let (t0, v0) = if i > 0 { (bins[i - 1].t_cut_s, bins[i - 1].vb_mm) } else { (0.0, 0.0) };
    let t_lim = (t0 + (bins[i].t_cut_s - t0) * (limit - v0) / (bins[i].vb_mm - v0).max(1e-12)) / 60.0;
    assert!((tr.life_min - t_lim).abs() < 0.25 * t_lim, "new-tool life {} min vs the simulated limit crossing {} min", tr.life_min, t_lim);
}

#[test]
fn second_pass_fatigue_follows_the_passes_that_run() {
    let p = preset("스테인리스");
    let dir = temp_dir("passfatigue");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    let d = &j.comp.draft;
    assert!(d.need && d.thermal_damage_pass2 > 0.0, "the stainless finishing preset needs a second pass that heats and quenches the edge");
    let run = |a: CompAction| {
        let mut dec = j.comp.decision.clone();
        dec.final_action = a;
        wearcomp::finalize(&p, &j.ctx, &j.report, &j.segs, &j.log, d, &dec, &opts)
    };
    let hold = run(CompAction::Hold);
    assert_eq!(hold.thermal_damage_pass2, 0.0);
    assert!((hold.thermal_damage_after - d.thermal_damage_now).abs() < 1e-12);
    let mut seen = Vec::new();
    for a in [CompAction::Execute, CompAction::ExecuteReduced, CompAction::Split] {
        let o = run(a);
        assert!(o.thermal_damage_pass2 > 0.0, "{:?}", a);
        assert!(o.thermal_damage_pass2 <= d.thermal_damage_pass2 * 1.05 + 1e-12, "{:?}: the gate uses the worst current-tool plan {} < {}", a, d.thermal_damage_pass2, o.thermal_damage_pass2);
        assert!((o.thermal_damage_after - (d.thermal_damage_now + o.thermal_damage_pass2)).abs() < 1e-12);
        seen.push(o.thermal_damage_pass2);
    }
    assert!(seen.iter().any(|v| (v - seen[0]).abs() > 1e-6), "each plan carries the fatigue of its own passes: {:?}", seen);
    let change = run(CompAction::ToolChange);
    assert!(change.thermal_damage_pass2 > 0.0 && (change.thermal_damage_after - change.thermal_damage_pass2).abs() < 1e-12);
    let split = run(CompAction::Split);
    let out = record(&store, &mut row, &p, &j, &split, "PF", None, false);
    let stored = out.tool.unwrap().thermal_damage;
    assert!((stored - (j.start.thermal_damage + j.report.thermal_damage + split.thermal_damage_pass2)).abs() < 1e-9, "stored {}", stored);
}

#[test]
fn dlc_on_titanium_raises_a_compatibility_gate_and_job_warning() {
    let mut p = preset("티타늄");
    let mut s = endmill_model::profile::EndMillMockupSetting::default_8mm_3flute_dlc();
    s.stickout_mm = p.endmill_setting.stickout_mm;
    if s.validate().is_err() {
        s.stickout_mm = None;
    }
    p.endmill_setting = s;
    let dir = temp_dir("compat");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, None, None, &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    assert!(r.report.compat_severity >= 2);
    assert!(r.report.warnings.iter().any(|w| w.starts_with("공구·소재 궁합")));
    let g = r.outcome.draft.gates.iter().find(|g| g.id == "tool_compat").expect("compat gate");
    assert!(g.severity >= 1 && g.blocks.is_empty());
    if g.severity >= 2 {
        assert!(r.outcome.decision.operator_confirm);
    }
}

fn analysis(m: WorkpieceMaterial, coating: &str, method: CoolantMethod) -> (CutContext, physics::CutAnalysis) {
    let mut p = preset("탄소강");
    p.workpiece_setup.material = m.clone();
    if let WorkpieceMaterial::AlloySteel { hardness_hrc } = m {
        p.workpiece_setup.hardness_hrc = Some(hardness_hrc);
    }
    p.endmill_setting.coating_name = if coating.is_empty() { None } else { Some(coating.to_string()) };
    p.coolant_config = CoolantConfig::from_method(&method);
    p.conditions = endmill_model::CuttingCalculator::recommend_conditions(&m, 10.0, 4, true).unwrap();
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let a = physics::analyze_cut(&ctx, &p.conditions, true, body);
    (ctx, a)
}

#[test]
fn coating_breakthrough_moves_an_incompatible_coating_toward_the_substrate() {
    let rate_at = |m: WorkpieceMaterial, c: &str, vb: f64| {
        let (ctx, a) = analysis(m, c, CoolantMethod::Flood);
        let mut c2 = ctx.clone();
        c2.tool.flank_wear_mm = vb;
        let co = physics::cutting_coeffs(&c2);
        let eng = endmill_model::physics::Engagement::side(a.ap_mm, a.ae_mm, ctx.tool.diameter_mm, true);
        let f = physics::mechanistic_forces(&c2.tool, &co, a.fz_mm, a.rpm, &eng, 36, 8);
        let th = physics::thermal(&c2, &co, &f, a.vc_effective_m_min, a.rpm, &eng, 1.0, 0.03);
        physics::wear_at(&c2, &f, &th, a.vc_effective_m_min, a.fz_mm, &eng, physics::reference_state(&ctx), vb)
    };
    let dlc0 = rate_at(WorkpieceMaterial::Titanium, "DLC", 0.0);
    let dlc1 = rate_at(WorkpieceMaterial::Titanium, "DLC", 0.1);
    assert_eq!(dlc0.coating_blend, 1.0);
    assert!(dlc1.coating_exposure > 0.8 && dlc1.coating_blend < 0.7, "exposure {} blend {}", dlc1.coating_exposure, dlc1.coating_blend);
    let altin = rate_at(WorkpieceMaterial::CarbonSteel, "AlTiN", 0.1);
    assert!(altin.coating_exposure > 0.8 && (altin.coating_blend - 1.0).abs() < 1e-12, "a compatible coating keeps its calibrated benefit");
    let diamond = rate_at(WorkpieceMaterial::CarbonSteel, "Diamond", 0.1);
    assert!((diamond.coating_blend - 1.0).abs() < 1e-12, "a thick CVD diamond layer delaminates instead of wearing through");
    assert!(toolwear::exposed_wear_rate(3.0, 1.0, 0.5) < 1.6 && toolwear::exposed_wear_rate(3.0, 1.0, 0.0) == 3.0);
    assert_eq!(toolwear::exposed_wear_rate(0.5, 1.0, 0.9), 0.5);
}

#[test]
fn interrupted_cutting_with_flood_accumulates_thermal_fatigue_and_mql_does_not() {
    let hard = WorkpieceMaterial::AlloySteel { hardness_hrc: 60 };
    let (_, flood) = analysis(hard.clone(), "AlTiN", CoolantMethod::Flood);
    let (_, mist) = analysis(hard.clone(), "AlTiN", CoolantMethod::Mist);
    let (_, dry) = analysis(hard, "AlTiN", CoolantMethod::Dry);
    assert!(flood.thermal_fatigue_per_min > 10.0 * mist.thermal_fatigue_per_min.max(1e-9), "flood {} mist {}", flood.thermal_fatigue_per_min, mist.thermal_fatigue_per_min);
    assert!(dry.thermal_fatigue_per_min <= mist.thermal_fatigue_per_min);
    assert!(flood.notes.iter().any(|n| n.contains("열피로")));
    let span = 60f64.to_radians();
    assert!(toolwear::idle_factor(span, 2000.0) > toolwear::idle_factor(span, 12000.0));
    assert_eq!(toolwear::idle_factor(2.0 * std::f64::consts::PI, 2000.0), 0.0);
    assert!(toolwear::cycles_to_crack(600.0) < toolwear::cycles_to_crack(400.0));
    assert!(!toolwear::cycles_to_crack(50.0).is_finite());
    let p = preset("스테인리스");
    let dir = temp_dir("fatigue");
    let mut store = Store::open(&dir.join("library")).unwrap();
    let mut sds = SdsStore::open(&dir.join("sds"));
    let opts = CompOptions::default();
    let (j1, mut row) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!(j1.report.thermal_damage > 0.0, "the stainless finishing preset runs interrupted with flood/through-tool coolant");
    let o1 = record(&store, &mut row, &p, &j1, &j1.comp, "F1", None, false);
    let d1 = o1.tool.unwrap().thermal_damage;
    assert!(d1 >= j1.report.thermal_damage - 1e-12);
    let (j2, mut row2) = tracked_job(&mut store, &mut sds, &p, &opts);
    assert!((j2.start.thermal_damage - d1).abs() < 1e-12);
    assert!(j2.comp.draft.gates.iter().any(|g| g.id == "thermal_fatigue"));
    let o2 = record(&store, &mut row2, &p, &j2, &j2.comp, "F2", None, false);
    assert!(o2.tool.unwrap().thermal_damage > d1);
    let (fresh, _) = store.reset_tool_for(&p, "reset").unwrap();
    assert_eq!(fresh.thermal_damage, 0.0);
}

#[test]
fn opening_a_version_one_library_adds_the_tool_tables_once() {
    let dir = temp_dir("migrate");
    {
        let _ = Store::open(&dir.join("library")).unwrap();
    }
    let path = dir.join("library").join("library.sqlite");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("DROP TABLE cut_records; DROP TABLE tool_wear_events; DROP TABLE job_passes; DROP TABLE jobs; DROP TABLE tool_instances; UPDATE meta SET value = '1' WHERE key = 'schema_version';")
            .unwrap();
    }
    for _ in 0..2 {
        let store = Store::open(&dir.join("library")).unwrap();
        assert_eq!(store.rdb.meta("schema_version").unwrap().as_deref(), Some("2"));
        assert!(store.rdb.list_tools(true, 5).unwrap().is_empty());
        assert!(store.rdb.jobs(None, None, 5).unwrap().is_empty());
    }
}
