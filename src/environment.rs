use crate::physics::{CutContext, ThermalBody};
use crate::profile::{CoolantMethod, ShopEnvironment, STANDARD_HUMIDITY_PCT};
use crate::tribology::{smoothstep, WpChem};
use serde::{Deserialize, Serialize};

pub const MACHINE_SCALE_EXPANSION: f64 = 10.5e-6;
pub const MACHINE_DRIFT_UM_PER_K: f64 = 1.5;
pub const DRIFT_HORIZON_MIN: f64 = 10.0;
pub const CORROSION_RH_PCT: f64 = 60.0;

pub fn moisture_exposure(method: &CoolantMethod) -> f64 {
    match method {
        CoolantMethod::Dry => 1.0,
        CoolantMethod::AirBlast => 0.6,
        CoolantMethod::Mist => 0.3,
        CoolantMethod::Flood | CoolantMethod::ThroughTool => 0.0,
    }
}

pub fn gas_access(vc: f64) -> f64 {
    1.0 / (1.0 + (vc.max(0.0) / 250.0).powi(2))
}

pub fn adsorption_window(t_c: f64) -> f64 {
    1.0 - 0.8 * smoothstep((t_c - 150.0) / 450.0)
}

fn rh_delta(env: &ShopEnvironment) -> f64 {
    (env.humidity_pct - STANDARD_HUMIDITY_PCT) / 100.0
}

pub fn friction_sensitivity(ctx: &CutContext) -> f64 {
    let base = match ctx.tool.coating.family.as_str() {
        "DLC" => 0.6,
        "Diamond" => -0.4,
        _ => -0.15,
    };
    base + if ctx.wp.chem == WpChem::Aluminum { -0.10 } else { 0.0 }
}

pub fn friction_factor(ctx: &CutContext, vc: f64, t_int: f64) -> f64 {
    let x = moisture_exposure(&ctx.coolant.method);
    if x <= 0.0 {
        return 1.0;
    }
    let d = rh_delta(&ctx.env);
    (1.0 + friction_sensitivity(ctx) * d * x * gas_access(vc) * adsorption_window(t_int)).clamp(0.6, 1.4)
}

pub fn oxidation_factor(ctx: &CutContext) -> f64 {
    let x = moisture_exposure(&ctx.coolant.method);
    if x <= 0.0 {
        return 1.0;
    }
    let k = if ctx.tool.coating.protective_oxide { 0.2 } else { 0.6 };
    (1.0 + k * rh_delta(&ctx.env) * x).clamp(0.7, 1.4)
}

pub fn cfrp_moisture_pct(rh_pct: f64) -> f64 {
    1.6 * (rh_pct / 100.0).clamp(0.0, 1.0).powf(1.4)
}

pub fn tg_shift_c(ctx: &CutContext) -> f64 {
    if ctx.wp.chem != WpChem::CarbonFiber {
        return 0.0;
    }
    -15.0 * (cfrp_moisture_pct(ctx.env.humidity_pct) - cfrp_moisture_pct(STANDARD_HUMIDITY_PCT))
}

pub fn workpiece_limit_c(ctx: &CutContext) -> Option<f64> {
    ctx.wp.temp_limit_c.map(|l| l + tg_shift_c(ctx))
}

pub fn scale_error_um(ctx: &CutContext, size_mm: f64) -> f64 {
    let env = &ctx.env;
    size_mm.max(0.0) * 1000.0 * (MACHINE_SCALE_EXPANSION * (env.ambient_c - env.reference_c) - ctx.wp.expansion * (ctx.bulk_c() - env.reference_c))
}

pub fn drift_um(env: &ShopEnvironment) -> f64 {
    MACHINE_DRIFT_UM_PER_K * env.swing_c_per_h.max(0.0) * DRIFT_HORIZON_MIN / 60.0
}

pub fn condensation_risk(ctx: &CutContext) -> bool {
    let dew = ctx.env.dew_point_c();
    let coldest = if ctx.coolant.is_liquid() { ctx.coolant.temperature_c } else { ctx.coolant.ambient_c() };
    coldest < dew + 1.0
}

pub fn corrosion_risk(ctx: &CutContext) -> bool {
    ctx.wp.chem == WpChem::Iron && ctx.wp.key != "stainless" && (ctx.env.humidity_pct >= CORROSION_RH_PCT || condensation_risk(ctx))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvironmentReport {
    pub ambient_c: f64,
    pub humidity_pct: f64,
    pub reference_c: f64,
    pub pressure_kpa: f64,
    pub dew_point_c: f64,
    pub wet_bulb_c: f64,
    pub humidity_ratio_g_kg: f64,
    pub air_density: f64,
    pub coolant_c: f64,
    pub coolant_auto: bool,
    pub coolant_basis: String,
    pub bulk_c: f64,
    pub boiling_c: f64,
    pub moisture_exposure: f64,
    pub friction_factor: f64,
    pub oxidation_factor: f64,
    pub tg_shift_c: f64,
    pub scale_error_um: f64,
    pub drift_um: f64,
    pub condensation: bool,
    pub corrosion: bool,
    pub notes: Vec<String>,
}

pub fn coolant_basis(ctx: &CutContext) -> String {
    let env = &ctx.env;
    if !ctx.coolant.temperature_auto {
        return "프로필에 지정된 냉각 매체 온도".into();
    }
    match ctx.coolant.method {
        CoolantMethod::Flood | CoolantMethod::ThroughTool => match env.chiller_c {
            Some(c) => format!("칠러 설정 {:.1} °C", c),
            None => format!(
                "실내 {:.1} °C · 펌프 일 {:+.1} K · 증발(습구 {:.1} °C 쪽) {:+.1} K",
                env.ambient_c,
                ctx.coolant.temperature_c - env.ambient_c + crate::profile::SUMP_EVAPORATION_PULL * (env.wet_bulb_depression() - env.standard_depression()),
                env.wet_bulb_c(),
                -crate::profile::SUMP_EVAPORATION_PULL * (env.wet_bulb_depression() - env.standard_depression())
            ),
        },
        CoolantMethod::AirBlast | CoolantMethod::Mist => format!(
            "실내 {:.1} °C − 노즐 팽창(줄-톰슨) {:.1} K",
            env.ambient_c,
            env.ambient_c - ctx.coolant.temperature_c
        ),
        CoolantMethod::Dry => format!("실내 공기 {:.1} °C", env.ambient_c),
    }
}

pub fn report(ctx: &CutContext, body: ThermalBody, vc: f64, t_int: f64) -> EnvironmentReport {
    let env = &ctx.env;
    let mut notes = Vec::new();
    let ff = friction_factor(ctx, vc, t_int);
    let of = oxidation_factor(ctx);
    let tg = tg_shift_c(ctx);
    let scale = scale_error_um(ctx, body.size_mm);
    let drift = drift_um(env);
    let condensation = condensation_risk(ctx);
    let corrosion = corrosion_risk(ctx);
    if (ff - 1.0).abs() >= 0.02 {
        notes.push(format!(
            "습도 {:.0}% 에서 {} 계면의 흡착수·수증기 영향으로 마찰계수가 기준(RH 50%) 대비 ×{:.2} 입니다{}",
            env.humidity_pct,
            ctx.tool.coating.family,
            ff,
            if ctx.tool.coating.family == "DLC" { " (수소화 DLC 는 습할수록 마찰 증가)" } else if ctx.tool.coating.family == "Diamond" { " (다이아몬드는 수분이 표면 결합을 안정화해 마찰 감소)" } else { "" }
        ));
    }
    if (of - 1.0).abs() >= 0.03 {
        notes.push(format!("습도에 따른 수증기 산화 활성으로 산화 마모 항이 기준 대비 ×{:.2} 입니다", of));
    }
    if tg.abs() >= 1.0 {
        notes.push(format!(
            "CFRP 평형 함수율 {:.2}% (RH {:.0}%) → 수지 Tg 기준 대비 {:+.0} K, 소재 허용 온도도 같은 만큼 이동합니다",
            cfrp_moisture_pct(env.humidity_pct),
            env.humidity_pct,
            tg
        ));
    }
    if scale.abs() >= 1.0 {
        notes.push(format!(
            "가공 온도(기계 {:.1} °C · 소재 {:.1} °C)와 측정 기준 {:.0} °C 의 열팽창 차이로 {:.0} mm 길이에서 {:+.1} µm 치수 편차가 생깁니다 (측정실 온도에서 확인)",
            env.ambient_c,
            ctx.bulk_c(),
            env.reference_c,
            body.size_mm,
            -scale
        ));
    }
    if drift >= 0.5 {
        notes.push(format!("실내 온도 변동 {:.1} °C/h 로 기계 구조 열변위가 {:.0}분에 {:.1} µm 누적됩니다", env.swing_c_per_h, DRIFT_HORIZON_MIN, drift));
    }
    if condensation {
        notes.push(format!(
            "냉각 매체 {:.1} °C 가 이슬점 {:.1} °C 에 가까워 공구·소재·기계 표면에 결로가 생길 수 있습니다",
            if ctx.coolant.is_liquid() { ctx.coolant.temperature_c } else { ctx.coolant.ambient_c() },
            env.dew_point_c()
        ));
    }
    if corrosion {
        notes.push(format!("철계 피삭재 + RH {:.0}% 이상(또는 결로) 조건이라 가공 후 표면 녹 발생 위험이 있습니다 — 방청·건조 권장", CORROSION_RH_PCT));
    }
    if (env.boiling_c() - 100.0).abs() >= 1.0 && ctx.coolant.fluid.is_water() {
        notes.push(format!("대기압 {:.1} kPa 에서 수용성 냉각액 끓는점이 {:.1} °C 라 비등 시작 온도가 함께 이동합니다", env.pressure_kpa, env.boiling_c()));
    }
    EnvironmentReport {
        ambient_c: env.ambient_c,
        humidity_pct: env.humidity_pct,
        reference_c: env.reference_c,
        pressure_kpa: env.pressure_kpa,
        dew_point_c: env.dew_point_c(),
        wet_bulb_c: env.wet_bulb_c(),
        humidity_ratio_g_kg: env.humidity_ratio() * 1000.0,
        air_density: env.air_density(),
        coolant_c: ctx.coolant.temperature_c,
        coolant_auto: ctx.coolant.temperature_auto,
        coolant_basis: coolant_basis(ctx),
        bulk_c: ctx.bulk_c(),
        boiling_c: env.boiling_c(),
        moisture_exposure: moisture_exposure(&ctx.coolant.method),
        friction_factor: ff,
        oxidation_factor: of,
        tg_shift_c: tg,
        scale_error_um: scale,
        drift_um: drift,
        condensation,
        corrosion,
        notes,
    }
}
