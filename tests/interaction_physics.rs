use endmill_model::physics::{self, cutting_coeffs, mechanistic_forces, CutContext, Engagement, ThermalBody};
use endmill_model::profile::{MachiningPreset, MachiningProfile};
use endmill_model::workpiece_setup::ClampingMethod;
use endmill_model::WorkpieceMaterial;

fn steel() -> MachiningProfile {
    MachiningProfile::from_preset(&MachiningPreset::default_steel_general(), "steel")
}

#[test]
fn flank_wear_raises_radial_force_and_edge_heat() {
    let p = steel();
    let mut ctx = CutContext::from_profile(&p);
    let co = cutting_coeffs(&ctx);
    let d = ctx.tool.diameter_mm;
    let eng = Engagement::side(10.0, 3.0, d, true);
    let rpm = p.conditions.spindle_rpm as f64;
    let fresh = mechanistic_forces(&ctx.tool, &co, 0.05, rpm, &eng, 72, 8);
    ctx.tool.flank_wear_mm = 0.2;
    let worn = mechanistic_forces(&ctx.tool, &co, 0.05, rpm, &eng, 72, 8);
    assert!(worn.radial_load_n_per_mm > 1.5 * fresh.radial_load_n_per_mm, "{} vs {}", worn.radial_load_n_per_mm, fresh.radial_load_n_per_mm);
    assert!(fresh.flank_share == 0.0 && worn.flank_share > 0.3);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let tf = physics::thermal(&ctx, &co, &fresh, 180.0, rpm, &eng, body.mass_kg, body.area_m2);
    let tw = physics::thermal(&ctx, &co, &worn, 180.0, rpm, &eng, body.mass_kg, body.area_m2);
    assert!(tw.interface_c > tf.interface_c);
    assert!(tw.heat_to_workpiece_share > 0.02 && tw.heat_to_workpiece_share < 0.7);
}

#[test]
fn three_phase_trajectory_is_consistent() {
    let p = steel();
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let a = physics::analyze_cut(&ctx, &p.conditions, true, body);
    let tr = &a.trajectory;
    assert!(tr.life_min.is_finite() && tr.life_min > 0.0);
    assert!(tr.life_min < tr.linear_life_min, "{} {}", tr.life_min, tr.linear_life_min);
    assert!(tr.acceleration > 1.2, "acceleration {}", tr.acceleration);
    assert!(a.wear.chatter_factor >= 1.0);
    assert!((a.wear.tool_life_min * a.wear.chatter_factor - tr.life_min).abs() < 1e-6 * tr.life_min);
    let early = tr.vb_at(0.02 * tr.life_min) / (0.02 * tr.life_min);
    let mid = (tr.vb_at(0.5 * tr.life_min) - tr.vb_at(0.4 * tr.life_min)) / (0.1 * tr.life_min);
    assert!(early > mid, "break-in {} vs steady {}", early, mid);
    let v = tr.vb_at(0.6 * tr.life_min);
    assert!((tr.time_at(v) - 0.6 * tr.life_min).abs() < 0.02 * tr.life_min);
    assert!((tr.vb_at(tr.life_min) - tr.vb_limit_mm).abs() < 0.02 * tr.vb_limit_mm);
    assert!(tr.growth_from(0.05, 5.0) > 0.0);
}

#[test]
fn minimum_chip_thickness_and_work_hardening() {
    let mut p = steel();
    p.workpiece_setup.material = WorkpieceMaterial::StainlessSteel;
    let mut ctx = CutContext::from_profile(&p);
    ctx.tool.runout_um = 0.0;
    let co = cutting_coeffs(&ctx);
    let d = ctx.tool.diameter_mm;
    let z = ctx.tool.flutes as f64;
    let rpm = 3000.0;
    let eng = Engagement::side(5.0, 0.5, d, true);
    let tiny = mechanistic_forces(&ctx.tool, &co, 0.004, rpm, &eng, 72, 8);
    let normal = mechanistic_forces(&ctx.tool, &co, 0.06, rpm, &eng, 72, 8);
    assert!(tiny.ploughing_share > normal.ploughing_share);
    let e_tiny = tiny.cutting_power_kw / (5.0 * 0.5 * 0.004 * z * rpm);
    let e_norm = normal.cutting_power_kw / (5.0 * 0.5 * 0.06 * z * rpm);
    assert!(e_tiny > 1.5 * e_norm, "specific energy {} vs {}", e_tiny, e_norm);
    let mut co0 = co.clone();
    co0.work_hardening = 0.0;
    let f0 = mechanistic_forces(&ctx.tool, &co0, 0.02, rpm, &eng, 72, 8);
    let f1 = mechanistic_forces(&ctx.tool, &co, 0.02, rpm, &eng, 72, 8);
    assert!(f1.f_res_mean > f0.f_res_mean);
}

#[test]
fn thermal_softening_and_heat_partition_trends() {
    let p = steel();
    let ctx = CutContext::from_profile(&p);
    let base = cutting_coeffs(&ctx);
    let slow = physics::softening_factor(&ctx, &base, 80.0, 0.03);
    let fast = physics::softening_factor(&ctx, &base, 400.0, 0.03);
    assert!(fast < slow, "{} {}", fast, slow);
    assert!(physics::boothroyd_partition(0.1) > physics::boothroyd_partition(50.0));
    let co = physics::cutting_coeffs_at(&ctx, 400.0, 0.03);
    assert!((co.softening - fast).abs() < 1e-12);
}

#[test]
fn stickout_and_clamping_change_dynamics_and_budget() {
    let mut p = steel();
    p.endmill_setting.stickout_mm = Some(30.0);
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let a1 = physics::analyze_cut(&ctx, &p.conditions, true, body);
    p.endmill_setting.stickout_mm = Some(60.0);
    let ctx2 = CutContext::from_profile(&p);
    let a2 = physics::analyze_cut(&ctx2, &p.conditions, true, body);
    assert!(a2.stability.fn_hz < a1.stability.fn_hz);
    assert!(a2.stability.ap_crit_mm < a1.stability.ap_crit_mm);
    p.endmill_setting.stickout_mm = Some(30.0);
    p.workpiece_setup.clamping = ClampingMethod::VacuumChuck;
    let ctx3 = CutContext::from_profile(&p);
    let a3 = physics::analyze_cut(&ctx3, &p.conditions, true, body);
    assert!(a3.fixture_deflection_um > a1.fixture_deflection_um);
    assert!(a3.error_budget_um.total_um > a1.error_budget_um.total_um);
}

#[test]
fn recommendation_respects_chatter_limit() {
    let mut p = steel();
    p.endmill_setting.stickout_mm = Some(70.0);
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let rec = physics::recommend(&ctx, true, body, physics::Purpose::Roughing);
    let a = physics::analyze_cut(&ctx, &rec.conditions, true, body);
    assert!(
        a.stability.margin >= 0.9 || rec.notes.iter().any(|n| n.contains("채터")),
        "margin {} notes {:?}",
        a.stability.margin,
        rec.notes
    );
}

#[test]
fn measured_frf_drives_recommendation_into_stable_zone() {
    let mut p = steel();
    p.endmill_setting.stickout_mm = Some(45.0);
    p.machine.tip_fn_hz = Some(2200.0);
    p.machine.tip_stiffness_n_per_um = Some(6.0);
    p.machine.tip_frf_stickout_mm = Some(45.0);
    p.machine.damping_ratio = 0.035;
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let rec = physics::recommend(&ctx, true, body, physics::Purpose::Roughing);
    let a = physics::analyze_cut(&ctx, &rec.conditions, true, body);
    assert!(a.stability.measured && !a.stability.stickout_scaled);
    assert!((a.stability.fn_hz - 2200.0).abs() < 1e-6);
    assert!(a.stability.margin >= 0.95, "margin {} notes {:?}", a.stability.margin, rec.notes);
    assert!(rec.notes.iter().any(|n| n.contains("실측 FRF")), "{:?}", rec.notes);
}

fn case(m: WorkpieceMaterial, coating: &str, coolant: endmill_model::profile::CoolantMethod) -> (CutContext, physics::CutAnalysis) {
    let mut p = steel();
    p.workpiece_setup.material = m.clone();
    if let WorkpieceMaterial::AlloySteel { hardness_hrc } = m {
        p.workpiece_setup.hardness_hrc = Some(hardness_hrc);
    }
    p.endmill_setting.coating_name = if coating.is_empty() { None } else { Some(coating.to_string()) };
    p.coolant_config = endmill_model::profile::CoolantConfig::from_method(&coolant);
    p.conditions = endmill_model::CuttingCalculator::recommend_conditions(&m, 10.0, 4, true).unwrap();
    let ctx = CutContext::from_profile(&p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let a = physics::analyze_cut(&ctx, &p.conditions, true, body);
    (ctx, a)
}

#[test]
fn coating_workpiece_chemistry_ranks_like_practice() {
    use endmill_model::profile::CoolantMethod::Mist;
    let life = |m: WorkpieceMaterial, c: &str| case(m, c, Mist).1.wear.tool_life_min;
    let steel_altin = life(WorkpieceMaterial::CarbonSteel, "AlTiN");
    assert!(steel_altin > 3.0 * life(WorkpieceMaterial::CarbonSteel, "Diamond"));
    assert!(steel_altin > 2.0 * life(WorkpieceMaterial::CarbonSteel, "DLC"));
    assert!(steel_altin > 3.0 * life(WorkpieceMaterial::CarbonSteel, ""));
    let al_altin = life(WorkpieceMaterial::Aluminum, "AlTiN");
    let al_dlc = life(WorkpieceMaterial::Aluminum, "DLC");
    assert!(al_dlc > 2.0 * al_altin && life(WorkpieceMaterial::Aluminum, "Diamond") > al_dlc);
    assert!(life(WorkpieceMaterial::Aluminum, "") > al_altin);
    assert!(life(WorkpieceMaterial::Titanium, "AlCrN") > life(WorkpieceMaterial::Titanium, "TiN"));
    let hard = WorkpieceMaterial::AlloySteel { hardness_hrc: 60 };
    assert!(life(hard.clone(), "nACo") > life(hard.clone(), "AlTiN") && life(hard.clone(), "AlTiN") > 3.0 * life(hard, "TiN"));
    let (_, d) = case(WorkpieceMaterial::CarbonSteel, "Diamond", Mist);
    assert!(d.tribology.compat.severity >= 2 && d.tribology.dissolution > 0.3, "{:?}", d.tribology.compat);
    let (_, a) = case(WorkpieceMaterial::Aluminum, "AlTiN", Mist);
    assert!(a.tribology.compat.messages.iter().any(|m| m.contains("응착")));
    assert_eq!(d.regime.len(), endmill_model::tribology::REGIME_DIM);
}

#[test]
fn coolant_effect_depends_on_workpiece_and_temperature() {
    use endmill_model::profile::CoolantMethod::{AirBlast, Dry, Flood, Mist, ThroughTool};
    let (_, ti_dry) = case(WorkpieceMaterial::Titanium, "AlTiN", Dry);
    let (_, ti_hpc) = case(WorkpieceMaterial::Titanium, "AlTiN", ThroughTool);
    assert!(ti_hpc.wear.tool_life_min > 2.5 * ti_dry.wear.tool_life_min);
    assert!(ti_hpc.thermal.interface_c < ti_dry.thermal.interface_c - 100.0);
    assert!(ti_hpc.coeffs.chip_lift > 0.1);
    let hard = WorkpieceMaterial::AlloySteel { hardness_hrc: 55 };
    let (_, h_air) = case(hard.clone(), "AlTiN", AirBlast);
    let (_, h_flood) = case(hard, "AlTiN", Flood);
    assert!(h_flood.thermal.thermal_crack_risk > h_air.thermal.thermal_crack_risk + 0.2);
    assert!(h_air.wear.tool_life_min > h_flood.wear.tool_life_min);
    let (_, s_dry) = case(WorkpieceMaterial::CarbonSteel, "AlTiN", Dry);
    let (_, s_mist) = case(WorkpieceMaterial::CarbonSteel, "AlTiN", Mist);
    assert!(s_mist.coeffs.mu < s_dry.coeffs.mu && s_mist.coeffs.lubricant_access > 0.05);
    let (_, in_flood) = case(WorkpieceMaterial::Inconel, "AlTiN", Flood);
    let (_, in_hpc) = case(WorkpieceMaterial::Inconel, "AlTiN", ThroughTool);
    assert!(in_hpc.thermal.cooling.effectiveness > in_flood.thermal.cooling.effectiveness);
    assert!(in_hpc.thermal.cooling.boiling_factor > in_flood.thermal.cooling.boiling_factor);
    let (_, cfrp) = case(WorkpieceMaterial::CFRP, "Diamond", Flood);
    assert!(cfrp.tribology.compat.severity >= 2);
}

#[test]
fn lubricant_penetration_and_wear_fall_with_speed() {
    let p = steel();
    let ctx = CutContext::from_profile(&p);
    let slow = endmill_model::tribology::contact_friction(&ctx, 60.0, 500.0);
    let fast = endmill_model::tribology::contact_friction(&ctx, 300.0, 500.0);
    assert!(slow.lubricant_access > fast.lubricant_access && slow.mu_eff < fast.mu_eff);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let mut lives = Vec::new();
    for f in [0.5, 1.0, 2.0] {
        let mut c = p.conditions.clone();
        c.spindle_rpm = (c.spindle_rpm as f64 * f).round() as u32;
        c.feed_rate_mm_min *= f;
        let a = physics::analyze_cut(&ctx, &c, true, body);
        lives.push((a.thermal.interface_c, a.wear.tool_life_min));
    }
    assert!(lives[0].0 < lives[1].0 && lives[1].0 < lives[2].0, "{:?}", lives);
    assert!(lives[0].1 > 2.0 * lives[1].1 && lives[1].1 > 2.0 * lives[2].1, "{:?}", lives);
}

#[test]
fn material_properties_follow_temperature_and_rate() {
    let ti = physics::WorkpieceProps::of(&WorkpieceMaterial::Titanium);
    let st = physics::WorkpieceProps::of(&WorkpieceMaterial::CarbonSteel);
    assert!(ti.k_at(500.0) > 1.5 * ti.k_at(20.0));
    assert!(st.k_at(500.0) < 0.8 * st.k_at(20.0));
    assert!(st.flow_stress(0.5, 1.0e4, 600.0) < st.flow_stress(0.5, 1.0e4, 200.0));
    assert!(st.flow_stress(0.5, 1.0e5, 400.0) > st.flow_stress(0.5, 1.0, 400.0));
    let shares = endmill_model::tribology::normalized_shares(&ti.mech_shares);
    assert!((shares.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    let p = steel();
    let ctx = CutContext::from_profile(&p);
    let r = physics::reference_state(&ctx);
    assert!(r.t_c > 100.0 && r.raw.iter().all(|v| *v > 0.0));
}
