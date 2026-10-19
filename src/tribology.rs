use crate::physics::{CoatingTribology, CutContext, CuttingCoeffs, ForceResult, ToolGeometry, WorkpieceProps};
use crate::profile::{CoolantMethod, ShopEnvironment};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WpChem {
    #[default]
    Iron,
    Nickel,
    Titanium,
    Aluminum,
    CarbonFiber,
}

impl WpChem {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Iron => "iron",
            Self::Nickel => "nickel",
            Self::Titanium => "titanium",
            Self::Aluminum => "aluminum",
            Self::CarbonFiber => "carbon_fiber",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Iron => "철계",
            Self::Nickel => "니켈계",
            Self::Titanium => "티타늄계",
            Self::Aluminum => "알루미늄계",
            Self::CarbonFiber => "탄소섬유 복합재",
        }
    }

    pub fn reactive(&self) -> bool {
        matches!(self, Self::Nickel | Self::Titanium)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CoatingChem {
    pub al: bool,
    pub ti: bool,
    pub cr: bool,
    pub si: bool,
    pub zr: bool,
    pub carbon: bool,
    pub bare: bool,
}

impl CoatingChem {
    const fn of(al: bool, ti: bool, cr: bool, si: bool, zr: bool, carbon: bool, bare: bool) -> Self {
        Self { al, ti, cr, si, zr, carbon, bare }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CoatingProps {
    pub friction: f64,
    pub max_temp_c: f64,
    pub hardness_hv: f64,
    pub wear_factor: f64,
    pub ferrous_ok: bool,
    pub oxidation_c: f64,
    pub hot_hardness_c: f64,
    pub conductivity: f64,
    pub elastic_gpa: f64,
    pub expansion: f64,
    pub thickness_um: f64,
    pub chem: CoatingChem,
    pub protective_oxide: bool,
    pub crack_tolerance: f64,
    pub diffusion: f64,
    pub oxidation: f64,
}

pub fn coating_family(n: &str) -> &'static str {
    if n.is_empty() || n.contains("uncoated") || n.contains("bare") || n.contains("무코팅") || n == "none" {
        return "uncoated";
    }
    if n.contains("naco") {
        return "nACo";
    }
    if n.contains("tisin") || n.contains("sisn") {
        return "TiSiN";
    }
    if n.contains("alcrn") || n.contains("crall") {
        return "AlCrN";
    }
    if n.contains("altin") {
        return "AlTiN";
    }
    if n.contains("tialn") {
        return "TiAlN";
    }
    if n.contains("ticn") {
        return "TiCN";
    }
    if n.contains("dlc") || n.contains("diamondlike") {
        return "DLC";
    }
    if n.contains("diamond") || n.contains("cvd") || n.contains("pcd") {
        return "Diamond";
    }
    if n.contains("zrn") {
        return "ZrN";
    }
    if n.contains("crn") {
        return "CrN";
    }
    if n.contains("tin") {
        return "TiN";
    }
    if n.contains("hipims") {
        return "HiPIMS";
    }
    "unknown"
}

pub fn coating_props(family: &str) -> CoatingProps {
    let c = |friction: f64,
             max_temp_c: f64,
             hardness_hv: f64,
             wear_factor: f64,
             ferrous_ok: bool,
             oxidation_c: f64,
             hot_hardness_c: f64,
             conductivity: f64,
             elastic_gpa: f64,
             expansion: f64,
             thickness_um: f64,
             chem: CoatingChem,
             protective_oxide: bool,
             crack_tolerance: f64,
             diffusion: f64,
             oxidation: f64| CoatingProps {
        friction,
        max_temp_c,
        hardness_hv,
        wear_factor,
        ferrous_ok,
        oxidation_c,
        hot_hardness_c,
        conductivity,
        elastic_gpa,
        expansion,
        thickness_um,
        chem,
        protective_oxide,
        crack_tolerance,
        diffusion,
        oxidation,
    };
    match family {
        "uncoated" => c(0.60, 800.0, 1600.0, 1.00, true, 600.0, 900.0, 80.0, 600.0, 5.5e-6, 0.0, CoatingChem::of(false, false, false, false, false, false, true), false, 1.00, 2.60, 2.00),
        "nACo" => c(0.45, 1200.0, 4000.0, 0.32, true, 1200.0, 1100.0, 3.0, 450.0, 7.0e-6, 3.0, CoatingChem::of(true, true, false, true, false, false, false), true, 0.85, 0.70, 0.75),
        "TiSiN" => c(0.45, 1100.0, 3800.0, 0.35, true, 1100.0, 1050.0, 4.0, 450.0, 7.0e-6, 3.0, CoatingChem::of(false, true, false, true, false, false, false), true, 0.90, 0.80, 0.80),
        "AlCrN" => c(0.40, 1100.0, 3200.0, 0.38, true, 1100.0, 1050.0, 4.5, 400.0, 8.0e-6, 3.0, CoatingChem::of(true, false, true, false, false, false, false), true, 0.95, 0.80, 0.85),
        "AlTiN" => c(0.50, 900.0, 3500.0, 0.40, true, 900.0, 1050.0, 3.5, 430.0, 7.5e-6, 3.0, CoatingChem::of(true, true, false, false, false, false, false), true, 1.00, 1.00, 1.00),
        "TiAlN" => c(0.45, 800.0, 3300.0, 0.45, true, 800.0, 950.0, 4.5, 450.0, 7.5e-6, 3.0, CoatingChem::of(true, true, false, false, false, false, false), true, 1.00, 1.15, 1.10),
        "TiCN" => c(0.40, 400.0, 3000.0, 0.60, true, 450.0, 650.0, 20.0, 450.0, 8.0e-6, 3.0, CoatingChem::of(false, true, false, false, false, false, false), false, 0.95, 1.50, 1.60),
        "DLC" => c(0.12, 400.0, 2500.0, 0.50, false, 350.0, 400.0, 1.0, 200.0, 2.3e-6, 2.0, CoatingChem::of(false, false, false, false, false, true, false), false, 1.05, 6.00, 2.00),
        "Diamond" => c(0.10, 700.0, 9000.0, 0.10, false, 650.0, 900.0, 1000.0, 1050.0, 1.2e-6, 10.0, CoatingChem::of(false, false, false, false, false, true, false), false, 1.10, 60.0, 2.50),
        "ZrN" => c(0.35, 550.0, 2500.0, 0.60, false, 550.0, 700.0, 20.0, 400.0, 7.2e-6, 3.0, CoatingChem::of(false, false, false, false, true, false, false), false, 1.00, 2.00, 1.40),
        "CrN" => c(0.40, 700.0, 1800.0, 0.65, true, 700.0, 750.0, 12.0, 300.0, 9.4e-6, 4.0, CoatingChem::of(false, false, true, false, false, false, false), true, 0.90, 1.60, 0.90),
        "TiN" => c(0.50, 600.0, 2300.0, 0.70, true, 550.0, 700.0, 19.0, 450.0, 9.4e-6, 3.0, CoatingChem::of(false, true, false, false, false, false, false), false, 1.00, 1.60, 1.50),
        "HiPIMS" => c(0.45, 1000.0, 3600.0, 0.38, true, 950.0, 1050.0, 3.5, 450.0, 7.5e-6, 3.0, CoatingChem::of(true, true, false, false, false, false, false), true, 0.90, 0.95, 0.95),
        _ => c(0.50, 800.0, 3000.0, 0.50, true, 800.0, 900.0, 10.0, 400.0, 8.0e-6, 3.0, CoatingChem::default(), false, 1.00, 1.20, 1.10),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Medium {
    #[default]
    None,
    Air,
    OilMist,
    WaterEmulsion,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CoolantFluid {
    pub medium: Medium,
    pub k: f64,
    pub rho: f64,
    pub cp: f64,
    pub nu: f64,
    pub pr: f64,
    pub boil_c: f64,
    pub leidenfrost_c: f64,
    pub boundary_mu: f64,
    pub film_strength: f64,
    pub penetration_m_min: f64,
    pub wetting: f64,
    pub evaporative: f64,
    pub lift_scale: f64,
}

impl Default for CoolantFluid {
    fn default() -> Self {
        Self::of(&CoolantMethod::Dry)
    }
}

impl CoolantFluid {
    pub fn of(method: &CoolantMethod) -> Self {
        let air = |medium: Medium, boundary_mu: f64, film_strength: f64, penetration_m_min: f64, wetting: f64, evaporative: f64, lift_scale: f64| Self {
            medium,
            k: 0.026,
            rho: 1.2,
            cp: 1005.0,
            nu: 1.6e-5,
            pr: 0.71,
            boil_c: 0.0,
            leidenfrost_c: 0.0,
            boundary_mu,
            film_strength,
            penetration_m_min,
            wetting,
            evaporative,
            lift_scale,
        };
        let water = |wetting: f64| Self {
            medium: Medium::WaterEmulsion,
            k: 0.6,
            rho: 995.0,
            cp: 4100.0,
            nu: 0.9e-6,
            pr: 6.2,
            boil_c: 100.0,
            leidenfrost_c: 260.0,
            boundary_mu: 0.15,
            film_strength: 0.45,
            penetration_m_min: 80.0,
            wetting,
            evaporative: 1.0,
            lift_scale: 1.0,
        };
        match method {
            CoolantMethod::Dry => air(Medium::None, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0),
            CoolantMethod::AirBlast => air(Medium::Air, 0.35, 0.12, 250.0, 1.0, 1.0, 0.3),
            CoolantMethod::Mist => air(Medium::OilMist, 0.08, 0.85, 120.0, 0.8, 2.5, 0.2),
            CoolantMethod::Flood => water(0.6),
            CoolantMethod::ThroughTool => water(0.9),
        }
    }

    pub fn at(method: &CoolantMethod, env: &ShopEnvironment, temperature_c: f64) -> Self {
        let mut f = Self::of(method);
        match f.medium {
            Medium::None | Medium::Air | Medium::OilMist => {
                let std_env = ShopEnvironment::standard();
                let mut local = env.clone();
                local.ambient_c = temperature_c.clamp(-10.0, 60.0);
                f.k *= local.air_conductivity() / std_env.air_conductivity();
                f.rho *= local.air_density() / std_env.air_density();
                f.cp *= local.air_cp() / std_env.air_cp();
                f.nu *= local.air_kinematic() / std_env.air_kinematic();
                f.pr *= local.air_prandtl() / std_env.air_prandtl();
            }
            Medium::WaterEmulsion => {
                let t = temperature_c.clamp(1.0, 90.0);
                let t_ref = crate::profile::STANDARD_AMBIENT_C;
                let mu_rel = crate::profile::water_viscosity_pa_s(t) / crate::profile::water_viscosity_pa_s(t_ref);
                let k_rel = crate::profile::water_conductivity(t) / crate::profile::water_conductivity(t_ref);
                f.k *= k_rel;
                f.nu *= mu_rel;
                f.pr *= mu_rel / k_rel;
                let shift = env.boiling_c() - 100.0;
                f.boil_c += shift;
                f.leidenfrost_c += shift;
            }
        }
        f
    }

    pub fn is_water(&self) -> bool {
        self.medium == Medium::WaterEmulsion
    }

    pub fn label(&self) -> &'static str {
        match self.medium {
            Medium::None => "정체 공기",
            Medium::Air => "압축 공기",
            Medium::OilMist => "에스터 오일 미스트",
            Medium::WaterEmulsion => "수용성 에멀전",
        }
    }
}

pub fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

pub fn smoothstep(x: f64) -> f64 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn reference_interface_c(wp: &WorkpieceProps) -> f64 {
    20.0 + 0.42 * (wp.melt_c - 20.0)
}

fn hot(hv: f64, hot_c: f64, t_c: f64) -> f64 {
    let span = (hot_c - 300.0).max(100.0);
    hv * (1.0 - 0.45 * ((t_c - 300.0) / span).clamp(0.0, 1.3))
}

pub fn hot_hardness(coat: &CoatingTribology, t_c: f64) -> f64 {
    hot(coat.hardness_hv, coat.hot_hardness_c, t_c)
}

pub const SUBSTRATE_HV: f64 = 1600.0;
pub const SUBSTRATE_HOT_C: f64 = 900.0;
pub const COATING_DEPTH_UM: f64 = 2.0;

pub fn effective_hardness(coat: &CoatingTribology, t_c: f64) -> f64 {
    let sub = hot(SUBSTRATE_HV, SUBSTRATE_HOT_C, t_c);
    if coat.thickness_um <= 0.0 {
        return sub;
    }
    let w = coat.thickness_um / (coat.thickness_um + COATING_DEPTH_UM);
    (1.0 - w) * sub + w * hot_hardness(coat, t_c)
}

pub fn abrasive_response(r: f64) -> f64 {
    let r = r.max(0.0);
    let r4 = r.powi(4);
    0.1 * r.powf(1.5) * (1.0 + 9.0 * r4 / (16.0 + r4))
}

pub fn abrasion_index(surface_hv: f64, wp: &WorkpieceProps, t_c: f64) -> f64 {
    let h = surface_hv.max(100.0);
    let matrix = wp.matrix_hv * wp.jc_thermal(t_c).max(0.05);
    wp.hard_phase_frac * abrasive_response(wp.hard_phase_hv / h) + (1.0 - wp.hard_phase_frac) * 0.04 * (matrix / h).powf(1.5)
}

pub fn workpiece_abrasiveness(wp: &WorkpieceProps) -> f64 {
    abrasion_index(SUBSTRATE_HV, wp, 20.0)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct PairTribology {
    pub adhesion_mod: f64,
    pub adhesion: f64,
    pub mu_factor: f64,
    pub diffusion: f64,
    pub dissolution: f64,
    pub abrasion: f64,
    pub oxidation: f64,
}

fn adhesion_modifier(c: &CoatingChem, chem: WpChem) -> f64 {
    match chem {
        WpChem::Aluminum => {
            if c.carbon {
                0.45
            } else if c.zr {
                0.7
            } else if c.al {
                1.35
            } else if c.cr {
                0.85
            } else if c.ti {
                1.1
            } else if c.bare {
                0.9
            } else {
                1.0
            }
        }
        // Ti 와 친화성이 높은 것은 Al 이 없는 Ti 계 코팅(TiN·TiCN·TiSiN)이다 — TiAlN·AlTiN·nACo 는 Al 산화막이 계면을 덮는다
        // (`compatibility` 의 경고도 Al 없는 Ti 계만 낸다). 예전에는 Ti 를 포함한 모든 코팅에 1.12 를 매겨, 건식 Ti 밀링에서 TiAlN 이 무코팅보다
        // VB 가 약 30 % 작다는 실측(Uddin 외 2017 — 10 m 에서 0.144 대 0.21 mm)과 반대로 TiAlN 이 무코팅보다 빨리 닳았다
        WpChem::Titanium => {
            if c.carbon {
                1.8
            } else if c.ti && !c.al {
                1.12
            } else if c.cr {
                0.95
            } else {
                1.0
            }
        }
        WpChem::Nickel => {
            if c.bare {
                1.15
            } else if c.al && !c.ti {
                0.9
            } else if c.ti && !c.al {
                1.1
            } else {
                1.0
            }
        }
        WpChem::Iron => {
            if c.bare {
                1.10
            } else if c.carbon {
                0.8
            } else if c.cr && !c.al {
                1.25
            } else {
                1.0
            }
        }
        WpChem::CarbonFiber => {
            if c.carbon {
                0.7
            } else {
                1.0
            }
        }
    }
}

fn dissolution_onset_c(chem: WpChem, coat: &CoatingTribology) -> f64 {
    let base = match chem {
        WpChem::Iron => 450.0,
        WpChem::Nickel => 500.0,
        WpChem::Titanium => 450.0,
        _ => 1.0e4,
    };
    if coat.family == "DLC" {
        base - 150.0
    } else if coat.family == "Diamond" && matches!(chem, WpChem::Iron | WpChem::Nickel) {
        // 다이아몬드의 Fe·Ni 촉매 흑연화는 700 °C 위에서 빨라진다 (Chen·Zhang·Arsecularatne 2007 IJMTM 47 — 약 970 K,
        // Zou 외 2017 Crystals 7:116 DTA 공기 중 740 °C, Shimada 외 2004 — 1000 K 위 C–Fe 원자 교환 · 900 K 아래 산화).
        // 그 아래에서도 화학 마모는 크다 (pair_coat 의 바닥 몫 0.25 — 연강에서 황동보다 약 10⁴배, Thornton·Wilks 1978)
        700.0
    } else {
        base
    }
}

pub fn tribofilm_factor(coat: &CoatingTribology, chem: WpChem, t_c: f64) -> f64 {
    if coat.chem.al && coat.protective_oxide && !matches!(chem, WpChem::Aluminum | WpChem::CarbonFiber) {
        1.0 - 0.3 * sigmoid((t_c - 800.0) / 60.0)
    } else {
        1.0
    }
}

pub fn pair(tool: &ToolGeometry, wp: &WorkpieceProps, t_c: f64) -> PairTribology {
    pair_coat(&tool.coating, wp, t_c)
}

pub fn pair_coat(coat: &CoatingTribology, wp: &WorkpieceProps, t_c: f64) -> PairTribology {
    let c = &coat.chem;
    let m_adh = adhesion_modifier(c, wp.chem);
    let film = tribofilm_factor(coat, wp.chem, t_c);
    let mut dissolution = 0.0;
    let diffusion = if c.carbon {
        match wp.chem {
            WpChem::Iron | WpChem::Nickel => {
                dissolution = sigmoid((t_c - dissolution_onset_c(wp.chem, coat)) / 40.0);
                coat.diffusion * (0.25 + 0.75 * dissolution)
            }
            WpChem::Titanium => {
                dissolution = sigmoid((t_c - dissolution_onset_c(wp.chem, coat)) / 40.0);
                if coat.family == "Diamond" {
                    // 다이아몬드 × Ti: 계면에 TiC·(Ti,V)C 층이 생겨 확산 장벽이 된다 — 탄소 용해가 폭주하지 않고 무코팅 초경 수준
                    // (Lindvall 외 2023 JECS 43 확산쌍, Abang Kamaruddin 외 2016 IJAT 10). 코팅은 응착·박리로 잃는다:
                    // 다이아몬드 코팅 엔드밀 Ti-6Al-4V 에어 56 m/min 에서 약 5 분에 일부 박리, VB 0.2 mm 까지 TiAlN 의 약 0.8배 (Qin 외 2014 JAMDSM 8)
                    1.2 * (0.85 + 0.15 * dissolution)
                } else {
                    3.0 * (0.25 + 0.75 * dissolution)
                }
            }
            _ => 0.5,
        }
    } else {
        match wp.chem {
            WpChem::Iron => coat.diffusion,
            WpChem::Nickel => coat.diffusion,
            // 무코팅 초경의 Ti 확산을 0.45 배로 깎던 예외를 없앴다 — 근거가 없었고 (docs/tool-life-and-pair-matrix.md 6번 '재검토 필요'),
            // 건식 Ti 밀링의 정상 마모 속도는 무코팅이 TiAlN 의 약 2.6배다 (Uddin 외 2017: 0.0185 대 0.0072 mm/m). 코팅의 확산 장벽 배수는
            // 다른 피삭재와 같고, Ti 와 화학 친화성이 큰 Al 없는 Ti 계 코팅만 1.1 배
            WpChem::Titanium => coat.diffusion * if c.ti && !c.al { 1.1 } else { 1.0 },
            WpChem::Aluminum | WpChem::CarbonFiber => 1.0,
        }
    };
    PairTribology {
        adhesion_mod: m_adh,
        adhesion: wp.adhesion * m_adh * film,
        mu_factor: m_adh.sqrt(),
        diffusion: diffusion * film,
        dissolution,
        abrasion: abrasion_index(effective_hardness(coat, t_c), wp, t_c),
        oxidation: coat.oxidation * sigmoid((t_c - coat.oxidation_c) / 45.0),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Friction {
    pub mu_pair: f64,
    pub mu_thermal: f64,
    pub mu_eff: f64,
    pub sticking: f64,
    pub penetration: f64,
    pub lubricant_access: f64,
    pub chip_lift: f64,
}

pub fn chip_lift(ctx: &CutContext) -> f64 {
    let f = &ctx.coolant.fluid;
    let p = ctx.coolant.pressure_bar.max(0.0) * f.lift_scale;
    (0.12 * (1.0 + p / 5.0).ln() * ctx.wp.lift_factor).clamp(0.0, 0.35)
}

pub fn contact_friction(ctx: &CutContext, vc: f64, t_int: f64) -> Friction {
    let coat = &ctx.tool.coating;
    let wp = &ctx.wp;
    let p = pair(&ctx.tool, wp, t_int);
    // 절삭 중 겉보기 마찰: 피삭재 기준값 × 작은 코팅 보정 (`pairref`). 예전에는 코팅의 핀온디스크 마찰 × 피삭재 응착 × √응착 보정이라
    // DLC·다이아몬드가 μ 0.05~0.10 이 되어 알루미늄 정삭·CFRP 의 kc 가 40 % 깎였다 — 실측 코팅 효과는 절삭력 0~6 %
    let mu_pair = crate::pairref::cutting_mu(ctx);
    let s = |t: f64| wp.jc_thermal(t).max(0.05);
    let soft = (s(t_int) / s(reference_interface_c(wp))).powf(0.25).clamp(0.75, 1.25);
    let over = ((t_int - coat.oxidation_c) / 250.0).clamp(0.0, 1.0);
    // 탄소 코팅이 산화·용해로 무너져도 겉보기 마찰은 모재(무코팅) 수준으로 돌아갈 뿐이다 — 철계에서는 `coating_mu_factor` 가 이미 1.0 이다.
    // 예전 1 + over + 2·dissolution (최대 ×4) 은 μ 를 상한 1.2 까지 밀어 계면 온도를 다시 올리는 되먹임을 만들었다
    // (2026-10-18 행렬: 다이아몬드·DLC × 30 HRC 강 1,046~1,052 °C, 같은 조건 PVD 코팅 약 600 °C). 화학 공격은 마모율(`pair_coat` 확산 항)에만 넣는다
    let breakdown = if coat.chem.carbon {
        1.0 + 0.12 * over.max(p.dissolution)
    } else if coat.protective_oxide {
        1.0 + 0.05 * over
    } else {
        1.0 + 0.12 * over
    };
    let mu_thermal = mu_pair * soft * breakdown * crate::environment::friction_factor(ctx, vc, t_int);
    let theta = ((t_int - 20.0) / (wp.melt_c - 20.0).max(100.0)).clamp(0.0, 1.0);
    let sticking = (0.15 + 0.4 * p.adhesion.min(2.0) * theta.sqrt()).clamp(0.05, 0.9);
    let fl = &ctx.coolant.fluid;
    let v_pen = fl.penetration_m_min
        * if fl.is_water() {
            (ctx.coolant.pressure_bar.max(0.5) / 2.0).powf(0.25)
        } else {
            1.0
        };
    let penetration = fl.film_strength / (1.0 + (vc.max(0.0) / v_pen.max(1.0)).powi(2));
    let access = penetration * (1.0 - sticking);
    let mu_boundary = fl.boundary_mu * (coat.friction / 0.5).sqrt().clamp(0.4, 1.4) * p.mu_factor;
    let mu_lub = mu_thermal - (mu_thermal - mu_boundary).max(0.0) * access;
    let lift = chip_lift(ctx);
    Friction {
        mu_pair,
        mu_thermal,
        mu_eff: (mu_lub * (1.0 - 0.6 * lift)).clamp(0.05, 1.2),
        sticking,
        penetration,
        lubricant_access: access,
        chip_lift: lift,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cooling {
    pub jet_velocity_m_s: f64,
    pub h_film: f64,
    pub boiling_factor: f64,
    pub boiling_state: String,
    pub h_eff: f64,
    pub conduction: f64,
    pub effectiveness: f64,
    pub body_effectiveness: f64,
    pub contact_length_mm: f64,
}

pub const COOLED_AREA_RATIO: f64 = 5.0;
pub const EFFECTIVENESS_SATURATION: f64 = 0.6;

pub fn jet_velocity(ctx: &CutContext, vc: f64) -> f64 {
    let c = &ctx.coolant;
    match c.fluid.medium {
        Medium::None => (vc / 60.0).max(0.2),
        Medium::Air | Medium::OilMist => 150.0 * (c.pressure_bar / 6.0).clamp(0.1, 2.0).sqrt(),
        Medium::WaterEmulsion => 0.8 * (2.0 * c.pressure_bar.max(0.1) * 1.0e5 / c.fluid.rho).sqrt(),
    }
}

pub fn film_coefficient(ctx: &CutContext, vc: f64) -> f64 {
    let fl = &ctx.coolant.fluid;
    let d = (ctx.tool.diameter_mm / 1000.0).max(1e-3);
    let re = jet_velocity(ctx, vc) * d / fl.nu;
    let nu_num = 0.664 * re.sqrt() * fl.pr.cbrt();
    nu_num * fl.k / d * fl.wetting * fl.evaporative
}

pub fn leidenfrost_c(ctx: &CutContext) -> f64 {
    let fl = &ctx.coolant.fluid;
    fl.leidenfrost_c + 9.0 * (ctx.coolant.pressure_bar - 1.0).max(0.0)
}

pub fn boiling(ctx: &CutContext, surface_c: f64) -> (f64, &'static str) {
    let fl = &ctx.coolant.fluid;
    if !fl.is_water() {
        return (1.0, "기체 대류");
    }
    let leid = leidenfrost_c(ctx);
    let nucleate = sigmoid((surface_c - (fl.boil_c + 10.0)) / 15.0);
    let film = sigmoid((surface_c - leid) / 30.0);
    let factor = (1.0 + nucleate - 1.85 * film).max(0.12);
    let state = if film > 0.5 {
        "막비등(라이덴프로스트)"
    } else if nucleate > 0.5 {
        "핵비등"
    } else {
        "단상 대류"
    };
    (factor, state)
}

pub fn interface_cooling(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64, rise_c: f64) -> Cooling {
    let t0 = ctx.coolant.temperature_c;
    let h0 = film_coefficient(ctx, vc);
    let (b, state) = boiling(ctx, t0 + 0.4 * rise_c.max(0.0));
    let h_eff = h0 * b;
    let phi = co.shear_angle_deg.to_radians().clamp(0.1, 1.2);
    let rake = ctx.tool.rake_deg.to_radians();
    let xi = ((phi - rake).cos() / phi.sin()).clamp(1.0, 5.0);
    let h_mm = f.h_mean_mm.max(1e-3);
    let lc = (2.0 * xi * h_mm * (1.0 - 0.5 * co.chip_lift) / 1000.0).max(2e-5);
    let v_chip = (vc / 60.0 / xi).max(1e-3);
    let t_mid = t0 + 0.5 * rise_c.max(0.0);
    let conduction = ctx.wp.k_at(t_mid) * (v_chip / (ctx.wp.diffusivity_at(t_mid) * lc)).sqrt();
    let g_c = h_eff * COOLED_AREA_RATIO;
    let raw = g_c / (g_c + conduction);
    Cooling {
        jet_velocity_m_s: jet_velocity(ctx, vc),
        h_film: h0,
        boiling_factor: b,
        boiling_state: state.to_string(),
        h_eff,
        conduction,
        effectiveness: (raw / (1.0 + raw / EFFECTIVENESS_SATURATION)).clamp(0.0, 0.6),
        body_effectiveness: h0 / (h0 + 2000.0),
        contact_length_mm: lc * 1000.0,
    }
}

pub fn transient_factor(ctx: &CutContext, contact_length_mm: f64, chip_conductance: f64, contact_s: f64) -> f64 {
    if contact_s <= 0.0 {
        return 1.0;
    }
    let lc = (contact_length_mm.max(0.005)) / 1000.0;
    let k = ctx.tool.tool_conductivity.max(1.0);
    let kappa = ctx.tool.tool_diffusivity().max(1e-9);
    let g_ss = k / lc;
    let t_star = lc * lc / (std::f64::consts::PI * kappa);
    let gc = chip_conductance.max(1.0);
    let a = gc + g_ss;
    let integral = |u_max: f64| {
        let w0 = g_ss;
        let w1 = gc * u_max + g_ss;
        let j = ((w1 * w1 - w0 * w0) / 2.0 - 2.0 * g_ss * (w1 - w0) + g_ss * g_ss * (w1 / w0).ln()) / gc.powi(3);
        2.0 * t_star * a * j
    };
    let avg = if contact_s < t_star {
        integral((contact_s / t_star).sqrt()) / contact_s
    } else {
        (integral(1.0) + (contact_s - t_star)) / contact_s
    };
    avg.clamp(0.3, 1.0)
}

/// 길이 a 의 띠 위 균일 열원이 너비 s = √(4κτ) 의 가우스 핵으로 퍼질 때, 띠 위 평균에 남는 몫 (무차원, s → 0 에서 1).
/// ∫₀ᵃ∫₀ᵃ G dx′dx / a 의 닫힌 꼴: erf(a/s) − (1 − exp(−a²/s²))·s/(a√π)
fn strip_overlap(a: f64, s: f64) -> f64 {
    if s <= 1e-15 * a.max(1e-15) {
        return 1.0;
    }
    let r = a / s;
    (crate::toolwear::erf(r) - (1.0 - (-r * r).exp()) / (r * std::f64::consts::PI.sqrt())).clamp(0.0, 1.0)
}

/// 반무한 공구 표면의 a × b 직사각형에 균일 열유속 q 를 t 초 동안 넣었을 때 직사각형 평균 온도 상승 × k/q (m).
/// 순간 점열원의 해를 면적에 적분한 것이다 (Carslaw·Jaeger): R(t) = 2√(κ/π) ∫₀^√t S_a(2√κ u)·S_b(2√κ u) du.
/// 짧은 시간에는 1차원 2√(κt/π) (예전 `transient_factor` 의 근거와 같다), 길이 a 를 넘으면 띠 열원처럼 로그로, 길이 b 를 넘으면 3차원 정상값으로 간다.
/// `t = ∞` 이면 정상값 (연속 절삭)
pub fn rect_response(t: f64, a: f64, b: f64, kappa: f64) -> f64 {
    let a = a.max(1e-7);
    let b = b.max(a);
    let k2 = 2.0 * kappa.max(1e-9).sqrt();
    let u_lo = 1e-3 * a / k2;
    let u_tail = 40.0 * b / k2;
    let u_hi = if t.is_finite() { t.max(0.0).sqrt().min(u_tail) } else { u_tail };
    if u_hi <= u_lo {
        return 2.0 * (kappa * t.max(0.0) / std::f64::consts::PI).sqrt();
    }
    // u_lo 아래는 S ≈ 1, 위는 로그 간격 적분
    let n = 48;
    let (l0, l1) = (u_lo.ln(), u_hi.ln());
    let mut acc = u_lo;
    for i in 0..n {
        let x = l0 + (l1 - l0) * (i as f64 + 0.5) / n as f64;
        let u = x.exp();
        let s = k2 * u;
        acc += strip_overlap(a, s) * strip_overlap(b, s) * u * (l1 - l0) / n as f64;
    }
    // 정상값의 꼬리: s ≫ b 에서 S_a·S_b ≈ ab/(π s²) → ∫ du = ab/(π k2² u)
    if !t.is_finite() || t.sqrt() > u_tail {
        acc += a * b / (std::f64::consts::PI * k2 * k2 * u_tail);
    }
    2.0 * (kappa / std::f64::consts::PI).sqrt() * acc
}

/// 단속 절삭(밀링)의 계면 열 저항 — 물림 시간 동안 평균한 1/(g_c + g_t(t)) (m²K/W). 같은 열유속이면 계면 온도 상승이 이 값에 비례한다.
///
/// 날은 회전마다 물림 시간 t_e 동안만 칩과 닿고 나머지 시간에 식는다. 계면 온도는 칩 쪽 전도 g_c (움직이는 열원)와 공구 쪽 전도
/// g_t(t) = k / R(t) 가 열을 나눠 가지며 정해지고 (온도 ∝ 1/(g_c + g_t)), 공구 쪽은 접촉 길이 l_c × 날 접촉 길이 b 의 면에 열이
/// 쌓이는 만큼 식는 힘이 줄어든다 (`rect_response`). 연속 절삭(t_e ≥ 주기)은 정상값 R(∞) 를 쓴다. 앞 회전들이 남긴 열은 평균 물림 비율
/// D = t_e/T 만큼 정상값의 먼 몫(R(∞) − R(T))으로 더한다 (주기 펄스 열원의 중첩을 적분으로 근사). 같은 조건에서 물림만 다른 두 칸 —
/// 반경 절입이 작아 짧게 물리는 정삭과 슬롯, 축 절입이 얕아 날 접촉이 짧은 칸과 깊은 칸 — 의 온도 차이가 이 식에서 나온다
/// (Stephenson·Ali 1992 의 단속 절삭 공구 온도와 같은 구조; Hosokawa 외 2000 의 고속 엔드밀 플랭크 온도가 연속 절삭보다 낮은 이유)
pub fn interrupted_heating(contact_length_mm: f64, edge_length_mm: f64, chip_conductance: f64, k_tool: f64, kappa_tool: f64, t_engage_s: f64, period_s: f64) -> f64 {
    let a = (contact_length_mm.max(0.002)) / 1000.0;
    let b = (edge_length_mm.max(contact_length_mm).max(0.002)) / 1000.0;
    let k = k_tool.max(1.0);
    let gc = chip_conductance.max(1.0);
    let r_inf = rect_response(f64::INFINITY, a, b, kappa_tool);
    if !(t_engage_s > 0.0) || !(period_s > 0.0) || t_engage_s >= 0.95 * period_s {
        return 1.0 / (gc + k / r_inf.max(1e-12));
    }
    let duty = t_engage_s / period_s;
    let memory = duty * (r_inf - rect_response(period_s, a, b, kappa_tool)).max(0.0);
    // 물림 시간 평균: t = t_e·w² 로 바꿔 시작부(빠른 변화)에 점을 모은다
    let n = 10;
    let mut acc = 0.0;
    for i in 0..n {
        let w = (i as f64 + 0.5) / n as f64;
        let r = rect_response(t_engage_s * w * w, a, b, kappa_tool) + memory;
        acc += 1.0 / (gc + k / r.max(1e-12)) * 2.0 * w / n as f64;
    }
    acc
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Barrier {
    pub tool_heat: f64,
    pub interface: f64,
}

impl Default for Barrier {
    fn default() -> Self {
        Self { tool_heat: 1.0, interface: 1.0 }
    }
}

pub fn barrier_of(ctx: &CutContext, thickness_um: f64, conductivity: f64, contact_length_mm: f64, chip_conductance: f64, contact_s: f64) -> Barrier {
    if thickness_um <= 0.0 {
        return Barrier::default();
    }
    let k = ctx.tool.tool_conductivity.max(1.0);
    let steady = (contact_length_mm.max(0.02) / 1000.0) / k;
    let transient = (std::f64::consts::PI * ctx.tool.tool_diffusivity() * contact_s.max(1e-5)).sqrt() / k;
    let r_sub = steady.max(transient);
    let r_chip = 1.0 / chip_conductance.max(1.0);
    let r_coat = thickness_um * 1e-6 / conductivity.max(0.1);
    let par = |a: f64, b: f64| a * b / (a + b);
    Barrier {
        tool_heat: ((r_chip + r_sub) / (r_chip + r_sub + r_coat)).clamp(0.3, 1.0),
        interface: (par(r_chip, r_sub + r_coat) / par(r_chip, r_sub)).clamp(1.0, 1.3),
    }
}

pub fn coating_barrier(ctx: &CutContext, contact_length_mm: f64, chip_conductance: f64, contact_s: f64) -> Barrier {
    let c = &ctx.tool.coating;
    barrier_of(ctx, c.thickness_um, c.conductivity, contact_length_mm, chip_conductance, contact_s)
}

pub fn reference_barrier(ctx: &CutContext, contact_length_mm: f64, chip_conductance: f64, contact_s: f64) -> Barrier {
    let r = coating_props("AlTiN");
    barrier_of(ctx, r.thickness_um, r.conductivity, contact_length_mm, chip_conductance, contact_s)
}

pub fn shock_coefficient(ctx: &CutContext) -> f64 {
    let t = &ctx.tool;
    t.elastic_gpa * 1000.0 * t.tool_expansion / (1.0 - t.poisson).max(0.5)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct ThermalShock {
    pub quench: f64,
    pub amplitude_c: f64,
    pub stress_mpa: f64,
    pub risk: f64,
    #[serde(default)]
    pub mismatch_mpa: f64,
    #[serde(default)]
    pub coating_risk: f64,
    #[serde(default)]
    pub severity: f64,
}

pub fn thermal_shock(ctx: &CutContext, wet_c: f64, h_eff: f64, barrier_rel: f64, interrupted: bool) -> ThermalShock {
    let quench = 0.05 + 0.95 * h_eff / (h_eff + 4000.0);
    let amplitude = (wet_c - ctx.coolant.temperature_c).max(0.0) * quench;
    let s_rel = shock_coefficient(ctx) / (600.0e3 * 5.5e-6 / 0.78);
    let tough = (10.0 / ctx.tool.fracture_toughness.max(1.0)).sqrt();
    let severity = amplitude * s_rel * tough * ctx.tool.coating.crack_tolerance * barrier_rel;
    let coat = &ctx.tool.coating;
    let mismatch = if coat.thickness_um > 0.0 {
        coat.elastic_gpa * 1000.0 * (coat.expansion - ctx.tool.tool_expansion).abs() * amplitude / 0.75
    } else {
        0.0
    };
    let substrate_risk = if interrupted { ((severity - 350.0) / 400.0).clamp(0.0, 1.0) } else { 0.0 };
    let coating_risk = if interrupted && coat.thickness_um > 0.0 { ((mismatch - 500.0) / 1500.0).clamp(0.0, 1.0) } else { 0.0 };
    ThermalShock {
        quench,
        amplitude_c: amplitude,
        stress_mpa: amplitude * barrier_rel * shock_coefficient(ctx),
        risk: 1.0 - (1.0 - substrate_risk) * (1.0 - 0.5 * coating_risk),
        mismatch_mpa: mismatch,
        coating_risk,
        severity,
    }
}

pub fn bue_risk(ctx: &CutContext, t_c: f64, access: f64) -> f64 {
    match ctx.wp.bue_range_c {
        Some((lo, hi)) => {
            let ramp = 30.0;
            let window = smoothstep((t_c - lo) / ramp) * (1.0 - smoothstep((t_c - (hi - ramp)) / ramp));
            let p = pair(&ctx.tool, &ctx.wp, t_c);
            let adh = (p.adhesion / 1.2).clamp(0.2, 1.4).powi(2);
            // 형상 적합도: 음의 경사·큰 호닝을 알루미늄·스테인리스에 쓰면 구성인선이 쉽게 생긴다 (`appfit`)
            (0.8 * window * adh * ctx.tool.fit_bue.clamp(1.0, 4.0) * (1.0 - 0.7 * access.clamp(0.0, 1.0))).clamp(0.0, 0.85)
        }
        None => 0.0,
    }
}

pub const MECH_KEYS: [&str; 4] = ["abrasion", "adhesion", "diffusion", "oxidation"];
pub const MECH_LABELS: [&str; 4] = ["연삭(경질상) 마모", "응착 마모", "확산·용해 마모", "산화·흑연화 마모"];

pub fn arrhenius(wp: &WorkpieceProps, t_c: f64) -> f64 {
    let t = t_c + 273.15;
    let ttr = wp.wear_transition_c + 273.15;
    if t <= ttr {
        (-wp.wear_activation_k / t).exp()
    } else {
        (-wp.wear_activation_k / ttr).exp() * (-wp.wear_activation_high_k * (1.0 / t - 1.0 / ttr)).exp()
    }
}

pub fn oxidation_environment(ctx: &CutContext) -> f64 {
    oxidation_environment_coat(ctx, &ctx.tool.coating)
}

pub fn oxidation_environment_coat(ctx: &CutContext, coat: &CoatingTribology) -> f64 {
    crate::environment::oxidation_factor(ctx) * match ctx.coolant.method {
        CoolantMethod::Flood | CoolantMethod::ThroughTool => {
            if coat.protective_oxide {
                1.05
            } else {
                1.3
            }
        }
        CoolantMethod::AirBlast => 1.1,
        CoolantMethod::Mist => 0.9,
        CoolantMethod::Dry => 1.0,
    }
}

pub fn mechanism_raw(ctx: &CutContext, t_c: f64, stress: f64, slide: f64, duty_ratio: f64, bue: f64, access: f64) -> [f64; 4] {
    mechanism_raw_coat(ctx, &ctx.tool.coating, t_c, stress, slide, duty_ratio, bue, access)
}

/// 응착(attrition) 마모의 열 활성화. Usui 외 1984 (Wear 100:129) 의 특성 마모식 dW/dt = C·σn·Vs·exp(−λ/θ) 는 응착 이론에서 유도해 초경–강
/// 절삭으로 검증한 식이다 — 접촉면의 결합(용착)이 온도로 활성화되므로 응착 몫도 확산 몫과 같은 활성화 상수(`arrhenius`, 강 7,500 K · 877 °C 위
/// 21,000 K — Usui 의 λ 5,302 K · 1,150 K 위 21,950 K 와 같은 구조)를 따른다. 예전에는 확산 몫(강 45 %)만 온도를 따라 Taylor 기울기
/// d ln T / d ln Vc 가 −1.8~−2.5 였다 (AISI 1018 무코팅 초경 실측 −2.15~−2.8, Karandikar 외 2013·2021). 알루미늄·CFRP 의 응착은 구성인선·윤활이 정하고
/// (`bue_risk`, `lubricant_access`) 온도와 함께 늘지 않아 1
pub fn adhesion_activation(wp: &WorkpieceProps, t_c: f64) -> f64 {
    match wp.chem {
        WpChem::Iron | WpChem::Titanium | WpChem::Nickel => arrhenius(wp, t_c),
        _ => 1.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mechanism_raw_coat(ctx: &CutContext, coat: &CoatingTribology, t_c: f64, stress: f64, slide: f64, duty_ratio: f64, bue: f64, access: f64) -> [f64; 4] {
    let p = pair_coat(coat, &ctx.wp, t_c);
    [
        stress * slide * p.abrasion,
        stress * slide * p.adhesion * (1.0 + bue) * (1.0 - 0.6 * access.clamp(0.0, 1.0)) * adhesion_activation(&ctx.wp, t_c),
        stress * slide * p.diffusion * arrhenius(&ctx.wp, t_c),
        (0.15 + p.oxidation * oxidation_environment_coat(ctx, coat)) * duty_ratio.max(0.0).sqrt(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WearMechanism {
    pub key: String,
    pub label: String,
    pub share: f64,
    pub ratio: f64,
}

pub fn normalized_shares(s: &[f64; 4]) -> [f64; 4] {
    let total: f64 = s.iter().map(|v| v.max(0.0)).sum();
    if total <= 1e-9 {
        return [0.30, 0.15, 0.45, 0.10];
    }
    [s[0].max(0.0) / total, s[1].max(0.0) / total, s[2].max(0.0) / total, s[3].max(0.0) / total]
}

pub fn mechanism_report(shares: &[f64; 4], parts: &[f64; 4]) -> (Vec<WearMechanism>, String) {
    let total: f64 = parts.iter().sum::<f64>().max(1e-12);
    let list: Vec<WearMechanism> = (0..4)
        .map(|i| WearMechanism {
            key: MECH_KEYS[i].into(),
            label: MECH_LABELS[i].into(),
            share: parts[i] / total,
            ratio: if shares[i] > 0.0 { parts[i] / shares[i] } else { 0.0 },
        })
        .collect();
    let dominant = list
        .iter()
        .max_by(|a, b| a.share.partial_cmp(&b.share).unwrap_or(std::cmp::Ordering::Equal))
        .map(|m| m.key.clone())
        .unwrap_or_default();
    (list, dominant)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Compat {
    pub severity: u8,
    pub messages: Vec<String>,
    #[serde(default)]
    pub coolant_hint: Option<String>,
    #[serde(default)]
    pub chemical: bool,
}

pub fn compatibility(ctx: &CutContext, t_c: f64) -> Compat {
    let coat = &ctx.tool.coating;
    let wp = &ctx.wp;
    let c = &coat.chem;
    let mut out = Compat::default();
    let mut push = |sev: u8, msg: String| {
        out.severity = out.severity.max(sev);
        out.messages.push(msg);
    };
    let mut coolant_hint: Option<String> = None;
    let mut chemical = false;
    if c.carbon && coat.family == "Diamond" && wp.chem == WpChem::Titanium {
        // TiC 장벽으로 확산이 막혀 화학적 폭주는 없다 — 대신 응착·박리 (pair_coat 의 Ti 분기 근거 참고)
        chemical = true;
        push(
            2,
            format!(
                "다이아몬드 코팅은 티타늄과 TiC 층을 만들어 탄소 확산이 막히므로 화학적 폭주는 없지만, 응착·박리로 몇 분 안에 코팅이 일부 떨어질 수 있습니다 (현재 날끝 {:.0}°C) — 수명은 TiAlN 의 약 0.8배 실측, 냉각·날끝 확인 권장",
                t_c
            ),
        );
    } else if c.carbon && matches!(wp.chem, WpChem::Iron | WpChem::Nickel | WpChem::Titanium) {
        let onset = dissolution_onset_c(wp.chem, coat);
        chemical = true;
        // 다이아몬드 × 철·니켈은 온도와 관계없이 쓰지 않는다 (흑연화 700 °C 위, 그 아래에서도 산화·원자 교환 화학 마모)
        let severe = coat.family == "Diamond" || t_c > onset - 50.0;
        push(
            if severe { 3 } else { 2 },
            format!(
                "{} 코팅은 {} 피삭재와 {:.0}°C 부근부터 탄소 확산·흑연화·탄화물 반응으로 급격히 마모됩니다 (현재 날끝 {:.0}°C) — AlTiN·AlCrN·nACo 계열 권장",
                coat.family,
                wp.chem.label(),
                onset,
                t_c
            ),
        );
    }
    if c.al && wp.chem == WpChem::Aluminum {
        chemical = true;
        push(1, format!("{} 은 Al 을 함유해 알루미늄과 응착 친화성이 높아 용착·구성인선이 생기기 쉽습니다 — 무코팅 연마날·DLC·ZrN·다이아몬드 권장", coat.family));
    }
    if c.ti && !c.al && wp.chem == WpChem::Titanium {
        chemical = true;
        push(1, format!("{} 은 Ti 계 코팅이라 티타늄과 화학 친화성(응착·확산)이 높습니다 — AlCrN·미립 무코팅 초경 권장", coat.family));
    }
    if wp.chem == WpChem::CarbonFiber && ctx.coolant.fluid.is_water() {
        coolant_hint = Some("air_blast".into());
        push(2, "CFRP 수지가 수용성 냉각액을 흡수(팽윤·Tg 저하·층간 박리)하므로 건식·에어·집진 방식이 권장됩니다".into());
    }
    if wp.chem == WpChem::Aluminum && matches!(ctx.coolant.method, CoolantMethod::Dry) {
        coolant_hint = Some("mist".into());
        push(1, "알루미늄 건식 절삭은 칩 용착이 심해집니다 — MQL 또는 플러드 권장".into());
    }
    if t_c > coat.oxidation_c {
        chemical = true;
        push(
            if t_c > coat.oxidation_c + 100.0 { 2 } else { 1 },
            format!("날끝 {:.0}°C 가 {} 의 산화·흑연화 개시 온도 {:.0}°C 를 넘어 코팅 보호층이 소모됩니다", t_c, coat.family, coat.oxidation_c),
        );
    }
    out.coolant_hint = coolant_hint;
    out.chemical = chemical;
    out
}

pub fn speed_capability(coat: &CoatingTribology, wp: &WorkpieceProps) -> (f64, Option<String>) {
    let base: f64 = match coat.family.as_str() {
        "uncoated" => 0.6,
        "TiN" | "CrN" | "ZrN" => 0.8,
        "TiCN" => 0.85,
        "nACo" | "TiSiN" | "AlCrN" => 1.1,
        _ => 1.0,
    };
    let c = &coat.chem;
    match wp.chem {
        WpChem::Aluminum => {
            if c.bare {
                (1.0, Some("알루미늄은 무코팅 연마날이 표준이라 절삭속도 감속을 적용하지 않았습니다".into()))
            } else if c.carbon {
                (base.max(1.0) * 1.15, Some(format!("{} 는 알루미늄과 응착이 낮아 절삭속도를 15% 올렸습니다", coat.family)))
            } else if c.al {
                (base.min(1.0) * 0.85, Some(format!("{} 의 Al–Al 응착을 고려해 절삭속도를 15% 낮췄습니다", coat.family)))
            } else {
                (base, None)
            }
        }
        WpChem::CarbonFiber => {
            if c.carbon {
                (base.max(1.0) * 1.3, Some(format!("{} 는 탄소섬유 연삭 마모에 강해 절삭속도를 30% 올렸습니다", coat.family)))
            } else if c.bare {
                (0.8, None)
            } else {
                (base, None)
            }
        }
        WpChem::Iron | WpChem::Nickel => {
            if c.carbon {
                (0.5, Some(format!("{} 는 {} 소재와 탄소 확산 반응이 있어 절삭속도를 절반으로 제한했습니다 (코팅 변경 권장)", coat.family, wp.chem.label())))
            } else {
                (base, None)
            }
        }
        WpChem::Titanium => {
            if c.bare {
                (0.8, None)
            } else if c.carbon {
                (0.7, Some(format!("{} 는 티타늄과 TiC 반응이 있어 절삭속도를 30% 낮췄습니다", coat.family)))
            } else if c.ti && !c.al {
                (base * 0.9, None)
            } else {
                (base, None)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TribologyReport {
    pub workpiece_chem: String,
    pub coating_family: String,
    pub coolant_medium: String,
    pub mu_pair: f64,
    pub mu_thermal: f64,
    pub mu_eff: f64,
    pub sticking: f64,
    pub penetration: f64,
    pub lubricant_access: f64,
    pub chip_lift: f64,
    pub adhesion: f64,
    pub diffusion: f64,
    pub dissolution: f64,
    pub abrasion_ratio: f64,
    pub oxidation_activity: f64,
    pub hot_hardness_hv: f64,
    pub strain_rate: f64,
    pub rate_factor: f64,
    pub softening: f64,
    pub cooling: Cooling,
    pub coating_barrier: Barrier,
    pub shock: ThermalShock,
    pub flow_stress_mpa: f64,
    pub compat: Compat,
}

pub fn report(ctx: &CutContext, co: &CuttingCoeffs, cooling: &Cooling, shock: ThermalShock, barrier: Barrier, t_c: f64, vc: f64) -> TribologyReport {
    let fr = contact_friction(ctx, vc, t_c);
    let p = pair(&ctx.tool, &ctx.wp, t_c);
    let reference = abrasion_index(effective_hardness(&tribology_reference_coating(), t_c), &ctx.wp, t_c).max(1e-12);
    TribologyReport {
        workpiece_chem: ctx.wp.chem.label().into(),
        coating_family: ctx.tool.coating.family.clone(),
        coolant_medium: ctx.coolant.fluid.label().into(),
        mu_pair: fr.mu_pair,
        mu_thermal: fr.mu_thermal,
        mu_eff: fr.mu_eff,
        sticking: fr.sticking,
        penetration: fr.penetration,
        lubricant_access: fr.lubricant_access,
        chip_lift: fr.chip_lift,
        adhesion: p.adhesion,
        diffusion: p.diffusion,
        dissolution: p.dissolution,
        abrasion_ratio: p.abrasion / reference,
        oxidation_activity: p.oxidation,
        hot_hardness_hv: effective_hardness(&ctx.tool.coating, t_c),
        strain_rate: co.strain_rate,
        rate_factor: co.rate_factor,
        softening: co.softening,
        cooling: cooling.clone(),
        coating_barrier: barrier,
        shock,
        flow_stress_mpa: ctx.wp.flow_stress(1.0, co.strain_rate.max(1.0), 20.0 + 0.5 * (t_c - 20.0)),
        compat: compatibility(ctx, t_c),
    }
}

pub fn tribology_reference_coating() -> CoatingTribology {
    crate::physics::tribology_from_name(Some("AlTiN"))
}

pub fn regime_vector(ctx: &CutContext, rep: &TribologyReport, shares: &[f64], vc: f64, t_c: f64, crack: f64, bue: f64) -> Vec<f32> {
    let s = |i: usize| shares.get(i).cloned().unwrap_or(0.0);
    let v = [
        (rep.mu_eff / 0.45).max(0.05).ln(),
        rep.sticking - 0.45,
        rep.lubricant_access * 2.0,
        ((t_c + 273.15) / 973.15).max(0.1).ln() * 3.0,
        rep.adhesion.max(0.05).ln(),
        rep.diffusion.max(0.01).ln() * 0.5,
        rep.abrasion_ratio.max(0.01).ln() * 0.5,
        rep.oxidation_activity.min(3.0),
        crack,
        bue,
        s(0) * 1.5,
        s(1) * 1.5,
        s(2) * 1.5,
        s(3) * 1.5,
        rep.cooling.effectiveness * 2.0,
        (vc / ctx.wp.ref_vc.max(1.0)).max(0.05).ln(),
    ];
    v.iter().map(|x| *x as f32).collect()
}

pub const REGIME_DIM: usize = 16;

