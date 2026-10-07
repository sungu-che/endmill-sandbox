use crate::cutting::{CuttingConditions, WorkpieceMaterial};
use crate::environment::EnvironmentReport;
use crate::ingest::material_from_key;
use crate::physics::{self, CutAnalysis, ThermalBody};
use crate::pipeline::{calibrated_context, coolant_type_of, default_purpose, purpose_from_key, purpose_key, CalibrationApplied};
use crate::profile::{CoolantConfig, MachiningProfile, ToolNose};
use crate::sds::SdsStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalcOverrides {
    #[serde(default)]
    pub workpiece: Option<String>,
    #[serde(default)]
    pub hardness_hrc: Option<u8>,
    #[serde(default)]
    pub diameter_mm: Option<f64>,
    #[serde(default)]
    pub flute_count: Option<u8>,
    #[serde(default)]
    pub is_high_end: Option<bool>,
    #[serde(default)]
    pub coolant: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalcContext {
    pub profile_name: String,
    pub tool_name: String,
    pub tool_label: String,
    pub coating: String,
    pub substrate: String,
    pub workpiece_key: String,
    pub workpiece_label: String,
    pub hardness_hrc: Option<u8>,
    pub diameter_mm: f64,
    pub flute_count: u8,
    pub is_high_end: bool,
    pub coolant_key: String,
    pub coolant_label: String,
    pub coolant_c: f64,
    pub coolant_auto: bool,
    pub purpose: String,
    pub environment_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CalcSummary {
    pub mrr_cm3_min: f64,
    pub force_peak_n: f64,
    pub force_mean_n: f64,
    pub torque_peak_nm: f64,
    pub spindle_power_kw: f64,
    pub available_power_kw: f64,
    pub deflection_peak_um: f64,
    pub wall_deflection_um: f64,
    pub interface_dry_c: f64,
    pub interface_c: f64,
    pub coolant_reduction_pct: f64,
    pub tool_body_c: f64,
    pub tool_life_min: Option<f64>,
    pub vb_rate_mm_per_min: f64,
    pub mu_eff: f64,
    pub dominant_wear: String,
    pub dominant_share: f64,
    pub thermal_crack_risk: f64,
    pub bue_risk: f64,
    pub chatter_margin: Option<f64>,
    pub stable: bool,
    pub error_budget_um: f64,
    pub environment_um: f64,
    pub tolerance_um: f64,
    pub utilization: f64,
    pub ra_um: f64,
    pub compat: Vec<String>,
}

impl CalcSummary {
    pub fn of(a: &CutAnalysis) -> Self {
        let dom = a.wear.mechanisms.iter().find(|m| m.key == a.wear.dominant);
        Self {
            mrr_cm3_min: a.mrr_cm3_min,
            force_peak_n: a.forces.f_res_peak,
            force_mean_n: a.forces.f_res_mean,
            torque_peak_nm: a.forces.torque_peak_nm,
            spindle_power_kw: a.spindle_power_kw,
            available_power_kw: a.available_power_kw,
            deflection_peak_um: a.deflection_peak_um,
            wall_deflection_um: a.wall.deflection_um,
            interface_dry_c: a.thermal.interface_dry_c,
            interface_c: a.thermal.interface_c,
            coolant_reduction_pct: a.thermal.coolant_reduction_pct,
            tool_body_c: a.thermal.tool_body_c,
            tool_life_min: Some(a.wear.tool_life_min).filter(|v| v.is_finite()),
            vb_rate_mm_per_min: a.wear.vb_rate_mm_per_min,
            mu_eff: a.coeffs.mu,
            dominant_wear: dom.map(|m| m.label.clone()).unwrap_or_default(),
            dominant_share: dom.map(|m| m.share).unwrap_or(0.0),
            thermal_crack_risk: a.thermal.thermal_crack_risk,
            bue_risk: a.thermal.bue_risk,
            chatter_margin: Some(a.stability.margin).filter(|v| v.is_finite()),
            stable: a.stability.stable || !a.stability.margin.is_finite(),
            error_budget_um: a.error_budget_um.total_um,
            environment_um: a.error_budget_um.environment_um,
            tolerance_um: a.error_budget_um.tolerance_um,
            utilization: a.error_budget_um.utilization,
            ra_um: a.surface.wall_ra_um,
            compat: a.tribology.compat.messages.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalcOutcome {
    pub context: CalcContext,
    pub overrides: Vec<String>,
    pub purpose: String,
    pub recommended: CuttingConditions,
    pub current: CuttingConditions,
    pub analysis: CalcSummary,
    pub current_analysis: CalcSummary,
    pub environment: EnvironmentReport,
    pub calibration: Vec<CalibrationApplied>,
    pub notes: Vec<String>,
}

pub fn workpiece_key(m: &WorkpieceMaterial) -> &'static str {
    m.family_key()
}

pub fn context_of(p: &MachiningProfile, purpose: Option<&str>) -> CalcContext {
    let s = &p.endmill_setting;
    let env = &p.machine.environment;
    let material = p.workpiece_setup.effective_material();
    let hardness = match material {
        WorkpieceMaterial::AlloySteel { hardness_hrc } => Some(hardness_hrc),
        _ => p.workpiece_setup.hardness_hrc,
    };
    let purpose = purpose.map(|k| purpose_key(purpose_from_key(k))).unwrap_or_else(|| purpose_key(default_purpose(p)));
    CalcContext {
        profile_name: p.name.clone(),
        tool_name: s.name.clone(),
        tool_label: format!("Ø{} {}날 {}", fmt_num(s.diameter_mm), s.flute_count, s.shape_label()),
        coating: s.coating_name.clone().unwrap_or_else(|| "무코팅".into()),
        substrate: s.substrate.label().into(),
        workpiece_key: workpiece_key(&material).into(),
        workpiece_label: p.workpiece_setup.material_label(),
        hardness_hrc: hardness,
        diameter_mm: s.diameter_mm,
        flute_count: s.flute_count,
        is_high_end: s.is_high_end,
        coolant_key: p.coolant_config.method.key().into(),
        coolant_label: p.coolant_config.method.label(),
        coolant_c: p.coolant_config.temperature_in(env),
        coolant_auto: p.coolant_config.temperature_c.is_none(),
        purpose: purpose.into(),
        environment_label: env.label(),
    }
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{:.0}", v)
    } else {
        format!("{:.2}", v).trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub fn apply_overrides(profile: &MachiningProfile, ov: &CalcOverrides) -> Result<(MachiningProfile, Vec<String>), String> {
    let mut p = profile.clone();
    let mut changes = Vec::new();
    let cur_material = p.workpiece_setup.effective_material();
    if let Some(key) = ov.workpiece.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        let hrc = ov.hardness_hrc.or(p.workpiece_setup.hardness_hrc);
        let next = material_from_key(key, hrc)?;
        if next.key() != cur_material.key() {
            let before = p.workpiece_setup.material_label();
            let family_changed = workpiece_key(&next) != workpiece_key(&cur_material);
            p.workpiece_setup.material = next.clone();
            match next {
                WorkpieceMaterial::AlloySteel { hardness_hrc } => p.workpiece_setup.hardness_hrc = Some(hardness_hrc),
                _ => {
                    if family_changed {
                        p.workpiece_setup.hardness_hrc = None;
                    }
                }
            }
            changes.push(format!("피삭재 {} → {}", before, p.workpiece_setup.material_label()));
        }
    }
    if let Some(d) = ov.diameter_mm {
        if !(d.is_finite() && d > 0.0) {
            return Err("직경은 0보다 커야 합니다".into());
        }
        let s = &mut p.endmill_setting;
        if (d - s.diameter_mm).abs() > 1e-6 {
            changes.push(format!("직경 {} → {} mm", fmt_num(s.diameter_mm), fmt_num(d)));
            let k = d / s.diameter_mm.max(1e-6);
            s.diameter_mm = d;
            if s.shank_diameter_mm < d {
                s.shank_diameter_mm = d;
            }
            if let ToolNose::CornerRadius { radius_mm } = s.nose {
                s.nose = ToolNose::CornerRadius { radius_mm: radius_mm.min(d / 2.0) };
            }
            if let Some(nd) = s.neck_diameter_mm {
                let scaled = nd * k;
                if scaled >= d || scaled <= 0.3 * d {
                    s.neck_diameter_mm = None;
                    s.reach_mm = None;
                } else {
                    s.neck_diameter_mm = Some(scaled);
                }
            }
        }
    }
    if let Some(z) = ov.flute_count {
        let s = &mut p.endmill_setting;
        if z != s.flute_count {
            changes.push(format!("날 수 {} → {}", s.flute_count, z));
            s.flute_count = z;
        }
    }
    if let Some(h) = ov.is_high_end {
        let s = &mut p.endmill_setting;
        if h != s.is_high_end {
            changes.push(format!("공구 등급 {} → {}", if s.is_high_end { "고급형" } else { "보급형" }, if h { "고급형" } else { "보급형" }));
            s.is_high_end = h;
        }
    }
    if let Some(key) = ov.coolant.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        let cfg = CoolantConfig::from_key(key).ok_or_else(|| format!("지원하지 않는 냉각: {}", key))?;
        if cfg.method != p.coolant_config.method {
            changes.push(format!("냉각 {} → {}", p.coolant_config.method.label(), cfg.method.label()));
            p.coolant_config = cfg;
            p.conditions.coolant = coolant_type_of(&p.coolant_config.method);
        }
    }
    p.endmill_setting.validate()?;
    Ok((p, changes))
}

pub fn run(profile: &MachiningProfile, ov: &CalcOverrides, sds: Option<&SdsStore>) -> Result<(MachiningProfile, CalcOutcome), String> {
    let (p, changes) = apply_overrides(profile, ov)?;
    let purpose = ov
        .purpose
        .as_deref()
        .filter(|k| !k.trim().is_empty())
        .map(purpose_from_key)
        .unwrap_or_else(|| default_purpose(&p));
    let (ctx, calibration) = calibrated_context(&p, sds);
    let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
    let rec = physics::recommend(&ctx, p.endmill_setting.is_high_end, body, purpose);
    let mut conds = rec.conditions.clone();
    conds.coolant = coolant_type_of(&p.coolant_config.method);
    let a = physics::analyze_cut(&ctx, &conds, true, body);
    let mut current = p.conditions.clone();
    current.coolant = conds.coolant.clone();
    let a_cur = physics::analyze_cut(&ctx, &current, true, body);
    let mut notes: Vec<String> = Vec::new();
    for n in rec.notes.iter().chain(a.notes.iter()) {
        if !notes.contains(n) {
            notes.push(n.clone());
        }
    }
    let key = purpose_key(purpose);
    let outcome = CalcOutcome {
        context: context_of(&p, Some(key)),
        overrides: changes,
        purpose: key.into(),
        recommended: conds,
        current,
        analysis: CalcSummary::of(&a),
        current_analysis: CalcSummary::of(&a_cur),
        environment: a.environment.clone(),
        calibration,
        notes,
    };
    Ok((p, outcome))
}

pub fn apply(profile: &MachiningProfile, ov: &CalcOverrides, sds: Option<&SdsStore>) -> Result<(MachiningProfile, CalcOutcome), String> {
    let (mut p, outcome) = run(profile, ov, sds)?;
    p.conditions = outcome.recommended.clone();
    if ov.purpose.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
        p.purpose = Some(outcome.purpose.clone());
    }
    p.modified_at = format!("{}", crate::store::now_ms() / 1000);
    Ok((p, outcome))
}

pub fn recommend_preset(preset: &crate::profile::MachiningPreset, env: &crate::profile::ShopEnvironment) -> crate::profile::MachiningPreset {
    let mut profile = MachiningProfile::from_preset(preset, &preset.name);
    profile.machine.environment = env.clone();
    let purpose = preset.purpose.as_deref().map(purpose_from_key).unwrap_or_else(|| default_purpose(&profile));
    let ctx = crate::physics::CutContext::from_profile(&profile);
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let rec = physics::recommend(&ctx, profile.endmill_setting.is_high_end, body, purpose);
    let mut out = preset.clone();
    let mut c = rec.conditions;
    c.coolant = coolant_type_of(&preset.coolant_config.method);
    out.conditions = c;
    out
}
