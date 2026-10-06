use endmill_model::cutting::{CuttingCalculator, WorkpieceMaterial};
use endmill_model::physics::{self, CutContext, ThermalBody};
use endmill_model::profile::{CoolantConfig, CoolantMethod, MachiningPreset, MachiningProfile};

fn main() {
    let mats = [
        ("AL6061", WorkpieceMaterial::Aluminum),
        ("S45C", WorkpieceMaterial::CarbonSteel),
        ("HRC50", WorkpieceMaterial::AlloySteel { hardness_hrc: 50 }),
        ("SUS304", WorkpieceMaterial::StainlessSteel),
        ("Ti64", WorkpieceMaterial::Titanium),
        ("IN718", WorkpieceMaterial::Inconel),
        ("CFRP", WorkpieceMaterial::CFRP),
    ];
    let coatings = ["", "TiN", "AlTiN", "AlCrN", "nACo", "DLC", "Diamond"];
    let coolants = [CoolantMethod::Dry, CoolantMethod::AirBlast, CoolantMethod::Mist, CoolantMethod::Flood, CoolantMethod::ThroughTool];
    println!(
        "{:<7} {:<8} {:<12} {:>5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8}  {:<16} {}",
        "소재", "코팅", "냉각", "μ", "접근", "°C", "저감%", "열균열", "BUE", "수명", "지배 마모", "비등/궁합"
    );
    for (mk, m) in mats.iter() {
        for c in coatings.iter() {
            for cm in coolants.iter() {
                let mut p = MachiningProfile::from_preset(&MachiningPreset::default_steel_general(), "probe");
                p.workpiece_setup.material = m.clone();
                if let WorkpieceMaterial::AlloySteel { hardness_hrc } = m {
                    p.workpiece_setup.hardness_hrc = Some(*hardness_hrc);
                }
                p.endmill_setting.coating_name = if c.is_empty() { None } else { Some(c.to_string()) };
                p.coolant_config = CoolantConfig::from_method(cm);
                p.conditions = CuttingCalculator::recommend_conditions(m, 10.0, 4, true).unwrap();
                let ctx = CutContext::from_profile(&p);
                let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
                let a = physics::analyze_cut(&ctx, &p.conditions, true, body);
                let dom = a
                    .wear
                    .mechanisms
                    .iter()
                    .find(|x| x.key == a.wear.dominant)
                    .map(|x| format!("{} {:.0}%", x.label, x.share * 100.0))
                    .unwrap_or_default();
                println!(
                    "{:<7} {:<8} {:<12} {:>5.2} {:>6.2} {:>6.0} {:>6.1} {:>6.2} {:>6.2} {:>8.1}  {:<16} {}{}",
                    mk,
                    if c.is_empty() { "무코팅" } else { c },
                    cm.key(),
                    a.coeffs.mu,
                    a.coeffs.lubricant_access,
                    a.thermal.interface_c,
                    a.thermal.coolant_reduction_pct,
                    a.thermal.thermal_crack_risk,
                    a.thermal.bue_risk,
                    a.wear.tool_life_min,
                    dom,
                    a.thermal.cooling.boiling_state,
                    if a.tribology.compat.severity > 0 { format!(" · 궁합 {}", a.tribology.compat.severity) } else { String::new() }
                );
            }
        }
    }
}