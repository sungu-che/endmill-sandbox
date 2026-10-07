use endmill_model::calc::{self, CalcOverrides};
use endmill_model::environment;
use endmill_model::gcode::ToolPathPattern;
use endmill_model::loadsim;
use endmill_model::physics::{self, tool_compliance, CutContext, ThermalBody, ToolGeometry, WorkpieceProps};
use endmill_model::profile::{
    boiling_point_c, CoolantConfig, CoolantMethod, EndMillMockupSetting, MachineLimits, MachiningPreset, MachiningProfile, ShopEnvironment,
    ToolSubstrate,
};
use endmill_model::store::attr::EndMillAttr;
use endmill_model::store::Store;
use endmill_model::viewport;

fn profile(preset: MachiningPreset) -> MachiningProfile {
    MachiningProfile::from_preset(&preset, "t")
}

fn with_env(mut p: MachiningProfile, ambient: f64, rh: f64) -> MachiningProfile {
    p.machine.environment.ambient_c = ambient;
    p.machine.environment.humidity_pct = rh;
    p
}

fn analyze(p: &MachiningProfile) -> physics::CutAnalysis {
    let ctx = CutContext::from_profile(p);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    physics::analyze_cut(&ctx, &p.conditions, true, body)
}

#[test]
fn psychrometrics_match_reference_values() {
    let e = ShopEnvironment::default();
    assert!((e.dew_point_c() - 11.1).abs() < 0.3, "dew {}", e.dew_point_c());
    assert!((e.wet_bulb_c() - 15.5).abs() < 0.6, "wet bulb {}", e.wet_bulb_c());
    assert!((e.air_density() - 1.19).abs() < 0.02, "rho {}", e.air_density());
    assert!((boiling_point_c(101.325) - 100.0).abs() < 0.3);
    assert!((boiling_point_c(84.5) - 95.0).abs() < 0.8, "boil {}", boiling_point_c(84.5));
    let mut humid = e.clone();
    humid.humidity_pct = 85.0;
    assert!(humid.dew_point_c() > e.dew_point_c() + 7.0);
    assert!(humid.wet_bulb_depression() < e.wet_bulb_depression());
    assert!(e.validate().is_ok());
    let mut bad = e.clone();
    bad.humidity_pct = 120.0;
    assert!(bad.validate().is_err());
}

#[test]
fn coolant_temperature_follows_room_pump_evaporation_and_chiller() {
    let env = ShopEnvironment::default();
    assert!((CoolantConfig::through_tool().temperature_in(&env) - 25.0).abs() < 1e-6);
    let flood = CoolantConfig::flood();
    let base = flood.temperature_in(&env);
    assert!((base - 22.45).abs() < 0.05, "flood {}", base);
    let mut hot = env.clone();
    hot.ambient_c = 30.0;
    assert!((flood.temperature_in(&hot) - base - 8.0).abs() < 0.6);
    let mut dry = env.clone();
    dry.humidity_pct = 20.0;
    let mut wet = env.clone();
    wet.humidity_pct = 85.0;
    assert!(flood.temperature_in(&dry) < base && flood.temperature_in(&wet) > base);
    let mut chilled = env.clone();
    chilled.chiller_c = Some(18.0);
    assert_eq!(flood.temperature_in(&chilled), 18.0);
    assert!(CoolantConfig::air_blast().temperature_in(&env) < env.ambient_c);
    let mut legacy = CoolantConfig::flood();
    legacy.temperature_c = Some(22.0);
    legacy.normalize_legacy();
    assert!(legacy.temperature_c.is_none());
    let mut explicit = CoolantConfig::flood();
    explicit.temperature_c = Some(19.5);
    explicit.normalize_legacy();
    assert_eq!(explicit.temperature_c, Some(19.5));
}

#[test]
fn room_temperature_shifts_coolant_path_and_dimensional_budget() {
    let base = profile(MachiningPreset::default_aluminum_roughing());
    let a20 = analyze(&with_env(base.clone(), 20.0, 50.0));
    let a30 = analyze(&with_env(base.clone(), 30.0, 50.0));
    assert!(a30.environment.coolant_c > a20.environment.coolant_c + 9.0);
    assert!(a30.thermal.interface_c > a20.thermal.interface_c + 5.0);
    assert!(a30.error_budget_um.environment_um.abs() > a20.error_budget_um.environment_um.abs() + 5.0);
    assert!(a30.error_budget_um.total_um > a20.error_budget_um.total_um);
    assert!(a30.notes.iter().any(|n| n.contains("측정 기준")), "{:?}", a30.notes);
    let mut chilled = with_env(base.clone(), 30.0, 85.0);
    chilled.machine.environment.chiller_c = Some(16.0);
    let ac = analyze(&chilled);
    assert!(ac.environment.condensation, "dew {} coolant {}", ac.environment.dew_point_c, ac.environment.coolant_c);
    let mut steel = profile(MachiningPreset::default_steel_general());
    steel.machine.environment.humidity_pct = 75.0;
    assert!(analyze(&steel).environment.corrosion);
}

#[test]
fn humidity_changes_dry_contact_friction_by_coating_chemistry() {
    let mut p = profile(MachiningPreset::default_aluminum_finishing());
    p.coolant_config = CoolantConfig::dry();
    let mu = |rh: f64, coating: &str| {
        let mut q = with_env(p.clone(), 22.0, rh);
        q.endmill_setting.coating_name = Some(coating.into());
        analyze(&q).coeffs.mu
    };
    assert!(mu(85.0, "DLC") > mu(20.0, "DLC") * 1.05);
    assert!(mu(85.0, "Diamond") < mu(20.0, "Diamond") * 0.97);
    assert!((mu(50.0, "DLC") - mu(50.0, "DLC")).abs() < 1e-12);
    let mut flood = with_env(p.clone(), 22.0, 85.0);
    flood.coolant_config = CoolantConfig::flood();
    let ctx = CutContext::from_profile(&flood);
    assert_eq!(environment::friction_factor(&ctx, 200.0, 300.0), 1.0);
    let mut cfrp = profile(MachiningPreset::default_cfrp_trimming());
    cfrp.machine.environment.humidity_pct = 90.0;
    let ctx = CutContext::from_profile(&cfrp);
    assert!(environment::tg_shift_c(&ctx) < -8.0);
    assert!(environment::workpiece_limit_c(&ctx).unwrap() < ctx.wp.temp_limit_c.unwrap());
}

#[test]
fn neck_and_substrate_change_tool_physics() {
    let wp = WorkpieceProps::of(&endmill_model::cutting::WorkpieceMaterial::CarbonSteel);
    let m = MachineLimits::default();
    let mut plain = EndMillMockupSetting::default_6mm_ball_long_neck();
    plain.neck_diameter_mm = None;
    plain.reach_mm = None;
    plain.stickout_mm = Some(24.0);
    let mut neck = EndMillMockupSetting::default_6mm_ball_long_neck();
    neck.stickout_mm = Some(24.0);
    assert!(neck.validate().is_ok());
    let tp = ToolGeometry::from_setting(&plain, &wp, &m, Some(&plain.substrate.base()));
    let tn = ToolGeometry::from_setting(&neck, &wp, &m, Some(&neck.substrate.base()));
    let kp = tool_compliance(&tp, 0.3, 1.0).stiffness_n_per_um;
    let kn = tool_compliance(&tn, 0.3, 1.0).stiffness_n_per_um;
    assert!(kn < kp * 0.95, "neck {} plain {}", kn, kp);
    let mut bad = neck.clone();
    bad.neck_diameter_mm = Some(6.5);
    assert!(bad.validate().is_err());
    bad.neck_diameter_mm = Some(5.5);
    bad.reach_mm = None;
    assert!(bad.validate().is_err());
    let mut hss = EndMillMockupSetting::default_10mm_4flute();
    hss.substrate = ToolSubstrate::Hss;
    hss.coating_name = None;
    let carbide = EndMillMockupSetting::default_10mm_4flute();
    let th = ToolGeometry::from_setting(&hss, &wp, &m, Some(&hss.substrate.base()));
    let tc = ToolGeometry::from_setting(&carbide, &wp, &m, Some(&carbide.substrate.base()));
    assert!(tool_compliance(&th, 5.0, 1.0).stiffness_n_per_um < 0.5 * tool_compliance(&tc, 5.0, 1.0).stiffness_n_per_um);
    let mut p = profile(MachiningPreset::default_steel_general());
    let life_c = analyze(&p).wear.tool_life_min;
    p.endmill_setting = hss.clone();
    let a = analyze(&p);
    assert!(a.wear.tool_life_min < life_c * 0.5, "hss {} carbide {}", a.wear.tool_life_min, life_c);
    let mut tuned = carbide.clone();
    tuned.rake_deg = Some(18.0);
    tuned.edge_radius_um = Some(3.0);
    let tt = ToolGeometry::from_setting(&tuned, &wp, &m, None);
    assert_eq!(tt.rake_deg, 18.0);
    assert_eq!(tt.edge_radius_um, 3.0);
}

#[test]
fn library_identity_keeps_old_keys_and_round_trips_new_fields() {
    let a = EndMillAttr::from_setting(&EndMillMockupSetting::default_10mm_4flute());
    assert_eq!(a.key(), "em:d10.00:z4:loc25.0:oal75.0:sh10.0:hx40:square:c-naco:vp1:carbide");
    let n = EndMillMockupSetting::default_6mm_ball_long_neck();
    let an = EndMillAttr::from_setting(&n);
    assert!(an.key().contains(":nk5.70x18.0") && an.key().contains(":carbide_uf"));
    let back = an.to_setting(Some(&EndMillAttr::alias_of(&n)));
    assert_eq!(back.neck(), n.neck());
    assert_eq!(back.substrate, ToolSubstrate::CarbideUltraFine);
    assert_eq!(back.rake_deg, n.rake_deg);
    assert!(back.validate().is_ok());
}

#[test]
fn calculation_reflects_profile_tool_coolant_and_overrides() {
    let p = profile(MachiningPreset::default_titanium_finishing());
    let (_, base) = calc::run(&p, &CalcOverrides::default(), None).unwrap();
    assert!(base.overrides.is_empty());
    let (_, dry) = calc::run(&p, &CalcOverrides { coolant: Some("dry".into()), ..Default::default() }, None).unwrap();
    assert!(dry.analysis.interface_c > base.analysis.interface_c + 20.0, "dry {} tt {}", dry.analysis.interface_c, base.analysis.interface_c);
    assert_eq!(dry.overrides.len(), 1);
    let mut tin = p.clone();
    tin.endmill_setting.coating_name = Some("TiN".into());
    let (_, t) = calc::run(&tin, &CalcOverrides::default(), None).unwrap();
    assert!(t.analysis.tool_life_min.unwrap() < base.analysis.tool_life_min.unwrap());
    let al = profile(MachiningPreset::default_aluminum_roughing());
    let (_, a) = calc::run(&al, &CalcOverrides::default(), None).unwrap();
    assert!(a.recommended.cutting_speed_m_min > 2.0 * base.recommended.cutting_speed_m_min);
    let (_, d12) = calc::run(&p, &CalcOverrides { diameter_mm: Some(12.0), flute_count: Some(5), ..Default::default() }, None).unwrap();
    assert_eq!(d12.context.diameter_mm, 12.0);
    assert_eq!(d12.overrides.len(), 2);
    assert!(d12.recommended.spindle_rpm < base.recommended.spindle_rpm);
    let hot = with_env(p.clone(), 32.0, 50.0);
    let (_, h) = calc::run(&hot, &CalcOverrides::default(), None).unwrap();
    assert!(h.environment.coolant_c > base.environment.coolant_c + 9.0);
    let (applied, out) = calc::apply(&p, &CalcOverrides { purpose: Some("roughing".into()), ..Default::default() }, None).unwrap();
    assert_eq!(applied.conditions.spindle_rpm, out.recommended.spindle_rpm);
    assert_eq!(out.purpose, "roughing");
    assert!(calc::run(&al, &CalcOverrides { workpiece: Some("alloy_steel".into()), ..Default::default() }, None).is_err());
    let (_, inherit) = calc::run(&p, &CalcOverrides { workpiece: Some("alloy_steel".into()), ..Default::default() }, None).unwrap();
    assert!(inherit.context.workpiece_label.contains("36"));
    let (_, hard) = calc::run(&p, &CalcOverrides { workpiece: Some("alloy_steel".into()), hardness_hrc: Some(55), ..Default::default() }, None).unwrap();
    assert!(hard.context.workpiece_label.contains("55"));
}

#[test]
fn residual_heat_lives_only_after_contact_and_decays() {
    let preset = calc::recommend_preset(&MachiningPreset::default_titanium_finishing(), &ShopEnvironment::default());
    let p = profile(preset);
    let ctx = CutContext::from_profile(&p);
    let pat = ToolPathPattern::from_key("pocket_zigzag", &p).unwrap();
    let r = loadsim::simulate(&p, &pat, &ctx);
    assert_eq!(r.segment_end_s.len(), endmill_model::gcode::GCodeGenerator::generate_synthetic_segments_with_pattern(&p, &pat).len());
    assert!(r.segment_end_s.windows(2).all(|w| w[1] >= w[0]));
    assert!((r.segment_end_s.last().unwrap() - r.total_time_min * 60.0).abs() < 1e-6);
    assert!(r.samples.iter().filter(|s| s.force_n <= 0.0).all(|s| s.wp_heat_w == 0.0 && s.preheat_c == 0.0));
    assert!(r.samples.iter().any(|s| s.wp_heat_w > 0.0 && (s.contact_dx.abs() + s.contact_dy.abs()) > 0.5));
    assert!(!r.heat_packets.is_empty());
    let pk = r.heat_packets[r.heat_packets.len() / 2];
    let at = |age: f64| r.heat.excess_at(&pk, pk.x, pk.y, pk.z, pk.t_s + age);
    assert!(at(0.0) > at(1.0) && at(1.0) > at(5.0) && at(5.0) > at(30.0));
    assert!(at(30.0) < 0.05 * at(0.0));
    let near = r.heat.excess_at(&pk, pk.x + 1.0, pk.y, pk.z, pk.t_s + 1.0);
    let far = r.heat.excess_at(&pk, pk.x + 10.0, pk.y, pk.z, pk.t_s + 1.0);
    assert!(near > far);
    assert!(r.max_preheat_c > 1.0);
    assert!(r.samples.iter().all(|s| s.wp_temp_c >= r.bulk_c - 1e-9));
    let vs = viewport::assemble(&p, pat.key(), &ctx, r);
    assert!(vs.samples.len() <= viewport::VIEWPORT_MAX_SAMPLES + 600);
    assert!(vs.decay.windows(2).all(|w| w[1].1 <= w[0].1 + 1e-9));
    assert!(vs.checks.len() >= 3);
}

#[test]
fn builtin_presets_cover_materials_and_sync_into_library() {
    let presets = MachiningPreset::builtin();
    assert_eq!(presets.len(), 8);
    let families: std::collections::BTreeSet<&str> = presets.iter().map(|p| p.workpiece_setup.effective_material().family_key()).collect();
    assert!(families.len() >= 7, "{:?}", families);
    for p in presets.iter() {
        assert!(p.endmill_setting.validate().is_ok(), "{}", p.name);
        assert!(p.purpose.is_some());
    }
    let dir = std::env::temp_dir().join(format!("endmill_builtin_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut st = Store::open(&dir).unwrap();
    st.sync_builtin_presets(&presets[..3]).unwrap();
    let added = st.ensure_builtin_presets(&presets).unwrap();
    assert_eq!(added.len(), 5);
    assert!(st.ensure_builtin_presets(&presets).unwrap().is_empty());
    let rows = st.rdb.list_presets().unwrap();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|r| r.builtin));
    let neck = rows.iter().find(|r| r.name == "금형강 고경도 정삭 프리셋").unwrap();
    assert!(neck.preset.endmill_setting.neck().is_some());
    assert!(neck.preset.conditions.axial_doc_mm < 1.0, "ball finishing ap {}", neck.preset.conditions.axial_doc_mm);
    let used = rows[0].endmill_id;
    assert!(st.delete_endmill(used).is_err());
    let mut custom = EndMillMockupSetting::default_8mm_3flute_dlc();
    custom.diameter_mm = 7.0;
    custom.name = "임시".into();
    let id = st.index_endmill(&custom).unwrap();
    assert!(st.delete_endmill(id).unwrap());
    assert!(st.rdb.endmill(id).unwrap().is_none());
    drop(st);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preset_purpose_drives_profile_calculation() {
    let preset = calc::recommend_preset(&MachiningPreset::default_superalloy_roughing(), &ShopEnvironment::default());
    let p = MachiningProfile::from_preset(&preset, "인코넬 황삭 프리셋");
    assert_eq!(p.purpose.as_deref(), Some("roughing"));
    let (_, out) = calc::run(&p, &CalcOverrides::default(), None).unwrap();
    assert_eq!(out.purpose, "roughing");
    assert_eq!(out.context.purpose, "roughing");
    let mut legacy = p.clone();
    legacy.purpose = None;
    legacy.name = "티타늄 정삭 (구버전)".into();
    legacy.normalize_legacy();
    assert_eq!(legacy.purpose.as_deref(), Some("finishing"));
    let (applied, fin) = calc::apply(&p, &CalcOverrides { purpose: Some("finishing".into()), ..Default::default() }, None).unwrap();
    assert_eq!(fin.purpose, "finishing");
    assert_eq!(applied.purpose.as_deref(), Some("finishing"));
    assert!(fin.recommended.radial_doc_mm < out.recommended.radial_doc_mm, "fin {} rough {}", fin.recommended.radial_doc_mm, out.recommended.radial_doc_mm);
}

#[test]
fn coolant_method_keys_cover_all() {
    for m in [CoolantMethod::AirBlast, CoolantMethod::Flood, CoolantMethod::Mist, CoolantMethod::ThroughTool, CoolantMethod::Dry] {
        let c = CoolantConfig::from_key(m.key()).unwrap();
        assert_eq!(c.method, m);
    }
}
