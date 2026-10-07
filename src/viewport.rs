use crate::gcode::{GCodeGenerator, ToolPathPattern, ToolPathSegment};
use crate::loadsim::{self, HeatFieldProps, HeatPacket, SimSample};
use crate::physics::CutContext;
use crate::pipeline::calibrated_context;
use crate::profile::{CoolantConfig, CoolantMethod, MachiningProfile, ShopEnvironment, ToolNose};
use crate::sds::SdsStore;
use crate::workpiece_setup::StockShape;
use serde::{Deserialize, Serialize};

pub const VIEWPORT_MAX_CELLS: usize = 600_000;
pub const VIEWPORT_MAX_SAMPLES: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolShape {
    pub diameter_mm: f64,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_mm: f64,
    pub stickout_mm: f64,
    pub flutes: u8,
    pub helix_deg: f64,
    pub nose: String,
    pub corner_r_mm: f64,
    pub neck_diameter_mm: f64,
    pub reach_mm: f64,
    pub coating: String,
    pub color_hex: String,
    pub rpm: u32,
}

impl ToolShape {
    pub fn of(p: &MachiningProfile) -> Self {
        let s = &p.endmill_setting;
        let family = crate::physics::tribology_from_name(s.coating_name.as_deref()).family;
        let color = match family.as_str() {
            "TiN" | "ZrN" => "#d4af37",
            "TiCN" => "#7d6a8a",
            "AlTiN" | "nACo" | "HiPIMS" => "#4b4a5c",
            "TiAlN" => "#6a5a86",
            "AlCrN" | "CrN" => "#8c96a3",
            "TiSiN" => "#a07a4a",
            "DLC" => "#2b2b30",
            "Diamond" => "#9aa4ad",
            _ => "#b8bcc2",
        };
        Self {
            diameter_mm: s.diameter_mm,
            loc_mm: s.loc_mm,
            oal_mm: s.oal_mm,
            shank_mm: s.shank_diameter_mm,
            stickout_mm: s.effective_stickout_mm(),
            flutes: s.flute_count,
            helix_deg: s.helix_angle_deg,
            nose: match s.nose {
                ToolNose::Square => "square".into(),
                ToolNose::Ball => "ball".into(),
                ToolNose::CornerRadius { .. } => "corner".into(),
            },
            corner_r_mm: s.nose.corner_radius(s.diameter_mm),
            neck_diameter_mm: s.neck().map(|(d, _)| d).unwrap_or(0.0),
            reach_mm: s.neck().map(|(_, r)| r).unwrap_or(0.0),
            coating: family,
            color_hex: color.into(),
            rpm: p.conditions.spindle_rpm,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolantVisual {
    pub key: String,
    pub label: String,
    pub m_code: String,
    pub spray_type: String,
    pub color_hex: String,
    pub pressure_bar: f64,
    pub flow_rate_l_min: f64,
    pub nozzle_count: u8,
    pub nozzle_angle_deg: f64,
    pub nozzle_distance_mm: f64,
    pub temperature_c: f64,
    pub temperature_auto: bool,
}

impl CoolantVisual {
    pub fn of(cfg: &CoolantConfig, env: &ShopEnvironment) -> Self {
        let (spray, color) = match cfg.method {
            CoolantMethod::AirBlast => ("gas", "#87CEEB"),
            CoolantMethod::Flood => ("liquid_stream", "#4169E1"),
            CoolantMethod::Mist => ("mist_particles", "#ADD8E6"),
            CoolantMethod::ThroughTool => ("internal_jet", "#00CED1"),
            CoolantMethod::Dry => ("none", "#666666"),
        };
        Self {
            key: cfg.method.key().into(),
            label: cfg.method.label(),
            m_code: cfg.method.gcode_m_code().into(),
            spray_type: spray.into(),
            color_hex: color.into(),
            pressure_bar: cfg.pressure_bar,
            flow_rate_l_min: cfg.flow_rate_l_min,
            nozzle_count: cfg.nozzle_count,
            nozzle_angle_deg: cfg.nozzle_angle_deg,
            nozzle_distance_mm: cfg.nozzle_distance_mm,
            temperature_c: cfg.temperature_in(env),
            temperature_auto: cfg.temperature_c.is_none(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockView {
    pub width_mm: f64,
    pub height_mm: f64,
    pub thickness_mm: f64,
    pub cylindrical: bool,
    pub diameter_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportSim {
    pub profile_name: String,
    pub pattern: String,
    pub tool: ToolShape,
    pub coolant: CoolantVisual,
    pub stock: StockView,
    pub samples: Vec<SimSample>,
    pub segment_end_s: Vec<f64>,
    pub total_s: f64,
    pub cut_s: f64,
    pub heat_packets: Vec<HeatPacket>,
    pub heat: HeatFieldProps,
    pub bulk_c: f64,
    pub ambient_c: f64,
    pub max_force_n: f64,
    pub max_interface_c: f64,
    pub max_local_c: f64,
    pub max_preheat_c: f64,
    pub decay: Vec<(f64, f64)>,
    pub checks: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn build(profile: &MachiningProfile, pattern_key: &str, sds: Option<&SdsStore>) -> Result<ViewportSim, String> {
    profile.endmill_setting.validate()?;
    let pattern = ToolPathPattern::from_key(pattern_key, profile)?;
    let segments = GCodeGenerator::generate_synthetic_segments_with_pattern(profile, &pattern);
    if segments.is_empty() {
        return Err("시뮬레이션할 이동 명령이 없습니다".into());
    }
    let (ctx, _) = calibrated_context(profile, sds);
    let rep = loadsim::simulate_segments_res(profile, &segments, pattern.key(), &ctx, VIEWPORT_MAX_CELLS);
    Ok(assemble(profile, pattern.key(), &ctx, rep))
}

pub fn build_with_context(profile: &MachiningProfile, pattern_key: &str, ctx: &CutContext) -> Result<ViewportSim, String> {
    profile.endmill_setting.validate()?;
    let pattern = ToolPathPattern::from_key(pattern_key, profile)?;
    let segments = GCodeGenerator::generate_synthetic_segments_with_pattern(profile, &pattern);
    if segments.is_empty() {
        return Err("시뮬레이션할 이동 명령이 없습니다".into());
    }
    let rep = loadsim::simulate_segments_res(profile, &segments, pattern.key(), ctx, VIEWPORT_MAX_CELLS);
    Ok(assemble(profile, pattern.key(), ctx, rep))
}

pub fn build_report_prog(
    profile: &MachiningProfile,
    pattern_key: &str,
    ctx: &CutContext,
    progress: &mut dyn FnMut(f64) -> bool,
) -> Result<(ViewportSim, loadsim::LoadSimReport, Vec<ToolPathSegment>), String> {
    profile.endmill_setting.validate()?;
    let pattern = ToolPathPattern::from_key(pattern_key, profile)?;
    let segments = GCodeGenerator::generate_synthetic_segments_with_pattern(profile, &pattern);
    if segments.is_empty() {
        return Err("시뮬레이션할 이동 명령이 없습니다".into());
    }
    let rep = loadsim::simulate_segments_prog(profile, &segments, pattern.key(), ctx, VIEWPORT_MAX_CELLS, progress)?;
    let view = assemble(profile, pattern.key(), ctx, rep.clone());
    Ok((view, rep, segments))
}

pub fn assemble(profile: &MachiningProfile, pattern: &str, ctx: &CutContext, rep: loadsim::LoadSimReport) -> ViewportSim {
    let cut: Vec<&SimSample> = rep.samples.iter().filter(|s| s.force_n > 0.0).collect();
    let air: Vec<&SimSample> = rep.samples.iter().filter(|s| s.force_n <= 0.0).collect();
    let air_force = air.iter().map(|s| s.force_n.abs() + s.wp_heat_w.abs()).fold(0.0, f64::max);
    let max_local = rep.samples.iter().map(|s| s.wp_local_c).fold(f64::NEG_INFINITY, f64::max);
    let mut decay = Vec::new();
    if let Some(last) = rep.heat_packets.iter().rev().find(|p| p.q_j > 0.0) {
        for age in [0.0, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0] {
            let t = last.t_s + age;
            let ex: f64 = rep.heat_packets.iter().map(|p| rep.heat.excess_at(p, last.x, last.y, last.z, t)).sum();
            decay.push((age, ex));
        }
    }
    let mut checks = vec![
        format!(
            "절삭력·응력은 소재가 실제로 제거되는 샘플에서만 계산됩니다: 절삭 {}개 · 공중/급이송 {}개 (공중 구간 최대 힘·입열 {:.3})",
            cut.len(),
            air.len(),
            air_force
        ),
        format!(
            "입열은 접촉 지점의 열 패킷 {}개로 남아 3차원 확산(열확산율 {:.1} mm²/s)과 표면 냉각(h {:.0} W/m²K)으로 감쇠하고, 소재 전체는 덩어리 온도 {:.1} → {:.1} °C 로 따로 누적·냉각됩니다",
            rep.heat_packets.len(),
            rep.heat.kappa_m2_s * 1e6,
            rep.heat.h_surface,
            rep.bulk_c,
            rep.wp_temp_end_c
        ),
    ];
    if decay.len() >= 5 {
        checks.push(format!(
            "마지막 절삭 지점의 잔류 과열: 직후 {:.0} K → 1초 {:.0} K → 5초 {:.1} K → 30초 {:.2} K (공구가 지나간 뒤 감쇠)",
            decay[0].1, decay[2].1, decay[4].1, decay[6].1
        ));
    }
    if rep.max_preheat_c > 1.0 {
        checks.push(format!(
            "이전 패스 잔류열에 의한 절삭 지점 예열 최대 {:.0} K 를 날끝 온도 계산의 시작 온도에 반영 (예열 소재의 유동응력 감소도 함께 반영)",
            rep.max_preheat_c
        ));
    }
    let stride = (rep.samples.len() / VIEWPORT_MAX_SAMPLES).max(1);
    let last = rep.samples.len().saturating_sub(1);
    let mut samples: Vec<SimSample> = Vec::with_capacity(rep.samples.len() / stride + 2);
    let mut prev_cut = false;
    for (i, s) in rep.samples.iter().enumerate() {
        let is_cut = s.force_n > 0.0;
        if i % stride == 0 || i == last || is_cut != prev_cut {
            samples.push(s.clone());
        }
        prev_cut = is_cut;
    }
    let w = &profile.workpiece_setup;
    let (cyl, dia) = match w.shape {
        StockShape::Cylindrical { diameter_mm } => (true, diameter_mm),
        _ => (false, 0.0),
    };
    ViewportSim {
        profile_name: profile.name.clone(),
        pattern: pattern.into(),
        tool: ToolShape::of(profile),
        coolant: CoolantVisual::of(&profile.coolant_config, &profile.machine.environment),
        stock: StockView {
            width_mm: w.width_mm,
            height_mm: w.height_mm,
            thickness_mm: w.thickness_mm,
            cylindrical: cyl,
            diameter_mm: dia,
        },
        total_s: rep.total_time_min * 60.0,
        cut_s: rep.cut_time_min * 60.0,
        segment_end_s: rep.segment_end_s.clone(),
        heat_packets: rep.heat_packets.clone(),
        heat: rep.heat,
        bulk_c: rep.bulk_c,
        ambient_c: ctx.env.ambient_c,
        max_force_n: rep.max_force_n,
        max_interface_c: rep.max_temp_c,
        max_local_c: if max_local.is_finite() { max_local } else { rep.bulk_c },
        max_preheat_c: rep.max_preheat_c,
        decay,
        checks,
        warnings: rep.warnings.clone(),
        samples,
    }
}
