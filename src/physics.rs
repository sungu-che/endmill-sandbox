use crate::cutting::{CoolantType, CuttingConditions, WorkpieceMaterial};
use crate::material::{CarbideGrade, ToolBaseMaterial};
use crate::profile::{CoolantConfig, CoolantMethod, EndMillMockupSetting, MachineLimits, MachiningProfile, ShopEnvironment, ToolNose};
use crate::tribology::{self, CoatingChem, CoolantFluid, Cooling, ThermalShock, TribologyReport, WearMechanism, WpChem};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct JohnsonCook {
    pub a: f64,
    pub b: f64,
    pub n: f64,
    pub c: f64,
    pub m: f64,
    pub t_melt_c: f64,
    pub rate_ref: f64,
}

impl JohnsonCook {
    pub const fn new(a: f64, b: f64, n: f64, c: f64, m: f64, t_melt_c: f64) -> Self {
        Self { a, b, n, c, m, t_melt_c, rate_ref: 1.0 }
    }

    pub fn thermal(&self, t_c: f64) -> f64 {
        let t = ((t_c - 20.0) / (self.t_melt_c - 20.0).max(1.0)).clamp(0.0, 1.0);
        1.0 - t.powf(self.m)
    }

    pub fn rate(&self, strain_rate: f64) -> f64 {
        1.0 + self.c * (strain_rate.max(self.rate_ref) / self.rate_ref).ln()
    }

    pub fn flow_stress(&self, strain: f64, strain_rate: f64, t_c: f64) -> f64 {
        (self.a + self.b * strain.max(0.0).powf(self.n)) * self.rate(strain_rate) * self.thermal(t_c)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkpieceProps {
    pub key: String,
    pub label: String,
    pub density: f64,
    pub specific_heat: f64,
    pub conductivity: f64,
    pub expansion: f64,
    pub elastic_gpa: f64,
    pub hardness_hrc: f64,
    pub kc11: f64,
    pub mc: f64,
    pub rake_deg: f64,
    pub adhesion: f64,
    pub wear_activation_k: f64,
    pub thermal_factor: f64,
    pub melt_c: f64,
    pub temp_limit_c: Option<f64>,
    pub bue_range_c: Option<(f64, f64)>,
    pub ref_vc: f64,
    pub fz_coeff: f64,
    pub ap_ratio: f64,
    pub ae_ratio: f64,
    pub edge_radius_um: f64,
    pub ref_life_min: f64,
    pub min_chip_ratio: f64,
    pub work_hardening: f64,
    pub softening_m: f64,
    pub process_damping: f64,
    #[serde(default)]
    pub chem: WpChem,
    #[serde(default)]
    pub thermo: Vec<(f64, f64, f64)>,
    #[serde(default)]
    pub jc: Option<JohnsonCook>,
    #[serde(default)]
    pub poisson: f64,
    #[serde(default)]
    pub matrix_hv: f64,
    #[serde(default)]
    pub hard_phase_hv: f64,
    #[serde(default)]
    pub hard_phase_frac: f64,
    #[serde(default)]
    pub wear_activation_high_k: f64,
    #[serde(default)]
    pub wear_transition_c: f64,
    #[serde(default)]
    pub mech_shares: [f64; 4],
    #[serde(default)]
    pub lift_factor: f64,
    /// 반경력/접선력 비(kr)의 하한. Merchant 관계 tan(β−α) 는 직교절삭의 추력비라 엔드밀 측정 계수(Krc/Ktc)보다 낮게 나올 수 있다.
    /// 알루미늄은 6061·AlMgSi 엔드밀 6세트가 0.33~0.52 (Rubeo & Schmitz 2016 Precis. Eng. 45:311, Schmitz 외 2022 IFAC-PapersOnLine 55(36):103,
    /// Honeycutt & Schmitz 2018 JMSE 140:120801, Gradišek 외 2004 IJMTM 44:401), 반경 힘이 작으면 벽 순간의 접선·반경 성분이 상쇄되어 휨이 과소평가된다.
    #[serde(default = "default_kr_floor")]
    pub kr_floor: f64,
    /// 절삭 중 공구–칩 겉보기 마찰계수 (무코팅 초경 기준, 힘 측정에서 역산한 값 — 핀온디스크 값이 아님).
    /// 1045 직교절삭 0.38~0.70 (중앙 0.50), Ti-6Al-4V 0.36~0.42 (날끝 힘 제외)·FE 관행 0.5~0.7, 304 FE 0.6,
    /// 알루미늄 엔드밀 Krc/Ktc 0.46 (경사 10~15°) → 0.7~0.85, CFRP 섬유각에 따라 0.2~0.6. 코팅은 `pairref::coating_mu_factor` 로 작게만 바꾼다
    #[serde(default = "default_cut_mu")]
    pub cut_mu: f64,
    /// 날끝 접선 계수 배수: Kte = 0.011·kc1.1·√(r/10 µm) × 이 값. Ti-6Al-4V 엔드밀 실측 Kte 41 N/mm (Ø12 4날)·직교 24 N/mm →
    /// 식의 17 N/mm 보다 1.4~2.4배, 304 선삭 날끝 절편 51 N/mm (r ≈ 20 µm)
    #[serde(default = "one")]
    pub kte_factor: f64,
    /// 날끝 반경/접선 비 Kre/Kte. 알루미늄 엔드밀 4세트 0.83~1.0, 1045 직교(−7°) 1.5~2, Ti-6Al-4V 1.8~1.9, 304 선삭 0.7, 58 HRC 볼 0.7
    #[serde(default = "default_kre_over_kte")]
    pub kre_over_kte: f64,
    /// 공구가 여유각을 지정하지 않았을 때의 1차 여유각 (°) — 피삭재용 초경 엔드밀 설계값 (제조사 기술자료·특허 합의값)
    #[serde(default = "default_clearance_deg")]
    pub clearance_deg: f64,
    /// 공구 수명 판정 VB (mm, 균일 플랭크 마모). ISO 8688-2 의 0.3 mm 가 기본이고 Ti 0.2 (평균)·스테인리스 0.2·
    /// 고경도 정삭 0.1~0.2·CFRP 0.1~0.2 (섬유 미절단·박리가 먼저 옴)를 쓴다. 부품 공차 규칙(≤20 µm → 0.2)과 작은 쪽을 쓴다
    #[serde(default = "default_vb_criterion")]
    pub vb_criterion_mm: f64,
    /// 길들이기(초기 급마모) 크기 (µm) — 수명의 비율이 아니라 날끝 상태로 정해지는 양. HRC 52 날카로운 볼 30~63 µm (PHM2010),
    /// 호닝 8~10 µm 코팅 316L 0~20 µm (30분간 마모 안 보임), Ti-6Al-4V AlCrN 5 m 이송에 115 µm (Vc 150)
    #[serde(default = "default_breakin_vb_um")]
    pub breakin_vb_um: f64,
    /// 길들이기 시상수 (분, 절삭 시간). 수 m 의 이송 경로 안에 끝난다 — 예전의 '선형 수명의 3 %' 는 수명이 긴 조건에서 10~20분이 되었다
    #[serde(default = "default_breakin_tau_min")]
    pub breakin_tau_min: f64,
    /// 길들이기 크기의 절삭속도 지수 (Ti·Ni 는 속도가 오르면 초기 날끝 손상이 커진다). 0 이면 속도 무관
    #[serde(default)]
    pub breakin_vc_exp: f64,
    /// 초경 엔드밀 슬롯(ae = D) 권장 ap/D — 제조사 자료로 확인한 소재만 (없으면 판정하지 않음). 합성 포켓 지그재그는 슬롯형 이동(첫 줄 · 줄 끝
    /// 피치 이동)이 이 깊이를 넘지 않게 단계 절입하고(`gcode::pocket_plan`), 슬롯 패턴이 넘으면 경고한다 (`gcode::slot_note`)
    #[serde(default)]
    pub slot_ap_ratio: Option<f64>,
    /// 헬리컬 진입 경사각 (°, 공구 중심 경로 기준 — 회전당 하강 = π·(구멍 지름 − D)·tan α) — 제조사 자료로 확인한 소재만 (`gcode::pocket_plan`)
    #[serde(default)]
    pub ramp_deg: Option<f64>,
    /// 벽 정삭을 상향(업)밀링으로 — 제조사가 그 소재에 상향을 지시할 때만 (없으면 하향: Sandvik · Iscar · Harvey 의 첫 선택)
    #[serde(default)]
    pub finish_up_milling: bool,
}

fn default_kr_floor() -> f64 {
    0.30
}

fn default_cut_mu() -> f64 {
    0.50
}

fn default_kre_over_kte() -> f64 {
    1.5
}

fn default_clearance_deg() -> f64 {
    10.0
}

fn default_vb_criterion() -> f64 {
    0.3
}

fn default_breakin_vb_um() -> f64 {
    30.0
}

fn default_breakin_tau_min() -> f64 {
    2.0
}

fn hv_of_hrc(h: f64) -> f64 {
    interp(&[(20.0, 240.0), (30.0, 302.0), (40.0, 392.0), (45.0, 446.0), (50.0, 513.0), (55.0, 595.0), (60.0, 697.0), (65.0, 832.0)], h)
}

fn alloy_thermo(h: f64) -> Vec<(f64, f64, f64)> {
    let k20 = interp(&[(20.0, 40.0), (30.0, 36.0), (45.0, 27.0), (60.0, 20.0)], h);
    let w = ((h - 35.0) / 15.0).clamp(0.0, 1.0);
    let low = [1.0, 0.95, 0.87, 0.78, 0.70];
    let high = [1.0, 1.10, 1.15, 1.15, 1.12];
    let cp = [460.0, 500.0, 560.0, 620.0, 680.0];
    [20.0, 200.0, 400.0, 600.0, 800.0]
        .iter()
        .enumerate()
        .map(|(i, t)| (*t, k20 * (low[i] * (1.0 - w) + high[i] * w), cp[i]))
        .collect()
}

fn alloy_jc(h: f64) -> JohnsonCook {
    let uts = 3.2 * hv_of_hrc(h);
    let d = h - 30.0;
    JohnsonCook::new(0.82 * uts, 0.25 * uts, (0.22 - 0.0047 * d).max(0.08), 0.015 + 0.0003 * d.max(0.0), 1.0 + 0.008 * d.max(0.0), 1480.0 - 1.5 * d.max(0.0))
}

fn alloy_shares(h: f64) -> [f64; 4] {
    let w = ((h - 35.0) / 20.0).clamp(0.0, 1.0);
    let lo = [0.35, 0.12, 0.43, 0.10];
    let hi = [0.55, 0.05, 0.28, 0.12];
    [0, 1, 2, 3].map(|i| lo[i] * (1.0 - w) + hi[i] * w)
}

fn table_at(table: &[(f64, f64, f64)], t: f64, pick: fn(&(f64, f64, f64)) -> f64) -> Option<f64> {
    let first = table.first()?;
    if t <= first.0 {
        return Some(pick(first));
    }
    for w in table.windows(2) {
        if t <= w[1].0 {
            let r = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
            return Some(pick(&w[0]) + (pick(&w[1]) - pick(&w[0])) * r);
        }
    }
    table.last().map(pick)
}

fn interp(points: &[(f64, f64)], x: f64) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    points[points.len() - 1].1
}

impl WorkpieceProps {
    pub fn of(m: &WorkpieceMaterial) -> Self {
        let base = |key: &str, label: &str| WorkpieceProps {
            key: key.into(),
            label: label.into(),
            density: 7850.0,
            specific_heat: 486.0,
            conductivity: 51.9,
            expansion: 11.5e-6,
            elastic_gpa: 205.0,
            hardness_hrc: 15.0,
            kc11: 1600.0,
            mc: 0.25,
            rake_deg: 10.0,
            adhesion: 1.0,
            wear_activation_k: 7500.0,
            // 1.0 → 0.88: C45 엔드밀 Vc 180 에서 606 °C 로, 문헌 대역 420~580 °C 보다 높았다 (Hosokawa 외 2000, 精密工学会誌 66:1786 의 Vc 628 플랭크 770 °C 를
            // Vc 180 으로 환산하면 470~550 °C, 4140 Vc 300 플랭크 304~345 °C)
            thermal_factor: 0.88,
            melt_c: 1460.0,
            temp_limit_c: None,
            bue_range_c: Some((250.0, 450.0)),
            ref_vc: m.reference_cutting_speed(),
            fz_coeff: 0.005,
            ap_ratio: 1.0,
            ae_ratio: 0.3,
            edge_radius_um: 12.0,
            ref_life_min: 60.0,
            min_chip_ratio: 0.25,
            work_hardening: 0.08,
            softening_m: 1.0,
            process_damping: 1.0,
            chem: WpChem::Iron,
            thermo: vec![
                (20.0, 51.9, 486.0),
                (100.0, 50.7, 486.0),
                (200.0, 48.1, 519.0),
                (300.0, 45.2, 557.0),
                (400.0, 41.7, 599.0),
                (500.0, 38.1, 650.0),
                (600.0, 34.7, 710.0),
                (700.0, 31.0, 820.0),
                (800.0, 26.0, 700.0),
                (1000.0, 27.0, 650.0),
            ],
            jc: Some(JohnsonCook::new(553.1, 600.8, 0.234, 0.0134, 1.0, 1460.0)),
            poisson: 0.29,
            matrix_hv: 180.0,
            hard_phase_hv: 900.0,
            hard_phase_frac: 0.07,
            wear_activation_high_k: 21000.0,
            wear_transition_c: 877.0,
            mech_shares: [0.30, 0.15, 0.45, 0.10],
            lift_factor: 0.8,
            // 45강(C45) 초경 엔드밀 실측 Kr/Kt 0.34~0.38 (Sun 외 2023, Sensors 23:8954 — 2·4날 초경과 3날 HSS 3세트)
            kr_floor: 0.33,
            cut_mu: 0.50,
            kte_factor: 1.0,
            kre_over_kte: 1.5,
            // 탄소·합금강용 초경 엔드밀 1차 여유각 6~9° (2차 14~17°)
            clearance_deg: 8.0,
            vb_criterion_mm: 0.3,
            breakin_vb_um: 30.0,
            breakin_tau_min: 2.0,
            breakin_vc_exp: 0.0,
            slot_ap_ratio: None,
            ramp_deg: None,
            finish_up_milling: false,
        };
        match m {
            WorkpieceMaterial::Aluminum => WorkpieceProps {
                density: 2700.0,
                specific_heat: 896.0,
                conductivity: 167.0,
                expansion: 23.6e-6,
                elastic_gpa: 69.0,
                hardness_hrc: 0.0,
                // 700 → 600: 이 상수에 날끝 항(kte)을 따로 더하면 칩 29 µm 에서 비에너지가 2.1~2.3 J/mm³ 로, 문헌(엔드밀 계수 세트를
                // 같은 형상에 적용한 중앙값 약 1.5, 범위 0.8~3.1)·직교절삭 MPM(0.9~1.3)·AA2024 직교 실측(h 0.1 mm 에서 1.0~1.3 → 29 µm 에서 1.4~1.9)보다
                // 높았다. 600 이면 같은 조건에서 약 1.5 J/mm³.
                kc11: 600.0,
                mc: 0.25,
                rake_deg: 18.0,
                adhesion: 1.25,
                thermal_factor: 1.0,
                wear_activation_k: 3500.0,
                melt_c: 650.0,
                bue_range_c: Some((150.0, 300.0)),
                fz_coeff: 0.010,
                ae_ratio: 0.5,
                edge_radius_um: 4.0,
                // 240 → 60: 기준 상태(Ø10, ae 0.5D, ap 1D, 300 m/min)의 240분은 공구 하나당 2~5만 cm³ 로, 같은 Ø6 급 초경 엔드밀의
                // 알루미늄 실측(VB 0.2~0.4 mm 까지 59~342분, 119~684 cm³; Grigoriev 외 2020 Coatings 10:1038 등 — 본문은 0.4 mm 까지 무코팅 99분 · DLC 342분(684 cm³),
                // 59분·119 cm³ 는 그림에서 읽은 값)보다 한 자릿수 길었다. 이 앵커가 경로 수명을
                // 841분/2,100 cm³ 로 만들었고, 길들이기 시상수(선형 수명의 3%)도 9.6분으로 문헌(0.5~5분)보다 늘렸다. 60 이면 알루미늄 황삭
                // 프리셋(rect_profile)의 경로 수명이 283분/702 cm³, 시상수 4.0분 — 문헌 대역 위쪽 끝이라 감사가 노란 플래그를 세운다
                // (확신도 보통: 실측 VB 계열이 들어오면 SDS 원장 보정으로 대체됨).
                ref_life_min: 60.0,
                min_chip_ratio: 0.30,
                work_hardening: 0.03,
                softening_m: 1.34,
                // 0.4 → 1.0: 압입 계수 Ksp 가 Al7075 4e4 N/mm³ (Budak & Tunç 2010, CIRP Annals 59:403 — 2차 인용으로 확인), Ti-6Al-4V 3e4 로 같은 자릿수 — 알루미늄의 공정감쇠가 약한 것은
                // 계수가 작아서가 아니라 절삭속도가 높아 파장이 길기 때문이고, 그 효과는 `process_damping_gain` 의 파장 항에 이미 있다 (확신도 낮음)
                process_damping: 1.0,
                chem: WpChem::Aluminum,
                thermo: vec![
                    (20.0, 167.0, 896.0),
                    (100.0, 172.0, 925.0),
                    (200.0, 177.0, 970.0),
                    (300.0, 180.0, 1010.0),
                    (400.0, 180.0, 1050.0),
                    (500.0, 175.0, 1090.0),
                ],
                jc: Some(JohnsonCook::new(324.0, 114.0, 0.42, 0.002, 1.34, 582.0)),
                poisson: 0.33,
                matrix_hv: 107.0,
                hard_phase_hv: 800.0,
                hard_phase_frac: 0.015,
                wear_activation_high_k: 7000.0,
                wear_transition_c: 450.0,
                mech_shares: [0.12, 0.85, 0.02, 0.01],
                lift_factor: 0.6,
                // 엔드밀 측정 Krc/Ktc 0.33~0.52 (중앙값 약 0.4). ZOA 안정선도 kr ≥ 0.4 쪽이 보수적이다 (kr 0.35 부근에서 특이 봉우리)
                kr_floor: 0.40,
                // 엔드밀 Krc/Ktc 0.46 (경사 10~15°) 에서 β ≈ 35~40° → μ 0.7~0.85
                cut_mu: 0.78,
                // 엔드밀 4세트 Kre/Kte 0.83~1.0 (Rubeo & Schmitz 2016; AlSi9Cu3 2세트 0.83·0.86 — Andersson Lassila 외 2024, Sci. Rep. 14:1459)
                kre_over_kte: 0.9,
                // 알루미늄용 엔드밀 1차 여유각 10~14° (2차 ~20°)
                clearance_deg: 12.0,
                // 연마날·DLC 의 길들이기는 20 µm 이하 (직접 실측 없음, 확신도 낮음)
                breakin_vb_um: 15.0,
                breakin_tau_min: 2.0,
                // 2~3날 초경 슬롯 ap: SGS S-Carb · YG-1 Alu-Power HPC · Haas HSAM2 1.0·D (같은 양식의 표), Helical H45AL-C-3 1.0~1.5·D,
                // Guhring Uni-Pro 0.5~1.0·D, Mitsubishi Alimaster 0.5·D (Ø12~25) — 주류 1.0
                slot_ap_ratio: Some(1.0),
                // 헬리컬 경사 (공구 중심 경로): IMCO M202/M203·Streaker 3~5° (슬롯 이송), Seco JHP490 회전당 5 % Dc (구멍 1.3·Dc → 3.0°), JS45x 10 % (6.1°)
                ramp_deg: Some(3.0),
                ..base("aluminum", "알루미늄 합금")
            },
            WorkpieceMaterial::CarbonSteel => WorkpieceProps {
                hardness_hrc: 15.0,
                // 4날 초경 슬롯 ap (Ø10): SGS Z-Carb AP · OSG AE-VMS · Haas HTPM 1.0·D, Kennametal HARVI I TE P2 1.25·D, Helical HSV-4 0.75~1.25·D,
                // Guhring Uni-Pro 0.5~1.0·D — 주류 1.0
                slot_ap_ratio: Some(1.0),
                // 헬리컬 경사: Seco JS554 (4날 범용) 회전당 2 % Dc → 1.2° · JS514 3 % → 1.8°, IMCO M924 1~2.5° — 작은 쪽
                ramp_deg: Some(1.2),
                ..base("carbon_steel", "탄소강 (S45C)")
            },
            WorkpieceMaterial::AlloySteel { hardness_hrc } => {
                let h = *hardness_hrc as f64;
                // Sandvik MC 표 (h = 1 mm, 경사 0°): 50 HRC 3090, 55 HRC 3690, 60 HRC 4330, 63 HRC 4750 (65 HRC 는 외삽 4950).
                // 예전 표의 60·65 HRC (3800·4200) 는 12~15 % 낮았다
                let kc = interp(
                    &[(20.0, 1950.0), (30.0, 2100.0), (40.0, 2500.0), (50.0, 3100.0), (55.0, 3690.0), (60.0, 4330.0), (63.0, 4750.0), (65.0, 4950.0)],
                    h,
                );
                let hard = h > 45.0;
                let fz = if h <= 30.0 {
                    0.005
                } else if h <= 45.0 {
                    0.0045
                } else if h <= 55.0 {
                    0.0035
                } else {
                    0.0025
                };
                let thermo = alloy_thermo(h);
                let jc = alloy_jc(h);
                WorkpieceProps {
                    density: 7800.0,
                    specific_heat: 460.0,
                    conductivity: thermo[0].1,
                    expansion: 11.5e-6,
                    elastic_gpa: 210.0,
                    hardness_hrc: h,
                    kc11: kc,
                    mc: 0.25,
                    rake_deg: (8.0 - 0.5 * (h - 40.0).max(0.0)).clamp(-6.0, 8.0),
                    adhesion: 0.95,
                    // 고경도 정삭에서 계면 온도를 과대평가했다 (58 HRC Ø6 볼 Vc 70 에서 745 °C, 문헌 대역 350~600 °C —
                    // H13 52 HRC 볼 엔드밀 Vc 200 까지 계면 200~400 °C, Dewes 외 1999 J. Mater. Process. Technol. 92–93:293). 전단 띠 형성으로 경사면에 가는 열이 줄어드는 몫
                    thermal_factor: interp(&[(25.0, 0.88), (40.0, 0.78), (50.0, 0.55), (58.0, 0.40), (65.0, 0.34)], h),
                    ref_life_min: if h > 45.0 { 35.0 } else { 60.0 },
                    bue_range_c: if h > 40.0 { None } else { Some((300.0, 480.0)) },
                    ref_vc: if h <= 30.0 {
                        150.0
                    } else if h <= 45.0 {
                        110.0
                    } else if h <= 55.0 {
                        90.0
                    } else {
                        70.0
                    },
                    fz_coeff: fz,
                    ae_ratio: if h > 45.0 { 0.1 } else { 0.3 },
                    edge_radius_um: if h > 45.0 { 20.0 } else { 12.0 },
                    // 고경도: 0.20 → 0.35 (볼 엔드밀 날카로운 날 h_min/r 0.66~0.80, 경화강 선삭 0.63 — Wojciechowski 외 2023, Tribol. Int. 181 · arXiv 2403.15554.
                    // 일반 호닝 날 0.2~0.4 는 이 논문에 없음 — 출처 미확인)
                    min_chip_ratio: if h > 45.0 { 0.35 } else { 0.25 },
                    work_hardening: if h > 45.0 { 0.02 } else { 0.06 },
                    process_damping: if h > 45.0 { 1.2 } else { 1.0 },
                    softening_m: jc.m,
                    melt_c: jc.t_melt_c,
                    thermo,
                    jc: Some(jc),
                    matrix_hv: hv_of_hrc(h),
                    hard_phase_hv: interp(&[(20.0, 1200.0), (30.0, 1300.0), (50.0, 1600.0), (60.0, 1700.0)], h),
                    hard_phase_frac: interp(&[(20.0, 0.02), (30.0, 0.02), (50.0, 0.05), (58.0, 0.12), (65.0, 0.15)], h),
                    mech_shares: alloy_shares(h),
                    kr_floor: if hard { 0.30 } else { 0.33 },
                    cut_mu: if hard { 0.42 } else { 0.48 },
                    // 58 HRC 볼 엔드밀 Kre/Kte 0.7 (날카로운 3.5 µm 날), T-랜드·호닝 공구는 더 크다 → 1.0 (확신도 낮음)
                    kre_over_kte: if hard { 1.0 } else { 1.5 },
                    // 고경도용 1차 여유각 5~8°
                    clearance_deg: if hard { 7.0 } else { 8.0 },
                    // 고경도 정삭 수명 판정 0.1~0.2 mm
                    vb_criterion_mm: if hard { 0.15 } else { 0.3 },
                    // 날카로운 초경 볼 HRC 52 첫 4초 절삭에 31~63 µm (PHM2010) — 수십 초~2분 안에 끝남
                    breakin_vb_um: if hard { 45.0 } else { 30.0 },
                    breakin_tau_min: if hard { 0.5 } else { 2.0 },
                    // 초경 슬롯 ap: ≤ 30 HRC 는 탄소강과 같은 1.0·D (Kennametal HPHVBN P0~P3 1.0·D, SGS Z-Carb AP ≤ 40 HRC 1.0·D),
                    // 30~45 HRC 0.75·D (Kennametal HPHVBN/UKDV P4 0.75 · OSG AE-VMS 1.0 · Helical ≥ 35 HRC 0.6~1.0 · OSG WXL 0.5 · SGS Z-Carb MD 0.4),
                    // > 45 HRC 0.4·D — 고경도 전용 공구 SGS Z-Carb MD (35~60 HRC) 0.4·D, OSG WXL (55~60 HRC) 0.5·D, GARANT Diabolo 70 (60~70 HRC) 0.05·D,
                    // NS Tool '측면 ap 의 50 % 이하'. 볼 엔드밀의 폭 전체 절삭 ap 는 제조사 자료가 없다 (`[UNKNOWN] pocket_path_strategy`)
                    slot_ap_ratio: Some(if h <= 30.0 {
                        1.0
                    } else if h <= 45.0 {
                        0.75
                    } else {
                        0.4
                    }),
                    // 헬리컬 경사: Seco JS554 (P) · JH120/JHP170 (H, 4·2날) · JH111 (H, 볼) 모두 회전당 2 % Dc → 1.2°, IMCO M924·M726 1~2.5°
                    ramp_deg: Some(1.2),
                    ..base(&format!("alloy_steel_hrc{}", hardness_hrc), &format!("합금강/금형강 (HRC {})", hardness_hrc))
                }
            }
            WorkpieceMaterial::StainlessSteel => WorkpieceProps {
                density: 8000.0,
                specific_heat: 500.0,
                conductivity: 16.2,
                expansion: 17.3e-6,
                elastic_gpa: 193.0,
                hardness_hrc: 20.0,
                kc11: 2000.0,
                mc: 0.21,
                // 12 → 10°: 스테인리스용 초경 엔드밀 경사 +8~12°
                rake_deg: 10.0,
                adhesion: 1.15,
                wear_activation_k: 7500.0,
                thermal_factor: 0.85,
                melt_c: 1400.0,
                bue_range_c: Some((300.0, 500.0)),
                fz_coeff: 0.004,
                ae_ratio: 0.25,
                edge_radius_um: 10.0,
                ref_life_min: 45.0,
                // 0.35 → 0.28: 304 선삭 h_min/r 0.25~0.30 (FE 해석, Zou 외 2026 Materials 19:591 — 실험은 힘·경도만 확인)
                min_chip_ratio: 0.28,
                work_hardening: 0.30,
                // 1.1 → 1.8: 공정감쇠 계수 C 가 304 4.1~5.8e5 N/m, 1018 강 1.3~2.0e5 (같은 공구·조건, Tyler·Schmitz 2014, NAMRI/SME 42) →
                // C 비 약 3.2 배. 이 모델의 이득은 계수의 제곱으로 커지므로 √3.2 ≈ 1.8
                process_damping: 1.8,
                thermo: vec![
                    (20.0, 16.2, 500.0),
                    (100.0, 16.6, 500.0),
                    (200.0, 17.9, 530.0),
                    (300.0, 19.0, 545.0),
                    (400.0, 20.3, 560.0),
                    (500.0, 21.5, 570.0),
                    (600.0, 22.8, 585.0),
                    (800.0, 25.4, 610.0),
                ],
                jc: Some(JohnsonCook::new(310.0, 1000.0, 0.65, 0.07, 1.0, 1400.0)),
                matrix_hv: 200.0,
                hard_phase_hv: 450.0,
                hard_phase_frac: 0.03,
                mech_shares: [0.25, 0.30, 0.35, 0.10],
                lift_factor: 1.0,
                // 304 선삭 Ft/Fc 기울기 0.30 (경사 10°)
                kr_floor: 0.30,
                // FE 관행 0.6, 강 직교 실측 중앙 0.50
                cut_mu: 0.55,
                // 304 선삭 날끝 절편 Fc 51 N/mm (r ≈ 20 µm) → 식보다 약 1.3배 (확신도 낮음)
                kte_factor: 1.3,
                kre_over_kte: 1.0,
                clearance_deg: 9.0,
                // 스테인리스 실무 판정 0.2 mm
                vb_criterion_mm: 0.2,
                // 호닝 8~10 µm nACo 코팅으로 316L 을 깎으면 30분간 마모가 보이지 않음 (Tymczyszyn 외 2025, Materials 18:1959 — 미세 마모는 30분 뒤부터) → 작은 길들이기
                breakin_vb_um: 20.0,
                breakin_tau_min: 2.0,
                // 초경 슬롯 ap (Ø8): OSG AE-VMS 1.0·D (D > 6, D ≤ 6 은 0.5), SGS Z-Carb AP · Haas HTPM 1.0·D, Helical HSV-3 (3날 부등분할) 1.0~1.5·D,
                // Kennametal HARVI I TE M1 1.25·D, Guhring Uni-Pro 0.5~1.0·D — 주류 1.0
                slot_ap_ratio: Some(1.0),
                // 헬리컬 경사: Seco JS553 (3날 부등분할) · JS513 회전당 3 % Dc → 1.8°, IMCO M525 1~2.5°
                ramp_deg: Some(1.8),
                ..base("stainless", "스테인리스강 (SUS304)")
            },
            WorkpieceMaterial::Titanium => WorkpieceProps {
                density: 4430.0,
                specific_heat: 526.0,
                conductivity: 6.7,
                expansion: 8.6e-6,
                elastic_gpa: 114.0,
                hardness_hrc: 36.0,
                kc11: 1400.0,
                mc: 0.23,
                rake_deg: 10.0,
                adhesion: 1.1,
                wear_activation_k: 6500.0,
                thermal_factor: 0.7,
                melt_c: 1650.0,
                bue_range_c: None,
                fz_coeff: 0.004,
                ae_ratio: 0.2,
                edge_radius_um: 10.0,
                ref_life_min: 35.0,
                min_chip_ratio: 0.30,
                work_hardening: 0.12,
                softening_m: 1.1,
                // 1.3 → 1.0: 공정감쇠 계수 C 가 Ti-6Al-4V 1.2~1.8e5 N/m 로 1018 강과 같은 수준 (Tyler·Schmitz 2014)
                process_damping: 1.0,
                chem: WpChem::Titanium,
                thermo: vec![
                    (20.0, 6.7, 526.0),
                    (100.0, 7.4, 553.0),
                    (200.0, 8.7, 574.0),
                    (300.0, 9.8, 595.0),
                    (400.0, 11.1, 616.0),
                    (500.0, 12.6, 636.0),
                    (600.0, 14.2, 657.0),
                    (800.0, 17.5, 700.0),
                    (1000.0, 20.5, 740.0),
                ],
                jc: Some(JohnsonCook::new(1098.0, 1092.0, 0.93, 0.014, 1.1, 1605.0)),
                poisson: 0.34,
                matrix_hv: 340.0,
                hard_phase_hv: 0.0,
                hard_phase_frac: 0.0,
                wear_activation_high_k: 20000.0,
                wear_transition_c: 850.0,
                mech_shares: [0.10, 0.45, 0.40, 0.05],
                lift_factor: 1.3,
                // Krc/Ktc: Ø12 4날 초경 엔드밀 실측 0.5 (Ktc 2000·Krc 1000 N/mm², Kte 41·Kre 77 N/mm — Li 외 2014, J. Vibroeng. 16:2657, Ti6Al4V), TC4 볼 실측 0.37
                // (Tian 외 2026, Sci. Rep. 16:6541 — 축 위치각에 따라 변하며 κ≈0.3 rad 부근 값), Budak 외 1996 (JMSE 118:216) 직교 데이터베이스의 경사 변환 0.29 → 중앙 0.37
                kr_floor: 0.37,
                // 무코팅 초경 직교절삭 실측 μ 0.4~0.6 (ETH — Klippel 외 2022 arXiv 2209.04197, 학술지판 IJAMT 135:2871 2024), 날끝 힘을 뺀 β 20~23° → 0.36~0.42
                cut_mu: 0.45,
                kte_factor: 1.7,
                kre_over_kte: 1.9,
                clearance_deg: 10.0,
                // Ti 판정 VB 평균 0.2 mm / 최대 0.3 mm (Liu 외 2019, Materials 12:1937 — 예전 표기 'Wu 외 2019' 는 확인 안 됨)
                vb_criterion_mm: 0.2,
                // AlCrN Vc 150 에서 5 m 이송에 115 µm, PVD 코팅이 수 초~수 분에 벗겨짐 → 권장 속도에서 50~100 µm, 속도에 따라 커짐
                breakin_vb_um: 60.0,
                breakin_tau_min: 1.5,
                breakin_vc_exp: 0.5,
                // 4날 초경 슬롯 ap (Ø10) 는 제조사마다 크게 갈린다: OSG AE-VMS 0.25·D · Haas HTPM 0.5·D · Guhring Uni-Pro 0.5~1.0·D · SGS Z-Carb AP ≤ 1.0·D ·
                // Helical HSV-4 0.6~1.25·D · Kennametal HPHVBN/UKDV 1.0·D · HARVI I TE 1.25·D — 가운데값 1.0 (퍼짐은 `[UNKNOWN] pocket_path_strategy`)
                slot_ap_ratio: Some(1.0),
                // 헬리컬 경사: Seco JHP770 · JHP750 회전당 3 % Dc → 1.8°, IMCO M525 1~2.5°
                ramp_deg: Some(1.8),
                ..base("titanium", "티타늄 (Ti-6Al-4V)")
            },
            WorkpieceMaterial::Inconel => WorkpieceProps {
                density: 8190.0,
                specific_heat: 435.0,
                conductivity: 11.4,
                expansion: 13.0e-6,
                elastic_gpa: 200.0,
                hardness_hrc: 40.0,
                kc11: 2900.0,
                mc: 0.25,
                rake_deg: 8.0,
                adhesion: 1.1,
                wear_activation_k: 8000.0,
                thermal_factor: 0.7,
                melt_c: 1300.0,
                bue_range_c: None,
                fz_coeff: 0.003,
                ae_ratio: 0.1,
                // 10 → 15 µm: 인코넬용 엔드밀 호닝 10~25 µm
                edge_radius_um: 15.0,
                ref_life_min: 25.0,
                min_chip_ratio: 0.35,
                work_hardening: 0.35,
                softening_m: 1.3,
                process_damping: 1.3,
                chem: WpChem::Nickel,
                thermo: vec![
                    (20.0, 11.4, 435.0),
                    (100.0, 12.5, 455.0),
                    (200.0, 14.0, 470.0),
                    (300.0, 15.5, 485.0),
                    (400.0, 17.0, 500.0),
                    (500.0, 18.3, 520.0),
                    (600.0, 19.6, 540.0),
                    (800.0, 22.8, 590.0),
                    (1000.0, 25.8, 640.0),
                ],
                jc: Some(JohnsonCook::new(1241.0, 622.0, 0.6522, 0.0134, 1.3, 1297.0)),
                matrix_hv: 440.0,
                hard_phase_hv: 2200.0,
                hard_phase_frac: 0.01,
                wear_activation_high_k: 20000.0,
                wear_transition_c: 850.0,
                mech_shares: [0.30, 0.25, 0.35, 0.10],
                lift_factor: 1.2,
                // 공개 엔드밀 계수 세트 없음 — 강과 스테인리스 사이로 둔 가정값 (확신도 낮음)
                kr_floor: 0.30,
                cut_mu: 0.55,
                kre_over_kte: 1.4,
                // 실측 인코넬용 엔드밀 1차 여유각 8.5° (MM Science J. 2020)
                clearance_deg: 9.0,
                // VBmax 0.3 mm (노치 마모가 먼저 오는 경우가 많음)
                vb_criterion_mm: 0.3,
                // Ti 와 같은 50~100 µm 대역의 하한 — 인코넬 실측(5 m 에 320~600 µm)은 권장 속도의 2.5~4배에서 난 날끝 붕괴라 그대로 쓰지 않는다
                breakin_vb_um: 50.0,
                breakin_tau_min: 1.0,
                breakin_vc_exp: 0.5,
                // Ni 계(S3) 초경 엔드밀 슬롯 ap: Kennametal HARVI II TE 5날 Ø12 R1 (이 프리셋 공구와 같은 급) 0.75·D · Seco JHP780 0.6·D ·
                // Helical HEV-5 0.25~0.5·D · Mitsubishi VQN6MVRB 0.5·D · Haas HTPM 0.5·D · OSG AE-VMS 0.25·D, 슬롯 fz 0.03~0.04 mm (Ø12) — 상한 0.75
                slot_ap_ratio: Some(0.75),
                // 헬리컬 경사: Seco JHP780 (HRSA) 회전당 3 % Dc (구멍 1.3·Dc → 1.8°), IMCO M525 1~2.5°, Haimer F1005LL 직선 램프 1~2°
                ramp_deg: Some(1.8),
                ..base("inconel", "인코넬 718")
            },
            WorkpieceMaterial::SuperAlloy => WorkpieceProps {
                density: 8250.0,
                specific_heat: 450.0,
                conductivity: 11.0,
                expansion: 12.5e-6,
                elastic_gpa: 210.0,
                hardness_hrc: 42.0,
                kc11: 3100.0,
                mc: 0.25,
                rake_deg: 6.0,
                adhesion: 1.1,
                wear_activation_k: 8000.0,
                thermal_factor: 0.7,
                melt_c: 1330.0,
                bue_range_c: None,
                fz_coeff: 0.0025,
                ae_ratio: 0.1,
                edge_radius_um: 10.0,
                ref_life_min: 20.0,
                min_chip_ratio: 0.35,
                work_hardening: 0.35,
                softening_m: 1.25,
                process_damping: 1.3,
                chem: WpChem::Nickel,
                thermo: vec![
                    (20.0, 11.0, 450.0),
                    (200.0, 13.3, 480.0),
                    (400.0, 16.0, 510.0),
                    (600.0, 18.7, 545.0),
                    (800.0, 21.5, 590.0),
                    (1000.0, 24.0, 640.0),
                ],
                jc: Some(JohnsonCook::new(1100.0, 1000.0, 0.58, 0.012, 1.25, 1330.0)),
                matrix_hv: 450.0,
                hard_phase_hv: 2200.0,
                hard_phase_frac: 0.015,
                wear_activation_high_k: 20000.0,
                wear_transition_c: 850.0,
                mech_shares: [0.30, 0.25, 0.35, 0.10],
                lift_factor: 1.2,
                // 공개 엔드밀 계수 세트 없음 — 강과 스테인리스 사이로 둔 가정값 (확신도 낮음)
                kr_floor: 0.30,
                cut_mu: 0.55,
                kre_over_kte: 1.4,
                // 실측 인코넬용 엔드밀 1차 여유각 8.5° (MM Science J. 2020)
                clearance_deg: 9.0,
                // VBmax 0.3 mm (노치 마모가 먼저 오는 경우가 많음)
                vb_criterion_mm: 0.3,
                // Ti 와 같은 50~100 µm 대역의 하한 — 인코넬 실측(5 m 에 320~600 µm)은 권장 속도의 2.5~4배에서 난 날끝 붕괴라 그대로 쓰지 않는다
                breakin_vb_um: 50.0,
                breakin_tau_min: 1.0,
                breakin_vc_exp: 0.5,
                // 제조사 슬롯 자료의 Ni 계 내열합금 묶음(S3)과 같은 값 (인코넬 참조)
                slot_ap_ratio: Some(0.75),
                // 헬리컬 경사: 인코넬과 같은 HRSA 묶음 (Seco JHP780 3 % Dc → 1.8°, IMCO M525 1~2.5°)
                ramp_deg: Some(1.8),
                ..base("superalloy", "수퍼알로이")
            },
            WorkpieceMaterial::CFRP => WorkpieceProps {
                density: 1550.0,
                specific_heat: 1000.0,
                conductivity: 5.0,
                expansion: 1.0e-6,
                elastic_gpa: 70.0,
                hardness_hrc: 0.0,
                // 600 → 420: 앱 로그(6 mm 판 관통, ap 실효 6 mm)에서 경로 평균 합력 342 N · 실효 kc1.1 397 이 실측 환산 215~240 N 의 1.4~1.6배였다.
                // 실측 Kienzle 맞춤 kc1.1 250~420 · mc 0.09~0.21 (Hamedanianpour 2013 ÉTS 석사논문, CVD 다이아몬드 6날 슬롯),
                // PCD 0° 경사 kc1.1 ≈ 340 (180~540, Karpat 외 2012 IJMTM 56:79 의 Ktc 를 환산) — 섬유각에 따라 ±50 % 움직인다
                kc11: 420.0,
                mc: 0.20,
                rake_deg: 10.0,
                adhesion: 0.6,
                wear_activation_k: 2500.0,
                // 0.42 → 0.5: 0.42 는 kc 가 높아서 생긴 온도를 깎던 값이다. kc 를 바로잡은 뒤 프리셋 계면 약 220 °C —
                // 측정 69~300 °C (새 공구 날끝 1.7 mm 열전대 300 °C · Delahaigue 외 2017 Fibers 5:46, Kerrigan·O'Donnell 2016 155~185 °C)
                thermal_factor: 0.5,
                melt_c: 400.0,
                // 180 → 350 °C: 이 값은 날끝(공구 쪽) 계면 온도와 비교된다. 수지 Tg 150~212 °C 는 가공면 온도라 그대로 쓰면 깨끗하게 깎인
                // 실측(새 공구 300 °C)까지 모두 과열이 된다. 마모 공구 475 °C 에서 수지 탄화 (Delahaigue 2017), 수지 연소 개시 315 °C (Kerrigan 2016) 사이
                temp_limit_c: Some(350.0),
                bue_range_c: None,
                fz_coeff: 0.006,
                ae_ratio: 0.5,
                edge_radius_um: 3.0,
                // 60 → 5 분 (AlTiN 기준 공구 Ø10 4날 · Vc 200 · fz 0.06 · ae 0.5D 에서 VB 0.2 mm 까지).
                // TiAlN 인서트 CFRP 밀링 VBmax 0.67~1.3 µm / 날 접촉 m (Ozkan 외 2020 Coatings 10:623) → 접촉 50 m/min 에서 33~65 µm/min → 3~6 분.
                // PVD 코팅은 약 1 분 안에 날끝에서 벗겨진다 (Wang 외 2013 드릴 자료, Sturtevant 2013 WSU 석사논문). 다이아몬드 코팅 라우터 실측 5~18 분
                // (Hamedanianpour 2013 — 0.085~0.4 µm/m) 과도 맞는다 — 예전 60 분이면 다이아몬드 칸이 120 분이었다
                ref_life_min: 5.0,
                min_chip_ratio: 0.20,
                work_hardening: 0.0,
                // 0.3 → 3.5: 단단히 고정한 CFRP 트리밍은 0차 이론이 채터(필요 이득 3~30배)를 예측하는 조건에서도 안정 —
                // Karpat 외 2012 (Ø10 PCD, ap 10 mm, 슬롯, 공구 모드 3.5 kHz · ζ 1.4~1.8 %), Hamedanianpour 2013 (6날, 슬롯 ap 4.44 mm, 6.7~13.4 krpm),
                // Delahaigue 외 2017 (돌출 35 mm, 슬롯 ap 3.6 mm). 섬유 탄성 복원이 플랭크를 길게 누르는 감쇠로 보고 (정량 자료 없음, 추정),
                // 파장이 긴 저주파 모드(홀더 600~700 Hz)는 여전히 채터한다 (Rusinek·Lajmert 2020 Materials 13:4105)
                process_damping: 3.5,
                chem: WpChem::CarbonFiber,
                thermo: vec![(20.0, 5.0, 1000.0), (100.0, 5.2, 1150.0), (200.0, 5.4, 1300.0), (300.0, 5.5, 1400.0)],
                jc: None,
                poisson: 0.3,
                matrix_hv: 40.0,
                hard_phase_hv: 1600.0,
                hard_phase_frac: 0.6,
                wear_activation_high_k: 5000.0,
                wear_transition_c: 300.0,
                mech_shares: [0.95, 0.03, 0.005, 0.015],
                lift_factor: 0.0,
                // CFRP 는 날끝 마찰(rubbing)이 지배하고 물리적인 엔드밀 계수 세트가 없다 — FE 마찰계수 0.2~0.6 (섬유각) 중간값 (확신도 낮음)
                kr_floor: 0.25,
                cut_mu: 0.30,
                kre_over_kte: 1.4,
                // CFRP 용 공구 1차 여유각 12~20°
                clearance_deg: 15.0,
                // 섬유 미절단·박리가 VB 보다 먼저 오는 경우가 많아 0.1~0.2 mm
                vb_criterion_mm: 0.2,
                // 20 µm / 2 분 → 30 µm / 0.35 분: 다이아몬드 코팅 라우터는 첫 1 m 슬롯(약 0.3~0.7 분) 안에 VB 25~40 µm (Hamedanianpour 2013)
                breakin_vb_um: 30.0,
                breakin_tau_min: 0.35,
                // 다이아몬드 공구 슬롯 ap: Amana CVD 12날 라우터 1×D (2×D 는 칩 하중 −25 %), Harvey 다이아몬드 컷 슬롯 1×D, OSG AERO-REC 1×D 미만 — 1.0.
                // 헬리컬 경사·구멍 지름은 제조사 자료 없음 (SGS 복합재 라우터 직선 램프 5° 뿐) → `gcode::ENTRY_RAMP_DEG_UNKNOWN`
                slot_ap_ratio: Some(1.0),
                // Sandvik CoroMill Plura 복합재: 상향(업)밀링 지시 — 벽 정삭도 상향
                finish_up_milling: true,
                ..base("cfrp", "CFRP")
            },
        }
    }

    pub fn k_at(&self, t_c: f64) -> f64 {
        table_at(&self.thermo, t_c, |p| p.1).unwrap_or(self.conductivity).max(0.1)
    }

    pub fn cp_at(&self, t_c: f64) -> f64 {
        table_at(&self.thermo, t_c, |p| p.2).unwrap_or(self.specific_heat).max(50.0)
    }

    pub fn rho_c_at(&self, t_c: f64) -> f64 {
        self.density * self.cp_at(t_c)
    }

    pub fn diffusivity_at(&self, t_c: f64) -> f64 {
        self.k_at(t_c) / self.rho_c_at(t_c)
    }

    pub fn jc_thermal(&self, t_c: f64) -> f64 {
        match &self.jc {
            Some(j) => j.thermal(t_c),
            None => {
                let span = (self.melt_c - 20.0).max(100.0);
                1.0 - ((t_c - 20.0) / span).clamp(0.0, 0.95).powf(self.softening_m.max(0.1))
            }
        }
    }

    pub fn flow_stress(&self, strain: f64, strain_rate: f64, t_c: f64) -> f64 {
        match &self.jc {
            Some(j) => j.flow_stress(strain, strain_rate, t_c),
            None => self.kc11 / 3.0 * self.jc_thermal(t_c),
        }
    }

    pub fn rho_c(&self) -> f64 {
        self.density * self.specific_heat
    }

    pub fn reference_coolant(&self) -> CoolantConfig {
        if self.key == "aluminum" {
            CoolantConfig::flood()
        } else if self.key == "cfrp" || self.hardness_hrc >= 45.0 {
            CoolantConfig::air_blast()
        } else if self.key == "titanium" || self.key == "inconel" || self.key == "superalloy" {
            CoolantConfig::through_tool()
        } else {
            CoolantConfig::flood()
        }
    }

    pub fn diffusivity(&self) -> f64 {
        self.conductivity / self.rho_c()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoatingTribology {
    pub name: String,
    pub family: String,
    pub friction: f64,
    pub max_temp_c: f64,
    pub hardness_hv: f64,
    pub wear_factor: f64,
    pub ferrous_ok: bool,
    #[serde(default)]
    pub oxidation_c: f64,
    #[serde(default)]
    pub hot_hardness_c: f64,
    #[serde(default)]
    pub conductivity: f64,
    #[serde(default)]
    pub elastic_gpa: f64,
    #[serde(default)]
    pub expansion: f64,
    #[serde(default)]
    pub thickness_um: f64,
    #[serde(default)]
    pub chem: CoatingChem,
    #[serde(default)]
    pub protective_oxide: bool,
    #[serde(default = "one")]
    pub crack_tolerance: f64,
    #[serde(default = "one")]
    pub diffusion: f64,
    #[serde(default = "one")]
    pub oxidation: f64,
}

pub fn tribology_from_name(name: Option<&str>) -> CoatingTribology {
    let raw = name.unwrap_or("").trim().to_string();
    let n = raw.to_lowercase().replace([' ', '-', '_'], "");
    let family = tribology::coating_family(&n);
    let p = tribology::coating_props(family);
    CoatingTribology {
        name: if raw.is_empty() { "무코팅".into() } else { raw },
        family: family.into(),
        friction: p.friction,
        max_temp_c: p.max_temp_c,
        hardness_hv: p.hardness_hv,
        wear_factor: p.wear_factor,
        ferrous_ok: p.ferrous_ok,
        oxidation_c: p.oxidation_c,
        hot_hardness_c: p.hot_hardness_c,
        conductivity: p.conductivity,
        elastic_gpa: p.elastic_gpa,
        expansion: p.expansion,
        thickness_um: p.thickness_um,
        chem: p.chem,
        protective_oxide: p.protective_oxide,
        crack_tolerance: p.crack_tolerance,
        diffusion: p.diffusion,
        oxidation: p.oxidation,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGeometry {
    pub diameter_mm: f64,
    pub flutes: u32,
    pub helix_deg: f64,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_mm: f64,
    pub nose: ToolNose,
    pub stickout_mm: f64,
    pub rake_deg: f64,
    pub clearance_deg: f64,
    pub edge_radius_um: f64,
    pub core_ratio: f64,
    pub runout_um: f64,
    pub variable_pitch: bool,
    pub elastic_gpa: f64,
    pub tool_conductivity: f64,
    pub tool_expansion: f64,
    pub tool_rho_c: f64,
    pub base_wear_factor: f64,
    pub coating: CoatingTribology,
    pub holder_stiffness_n_per_um: f64,
    pub flank_wear_mm: f64,
    pub tool_density: f64,
    pub damping_ratio: f64,
    pub holder_mass_kg: f64,
    pub measured_fn_hz: Option<f64>,
    pub measured_k_n_per_um: Option<f64>,
    pub measured_stickout_mm: Option<f64>,
    #[serde(default)]
    pub poisson: f64,
    #[serde(default)]
    pub fracture_toughness: f64,
    #[serde(default)]
    pub neck_diameter_mm: f64,
    #[serde(default)]
    pub reach_mm: f64,
    #[serde(default)]
    pub substrate: String,
    #[serde(default = "default_substrate_max_c")]
    pub substrate_max_c: f64,
    #[serde(default)]
    pub edge_default_um: f64,
    /// 공구 반경을 회전 상태로 재서 런아웃 과삭이 보정값에 이미 들어 있는가
    #[serde(default)]
    pub runout_compensated: bool,
    /// 스핀들 모드 (공구 끝 기준) — `MachineLimits::spindle_*`
    #[serde(default = "default_spindle_k")]
    pub spindle_stiffness_n_per_um: f64,
    #[serde(default = "default_spindle_fn")]
    pub spindle_fn_hz: f64,
    #[serde(default = "default_spindle_zeta")]
    pub spindle_zeta: f64,
    /// 용도 적합도(`appfit`) — 공구 형상·날끝·모재가 이 피삭재의 권장 창 밖이면 정상 마모율·치핑 위험·구성인선 경향을 키우는 배수.
    /// 기준 상태(정규화) 공구에서는 1 (`reset_to_reference_substrate`)
    #[serde(default = "one")]
    pub fit_wear: f64,
    #[serde(default = "one")]
    pub fit_chip: f64,
    #[serde(default = "one")]
    pub fit_bue: f64,
    /// 경사각·여유각·호닝을 공구에 입력하지 않아 피삭재 기본값을 썼는가 (그 소재용 공구라고 가정한 것)
    #[serde(default)]
    pub geometry_from_workpiece: bool,
}

fn default_spindle_k() -> f64 {
    30.0
}

fn default_spindle_fn() -> f64 {
    1000.0
}

fn default_spindle_zeta() -> f64 {
    0.05
}

fn default_substrate_max_c() -> f64 {
    1000.0
}

pub const CARBIDE_PROPS: (f64, f64, f64, f64, f64, f64, f64, f64) = (600.0, 80.0, 5.5e-6, 14500.0 * 220.0, 1.0, 14500.0, 0.22, 10.0);

pub const COATING_EDGE_GROWTH: f64 = 0.8;

/// 플랭크 마모 랜드 VB 가 만드는 날끝의 반경 방향 후퇴: KE = VB·tanα / (1 − tanα·tanγ) (Kronenberg 1966, Machining Science and Application; α 여유각, γ 경사각).
/// 예전에는 VB·tanα 만 썼다 — 경사 18°·여유 12° 알루미늄 공구에서 7 %, 경사 10°·여유 10° 에서 3 % 작게 읽는다 (음의 경사는 조금 작아짐)
pub fn radial_per_vb(clearance_deg: f64, rake_deg: f64) -> f64 {
    let ta = clearance_deg.clamp(2.0, 30.0).to_radians().tan();
    let tg = rake_deg.clamp(-30.0, 30.0).to_radians().tan();
    ta / (1.0 - ta * tg).max(0.5)
}

pub fn coated_edge_um(hone_um: f64, coating_um: f64) -> f64 {
    hone_um.max(0.0) + COATING_EDGE_GROWTH * coating_um.max(0.0)
}

impl ToolGeometry {
    pub fn from_setting(s: &EndMillMockupSetting, wp: &WorkpieceProps, machine: &MachineLimits, base: Option<&ToolBaseMaterial>) -> Self {
        let z = s.flute_count.max(1) as u32;
        let core = if z <= 2 {
            0.75
        } else if z == 3 {
            0.78
        } else {
            0.80
        };
        let (e, k, alpha, rho_c, wf, density, nu, kic) = match base {
            Some(ToolBaseMaterial::HSS(crate::material::HSSGrade::M42 | crate::material::HSSGrade::M35)) => (215.0, 24.0, 10.5e-6, 8200.0 * 460.0, 2.2, 8200.0, 0.29, 22.0),
            Some(ToolBaseMaterial::HSS(_)) => (210.0, 25.0, 11.0e-6, 8100.0 * 460.0, 3.0, 8100.0, 0.29, 25.0),
            Some(ToolBaseMaterial::CBN) => (680.0, 100.0, 4.7e-6, 3480.0 * 800.0, if wp.hardness_hrc >= 45.0 { 0.3 } else { 1.5 }, 3480.0, 0.15, 6.5),
            Some(ToolBaseMaterial::PCD) => (900.0, 500.0, 3.0e-6, 3500.0 * 510.0, if wp.key == "aluminum" || wp.key == "cfrp" { 0.1 } else { 10.0 }, 3500.0, 0.07, 8.5),
            Some(ToolBaseMaterial::SolidCarbide(CarbideGrade::UltraFine { .. })) => (620.0, 75.0, 5.2e-6, 14500.0 * 220.0, 0.9, 14500.0, 0.22, 9.0),
            _ => CARBIDE_PROPS,
        };
        let frf_ok = machine.tip_frf_tool.as_deref().map(|k| k == s.dynamic_key()).unwrap_or(true);
        let edge_default = if s.is_high_end { wp.edge_radius_um } else { wp.edge_radius_um * 0.7 };
        let coating = tribology_from_name(s.coating_name.as_deref());
        let edge_coated = coated_edge_um(edge_default, coating.thickness_um);
        let neck = s.neck();
        let stickout = s.effective_stickout_mm();
        // 감쇠비: 사용자가 넣은 값(탭 테스트)이 없으면 돌출비·홀더로 정한 공구 끝 모드 기본값 (예전에는 모든 공구 0.03)
        let damping = if machine.damping_is_user() {
            machine.damping_ratio
        } else {
            crate::pairref::default_damping(stickout / s.diameter_mm.max(1e-6), s.holder)
        };
        let mut t = Self {
            diameter_mm: s.diameter_mm,
            flutes: z,
            helix_deg: s.helix_angle_deg,
            loc_mm: s.loc_mm,
            oal_mm: s.oal_mm,
            shank_mm: s.shank_diameter_mm,
            nose: s.nose,
            stickout_mm: stickout,
            rake_deg: s.rake_deg.unwrap_or(wp.rake_deg),
            clearance_deg: s.clearance_deg.unwrap_or(wp.clearance_deg),
            edge_radius_um: s.edge_radius_um.unwrap_or(edge_coated),
            core_ratio: core,
            // 런아웃 TIR: 입력값 > 홀더 사양 > 공구 등급 가정 (고급형 3 µm 는 열박음·유압 수준, 보급형 10 µm 는 ER 정밀·표준 사이)
            runout_um: s.runout_um.or(s.holder.tir_um()).unwrap_or(if s.is_high_end { 3.0 } else { 10.0 }),
            variable_pitch: s.variable_pitch,
            elastic_gpa: e,
            tool_conductivity: k,
            tool_expansion: alpha,
            tool_rho_c: rho_c,
            base_wear_factor: wf,
            coating,
            holder_stiffness_n_per_um: machine.holder_stiffness_n_per_um,
            flank_wear_mm: 0.0,
            tool_density: density,
            damping_ratio: damping.clamp(0.005, 0.2),
            holder_mass_kg: machine.holder_mass_kg.max(0.0),
            measured_fn_hz: machine.tip_fn_hz.filter(|v| frf_ok && *v > 0.0),
            measured_k_n_per_um: machine.tip_stiffness_n_per_um.filter(|v| frf_ok && *v > 0.0),
            measured_stickout_mm: machine.tip_frf_stickout_mm.filter(|v| frf_ok && *v > 0.0),
            poisson: nu,
            fracture_toughness: kic,
            neck_diameter_mm: neck.map(|(d, _)| d).unwrap_or(0.0),
            reach_mm: neck.map(|(_, r)| r).unwrap_or(0.0),
            substrate: s.substrate.key().into(),
            substrate_max_c: s.substrate.max_temp_c(),
            edge_default_um: edge_default,
            runout_compensated: s.radius_measured_rotating,
            spindle_stiffness_n_per_um: machine.spindle_stiffness_n_per_um.max(1.0),
            spindle_fn_hz: machine.spindle_fn_hz.max(50.0),
            spindle_zeta: machine.spindle_zeta.clamp(0.005, 0.3),
            fit_wear: 1.0,
            fit_chip: 1.0,
            fit_bue: 1.0,
            geometry_from_workpiece: s.rake_deg.is_none() || s.clearance_deg.is_none() || s.edge_radius_um.is_none(),
        };
        t.refresh_fit(wp);
        t
    }

    /// 용도 적합도 배수를 이 피삭재 기준으로 다시 계산한다 (형상·모재를 바꾼 뒤 호출)
    pub fn refresh_fit(&mut self, wp: &WorkpieceProps) {
        let fit = crate::appfit::evaluate(self, wp, false);
        self.fit_wear = fit.wear_factor;
        self.fit_chip = fit.chip_factor;
        self.fit_bue = fit.bue_factor;
    }

    pub fn has_neck(&self) -> bool {
        self.neck_diameter_mm > 0.0 && self.reach_mm > self.loc_mm
    }

    /// 편심 진폭 e (mm). `runout_um` 은 다이얼 게이지로 읽는 TIR(전체 흔들림, 홀더 사양서의 "런아웃 3 µm" 같은 값)이고,
    /// 공구 중심이 스핀들 축에서 벗어난 거리 e 는 TIR/2 이다. 날 j 의 절삭 반경은 r + e·cos(ψj) 이므로 2날 공구에서 두 날의
    /// 칩 두께 차이(오프셋)는 2e = TIR 이다. (예전 구현은 runout_um 을 e 로 읽어 오프셋이 ±2·TIR 이 되었다 — 알루미늄 황삭
    /// 프리셋 TIR 10 µm 에서 ±20 µm, 평균 칩 두께의 약 70%: 피크/평균 힘 4.3, 평균 칩 두께 28.6 µm 로 부풀었다.)
    pub fn runout_ecc_mm(&self) -> f64 {
        0.5 * self.runout_um.max(0.0) / 1000.0
    }

    /// 런아웃으로 가장 긴 날이 벽을 더 깎는 양 (µm, 크기). 공구 반경을 회전 상태로 재서 보정했으면 0
    pub fn runout_overcut_um(&self) -> f64 {
        if self.runout_compensated {
            0.0
        } else {
            crate::surface::RUNOUT_EFFECTIVE * self.runout_ecc_mm() * 1000.0
        }
    }

    /// 공구 아래쪽(홀더 + 스핀들)의 정적 컴플라이언스 (mm/N). 예전에는 홀더 스프링(50 N/µm)만 있었다 —
    /// 짧은 공구에서는 스핀들 모드(20~60 N/µm)가 공구 끝 정적 강성의 큰 몫이다 (Ø20 L/D 3.75: 세 모드 직렬 ≈ 12 N/µm)
    pub fn support_compliance_mm_per_n(&self) -> f64 {
        1.0 / (self.holder_stiffness_n_per_um.max(1e-6) * 1000.0) + 1.0 / (self.spindle_stiffness_n_per_um.max(1e-6) * 1000.0)
    }

    pub fn reset_to_reference_substrate(&mut self, wp: &WorkpieceProps) {
        let (e, k, alpha, rho_c, wf, density, nu, kic) = CARBIDE_PROPS;
        self.elastic_gpa = e;
        self.tool_conductivity = k;
        self.tool_expansion = alpha;
        self.tool_rho_c = rho_c;
        self.base_wear_factor = wf;
        self.tool_density = density;
        self.poisson = nu;
        self.fracture_toughness = kic;
        self.rake_deg = wp.rake_deg;
        self.clearance_deg = wp.clearance_deg;
        if self.edge_default_um > 0.0 {
            self.edge_radius_um = coated_edge_um(self.edge_default_um, tribology::coating_props("AlTiN").thickness_um);
        }
        self.neck_diameter_mm = 0.0;
        self.reach_mm = 0.0;
        self.substrate = "carbide".into();
        self.substrate_max_c = 1000.0;
        self.fit_wear = 1.0;
        self.fit_chip = 1.0;
        self.fit_bue = 1.0;
    }

    pub fn radius(&self) -> f64 {
        self.diameter_mm / 2.0
    }

    /// 플랭크 마모 VB 1 mm 당 반경(날끝) 후퇴 (mm/mm) — `radial_per_vb`
    pub fn radial_per_vb(&self) -> f64 {
        radial_per_vb(self.clearance_deg, self.rake_deg)
    }

    pub fn local_radius(&self, z_from_tip: f64) -> (f64, f64) {
        let r = self.radius();
        let rc = self.nose.corner_radius(self.diameter_mm);
        if rc <= 0.0 || z_from_tip >= rc {
            return (r, 1.0);
        }
        let dz = rc - z_from_tip.max(0.0);
        let local = (r - rc) + (rc * rc - dz * dz).max(0.0).sqrt();
        let sin_k = ((rc * rc - dz * dz).max(0.0).sqrt() / rc).clamp(0.05, 1.0);
        (local.max(1e-4), sin_k)
    }

    pub fn tool_diffusivity(&self) -> f64 {
        self.tool_conductivity / self.tool_rho_c
    }
}

fn standard_room_c() -> f64 {
    crate::profile::STANDARD_AMBIENT_C
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolantState {
    pub method: CoolantMethod,
    pub pressure_bar: f64,
    pub flow_l_min: f64,
    pub temperature_c: f64,
    #[serde(default)]
    pub fluid: CoolantFluid,
    #[serde(default = "standard_room_c")]
    pub room_c: f64,
    #[serde(default)]
    pub temperature_auto: bool,
}

impl CoolantState {
    pub fn from_config(c: &CoolantConfig) -> Self {
        Self::from_config_env(c, &ShopEnvironment::standard())
    }

    pub fn from_config_env(c: &CoolantConfig, env: &ShopEnvironment) -> Self {
        let temperature = c.temperature_in(env);
        Self {
            method: c.method.clone(),
            pressure_bar: c.pressure_bar,
            flow_l_min: c.flow_rate_l_min,
            temperature_c: temperature,
            fluid: CoolantFluid::at(&c.method, env, temperature),
            room_c: env.ambient_c,
            temperature_auto: c.temperature_c.is_none(),
        }
    }

    pub fn from_type(t: &CoolantType) -> Self {
        let cfg = match t {
            CoolantType::Flood => CoolantConfig::flood(),
            CoolantType::Mist => CoolantConfig::mist(),
            CoolantType::AirBlast => CoolantConfig::air_blast(),
            CoolantType::Dry => CoolantConfig::dry(),
            CoolantType::ThroughTool => CoolantConfig::through_tool(),
        };
        Self::from_config(&cfg)
    }

    pub fn ambient_c(&self) -> f64 {
        if matches!(self.method, CoolantMethod::Dry) {
            self.room_c
        } else {
            self.temperature_c
        }
    }

    pub fn is_liquid(&self) -> bool {
        matches!(self.method, CoolantMethod::Flood | CoolantMethod::ThroughTool)
    }

    /// 공작물 표면 대류 열전달계수 (W/m²K). 절삭 부근 공작물 면에 닿는 냉각 기준이며
    /// 이전 패스 열의 감쇠(loadsim 예열), 덩어리 온도, 정상 상태 공작물 온도에 쓰인다.
    /// 문헌 값: 건식 7~100, 공기 분사 약 540 (실측), MQL 약 560~1,000 (실측),
    /// 수용성 플러드 1,000~2,700 (FE 모델·Kops & Arenson 1999, CIRP Annals 48:47), 유성 플러드 최대 10,000, 고압 액체 분사 10³~10⁴ 이상.
    /// (Rai & Xirouchakis 2009 IJAMT 42:429; Brandão 외 2010 JBSMSE 32:154; Shokrani 외 2021 Procedia CIRP 101:358;
    /// Stegmann 외 2024 Energies 17:3189 — 원형 톱 냉각 채널의 Nu 상관식이라 밀링 h 값의 직접 근거는 아님)
    /// 내부 급유도 액체이므로 플러드보다 낮을 수 없다 — 예전 값 320 은 공기 분사 수준이어서 니켈·티타늄처럼 열이 안 빠지는
    /// 소재에서 이전 패스 열이 과하게 쌓였다 (인코넬 황삭 경로 예열 335 K → 공칭 대비 마모 7.5배).
    pub fn workpiece_h_of(method: &CoolantMethod, flow_l_min: f64) -> f64 {
        20.0 + match method {
            CoolantMethod::Dry => 10.0,
            CoolantMethod::AirBlast => 230.0,
            CoolantMethod::Mist => 480.0,
            CoolantMethod::Flood => 2500.0 * (flow_l_min / 12.0).clamp(0.3, 1.5),
            CoolantMethod::ThroughTool => 3000.0,
        }
    }

    pub fn workpiece_h(&self) -> f64 {
        Self::workpiece_h_of(&self.method, self.flow_l_min)
    }

    pub fn idle_workpiece_h(&self) -> f64 {
        Self::workpiece_h_of(&CoolantMethod::Dry, 0.0)
    }

    pub fn chip_evacuation(&self) -> f64 {
        match self.method {
            CoolantMethod::Dry => 0.3,
            CoolantMethod::AirBlast => 0.7,
            CoolantMethod::Mist => 0.5,
            CoolantMethod::Flood => 0.8,
            CoolantMethod::ThroughTool => 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calibration {
    pub power: f64,
    pub wear: f64,
    pub deflection: f64,
    pub temperature: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            power: 1.0,
            wear: 1.0,
            deflection: 1.0,
            temperature: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutContext {
    pub wp: WorkpieceProps,
    pub tool: ToolGeometry,
    pub coolant: CoolantState,
    pub machine: MachineLimits,
    pub calib: Calibration,
    pub tolerance_mm: f64,
    pub allowance_mm: f64,
    pub fixture_stiffness_n_per_um: f64,
    #[serde(default)]
    pub env: ShopEnvironment,
    #[serde(default)]
    pub tool_age_min: f64,
}

impl CutContext {
    pub fn from_profile(p: &MachiningProfile) -> Self {
        let wp = WorkpieceProps::of(&p.workpiece_setup.effective_material());
        let base = p.endmill_setting.substrate.base();
        let tool = ToolGeometry::from_setting(&p.endmill_setting, &wp, &p.machine, Some(&base));
        let env = p.machine.environment.clone();
        Self {
            coolant: CoolantState::from_config_env(&p.coolant_config, &env),
            machine: p.machine.clone(),
            calib: Calibration::default(),
            tolerance_mm: p.workpiece_setup.effective_tolerance_mm(),
            allowance_mm: p.workpiece_setup.stock_allowance_mm.max(0.0),
            fixture_stiffness_n_per_um: p.workpiece_setup.clamping.stiffness_n_per_um(),
            wp,
            tool,
            env,
            tool_age_min: 0.0,
        }
    }

    pub fn bulk_c(&self) -> f64 {
        if self.coolant.is_liquid() {
            self.coolant.temperature_c
        } else {
            self.env.ambient_c
        }
    }

    /// 공구 수명 판정 VB (mm): 부품 공차 규칙(공차 ≤ 20 µm 이면 0.2, 아니면 ISO 8688-2 의 0.3)과 피삭재 판정 기준
    /// (Ti·스테인리스·CFRP 0.2, 고경도 정삭 0.15 — `WorkpieceProps::vb_criterion_mm`) 중 작은 쪽
    pub fn vb_limit_mm(&self) -> f64 {
        let tol_rule: f64 = if self.tolerance_mm <= 0.02 { 0.2 } else { 0.3 };
        tol_rule.min(self.wp.vb_criterion_mm.max(0.05))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MillMode {
    Up,
    Down,
    Slot,
    Center,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Engagement {
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub phi_st: f64,
    pub phi_ex: f64,
    pub mode: MillMode,
}

impl Engagement {
    pub fn side(ap: f64, ae: f64, d: f64, down: bool) -> Self {
        let ae = ae.clamp(0.0, d);
        if ae >= 0.999 * d {
            return Self {
                ap_mm: ap,
                ae_mm: d,
                phi_st: 0.0,
                phi_ex: PI,
                mode: MillMode::Slot,
            };
        }
        let phis = (1.0 - 2.0 * ae / d).clamp(-1.0, 1.0).acos();
        if down {
            Self {
                ap_mm: ap,
                ae_mm: ae,
                phi_st: PI - phis,
                phi_ex: PI,
                mode: MillMode::Down,
            }
        } else {
            Self {
                ap_mm: ap,
                ae_mm: ae,
                phi_st: 0.0,
                phi_ex: phis,
                mode: MillMode::Up,
            }
        }
    }

    pub fn centered(ap: f64, ae: f64, d: f64) -> Self {
        let phis = (1.0 - 2.0 * ae.clamp(0.0, d) / d).clamp(-1.0, 1.0).acos();
        Self {
            ap_mm: ap,
            ae_mm: ae,
            phi_st: PI / 2.0 - phis / 2.0,
            phi_ex: PI / 2.0 + phis / 2.0,
            mode: MillMode::Center,
        }
    }

    pub fn span(&self) -> f64 {
        (self.phi_ex - self.phi_st).max(0.0)
    }

    pub fn max_sin(&self) -> f64 {
        if self.phi_st <= PI / 2.0 && self.phi_ex >= PI / 2.0 {
            1.0
        } else {
            self.phi_st.sin().max(self.phi_ex.sin())
        }
    }

    pub fn wall_angle(&self) -> Option<f64> {
        match self.mode {
            MillMode::Up => Some(0.0),
            MillMode::Down => Some(PI),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuttingCoeffs {
    pub mu: f64,
    pub beta_deg: f64,
    pub shear_angle_deg: f64,
    pub kc11: f64,
    pub mc: f64,
    pub kr: f64,
    pub kte: f64,
    pub kre: f64,
    pub friction_factor: f64,
    pub friction_power_share: f64,
    #[serde(default = "one")]
    pub softening: f64,
    #[serde(default)]
    pub sigma_flank: f64,
    #[serde(default)]
    pub h_min_mm: f64,
    #[serde(default)]
    pub work_hardening: f64,
    #[serde(default)]
    pub hardened_depth_mm: f64,
    #[serde(default)]
    pub strain_rate: f64,
    #[serde(default = "one")]
    pub rate_factor: f64,
    #[serde(default)]
    pub chip_lift: f64,
    #[serde(default)]
    pub sticking: f64,
    #[serde(default)]
    pub lubricant_access: f64,
    #[serde(default)]
    pub friction_temp_c: f64,
    /// 축방향 힘 계수: dFa = kac_ratio × (전단 접선력) + kae × db. 헬릭스 날의 경사 절삭에서 생기는 공구를 뽑아 올리는 힘.
    /// 알루미늄 엔드밀 측정 Kac/Ktc 0.2~0.4 (경사절삭 이론은 35° 에서 0.16~0.33) — 0.35·tan(헬릭스)
    #[serde(default)]
    pub kac_ratio: f64,
    /// 날 길이당 축방향 날끝 힘 (N/mm). 측정 Kae 0~5 N/mm, 날끝 접선 계수의 약 0.2~0.3배
    #[serde(default)]
    pub kae: f64,
}

fn one() -> f64 {
    1.0
}

fn reference_chip_mm(ctx: &CutContext) -> f64 {
    (ctx.wp.fz_coeff * 10.0).clamp(0.003, 0.25) * 0.64
}

pub fn shear_strain_rate(co: &CuttingCoeffs, rake_deg: f64, vc: f64, h_mm: f64) -> f64 {
    let phi = co.shear_angle_deg.to_radians().clamp(0.1, 1.3);
    let alpha = rake_deg.to_radians();
    let vs = (vc / 60.0).max(1e-3) * alpha.cos() / (phi - alpha).cos().max(0.1);
    let l = (h_mm.max(1e-4) / 1000.0) / phi.sin();
    5.9 * vs / l
}

fn coeffs_from_friction(ctx: &CutContext, fr: &tribology::Friction, t_c: f64) -> CuttingCoeffs {
    let mu = fr.mu_eff.clamp(0.05, 1.2);
    let beta = mu.atan();
    let alpha = ctx.tool.rake_deg.to_radians();
    let x = (beta - alpha).clamp((-5f64).to_radians(), 50f64.to_radians());
    let g = |v: f64| 2.0 * v.cos() / (1.0 - v.sin());
    let x0 = 0.55f64.atan() - 6f64.to_radians();
    // 마찰이 피삭재 기준값(`cut_mu`)에서 코팅으로 조금만 바뀌므로 배수 범위도 좁다 (코팅의 절삭력 효과 0~6 %).
    // 경사각 효과는 따로 둔다: Merchant 전단각 관계 g(β − α) 는 경사 1° 에 kc 를 약 1.9 % 바꿔, 제조사 밀링 식 kc = kc1·hm^−mc·(1 − γ0/100)
    // (Sandvik Coromant — 1 %/°) 의 두 배다. 그래서 마찰(코팅)의 몫은 소재 기본 경사에서 Merchant 로 보고, 공구 경사가 기본에서 벗어난 몫은
    // (1 − γ/100) 비로 곱한다 — 기본 경사의 공구(내장 프리셋)는 예전과 같다. 예전에는 경사 0° 인서트가 경사 10° 엔드밀보다 kc +21 %,
    // 계면 +93 K 로 나와 AISI 1018 수명이 실측보다 더 짧았다
    let x_ref = (beta - ctx.wp.rake_deg.to_radians()).clamp((-5f64).to_radians(), 50f64.to_radians());
    let rake_rule = (1.0 - ctx.tool.rake_deg.clamp(-30.0, 30.0) / 100.0) / (1.0 - ctx.wp.rake_deg.clamp(-30.0, 30.0) / 100.0);
    let factor = (g(x_ref) / g(x0) * rake_rule).clamp(0.7, 1.3);
    let phi = PI / 4.0 - x / 2.0;
    let share = (beta.sin() * phi.sin() / ((beta - alpha).cos() * (phi - alpha).cos())).clamp(0.05, 0.8);
    let kc11 = ctx.wp.kc11 * factor * ctx.calib.power;
    let kte = 0.011 * ctx.wp.kc11 * (ctx.tool.edge_radius_um / 10.0).max(0.1).sqrt() * ctx.wp.kte_factor * ctx.calib.power;
    CuttingCoeffs {
        mu,
        beta_deg: beta.to_degrees(),
        shear_angle_deg: phi.to_degrees(),
        kc11,
        mc: ctx.wp.mc,
        kr: x.tan().clamp(ctx.wp.kr_floor.clamp(0.0, 0.9), 0.9),
        kte,
        kre: ctx.wp.kre_over_kte * kte,
        friction_factor: factor,
        friction_power_share: share,
        softening: 1.0,
        sigma_flank: 0.25 * ctx.wp.kc11 * ctx.calib.power,
        h_min_mm: ctx.wp.min_chip_ratio * ctx.tool.edge_radius_um / 1000.0,
        work_hardening: ctx.wp.work_hardening,
        hardened_depth_mm: 3.0 * ctx.tool.edge_radius_um / 1000.0,
        strain_rate: 0.0,
        rate_factor: 1.0,
        chip_lift: fr.chip_lift,
        sticking: fr.sticking,
        lubricant_access: fr.lubricant_access,
        friction_temp_c: t_c,
        kac_ratio: (0.35 * ctx.tool.helix_deg.to_radians().tan()).clamp(0.0, 0.6),
        kae: 0.3 * kte,
    }
}

fn with_strain_rate(ctx: &CutContext, mut co: CuttingCoeffs, vc: f64, h_mm: f64) -> CuttingCoeffs {
    co.strain_rate = shear_strain_rate(&co, ctx.tool.rake_deg, vc, h_mm);
    co.rate_factor = ctx.wp.jc.map(|j| j.rate(co.strain_rate)).unwrap_or(1.0);
    co
}

pub fn cutting_coeffs(ctx: &CutContext) -> CuttingCoeffs {
    let t = tribology::reference_interface_c(&ctx.wp);
    let fr = tribology::contact_friction(ctx, ctx.wp.ref_vc, t);
    with_strain_rate(ctx, coeffs_from_friction(ctx, &fr, t), ctx.wp.ref_vc, reference_chip_mm(ctx))
}

pub fn boothroyd_partition(x: f64) -> f64 {
    let lg = x.max(1e-6).log10();
    let b = if x <= 10.0 { 0.5 - 0.35 * lg } else { 0.3 - 0.15 * lg };
    b.clamp(0.03, 0.6)
}

/// 1차 전단 영역의 평균 온도 상승과 그 열의 공작물 쪽 몫 β (Boothroyd 분배).
///
/// 전단면 에너지는 전단 유동 응력 × 전단 변형률 τ·γ (γ = cos α / (sin φ · cos(φ − α)), Merchant·Oxley) 이고, 유동 응력은 Johnson-Cook 식을
/// 전단면 온도·변형률 속도에서 읽는다 — 온도가 응력을 낮추고 응력이 온도를 정하므로 고정점(이분법)으로 푼다 (Oxley 1989 의 평균 전단면 온도).
/// 예전에는 Kienzle 비절삭에너지 kc1.1·h^−mc 에서 2차 영역 몫만 빼서 썼다. 이 값에는 칩이 얇을수록 커지는 날끝 밀림 몫(크기효과)이 들어 있어
/// 58 HRC 볼 정삭(평균 칩 7 µm)의 전단면이 녹는점 가까이(1,400 °C)로 계산되었고, JC 열 연화가 그 근처에서 가팔라 Vc ±15 % 에 kc 가 +12 %/−9 % 로
/// 흔들렸다 — 그 결과 계면 온도가 속도와 함께 내려갔다 (Dewes 외 1999 의 H13 52 HRC 실측은 속도와 함께 오름). 계면 온도식이 같은 이유로 크기효과를
/// 자르는 것(`SIZE_EFFECT_MIN_H_MM`)과 같은 물리다: 밀림 에너지는 날끝·플랭크에서 소산되고 전단면을 데우지 않는다. JC 계수가 없는 CFRP 는 예전 식
pub fn shear_zone(ctx: &CutContext, co: &CuttingCoeffs, vc: f64, h_mm: f64) -> (f64, f64) {
    let h = h_mm.max(1e-4);
    let v = (vc / 60.0).max(1e-3);
    let tan_phi = co.shear_angle_deg.to_radians().tan().max(0.05);
    let t_in = ctx.bulk_c();
    let partition = |tm: f64| {
        let rho_c = ctx.wp.rho_c_at(tm);
        let rt = (rho_c * v * h * 1e-3 / ctx.wp.k_at(tm)).max(1e-6);
        (rho_c, boothroyd_partition(rt * tan_phi))
    };
    let Some(jc) = ctx.wp.jc else {
        let u = co.kc11 * h.powf(-co.mc) * 1.0e6;
        let eval = |tm: f64| {
            let (rho_c, beta) = partition(tm);
            ((1.0 - beta) * (1.0 - co.friction_power_share) * u / rho_c, beta)
        };
        let (first, _) = eval(t_in);
        return eval(t_in + 0.5 * first);
    };
    let phi = co.shear_angle_deg.to_radians().clamp(0.1, 1.3);
    let alpha = ctx.tool.rake_deg.to_radians();
    let gamma = (alpha.cos() / (phi.sin() * (phi - alpha).cos()).max(0.05)).clamp(0.5, 8.0);
    let eps = gamma / 3f64.sqrt();
    let rate = shear_strain_rate(co, ctx.tool.rake_deg, vc, h);
    let rise_at = |ts: f64| {
        let (rho_c, beta) = partition(ts);
        let tau = jc.flow_stress(eps, rate, ts).max(0.0) / 3f64.sqrt() * 1.0e6;
        ((1.0 - beta) * tau * gamma / rho_c, beta)
    };
    // T = t_in + 상승(T) 의 해: 상승은 T 에 대해 줄어들므로 [t_in, 녹는점] 에서 이분법
    let (mut lo, mut hi) = (t_in, jc.t_melt_c.max(t_in + 1.0));
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if t_in + rise_at(mid).0 > mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let ts = 0.5 * (lo + hi);
    (ts - t_in, rise_at(ts).1)
}

/// 기준 상태(소재 기준 속도·칩) 대비 전단 유동 응력의 비 — 전단면 온도의 열 연화 × 변형률 속도 경화 (`shear_zone`)
pub fn softening_factor(ctx: &CutContext, co: &CuttingCoeffs, vc: f64, h_mm: f64) -> f64 {
    let state = |v: f64, h: f64| {
        let (rise, _) = shear_zone(ctx, co, v, h);
        let rate = ctx.wp.jc.map(|j| j.rate(shear_strain_rate(co, ctx.tool.rake_deg, v, h))).unwrap_or(1.0);
        ctx.wp.jc_thermal(ctx.bulk_c() + rise).max(0.05) * rate
    };
    (state(vc, h_mm) / state(ctx.wp.ref_vc, reference_chip_mm(ctx)).max(1e-6)).clamp(0.75, 1.2)
}

pub fn cutting_coeffs_at_temp(ctx: &CutContext, vc: f64, h_mm: f64, t_int: f64) -> CuttingCoeffs {
    let base = cutting_coeffs(ctx);
    let s = softening_factor(ctx, &base, vc, h_mm);
    let fr = tribology::contact_friction(ctx, vc, t_int);
    let mut co = coeffs_from_friction(ctx, &fr, t_int);
    co.kc11 *= s;
    co.softening = s;
    with_strain_rate(ctx, co, vc, h_mm)
}

pub const FLANK_HEAT_GAIN: f64 = 1.0;

pub fn interface_rise(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64) -> (f64, f64) {
    interface_rise_from(ctx, co, f, vc, ctx.coolant.ambient_c())
}

/// 계면 온도식의 비절삭에너지에 쓰는 칩 두께 하한 (mm). Kienzle 의 h^−mc 크기효과를 이보다 얇은 칩으로 외삽하면 날끝 밀림 에너지까지
/// 경사면 열로 셈하게 된다 (밀림 열은 주로 플랭크·공작물로 간다). 이 하한이 없을 때 58 HRC 볼 정삭(평균 칩 7 µm)이 663~745 °C 로
/// 문헌 대역 350~600 °C (H13 52 HRC 볼 계면 200~400 °C, Dewes 외 1999) 보다 높았다
pub const SIZE_EFFECT_MIN_H_MM: f64 = 0.02;

pub fn interface_rise_from(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64, t0: f64) -> (f64, f64) {
    let h = f.h_mean_mm.max(1e-4);
    let u = co.kc11 * h.max(SIZE_EFFECT_MIN_H_MM).powf(-co.mc) * 1.0e6;
    let v = vc / 60.0;
    let rise = |tm: f64| {
        let pe = (v * h * 1e-3 / ctx.wp.diffusivity_at(tm)).max(1e-6);
        0.4 * u / ctx.wp.rho_c_at(tm) * pe.cbrt() * ctx.wp.thermal_factor * ctx.calib.temperature * (1.0 + FLANK_HEAT_GAIN * f.flank_power_share)
    };
    let first = rise(t0);
    (rise(t0 + 0.5 * first), t0)
}

pub fn estimate_interface_c(ctx: &CutContext, co: &CuttingCoeffs, vc: f64, h_mm: f64) -> f64 {
    let f = ForceResult {
        h_mean_mm: h_mm.max(1e-4),
        h_max_mm: h_mm.max(1e-4) / 0.64,
        ..Default::default()
    };
    let (raw, t0) = interface_rise(ctx, co, &f, vc);
    let dt = raw.min((0.9 * ctx.wp.melt_c - t0).max(50.0));
    let cooling = tribology::interface_cooling(ctx, co, &f, vc, dt);
    t0 + dt * (1.0 - cooling.effectiveness)
}

pub fn cutting_coeffs_at(ctx: &CutContext, vc: f64, h_mm: f64) -> CuttingCoeffs {
    let base = cutting_coeffs(ctx);
    let s = softening_factor(ctx, &base, vc, h_mm);
    let mut probe = base;
    probe.kc11 *= s;
    let t = estimate_interface_c(ctx, &probe, vc, h_mm);
    cutting_coeffs_at_temp(ctx, vc, h_mm, t)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForceResult {
    pub fx_mean: f64,
    pub fy_mean: f64,
    pub f_res_mean: f64,
    pub f_res_peak: f64,
    pub f_normal_peak: f64,
    pub torque_mean_nm: f64,
    pub torque_peak_nm: f64,
    pub cutting_power_kw: f64,
    pub h_max_mm: f64,
    pub h_mean_mm: f64,
    pub teeth_in_cut: f64,
    pub radial_load_n_per_mm: f64,
    pub fx_series: Vec<f64>,
    pub fy_series: Vec<f64>,
    pub pitch_rad: f64,
    #[serde(default)]
    pub ploughing_share: f64,
    #[serde(default)]
    pub rubbing_share: f64,
    #[serde(default)]
    pub flank_share: f64,
    #[serde(default)]
    pub flank_power_share: f64,
    /// 축방향(공구를 뽑아 올리는) 힘의 평균·피크 (N) — 헬릭스 경사 절삭 항 (`kac_ratio`, `kae`)
    #[serde(default)]
    pub fa_mean: f64,
    #[serde(default)]
    pub fa_peak: f64,
}

/// 슬라이스 하나의 축방향 힘 `dfa` (N): 전단 접선력에 비례하는 항 + 날 길이당 날끝 항.
#[inline]
pub fn slice_axial(co: &CuttingCoeffs, h: f64, db: f64) -> f64 {
    let wh = 1.0 + co.work_hardening * co.hardened_depth_mm / (h + co.hardened_depth_mm).max(1e-9);
    let ramp = if co.h_min_mm > 0.0 {
        let q = ((h - 0.5 * co.h_min_mm) / (0.5 * co.h_min_mm)).clamp(0.0, 1.0);
        q * q * (3.0 - 2.0 * q)
    } else {
        1.0
    };
    co.kac_ratio * co.kc11 * wh * h.powf(1.0 - co.mc) * db * ramp + co.kae * db
}

/// 슬라이스 하나(칩 두께 `h` mm, 폭 `db` mm)의 절삭 방향 힘 `dft` 와 반경 방향 힘 `dfr` (N).
/// 비절삭저항(Kienzle) × 가공경화 × 날끝 최소 칩두께 램프에 날끝 밀림(kte, kre) 과 플랭크 마모 접촉 항
/// (`flank_t`, `flank_r`)을 더한다. `mechanistic_forces` 가 쓰는 식을 그대로 꺼낸 것이며,
/// 물리 교차검증(`physcal`)이 같은 식으로 직교절삭의 힘을 비교하도록 공개한다.
#[inline]
pub fn slice_forces(co: &CuttingCoeffs, h: f64, db: f64, flank_t: f64, flank_r: f64) -> (f64, f64) {
    let wh = 1.0 + co.work_hardening * co.hardened_depth_mm / (h + co.hardened_depth_mm).max(1e-9);
    let ramp = if co.h_min_mm > 0.0 {
        let q = ((h - 0.5 * co.h_min_mm) / (0.5 * co.h_min_mm)).clamp(0.0, 1.0);
        q * q * (3.0 - 2.0 * q)
    } else {
        1.0
    };
    let shear = co.kc11 * wh * h.powf(1.0 - co.mc) * db * ramp;
    let dft = shear + co.kte * db + flank_t;
    let dfr = co.kr * shear + co.kre * db + flank_r;
    (dft, dfr)
}

pub fn mechanistic_forces(tool: &ToolGeometry, co: &CuttingCoeffs, fz: f64, rpm: f64, eng: &Engagement, steps: usize, slices: usize) -> ForceResult {
    let mut out = ForceResult::default();
    if eng.ap_mm <= 0.0 || eng.span() <= 0.0 || fz <= 0.0 {
        return out;
    }
    let z = tool.flutes.max(1) as usize;
    let pitch = 2.0 * PI / z as f64;
    let r = tool.radius();
    let tan_h = tool.helix_deg.to_radians().tan();
    let dz = eng.ap_mm / slices as f64;
    let runout_mm = tool.runout_ecc_mm();
    let mut fx_s = Vec::with_capacity(steps);
    let mut fy_s = Vec::with_capacity(steps);
    let mut t_sum = 0.0;
    let mut t_peak = 0.0f64;
    let mut fres_peak = 0.0f64;
    let mut fn_peak = 0.0f64;
    let mut h_sum = 0.0;
    let mut h_cnt = 0.0;
    let mut h_max = 0.0f64;
    let mut teeth_sum = 0.0;
    let mut fr_sum = 0.0;
    let mut len_sum = 0.0;
    let mu_flank = (co.mu * 0.6).clamp(0.05, 0.6);
    let mut plough_len = 0.0;
    let mut rub_t = 0.0;
    let mut flank_t = 0.0;
    let mut flank_r = 0.0;
    let mut tot_t = 0.0;
    let offsets: Vec<f64> = (0..z)
        .map(|j| {
            if z > 1 {
                let cur = (2.0 * PI * j as f64 / z as f64).cos();
                let prev = (2.0 * PI * ((j + z - 1) % z) as f64 / z as f64).cos();
                runout_mm * (cur - prev)
            } else {
                0.0
            }
        })
        .collect();
    let turns = if z > 1 && runout_mm.abs() > 1e-12 { z } else { 1 };
    let wt = 1.0 / turns as f64;
    let mut fx_c = vec![0.0; turns];
    let mut fy_c = vec![0.0; turns];
    let mut tq_c = vec![0.0; turns];
    let mut fa_c = vec![0.0; turns];
    let mut fa_sum = 0.0;
    let mut fa_peak = 0.0f64;
    let mut teeth = vec![false; turns * z];
    let mut fres_sum = 0.0;
    for s in 0..steps {
        let theta = pitch * s as f64 / steps as f64;
        fx_c.iter_mut().for_each(|v| *v = 0.0);
        fy_c.iter_mut().for_each(|v| *v = 0.0);
        tq_c.iter_mut().for_each(|v| *v = 0.0);
        fa_c.iter_mut().for_each(|v| *v = 0.0);
        teeth.iter_mut().for_each(|t| *t = false);
        for j in 0..z {
            for k in 0..slices {
                let zk = (k as f64 + 0.5) * dz;
                let (rl, sin_k) = tool.local_radius(zk);
                let lag = zk * tan_h / r;
                let mut phi = theta + j as f64 * pitch - lag;
                phi = phi.rem_euclid(2.0 * PI);
                if phi < eng.phi_st || phi > eng.phi_ex {
                    continue;
                }
                let (sp, cp) = phi.sin_cos();
                let db = dz / sin_k;
                for c in 0..turns {
                    let h = ((fz * sp + offsets[(j + z - c) % z]) * sin_k).max(0.0);
                    if h <= 1e-7 {
                        continue;
                    }
                    teeth[c * z + j] = true;
                    let dfr_w = co.sigma_flank * tool.flank_wear_mm * db;
                    let dft_w = mu_flank * dfr_w;
                    let (dft, dfr) = slice_forces(co, h, db, dft_w, dfr_w);
                    if h < co.h_min_mm {
                        plough_len += db * wt;
                    }
                    rub_t += (co.kte * db + dft_w) * wt;
                    flank_t += dft_w * wt;
                    flank_r += dfr_w * wt;
                    tot_t += dft * wt;
                    fx_c[c] += -dft * cp - dfr * sp;
                    fy_c[c] += dft * sp - dfr * cp;
                    tq_c[c] += dft * rl;
                    fa_c[c] += slice_axial(co, h, db);
                    h_sum += h;
                    h_cnt += 1.0;
                    h_max = h_max.max(h);
                    fr_sum += dfr * wt;
                    len_sum += db * wt;
                }
            }
        }
        let mut fx = 0.0;
        let mut fy = 0.0;
        let mut torque = 0.0;
        let mut fa = 0.0;
        for c in 0..turns {
            let fres = (fx_c[c] * fx_c[c] + fy_c[c] * fy_c[c]).sqrt();
            fres_peak = fres_peak.max(fres);
            fn_peak = fn_peak.max(fy_c[c].abs());
            t_peak = t_peak.max(tq_c[c]);
            fa_peak = fa_peak.max(fa_c[c]);
            fres_sum += fres * wt;
            fx += fx_c[c] * wt;
            fy += fy_c[c] * wt;
            torque += tq_c[c] * wt;
            fa += fa_c[c] * wt;
        }
        teeth_sum += teeth.iter().filter(|t| **t).count() as f64 * wt;
        t_sum += torque;
        fa_sum += fa;
        fx_s.push(fx);
        fy_s.push(fy);
    }
    let n = steps as f64;
    out.fx_mean = fx_s.iter().sum::<f64>() / n;
    out.fy_mean = fy_s.iter().sum::<f64>() / n;
    out.f_res_mean = fres_sum / n;
    out.f_res_peak = fres_peak;
    out.f_normal_peak = fn_peak;
    out.torque_mean_nm = t_sum / n / 1000.0;
    out.torque_peak_nm = t_peak / 1000.0;
    out.cutting_power_kw = out.torque_mean_nm * 2.0 * PI * rpm / 60.0 / 1000.0;
    out.h_max_mm = h_max;
    out.h_mean_mm = if h_cnt > 0.0 { h_sum / h_cnt } else { 0.0 };
    out.teeth_in_cut = teeth_sum / n;
    out.radial_load_n_per_mm = if len_sum > 0.0 { fr_sum / len_sum } else { 0.0 };
    out.fx_series = fx_s;
    out.fy_series = fy_s;
    out.pitch_rad = pitch;
    out.ploughing_share = if len_sum > 0.0 { plough_len / len_sum } else { 0.0 };
    out.rubbing_share = if tot_t > 0.0 { rub_t / tot_t } else { 0.0 };
    out.flank_share = if fr_sum > 0.0 { flank_r / fr_sum } else { 0.0 };
    out.flank_power_share = if tot_t > 0.0 { flank_t / tot_t } else { 0.0 };
    out.fa_mean = fa_sum / n;
    out.fa_peak = fa_peak;
    out
}

pub fn plunge_forces(tool: &ToolGeometry, co: &CuttingCoeffs, fz_axial: f64, rpm: f64) -> ForceResult {
    let mut out = ForceResult::default();
    if fz_axial <= 0.0 {
        return out;
    }
    let r = tool.radius();
    let z = tool.flutes.max(1) as f64;
    let ft_edge = co.kc11 * fz_axial.powf(1.0 - co.mc) * r + co.kte * r;
    let torque = z * ft_edge * r / 2.0 / 1000.0;
    out.torque_mean_nm = torque;
    out.torque_peak_nm = torque * 1.15;
    out.cutting_power_kw = torque * 2.0 * PI * rpm / 60.0 / 1000.0;
    let flank = co.sigma_flank * tool.flank_wear_mm * r;
    out.f_res_mean = z * (co.kr * ft_edge + flank) * 0.15;
    out.f_res_peak = out.f_res_mean * 1.5;
    out.rubbing_share = (co.kte * r / ft_edge.max(1e-9)).clamp(0.0, 1.0);
    out.h_max_mm = fz_axial;
    out.h_mean_mm = fz_axial;
    out.teeth_in_cut = z;
    out.radial_load_n_per_mm = co.kr * ft_edge / r.max(1e-6);
    out.pitch_rad = 2.0 * PI / z;
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compliance {
    pub stiffness_n_per_um: f64,
    pub load_point_from_holder_mm: f64,
    pub ei_equivalent: f64,
}

pub fn tool_sections(tool: &ToolGeometry) -> [(f64, f64, f64); 3] {
    let d_f = tool.core_ratio * tool.diameter_mm;
    let i_f = PI * d_f.powi(4) / 64.0;
    let i_s = PI * tool.shank_mm.max(d_f).powi(4) / 64.0;
    let l = tool.stickout_mm.max(tool.loc_mm.min(tool.stickout_mm));
    let lf = tool.loc_mm.min(l);
    let (ln, i_n) = if tool.has_neck() {
        let dn = tool.neck_diameter_mm.max(0.5 * d_f);
        ((tool.reach_mm.min(l) - lf).max(0.0), PI * dn.powi(4) / 64.0)
    } else {
        (0.0, i_s)
    };
    let ls = (l - lf - ln).max(0.0);
    [(0.0, ls, i_s), (ls, ls + ln, i_n), (ls + ln, l, i_f)]
}

/// 공구 끝 정적 컴플라이언스 (보 + 홀더 + 스핀들) — 휨·벽 오차용
pub fn tool_compliance(tool: &ToolGeometry, ap: f64, cal: f64) -> Compliance {
    compliance_with(tool, ap, cal, true)
}

/// 공구·홀더 모드만의 컴플라이언스 — 동역학에서 스핀들 모드를 따로 더할 때 쓴다
pub fn tool_compliance_lumped(tool: &ToolGeometry, ap: f64, cal: f64) -> Compliance {
    compliance_with(tool, ap, cal, false)
}

fn compliance_with(tool: &ToolGeometry, ap: f64, cal: f64, spindle: bool) -> Compliance {
    let e = tool.elastic_gpa * 1000.0;
    let l = tool.stickout_mm.max(tool.loc_mm.min(tool.stickout_mm));
    let a = (l - ap.max(0.0).min(l) / 2.0).max(1e-3);
    let mut delta_per_n = 0.0;
    for (x0, x1, i) in tool_sections(tool) {
        if x0 >= a {
            break;
        }
        let x1c = x1.min(a);
        if x1c <= x0 {
            continue;
        }
        delta_per_n += ((a - x0).powi(3) - (a - x1c).powi(3)) / (3.0 * e * i);
    }
    let holder = if spindle { tool.support_compliance_mm_per_n() } else { 1.0 / (tool.holder_stiffness_n_per_um * 1000.0) };
    let total = (delta_per_n + holder) * cal;
    Compliance {
        stiffness_n_per_um: 1.0 / (total * 1000.0),
        load_point_from_holder_mm: a,
        ei_equivalent: a.powi(3) / (3.0 * delta_per_n.max(1e-18)),
    }
}

pub fn deflection_at(comp: &Compliance, force_n: f64, x_from_holder: f64, holder_part: f64) -> f64 {
    let a = comp.load_point_from_holder_mm;
    let ei = comp.ei_equivalent;
    let beam = if x_from_holder >= a {
        force_n * a * a * (3.0 * x_from_holder - a) / (6.0 * ei)
    } else {
        force_n * x_from_holder * x_from_holder * (3.0 * a - x_from_holder) / (6.0 * ei)
    };
    (beam + force_n * holder_part) * 1000.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalResult {
    pub interface_dry_c: f64,
    pub interface_c: f64,
    pub coolant_reduction_pct: f64,
    pub tool_body_c: f64,
    pub tool_radial_growth_um: f64,
    pub tool_axial_growth_um: f64,
    pub engagement_time_ms: f64,
    /// 계면 온도 상승에 곱한 단속 절삭 열 축적 배수 (`interrupted_factor`, 0.3~1.5) — 피삭재 기준 물림에서 1, 짧게 물리면 작고 슬롯처럼 오래 물리면 크다.
    /// 이름은 예전 과도 계수(`tribology::transient_factor`, ≤ 1)에서 이어 왔다
    pub transient_factor: f64,
    pub workpiece_heat_w: f64,
    pub workpiece_steady_rise_c: f64,
    pub workpiece_time_constant_s: f64,
    pub thermal_crack_risk: f64,
    pub bue_risk: f64,
    pub over_coating_limit: bool,
    pub over_workpiece_limit: bool,
    #[serde(default)]
    pub heat_to_workpiece_share: f64,
    #[serde(default)]
    pub cooling: Cooling,
    #[serde(default)]
    pub shock: ThermalShock,
    #[serde(default)]
    pub barrier: tribology::Barrier,
    #[serde(default)]
    pub lubricant_access: f64,
}

/// 날 하나가 한 순간 칩과 닿는 날 길이 (mm). 헬릭스 날은 물림각 φ_e 동안 축 방향으로 R·φ_e/tan β 만큼만 동시에 물리므로
/// 깊은 축 절입에서도 날 하나의 접촉 길이는 그보다 길어지지 않는다 (헬릭스 0° 인서트는 ap 전체)
pub fn engaged_edge_length_mm(tool: &ToolGeometry, eng: &Engagement) -> f64 {
    let ap = eng.ap_mm.max(0.0);
    let beta = tool.helix_deg.clamp(0.0, 70.0).to_radians();
    let axial = if beta.tan() > 1e-3 { ap.min(tool.radius() * eng.span() / beta.tan()) } else { ap };
    axial / beta.cos().max(0.3)
}

/// 단속 절삭 열 축적 배수 (`tribology::interrupted_heating`) — 이 물림의 값을 피삭재 기준 물림(ae = ae_ratio·D, ap = ap_ratio·D,
/// 같은 공구·회전수·접촉 길이·칩 쪽 전도)의 값으로 나눈 비. 피삭재 표의 열 계수(`thermal_factor`)와 마모 기준 상태는 기준 물림에서 맞춘 값이라
/// 그 물림에서 1 이 되게 두고, 물림 시간·날 접촉 길이의 차이만 더한다: 반경 절입이 작아 짧게 물리면 공구가 덜 데워져 차갑고,
/// 슬롯처럼 오래 물리면 뜨겁다. 예전 `transient_factor` 는 접촉 길이 규모(수십 µs)의 데워짐만 봐 밀링의 ms 물림을 모두 정상으로 보았다
pub fn interrupted_factor(ctx: &CutContext, eng: &Engagement, rpm: f64, contact_length_mm: f64, chip_conductance: f64) -> f64 {
    if !(rpm > 0.0) || eng.span() <= 0.0 {
        return 1.0;
    }
    let tool = &ctx.tool;
    let period = 60.0 / rpm;
    let heating = |e: &Engagement| {
        tribology::interrupted_heating(
            contact_length_mm,
            engaged_edge_length_mm(tool, e),
            chip_conductance,
            tool.tool_conductivity,
            tool.tool_diffusivity(),
            e.span() / (2.0 * PI) * period,
            period,
        )
    };
    let d = tool.diameter_mm;
    let reference = Engagement::side((ctx.wp.ap_ratio * d).min(tool.loc_mm.max(1e-3)), ctx.wp.ae_ratio * d, d, true);
    (heating(eng) / heating(&reference).max(1e-15)).clamp(0.3, 1.5)
}

#[allow(clippy::too_many_arguments)]
pub fn thermal(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64, rpm: f64, eng: &Engagement, wp_mass_kg: f64, wp_area_m2: f64) -> ThermalResult {
    thermal_preheated(ctx, co, f, vc, rpm, eng, wp_mass_kg, wp_area_m2, 0.0)
}

#[allow(clippy::too_many_arguments)]
pub fn thermal_preheated(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64, rpm: f64, eng: &Engagement, wp_mass_kg: f64, wp_area_m2: f64, preheat_c: f64) -> ThermalResult {
    let h = f.h_mean_mm.max(1e-4);
    let base = ctx.coolant.ambient_c();
    let pre = preheat_c.max(0.0).min(0.5 * (ctx.wp.melt_c - base).max(0.0));
    let soft = if pre > 0.0 { (ctx.wp.jc_thermal(base + pre) / ctx.wp.jc_thermal(base).max(1e-6)).clamp(0.3, 1.0) } else { 1.0 };
    let (raw0, t0) = interface_rise_from(ctx, co, f, vc, base + pre);
    let raw = raw0 * soft;
    let cap = (0.9 * ctx.wp.melt_c - t0).max(50.0);
    let t_cut = if rpm > 0.0 { eng.span() / (2.0 * PI) * 60.0 / rpm } else { 0.0 };
    let steady = tribology::interface_cooling(ctx, co, f, vc, raw.min(cap));
    let tf = interrupted_factor(ctx, eng, rpm, steady.contact_length_mm, steady.conduction);
    let cooling = tribology::interface_cooling(ctx, co, f, vc, raw.min(cap) * tf);
    let contact_s = if t_cut > 0.0 { t_cut } else { 1.0 };
    let barrier = tribology::coating_barrier(ctx, cooling.contact_length_mm, cooling.conduction, contact_s);
    let reference = tribology::reference_barrier(ctx, cooling.contact_length_mm, cooling.conduction, contact_s);
    let dt = (raw * barrier.interface).min(cap);
    let solidus = 0.95 * ctx.wp.melt_c;
    let dry = (t0 + dt * tf).min(solidus);
    let wet = (t0 + dt * tf * (1.0 - cooling.effectiveness)).min(solidus);
    let body = base + 0.08 * (wet - base) * barrier.tool_heat * (1.0 - cooling.body_effectiveness);
    let (_, beta) = shear_zone(ctx, co, vc, h);
    let r_wp = ((1.0 - f.rubbing_share) * (1.0 - co.friction_power_share) * beta + f.rubbing_share * 0.5).clamp(0.02, 0.7);
    let p_wp = f.cutting_power_kw * 1000.0 * r_wp;
    let ha = ctx.coolant.workpiece_h() * wp_area_m2.max(1e-4);
    let mc = wp_mass_kg.max(1e-3) * ctx.wp.specific_heat;
    let interrupted = eng.span() < 1.9 * PI;
    let shock = tribology::thermal_shock(ctx, wet, cooling.h_eff, barrier.tool_heat / reference.tool_heat.max(1e-6), interrupted);
    let bue = tribology::bue_risk(ctx, wet, co.lubricant_access);
    ThermalResult {
        interface_dry_c: dry,
        interface_c: wet,
        coolant_reduction_pct: if dry - t0 > 0.0 { (dry - wet) / (dry - t0) * 100.0 } else { 0.0 },
        tool_body_c: body,
        tool_radial_growth_um: ctx.tool.tool_expansion * ctx.tool.radius() * (body - ctx.env.ambient_c) * 1000.0,
        tool_axial_growth_um: ctx.tool.tool_expansion * ctx.tool.stickout_mm * (body - ctx.env.ambient_c) * 0.5 * 1000.0,
        engagement_time_ms: t_cut * 1000.0,
        transient_factor: tf,
        workpiece_heat_w: p_wp,
        workpiece_steady_rise_c: p_wp / ha,
        workpiece_time_constant_s: mc / ha,
        thermal_crack_risk: shock.risk,
        bue_risk: bue,
        over_coating_limit: wet > 0.85 * ctx.tool.coating.max_temp_c,
        over_workpiece_limit: crate::environment::workpiece_limit_c(ctx).map(|l| wet > l).unwrap_or(false),
        heat_to_workpiece_share: r_wp,
        cooling,
        shock,
        barrier,
        lubricant_access: co.lubricant_access,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WearResult {
    pub vb_rate_mm_per_min: f64,
    pub tool_life_min: f64,
    pub radial_loss_rate_um_per_min: f64,
    pub vb_limit_mm: f64,
    pub temperature_factor: f64,
    pub stress_factor: f64,
    pub speed_factor: f64,
    pub coating_factor: f64,
    pub runout_factor: f64,
    #[serde(default)]
    pub linear_life_min: f64,
    #[serde(default = "one")]
    pub mechanism_factor: f64,
    #[serde(default)]
    pub mechanisms: Vec<WearMechanism>,
    #[serde(default)]
    pub dominant: String,
    #[serde(default = "one")]
    pub chatter_factor: f64,
    #[serde(default)]
    pub coating_exposure: f64,
    #[serde(default = "one")]
    pub coating_blend: f64,
    #[serde(default = "one")]
    pub evacuation_factor: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WearReference {
    pub t_c: f64,
    pub stress: f64,
    pub vc: f64,
    pub duty: f64,
    pub bue: f64,
    pub access: f64,
    pub raw: [f64; 4],
    pub shares: [f64; 4],
}

fn reference_setup(ctx: &CutContext) -> (CutContext, f64, Engagement, f64) {
    let mut rctx = ctx.clone();
    rctx.env = ShopEnvironment::standard();
    rctx.coolant = CoolantState::from_config_env(&rctx.wp.reference_coolant(), &rctx.env);
    rctx.calib = Calibration::default();
    let wp = rctx.wp.clone();
    rctx.tool.reset_to_reference_substrate(&wp);
    rctx.tool.coating = tribology_from_name(Some("AlTiN"));
    rctx.tool.runout_um = 3.0;
    rctx.tool.base_wear_factor = 1.0;
    rctx.tool.diameter_mm = 10.0;
    rctx.tool.loc_mm = 25.0;
    rctx.tool.stickout_mm = 35.0;
    rctx.tool.flutes = 4;
    rctx.tool.nose = ToolNose::Square;
    rctx.tool.flank_wear_mm = 0.0;
    let d = 10.0;
    let fz = (rctx.wp.fz_coeff * d).clamp(0.003, 0.25);
    let ae = rctx.wp.ae_ratio * d;
    let eng = Engagement::side(rctx.wp.ap_ratio * d, ae, d, true);
    let rpm = rctx.wp.ref_vc * 1000.0 / (PI * d);
    (rctx, fz, eng, rpm)
}

pub fn reference_state(ctx: &CutContext) -> WearReference {
    let (rctx, fz, eng, rpm) = reference_setup(ctx);
    let co = cutting_coeffs(&rctx);
    let f = mechanistic_forces(&rctx.tool, &co, fz, rpm, &eng, 48, 8);
    let th = thermal(&rctx, &co, &f, rctx.wp.ref_vc, rpm, &eng, 1.0, 0.03);
    let raw = tribology::mechanism_raw(&rctx, th.interface_c, 1.0, 1.0, 1.0, th.bue_risk, co.lubricant_access);
    WearReference {
        t_c: th.interface_c,
        stress: f.radial_load_n_per_mm.max(1e-6),
        vc: rctx.wp.ref_vc,
        duty: eng.span() / (2.0 * PI),
        bue: th.bue_risk,
        access: co.lubricant_access,
        raw: raw.map(|v| v.max(1e-12)),
        shares: tribology::normalized_shares(&rctx.wp.mech_shares),
    }
}

pub fn substrate_softening(ctx: &CutContext, t_c: f64) -> f64 {
    if ctx.tool.substrate_max_c >= 800.0 {
        return 1.0;
    }
    let over = t_c - 0.9 * ctx.tool.substrate_max_c;
    if over <= 0.0 {
        1.0
    } else {
        (over / 60.0).exp().min(25.0)
    }
}

/// 플랭크 마모 랜드의 기하 배수. 마모 랜드는 절삭 방향과 나란한 면이라 마모식(Usui 형 dW/dt = C·σn·Vs·exp(−λ/θ))이 주는 것은 그 면에 수직인
/// 마모 깊이, 곧 날끝의 반경 후퇴 KE 의 속도다. VB 는 그 투영 VB = KE / radial_per_vb (Kronenberg) 이므로 같은 마찰 조건에서 VB 성장 속도는
/// radial_per_vb 에 반비례한다 — 여유각 15° 공구는 8° 공구보다 VB 가 약 절반 속도로 자라고, 치수 손실(반경 후퇴) 속도는 같다.
/// 마모율의 앵커는 소재 기본 여유·경사의 기준 공구(`reference_setup`)라 그 비로 곱한다. 예전에는 여유각이 VB 속도에 들어가지 않고,
/// 용도 적합도 창 밖일 때만 마모 배수로 들어갔다 (`appfit` — 큰 여유각은 쐐기각으로 판정하도록 같이 바꿈)
pub fn flank_geometry_factor(ctx: &CutContext) -> f64 {
    let reference = radial_per_vb(ctx.wp.clearance_deg, ctx.wp.rake_deg);
    (reference / ctx.tool.radial_per_vb().max(1e-6)).clamp(0.3, 3.0)
}

fn bare_coating() -> &'static CoatingTribology {
    static BARE: std::sync::OnceLock<CoatingTribology> = std::sync::OnceLock::new();
    BARE.get_or_init(|| tribology_from_name(Some("uncoated")))
}

pub fn chip_confinement(eng: &Engagement) -> f64 {
    if eng.mode == MillMode::Slot {
        return 1.0;
    }
    tribology::smoothstep((eng.span() - 0.5 * PI) / (0.5 * PI))
}

pub fn wear(ctx: &CutContext, f: &ForceResult, th: &ThermalResult, vc: f64, fz: f64, eng: &Engagement, reference: WearReference) -> WearResult {
    wear_at(ctx, f, th, vc, fz, eng, reference, ctx.tool.flank_wear_mm)
}

#[allow(clippy::too_many_arguments)]
pub fn wear_at(ctx: &CutContext, f: &ForceResult, th: &ThermalResult, vc: f64, fz: f64, eng: &Engagement, reference: WearReference, vb_mm: f64) -> WearResult {
    let sf = (f.radial_load_n_per_mm * (1.0 - f.flank_share) / reference.stress).clamp(0.05, 20.0);
    let duty = eng.span() / (2.0 * PI);
    let duty_rel = duty / reference.duty.max(1e-6);
    let vf = (vc / reference.vc) * duty_rel;
    let raw = tribology::mechanism_raw(ctx, th.interface_c, sf, vf, duty_rel, th.bue_risk, th.lubricant_access);
    let mut parts: [f64; 4] = [0, 1, 2, 3].map(|i| reference.shares[i] * raw[i] / reference.raw[i]);
    let mech_coated: f64 = parts.iter().sum();
    let exposure = crate::toolwear::coating_exposure(&ctx.tool, vb_mm);
    let mut blend = 1.0;
    if exposure > 0.0 && crate::toolwear::progressive_coating(&ctx.tool) {
        let raw_b = tribology::mechanism_raw_coat(ctx, bare_coating(), th.interface_c, sf, vf, duty_rel, th.bue_risk, th.lubricant_access);
        let mech_bare: f64 = (0..4).map(|i| reference.shares[i] * raw_b[i] / reference.raw[i]).sum();
        let eff = crate::toolwear::exposed_wear_rate(mech_coated, mech_bare, exposure);
        if mech_coated > 1e-12 {
            blend = eff / mech_coated;
            parts = parts.map(|p| p * blend);
        }
    }
    let mech: f64 = parts.iter().sum();
    let at_ref = tribology::mechanism_raw(ctx, reference.t_c, 1.0, 1.0, 1.0, reference.bue, reference.access);
    let cf: f64 = (0..4).map(|i| reference.shares[i] * at_ref[i] / reference.raw[i]).sum();
    let thermal_share = (reference.shares[2] + reference.shares[3]).max(1e-9);
    let tf = (parts[2] + parts[3]) / thermal_share;
    let rf = (1.0 + 2.0 * ctx.tool.runout_ecc_mm() / fz.max(1e-4)).powf(1.0 - ctx.wp.mc).min(2.0);
    let evac = 1.0 + 0.3 * (1.0 - ctx.coolant.chip_evacuation()) * (0.3 + 0.7 * chip_confinement(eng));
    let base_rate = 0.2 / ctx.wp.ref_life_min.max(1.0);
    let rate = base_rate * mech * rf * ctx.tool.base_wear_factor * ctx.tool.fit_wear.max(1.0) * evac * ctx.calib.wear * (1.0 + 1.5 * th.thermal_crack_risk) * substrate_softening(ctx, th.interface_c) * flank_geometry_factor(ctx);
    let limit = ctx.vb_limit_mm();
    let (mechanisms, dominant) = tribology::mechanism_report(&reference.shares, &parts);
    WearResult {
        vb_rate_mm_per_min: rate,
        tool_life_min: if rate > 0.0 { limit / rate } else { f64::INFINITY },
        radial_loss_rate_um_per_min: rate * ctx.tool.radial_per_vb() * 1000.0,
        vb_limit_mm: limit,
        temperature_factor: tf,
        stress_factor: sf,
        speed_factor: vf,
        coating_factor: cf,
        runout_factor: rf,
        linear_life_min: if rate > 0.0 { limit / rate } else { f64::INFINITY },
        mechanism_factor: mech,
        mechanisms,
        dominant,
        chatter_factor: 1.0,
        coating_exposure: exposure,
        coating_blend: blend,
        evacuation_factor: evac,
    }
}

struct RawTrajectory {
    s: Vec<f64>,
    vb: Vec<f64>,
    rate: Vec<f64>,
    force: Vec<f64>,
    temp: Vec<f64>,
    life_s: f64,
    linear_s: f64,
    break_in_vb: f64,
    tau_s: f64,
    phase3_s: Option<f64>,
}

#[allow(clippy::too_many_arguments)]
fn trajectory_raw(ctx: &CutContext, co: &CuttingCoeffs, fz: f64, rpm: f64, eng: &Engagement, vce: f64, body: ThermalBody, reference: WearReference, kappa: f64) -> RawTrajectory {
    let limit = ctx.vb_limit_mm();
    let mut c = ctx.clone();
    c.tool.flank_wear_mm = 0.0;
    let eval = |c: &CutContext| {
        let f = mechanistic_forces(&c.tool, co, fz, rpm, eng, 36, 8);
        let th = thermal(c, co, &f, vce, rpm, eng, body.mass_kg, body.area_m2);
        let w = wear(c, &f, &th, vce, fz, eng, reference);
        (w.vb_rate_mm_per_min, f.f_res_mean, th.interface_c)
    };
    let (r0, f0, t0) = eval(&c);
    let r0 = r0.max(1e-12);
    let linear = limit / r0;
    // 길들이기: 조합 기준값(크기 µm, 시상수 분) — 예전의 '한계의 15 %, 선형 수명의 3 %' 는 수명이 긴 조건에서 10~20분짜리
    // 길들이기를 만들었다. 이 궤적의 시간은 나중에 κ 로 나누어 실제 시간이 되므로 시상수에 κ 를 미리 곱한다
    let b = crate::pairref::breakin(ctx, vce);
    let vb_b = b.vb_mm;
    let tau = (b.tau_min * kappa.max(1e-6)).max(1e-9);
    let ds = (linear / 150.0).max(1e-9);
    let mut out = RawTrajectory {
        s: Vec::new(),
        vb: Vec::new(),
        rate: Vec::new(),
        force: Vec::new(),
        temp: Vec::new(),
        life_s: f64::INFINITY,
        linear_s: linear,
        break_in_vb: vb_b,
        tau_s: tau,
        phase3_s: None,
    };
    let mut s = 0.0;
    let mut vb = 0.0f64;
    let mut min_rate = f64::INFINITY;
    let mut life: Option<f64> = None;
    // 3차(가속) 마모: 마모 랜드 → 발열 → 마모 되먹임 가운데 힘·온도 식에 없는 몫 (날끝 둥글어짐·코팅 상실·미세 결손)을
    // 피삭재 그룹별 가속 시작 VB 위에서 곱한다 — 경로 시뮬레이션(`loadsim`)과 같은 식 (`toollife::accel_factor`)
    let eol = crate::toollife::eol_params(crate::pairref::WpGroup::of(&ctx.wp));
    for _ in 0..1500 {
        c.tool.flank_wear_mm = vb;
        let (steady, force, temp) = if vb <= 0.0 { (r0, f0, t0) } else { eval(&c) };
        let steady = steady * crate::toollife::accel_factor(&eol, vb);
        let rate = steady + vb_b / tau * (-s / tau).exp();
        out.s.push(s);
        out.vb.push(vb);
        out.rate.push(rate);
        out.force.push(force);
        out.temp.push(temp);
        if s > 3.0 * tau {
            min_rate = min_rate.min(steady);
            if out.phase3_s.is_none() && steady > 1.5 * min_rate {
                out.phase3_s = Some(s);
            }
        }
        let next = vb + rate * ds;
        if life.is_none() && next >= limit {
            life = Some(s + (limit - vb) / rate.max(1e-12));
        }
        vb = next;
        s += ds;
        if vb >= 1.25 * limit || s > 4.0 * linear {
            c.tool.flank_wear_mm = vb;
            let (steady, force, temp) = eval(&c);
            out.s.push(s);
            out.vb.push(vb);
            out.rate.push(steady * crate::toollife::accel_factor(&eol, vb));
            out.force.push(force);
            out.temp.push(temp);
            break;
        }
    }
    out.life_s = life.unwrap_or_else(|| {
        let n = out.s.len() - 1;
        out.s[n] + (limit - out.vb[n]).max(0.0) / out.rate[n].max(1e-12)
    });
    out
}

pub fn wear_kappa(ctx: &CutContext) -> f64 {
    let (rctx, fz, eng, rpm) = reference_setup(ctx);
    let co = cutting_coeffs(&rctx);
    let reference = reference_state(ctx);
    let body = ThermalBody {
        mass_kg: 1.0,
        area_m2: 0.03,
        size_mm: 100.0,
    };
    let raw = trajectory_raw(&rctx, &co, fz, rpm, &eng, rctx.wp.ref_vc, body, reference, 1.0);
    (raw.life_s / raw.linear_s.max(1e-12)).clamp(0.2, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WearTrajectory {
    pub t_min: Vec<f64>,
    pub vb_mm: Vec<f64>,
    pub rate_mm_per_min: Vec<f64>,
    pub force_n: Vec<f64>,
    pub temp_c: Vec<f64>,
    pub life_min: f64,
    pub linear_life_min: f64,
    pub break_in_vb_mm: f64,
    pub break_in_tau_min: f64,
    pub phase3_min: Option<f64>,
    pub vb_limit_mm: f64,
    pub kappa: f64,
    pub calib_wear: f64,
    pub acceleration: f64,
}

impl WearTrajectory {
    pub fn vb_at(&self, t: f64) -> f64 {
        let n = self.t_min.len();
        if n == 0 || t <= 0.0 {
            return 0.0;
        }
        if t >= self.t_min[n - 1] {
            return self.vb_mm[n - 1] + self.rate_mm_per_min[n - 1] * (t - self.t_min[n - 1]);
        }
        let i = self.t_min.partition_point(|x| *x <= t).max(1);
        let (t0, t1) = (self.t_min[i - 1], self.t_min[i]);
        let (v0, v1) = (self.vb_mm[i - 1], self.vb_mm[i]);
        v0 + (v1 - v0) * (t - t0) / (t1 - t0).max(1e-12)
    }

    pub fn time_at(&self, vb: f64) -> f64 {
        let n = self.vb_mm.len();
        if n == 0 || vb <= 0.0 {
            return 0.0;
        }
        if vb >= self.vb_mm[n - 1] {
            return self.t_min[n - 1] + (vb - self.vb_mm[n - 1]) / self.rate_mm_per_min[n - 1].max(1e-12);
        }
        let i = self.vb_mm.partition_point(|x| *x <= vb).max(1);
        let (v0, v1) = (self.vb_mm[i - 1], self.vb_mm[i]);
        let (t0, t1) = (self.t_min[i - 1], self.t_min[i]);
        t0 + (t1 - t0) * (vb - v0) / (v1 - v0).max(1e-12)
    }

    pub fn growth_from(&self, vb0: f64, dt_min: f64) -> f64 {
        let v = vb0.max(0.0);
        self.vb_at(self.time_at(v) + dt_min.max(0.0)) - v
    }

    pub fn minutes_between(&self, vb0: f64, vb1: f64) -> f64 {
        let a = vb0.max(0.0);
        (self.time_at(vb1.max(a)) - self.time_at(a)).max(0.0)
    }

    pub fn steady_rate_at(&self, vb: f64) -> f64 {
        let n = self.t_min.len().min(self.vb_mm.len()).min(self.rate_mm_per_min.len());
        if n == 0 {
            return 0.0;
        }
        let tau = self.break_in_tau_min.max(1e-9);
        let st = |i: usize| (self.rate_mm_per_min[i] - self.break_in_vb_mm / tau * (-self.t_min[i] / tau).exp()).max(0.0);
        if vb <= self.vb_mm[0] {
            return st(0);
        }
        if vb >= self.vb_mm[n - 1] {
            return st(n - 1);
        }
        let i = self.vb_mm[..n].partition_point(|x| *x <= vb).clamp(1, n - 1);
        let (v0, v1) = (self.vb_mm[i - 1], self.vb_mm[i]);
        st(i - 1) + (st(i) - st(i - 1)) * (vb - v0) / (v1 - v0).max(1e-12)
    }

    pub fn steady_path_factor(&self, vb0: f64, vb1: f64, minutes: f64, steady_growth_mm: f64) -> Option<f64> {
        if self.t_min.is_empty() || !(minutes > 1e-6) || !(steady_growth_mm > 1e-9) {
            return None;
        }
        let a = vb0.max(0.0);
        let b = vb1.max(a);
        let mean = (self.steady_rate_at(a) + 4.0 * self.steady_rate_at(0.5 * (a + b)) + self.steady_rate_at(b)) / 6.0;
        let k = steady_growth_mm / minutes / mean.max(1e-12);
        if k.is_finite() && k > 0.0 {
            Some(k.clamp(0.02, 50.0))
        } else {
            None
        }
    }

    pub fn for_path(&self, k: f64) -> WearTrajectory {
        self.for_path_curve(&|_| k)
    }

    pub fn for_path_curve(&self, k_at: &dyn Fn(f64) -> f64) -> WearTrajectory {
        let limit = self.vb_limit_mm.max(1e-6);
        let tau = self.break_in_tau_min.max(1e-9);
        let vb_b = self.break_in_vb_mm.max(0.0);
        let rate = |t: f64, vb: f64| k_at(vb).max(1e-6) * self.steady_rate_at(vb) + vb_b / tau * (-t / tau).exp();
        let (mut t, mut vb) = (0.0f64, 0.0f64);
        let mut pts: Vec<(f64, f64, f64)> = Vec::new();
        let mut life: Option<f64> = None;
        let dv = limit / 300.0;
        for _ in 0..6000 {
            let r1 = rate(t, vb).max(1e-12);
            pts.push((t, vb, r1));
            if vb >= 1.25 * limit || t > 1e6 {
                break;
            }
            let dt = if t < 4.0 * tau { (0.125 * tau).min(dv / r1) } else { dv / r1 }.max(1e-9);
            let r2 = rate(t + 0.5 * dt, vb + 0.5 * dt * r1);
            let next = vb + dt * r2;
            if life.is_none() && next >= limit {
                life = Some(t + dt * (limit - vb) / (next - vb).max(1e-12));
            }
            t += dt;
            vb = next;
        }
        let n = pts.len();
        let keep: Vec<usize> = if n <= 160 {
            (0..n).collect()
        } else {
            let mut v: Vec<usize> = (0..160).map(|i| i * (n - 1) / 159).collect();
            v.dedup();
            v
        };
        let last = pts[n - 1];
        let r0 = pts[0].2;
        WearTrajectory {
            t_min: keep.iter().map(|i| pts[*i].0).collect(),
            vb_mm: keep.iter().map(|i| pts[*i].1).collect(),
            rate_mm_per_min: keep.iter().map(|i| pts[*i].2).collect(),
            force_n: Vec::new(),
            temp_c: Vec::new(),
            life_min: life.unwrap_or(last.0 + (limit - last.1).max(0.0) / last.2),
            linear_life_min: limit / (k_at(0.0).max(1e-6) * self.steady_rate_at(0.0)).max(1e-12),
            break_in_vb_mm: vb_b,
            break_in_tau_min: tau,
            phase3_min: None,
            vb_limit_mm: limit,
            kappa: self.kappa,
            calib_wear: self.calib_wear,
            acceleration: last.2 / r0.max(1e-12),
        }
    }

    pub fn scaled(&self, factor: f64) -> WearTrajectory {
        let k = factor.max(1e-6);
        if (k - 1.0).abs() < 1e-12 {
            return self.clone();
        }
        let mut out = self.clone();
        out.t_min = self.t_min.iter().map(|t| t / k).collect();
        out.rate_mm_per_min = self.rate_mm_per_min.iter().map(|r| r * k).collect();
        out.life_min = self.life_min / k;
        out.linear_life_min = self.linear_life_min / k;
        out.break_in_tau_min = self.break_in_tau_min / k;
        out.phase3_min = self.phase3_min.map(|v| v / k);
        out
    }
}

#[allow(clippy::too_many_arguments)]
pub fn wear_trajectory(ctx: &CutContext, co: &CuttingCoeffs, fz: f64, rpm: f64, eng: &Engagement, vce: f64, body: ThermalBody, kappa: f64) -> WearTrajectory {
    let reference = reference_state(ctx);
    let raw = trajectory_raw(ctx, co, fz, rpm, eng, vce, body, reference, kappa);
    let k = kappa.max(1e-6);
    let n = raw.s.len();
    let keep: Vec<usize> = if n <= 80 {
        (0..n).collect()
    } else {
        let mut v: Vec<usize> = (0..80).map(|i| i * (n - 1) / 79).collect();
        v.dedup();
        v
    };
    let first_rate = raw.rate.iter().cloned().fold(f64::INFINITY, f64::min).max(1e-12);
    let end_rate = raw.rate[n - 1];
    WearTrajectory {
        t_min: keep.iter().map(|i| raw.s[*i] / k).collect(),
        vb_mm: keep.iter().map(|i| raw.vb[*i]).collect(),
        rate_mm_per_min: keep.iter().map(|i| raw.rate[*i] * k).collect(),
        force_n: keep.iter().map(|i| raw.force[*i]).collect(),
        temp_c: keep.iter().map(|i| raw.temp[*i]).collect(),
        life_min: raw.life_s / k,
        linear_life_min: raw.linear_s / k,
        break_in_vb_mm: raw.break_in_vb,
        break_in_tau_min: raw.tau_s / k,
        phase3_min: raw.phase3_s.map(|v| v / k),
        vb_limit_mm: ctx.vb_limit_mm(),
        kappa: k,
        calib_wear: ctx.calib.wear,
        acceleration: end_rate / first_rate,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceResult {
    pub wall_ra_um: f64,
    pub feed_mark_um: f64,
    pub scallop_um: f64,
    #[serde(default)]
    pub min_chip_um: f64,
    #[serde(default)]
    pub chatter_penalty: f64,
    /// 날 궤적 포락선 모델 (`surface::wall_roughness`)
    #[serde(default)]
    pub wall_rz_um: f64,
    #[serde(default)]
    pub wall_ra_stable_um: f64,
    #[serde(default)]
    pub envelope_rt_um: f64,
    #[serde(default)]
    pub forming_teeth: u32,
    #[serde(default)]
    pub single_tooth: bool,
    #[serde(default)]
    pub chatter_wpv_um: f64,
}

/// 측벽 조도·코너 R 스캘럽. 측벽 Ra 는 날 궤적 포락선 모델 (`surface::wall_roughness`) — 런아웃·최소 칩·마모·구성인선·채터 반영
#[allow(clippy::too_many_arguments)]
pub fn surface(ctx: &CutContext, fz: f64, stepover: f64, h_min_mm: f64, h_max_mm: f64, bue_risk: f64, margin: f64, chatter_p: f64) -> SurfaceResult {
    let tool = &ctx.tool;
    let r = tool.radius();
    let rt = fz * fz / (8.0 * r) * 1000.0;
    let w = crate::surface::wall_roughness(r, tool.flutes, fz, tool.runout_ecc_mm(), h_min_mm, h_max_mm, tool.flank_wear_mm, crate::pairref::WpGroup::of(&ctx.wp), bue_risk, margin, chatter_p);
    let ra = w.ra_um;
    let rc = tool.nose.corner_radius(tool.diameter_mm);
    let scallop = if rc > 0.0 && stepover > 0.0 && stepover < 2.0 * rc {
        (rc - (rc * rc - stepover * stepover / 4.0).sqrt()) * 1000.0
    } else {
        0.0
    };
    SurfaceResult {
        wall_ra_um: ra,
        feed_mark_um: rt,
        scallop_um: scallop,
        min_chip_um: w.hmin_term_um,
        chatter_penalty: if w.ra_stable_um > 0.0 { ra / w.ra_stable_um - 1.0 } else { 0.0 },
        wall_rz_um: w.rz_um,
        wall_ra_stable_um: w.ra_stable_um,
        envelope_rt_um: w.rt_envelope_um,
        forming_teeth: w.forming_teeth,
        single_tooth: w.single_tooth,
        chatter_wpv_um: w.chatter_wpv_um,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallError {
    pub deflection_um: f64,
    pub deflection_top_um: f64,
    pub deflection_bottom_um: f64,
    pub wear_um_per_min: f64,
    pub thermal_tool_um: f64,
}

pub fn wall_error(ctx: &CutContext, f: &ForceResult, eng: &Engagement, th: &ThermalResult, wr: &WearResult) -> WallError {
    let comp = tool_compliance(&ctx.tool, eng.ap_mm, ctx.calib.deflection);
    let holder = ctx.tool.support_compliance_mm_per_n() * ctx.calib.deflection;
    let l = ctx.tool.stickout_mm;
    let mut top = 0.0;
    let mut bottom = 0.0;
    let mut mean = 0.0;
    let rc = ctx.tool.nose.corner_radius(ctx.tool.diameter_mm);
    if rc > 0.0 && eng.ap_mm < rc && ctx.tool.nose == ToolNose::Ball {
        let d = deflection_at(&comp, f.f_res_mean, (l - eng.ap_mm / 2.0).max(0.0), holder);
        return WallError {
            deflection_um: d,
            deflection_top_um: d,
            deflection_bottom_um: d,
            wear_um_per_min: wr.radial_loss_rate_um_per_min,
            thermal_tool_um: -th.tool_radial_growth_um,
        };
    }
    if let Some(wall) = eng.wall_angle() {
        let steps = f.fy_series.len().max(1);
        let slices = 8;
        let r = ctx.tool.radius();
        let tan_h = ctx.tool.helix_deg.to_radians().tan();
        for k in 0..slices {
            let zk = (k as f64 + 0.5) * eng.ap_mm / slices as f64;
            let theta = (wall + zk * tan_h / r).rem_euclid(f.pitch_rad.max(1e-9));
            let idx = ((theta / f.pitch_rad) * steps as f64).floor() as usize % steps;
            let fy = f.fy_series.get(idx).cloned().unwrap_or(0.0);
            let away = if eng.mode == MillMode::Down { fy } else { -fy };
            let d = deflection_at(&comp, away, (l - zk).max(0.0), holder);
            mean += d / slices as f64;
            if k == 0 {
                bottom = d;
            }
            if k == slices - 1 {
                top = d;
            }
        }
    }
    WallError {
        deflection_um: mean,
        deflection_top_um: top,
        deflection_bottom_um: bottom,
        wear_um_per_min: wr.radial_loss_rate_um_per_min,
        thermal_tool_um: -th.tool_radial_growth_um,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutAnalysis {
    pub rpm: f64,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub feed_mm_min: f64,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub mode: MillMode,
    pub engage_angle_deg: f64,
    pub chip_thinning_factor: f64,
    pub mrr_cm3_min: f64,
    pub vc_effective_m_min: f64,
    pub coeffs: CuttingCoeffs,
    pub forces: ForceSummary,
    pub spindle_power_kw: f64,
    pub available_power_kw: f64,
    pub stiffness_n_per_um: f64,
    pub deflection_peak_um: f64,
    pub wall: WallError,
    pub thermal: ThermalResult,
    pub wear: WearResult,
    pub surface: SurfaceResult,
    pub tolerance_mm: f64,
    pub error_budget_um: ErrorBudget,
    pub notes: Vec<String>,
    #[serde(default)]
    pub trajectory: WearTrajectory,
    #[serde(default)]
    pub stability: crate::dynamics::Stability,
    #[serde(default)]
    pub forced: crate::dynamics::ForcedVibration,
    #[serde(default)]
    pub fixture_deflection_um: f64,
    #[serde(default)]
    pub ploughing_share: f64,
    #[serde(default = "one")]
    pub softening: f64,
    #[serde(default)]
    pub tribology: TribologyReport,
    #[serde(default)]
    pub regime: Vec<f32>,
    #[serde(default)]
    pub environment: crate::environment::EnvironmentReport,
    #[serde(default)]
    pub thermal_fatigue_per_min: f64,
}

impl CutAnalysis {
    pub fn effective_trajectory(&self) -> WearTrajectory {
        self.trajectory.scaled(self.wear.chatter_factor.max(1.0))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceSummary {
    pub fx_mean: f64,
    pub fy_mean: f64,
    pub f_res_mean: f64,
    pub f_res_peak: f64,
    pub torque_mean_nm: f64,
    pub torque_peak_nm: f64,
    pub cutting_power_kw: f64,
    pub h_max_mm: f64,
    pub h_mean_mm: f64,
    pub teeth_in_cut: f64,
    #[serde(default)]
    pub fa_mean: f64,
    #[serde(default)]
    pub fa_peak: f64,
}

impl From<&ForceResult> for ForceSummary {
    fn from(f: &ForceResult) -> Self {
        Self {
            fx_mean: f.fx_mean,
            fy_mean: f.fy_mean,
            f_res_mean: f.f_res_mean,
            f_res_peak: f.f_res_peak,
            torque_mean_nm: f.torque_mean_nm,
            torque_peak_nm: f.torque_peak_nm,
            cutting_power_kw: f.cutting_power_kw,
            h_max_mm: f.h_max_mm,
            h_mean_mm: f.h_mean_mm,
            teeth_in_cut: f.teeth_in_cut,
            fa_mean: f.fa_mean,
            fa_peak: f.fa_peak,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBudget {
    pub deflection_um: f64,
    pub wear_10min_um: f64,
    pub thermal_tool_um: f64,
    pub thermal_workpiece_um: f64,
    pub total_um: f64,
    pub tolerance_um: f64,
    pub utilization: f64,
    #[serde(default)]
    pub fixture_um: f64,
    #[serde(default)]
    pub dynamic_um: f64,
    #[serde(default)]
    pub environment_um: f64,
    #[serde(default)]
    pub drift_um: f64,
    /// 런아웃으로 가장 긴 날이 더 깎는 양 (µm) — 공구 반경을 회전 상태로 재면 0
    #[serde(default)]
    pub runout_um: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ThermalBody {
    pub mass_kg: f64,
    pub area_m2: f64,
    pub size_mm: f64,
}

impl ThermalBody {
    pub fn of_setup(w: &crate::workpiece_setup::WorkpieceSetup, wp: &WorkpieceProps) -> Self {
        Self {
            mass_kg: (w.volume_cm3().max(1.0) * 1e-6) * wp.density,
            area_m2: w.surface_area_m2().max(1e-4),
            size_mm: w.width_mm.max(w.height_mm),
        }
    }
}

pub fn effective_vc(tool: &ToolGeometry, rpm: f64, ap: f64) -> f64 {
    let (rl, _) = tool.local_radius(ap.max(0.0));
    PI * 2.0 * rl * rpm / 1000.0
}

pub fn analyze_cut(ctx: &CutContext, conds: &CuttingConditions, down: bool, body: ThermalBody) -> CutAnalysis {
    let d = ctx.tool.diameter_mm;
    let rpm = conds.spindle_rpm as f64;
    let vc = PI * d * rpm / 1000.0;
    let z = ctx.tool.flutes.max(1) as f64;
    let fz = if rpm > 0.0 { conds.feed_rate_mm_min / (rpm * z) } else { conds.feed_per_tooth_mm };
    let ap = conds.axial_doc_mm.min(ctx.tool.loc_mm);
    let eng = Engagement::side(ap, conds.radial_doc_mm, d, down);
    let vce = effective_vc(&ctx.tool, rpm, ap);
    let h_est = (fz * eng.max_sin() * 0.64).max(1e-4);
    let first = cutting_coeffs_at(ctx, vce, h_est);
    let f0 = mechanistic_forces(&ctx.tool, &first, fz, rpm, &eng, 36, 8);
    let t0 = thermal(ctx, &first, &f0, vce, rpm, &eng, body.mass_kg, body.area_m2).interface_c;
    let co = cutting_coeffs_at_temp(ctx, vce, h_est, t0);
    let f = mechanistic_forces(&ctx.tool, &co, fz, rpm, &eng, 72, 12);
    let comp = tool_compliance(&ctx.tool, ap, ctx.calib.deflection);
    let th = thermal(ctx, &co, &f, vce, rpm, &eng, body.mass_kg, body.area_m2);
    let reference = reference_state(ctx);
    let kappa = wear_kappa(ctx);
    let trajectory = wear_trajectory(ctx, &co, fz, rpm, &eng, vce, body, kappa);
    let mut wr = wear(ctx, &f, &th, vce, fz, &eng, reference);
    wr.vb_rate_mm_per_min *= kappa;
    wr.radial_loss_rate_um_per_min *= kappa;
    wr.linear_life_min = if wr.vb_rate_mm_per_min > 0.0 { wr.vb_limit_mm / wr.vb_rate_mm_per_min } else { f64::INFINITY };
    wr.tool_life_min = trajectory.life_min;
    let stability = crate::dynamics::stability(
        &ctx.tool,
        &co,
        &eng,
        f.h_mean_mm,
        rpm,
        ap,
        ctx.calib.deflection,
        ctx.machine.max_rpm,
        vce,
        ctx.wp.process_damping,
    );
    let cwf = crate::toolwear::chatter_wear_factor(stability.margin, stability.measured);
    if cwf > 1.0 {
        wr.vb_rate_mm_per_min *= cwf;
        wr.radial_loss_rate_um_per_min *= cwf;
        wr.linear_life_min /= cwf;
        wr.tool_life_min /= cwf;
    }
    wr.chatter_factor = cwf;
    let wall = wall_error(ctx, &f, &eng, &th, &wr);
    let modal = crate::dynamics::tool_modal(&ctx.tool, ap, ctx.calib.deflection);
    let forced = crate::dynamics::forced_vibration(&f, &modal, rpm, ctx.tool.flutes, &eng);
    // 예전 채터 보정 `Ra × (1 + 1.5·(1 − 여유))` 대신 파상도(h_max·ap/ap_lim)를 날 궤적에 더하고 채터 확률로 가중한다
    let chatter_p = crate::toolwear::chatter_probability(stability.margin, stability.measured);
    let sf = surface(ctx, fz, conds.radial_doc_mm, co.h_min_mm, f.h_max_mm, th.bue_risk, stability.margin, chatter_p);
    let spindle = f.cutting_power_kw / ctx.machine.efficiency.max(0.1);
    let avail = ctx.machine.available_power_kw(rpm);
    let defl_peak = f.f_res_peak / comp.stiffness_n_per_um;
    let wear10 = trajectory.vb_at(10.0 * cwf) * ctx.tool.radial_per_vb() * 1000.0;
    let wp_rise = th.workpiece_steady_rise_c * (1.0 - (-600.0 / th.workpiece_time_constant_s.max(1.0)).exp());
    let wp_um = ctx.wp.expansion * body.size_mm * wp_rise * 1000.0;
    let fixture_um = f.fy_mean.abs() / ctx.fixture_stiffness_n_per_um.max(1e-6);
    let dynamic_um = forced.sle_um.abs();
    let wall_defl = wall.deflection_um;
    let env_rep = crate::environment::report(ctx, body, vce, th.interface_c);
    let env_um = env_rep.scale_error_um;
    let drift_um = env_rep.drift_um;
    let runout_um = ctx.tool.runout_overcut_um();
    let total = wall_defl.abs() + wear10 + th.tool_radial_growth_um.abs() + wp_um + fixture_um + dynamic_um + env_um.abs() + drift_um + runout_um;
    let tol_um = ctx.tolerance_mm * 1000.0;
    let mut notes = Vec::new();
    let thinning = if eng.mode != MillMode::Slot && eng.max_sin() > 0.0 { 1.0 / eng.max_sin() } else { 1.0 };
    if thinning > 1.3 {
        notes.push(format!(
            "반경 절입이 작아 실제 최대 칩두께가 프로그램 fz 의 {:.0}% 입니다 (칩 얇아짐).",
            100.0 / thinning
        ));
    }
    if f.h_max_mm < ctx.tool.edge_radius_um / 1000.0 {
        notes.push("최대 칩두께가 날끝 반경보다 작아 밀림(ploughing)·가공경화 위험이 있습니다.".into());
    }
    if f.ploughing_share > 0.25 {
        notes.push(format!(
            "접촉 날 길이의 {:.0}% 가 최소 칩두께 {:.1} µm 미만이라 칩 대신 밀림·마찰이 발생합니다.",
            f.ploughing_share * 100.0,
            co.h_min_mm * 1000.0
        ));
    }
    if ctx.wp.work_hardening >= 0.2 && f.h_mean_mm < 2.0 * co.hardened_depth_mm {
        notes.push(format!(
            "가공경화층(약 {:.0} µm) 안에서 칩을 깎고 있어 비절삭저항이 상승합니다. fz 를 경화층보다 크게 잡으세요.",
            co.hardened_depth_mm * 1000.0
        ));
    }
    if th.over_coating_limit {
        notes.push(format!(
            "날끝 온도 {:.0}°C 가 코팅 허용온도({:.0}°C)의 85% 를 넘습니다.",
            th.interface_c, ctx.tool.coating.max_temp_c
        ));
    }
    if th.thermal_crack_risk > 0.45 {
        notes.push(format!(
            "단속 절삭 + 급랭(열전달 {:.0} W/m²K, 급랭 진폭 {:.0} K, 열응력 약 {:.0} MPa) 조합으로 열균열(comb crack) 위험이 있습니다. 건식/에어/MQL 전환을 검토하세요.",
            th.cooling.h_eff, th.shock.amplitude_c, th.shock.stress_mpa
        ));
    }
    if th.bue_risk > 0.3 {
        notes.push("구성인선(BUE) 형성 온도 구간입니다. 절삭속도 상향 또는 윤활을 강화하세요.".into());
    }
    let rep = tribology::report(ctx, &co, &th.cooling, th.shock, th.barrier, th.interface_c, vce);
    for m in rep.compat.messages.iter() {
        notes.push(format!("소재 궁합: {}", m));
    }
    if th.cooling.boiling_state.starts_with("막비등") {
        notes.push(format!(
            "날끝 표면이 라이덴프로스트 온도({:.0}°C)를 넘어 증기막이 생겨 냉각 열전달이 {:.0}% 로 떨어집니다 — 고압 내부급유(증기막 파괴) 또는 MQL 권장.",
            tribology::leidenfrost_c(ctx),
            th.cooling.boiling_factor * 100.0
        ));
    }
    if ctx.coolant.fluid.film_strength > 0.3 && co.lubricant_access < 0.1 {
        notes.push(format!(
            "유효 절삭속도 {:.0} m/min 에서는 {} 이(가) 공구-칩 계면에 거의 침투하지 못해 (접근도 {:.2}) 윤활 효과가 작습니다.",
            vce,
            ctx.coolant.fluid.label(),
            co.lubricant_access
        ));
    }
    if co.chip_lift > 0.1 {
        notes.push(format!(
            "고압 냉각 제트가 칩을 들어 올려 접촉 길이가 {:.0}% 줄고 마찰계수가 낮아집니다.",
            co.chip_lift * 50.0
        ));
    }
    if let Some(m) = wr.mechanisms.iter().find(|m| m.key == wr.dominant) {
        notes.push(format!(
            "지배 마모 메커니즘: {} ({:.0}%, 기준 상태 대비 ×{:.2}) · 코팅-피삭재 궁합 계수 ×{:.2}",
            m.label,
            m.share * 100.0,
            m.ratio,
            wr.coating_factor
        ));
    }
    if spindle > avail {
        notes.push(format!("필요 동력 {:.2} kW 가 해당 회전수의 가용 동력 {:.2} kW 를 초과합니다.", spindle, avail));
    }
    if ctx.tool.substrate_max_c < 800.0 && th.interface_c > 0.9 * ctx.tool.substrate_max_c {
        notes.push(format!(
            "날끝 {:.0}°C 가 공구 모재({}) 허용 온도 {:.0}°C 에 근접·초과해 모재 연화로 마모가 ×{:.1} 빨라집니다.",
            th.interface_c,
            crate::profile::ToolSubstrate::from_key(&ctx.tool.substrate).map(|t| t.label()).unwrap_or(ctx.tool.substrate.as_str()),
            ctx.tool.substrate_max_c,
            substrate_softening(ctx, th.interface_c)
        ));
    }
    notes.extend(env_rep.notes.iter().cloned());
    notes.extend(stability.notes.iter().cloned());
    if forced.near_resonance {
        notes.push(format!(
            "날 통과 주파수 {:.0} Hz 가 공구 고유진동수 {:.0} Hz 근처라 강제진동이 커집니다 (진폭 {:.1} µm).",
            forced.tooth_hz, stability.fn_hz, forced.amplitude_um
        ));
    }
    if cwf > 1.02 {
        notes.push(format!(
            "안정 여유 {:.2}배{}의 채터 기대 손실로 플랭크 마모 속도 ×{:.2}, 공구 수명 {:.0}분 → {:.0}분 (채터가 심할수록 공구 수명이 크게 준다는 실험 결과를 완전 채터에서 수명 약 1/3 로 모델링하고 안정 한계 불확실성(로그 표준편차 {:.2}, {})으로 기대값 처리 · 채터 확률 {:.0}%)",
            stability.margin,
            if stability.stable { "(안정 판정)" } else { "(불안정 판정)" },
            cwf,
            trajectory.life_min,
            wr.tool_life_min,
            crate::toolwear::frf_log_sd(stability.measured),
            if stability.measured { "실측 FRF" } else { "모델 추정 FRF · 깊은 불안정 예측은 신뢰도 50%부터, 여유 1 이상은 100%" },
            crate::toolwear::chatter_probability(stability.margin, stability.measured) * 100.0
        ));
    }
    if wr.coating_blend < 0.98 {
        notes.push(format!(
            "코팅 관통: 마모 랜드의 {:.0}% 가 모재라 이 피삭재와 맞지 않는 코팅의 마모 속도가 모재 쪽으로 ×{:.2} 이동 (두 상이 같은 속도로 닳는 합성 마모 역혼합칙)",
            wr.coating_exposure * 100.0,
            wr.coating_blend
        ));
    }
    if let Some(p3) = trajectory.phase3_min {
        if p3 < trajectory.life_min {
            notes.push(format!(
                "마모 3단계(가속) 진입 예상 {:.0}분 — 플랭크 마모가 절삭력·온도를 키워 수명 말기 마모가 {:.1}배 빨라집니다.",
                p3, trajectory.acceleration
            ));
        }
    }
    let tf_per_min = crate::toolwear::thermal_fatigue_rate(th.shock.severity, eng.span(), rpm) * 60.0;
    if tf_per_min > 1.0 / 600.0 {
        let cycle_ms = if rpm > 0.0 { 60_000.0 / rpm } else { 0.0 };
        notes.push(format!(
            "단속 절삭 열피로: 날마다 회전당 가열 {:.1} ms · 냉각 {:.1} ms ({}) 반복으로 빗살 균열 개시까지 이 조건 절삭 약 {:.0}분 — 누적 손상은 공구 개체에 저장되어 다음 작업으로 이어짐",
            cycle_ms * eng.span() / (2.0 * PI),
            cycle_ms * (1.0 - eng.span() / (2.0 * PI)),
            ctx.coolant.method.label(),
            1.0 / tf_per_min
        ));
    }
    let shares: Vec<f64> = wr.mechanisms.iter().map(|m| m.share).collect();
    let regime = tribology::regime_vector(ctx, &rep, &shares, vce, th.interface_c, th.thermal_crack_risk, th.bue_risk);
    CutAnalysis {
        rpm,
        vc_m_min: vc,
        fz_mm: fz,
        feed_mm_min: conds.feed_rate_mm_min,
        ap_mm: ap,
        ae_mm: eng.ae_mm,
        mode: eng.mode,
        engage_angle_deg: eng.span().to_degrees(),
        chip_thinning_factor: thinning,
        mrr_cm3_min: ap * eng.ae_mm * conds.feed_rate_mm_min / 1000.0,
        vc_effective_m_min: vce,
        forces: ForceSummary::from(&f),
        spindle_power_kw: spindle,
        available_power_kw: avail,
        stiffness_n_per_um: comp.stiffness_n_per_um,
        deflection_peak_um: defl_peak,
        wall,
        surface: sf,
        tolerance_mm: ctx.tolerance_mm,
        error_budget_um: ErrorBudget {
            deflection_um: wall_defl,
            wear_10min_um: wear10,
            thermal_tool_um: th.tool_radial_growth_um,
            thermal_workpiece_um: wp_um,
            total_um: total,
            tolerance_um: tol_um,
            utilization: if tol_um > 0.0 { total / tol_um } else { 0.0 },
            fixture_um,
            dynamic_um,
            environment_um: env_um,
            drift_um,
            runout_um,
        },
        thermal: th,
        wear: wr,
        notes,
        trajectory,
        stability,
        forced,
        fixture_deflection_um: fixture_um,
        ploughing_share: f.ploughing_share,
        softening: co.softening,
        coeffs: co,
        tribology: rep,
        regime,
        environment: env_rep,
        thermal_fatigue_per_min: tf_per_min,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Purpose {
    Roughing,
    Finishing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub purpose: Purpose,
    pub conditions: CuttingConditions,
    pub fz_target_mm: f64,
    pub chip_thinning_factor: f64,
    pub deflection_budget_um: f64,
    pub predicted_wall_error_um: f64,
    pub predicted_tool_life_min: f64,
    pub notes: Vec<String>,
}

pub fn engagement_defaults(ctx: &CutContext, purpose: Purpose) -> (f64, f64) {
    let d = ctx.tool.diameter_mm;
    let hard = ctx.wp.hardness_hrc >= 45.0;
    match (ctx.tool.nose, purpose) {
        (ToolNose::Ball, Purpose::Roughing) => (if hard { 0.06 * d } else { 0.1 * d }, 0.1 * d),
        (ToolNose::Ball, Purpose::Finishing) => (if hard { 0.04 * d } else { 0.06 * d }, 0.05 * d),
        (_, Purpose::Roughing) => ((ctx.wp.ap_ratio * d).min(0.9 * ctx.tool.loc_mm), ctx.wp.ae_ratio * d),
        (_, Purpose::Finishing) => (
            (ctx.wp.ap_ratio * d).min(0.9 * ctx.tool.loc_mm),
            if ctx.allowance_mm > 0.0 { ctx.allowance_mm.min(0.2 * d) } else { 0.05 * d },
        ),
    }
}

pub fn recommend(ctx: &CutContext, is_high_end: bool, body: ThermalBody, purpose: Purpose) -> Recommendation {
    let d = ctx.tool.diameter_mm;
    let z = ctx.tool.flutes.max(1) as f64;
    let mut notes = Vec::new();
    let tier = if is_high_end { 1.0 } else { 0.55 };
    let (coat, coat_note) = tribology::speed_capability(&ctx.tool.coating, &ctx.wp);
    if let Some(n) = coat_note {
        notes.push(n);
    }
    // 열이 공구에 남는 저열전도 소재(스테인리스·Ti·Ni·CFRP, k < 25 W/mK)의 건식 절삭만 속도를 낮춘다
    let dry_penalty = if matches!(ctx.coolant.method, CoolantMethod::Dry) && ctx.wp.conductivity < 25.0 && ctx.wp.hardness_hrc < 45.0 {
        0.75
    } else {
        1.0
    };
    let (mut ap, mut ae) = engagement_defaults(ctx, purpose);
    let vc_target = ctx.wp.ref_vc * tier * coat * dry_penalty;
    let d_eff = 2.0 * ctx.tool.local_radius(ap).0;
    let mut rpm = vc_target * 1000.0 / (PI * d_eff.max(0.1 * d));
    if rpm > ctx.machine.max_rpm {
        rpm = ctx.machine.max_rpm;
        notes.push(format!(
            "최대 회전수 {:.0} rpm 으로 제한되어 유효 절삭속도가 {:.0} m/min 입니다.",
            ctx.machine.max_rpm,
            PI * d_eff * rpm / 1000.0
        ));
    }
    let fz_target = (ctx.wp.fz_coeff * d).clamp(0.003, 0.25)
        * if is_high_end { 1.0 } else { 0.8 }
        * if purpose == Purpose::Finishing { 0.8 } else { 1.0 };
    let budget_um = match purpose {
        Purpose::Roughing => (0.5 * ctx.allowance_mm * 1000.0).max(20.0),
        Purpose::Finishing => (0.3 * ctx.tolerance_mm * 1000.0).max(2.0),
    };
    let mut fz_scale: f64 = 1.0;
    let mut thinning;
    let mut fz;
    let mut conds;
    let mut last;
    let mut iter = 0;
    let mut moved_rpm = false;
    let mut chatter_noted = false;
    loop {
        let sin_k = ctx.tool.local_radius(ap).1;
        let ae_eff = if ctx.tool.nose == ToolNose::Ball { ae.min(d_eff) } else { ae };
        thinning = if ae_eff < d_eff / 2.0 {
            let phis = (1.0 - 2.0 * ae_eff / d_eff.max(1e-6)).clamp(-1.0, 1.0).acos();
            (1.0 / (phis.sin() * sin_k.max(0.2))).min(2.5)
        } else {
            1.0
        };
        fz = fz_target * fz_scale * thinning;
        let mut feed = rpm * z * fz;
        if feed > ctx.machine.max_feed_mm_min {
            feed = ctx.machine.max_feed_mm_min;
            fz = feed / (rpm * z);
        }
        conds = CuttingConditions {
            cutting_speed_m_min: PI * d * rpm / 1000.0,
            feed_rate_mm_min: feed,
            feed_per_tooth_mm: fz,
            axial_doc_mm: ap,
            radial_doc_mm: ae,
            spindle_rpm: rpm.round() as u32,
            coolant: match ctx.coolant.method {
                CoolantMethod::Flood => CoolantType::Flood,
                CoolantMethod::Mist => CoolantType::Mist,
                CoolantMethod::AirBlast => CoolantType::AirBlast,
                CoolantMethod::Dry => CoolantType::Dry,
                CoolantMethod::ThroughTool => CoolantType::ThroughTool,
            },
        };
        let a = analyze_cut(ctx, &conds, true, body);
        let power_ok = a.spindle_power_kw <= 0.9 * a.available_power_kw;
        let defl_ok = a.wall.deflection_um.abs() <= budget_um;
        let chatter_ok = a.stability.stable || !a.stability.margin.is_finite();
        let stable_rpm = a
            .stability
            .suggested_rpm
            .iter()
            .cloned()
            .find(|n| (n / rpm - 1.0).abs() <= 0.25 && *n <= ctx.machine.max_rpm);
        last = a;
        if power_ok && defl_ok && !chatter_ok && iter < 16 {
            if last.stability.measured {
                let st = &last.stability;
                let here = st.ap_lim_mm / st.process_damping_gain.max(1e-9);
                let hi = (1.25 * rpm).min(ctx.machine.max_rpm);
                let best = crate::dynamics::robust_peak(&st.envelope, 0.75 * rpm, hi, 0.02 * rpm);
                match best {
                    Some((n, a_rob)) if !moved_rpm && a_rob > 1.1 * here => {
                        let target = (0.9 * a_rob).min(ap);
                        notes.push(format!(
                            "채터 여유 {:.2}배 (실측 FRF) → ±2% 회전수 오차에도 한계 ap {:.2} mm 인 로브 S{:.0} 로 이동 (이전 S{:.0}){}.",
                            st.margin,
                            a_rob,
                            n,
                            rpm,
                            if target < ap { format!(", ap {:.2} → {:.2} mm", ap, target) } else { String::new() }
                        ));
                        rpm = n.round();
                        ap = target;
                        moved_rpm = true;
                    }
                    _ => {
                        let k = (0.9 * st.margin).clamp(0.05, 0.95);
                        notes.push(format!("채터 여유 {:.2}배 (실측 FRF) → ap {:.2} → {:.2} mm 로 축소.", st.margin, ap, ap * k));
                        ap *= k;
                    }
                }
                iter += 1;
                continue;
            }
            if !chatter_noted {
                let hint = stable_rpm.map(|n| format!(", 안정 로브 후보 S{:.0}", n)).unwrap_or_default();
                notes.push(format!(
                    "채터 여유 {:.2}배 (공구·홀더 모델 추정{}) — 탭 테스트로 공구 끝 고유진동수·강성·감쇠를 입력하면 회전수·ap 를 안정 한계에 맞춰 조정합니다.",
                    last.stability.margin, hint
                ));
                chatter_noted = true;
            }
        }
        if (power_ok && defl_ok) || iter >= 16 {
            if !power_ok {
                notes.push("동력 한계 내 조건을 찾지 못했습니다. 공구·기계 사양을 확인하세요.".into());
            }
            if !defl_ok {
                notes.push(format!(
                    "휨 예산 {:.1} µm 를 만족하지 못했습니다 (예측 {:.1} µm). 돌출 길이 단축 또는 공구 직경 상향을 검토하세요.",
                    budget_um,
                    last.wall.deflection_um.abs()
                ));
            }
            break;
        }
        if !power_ok {
            if ap > 0.3 * d && purpose == Purpose::Roughing {
                ap *= 0.8;
            } else {
                ae *= 0.85;
            }
        } else if fz_scale > 0.5 {
            fz_scale *= 0.9;
        } else if purpose == Purpose::Roughing && ae > 0.05 * d {
            ae *= 0.85;
        } else if ap > 0.2 * d {
            ap *= 0.85;
        } else {
            fz_scale *= 0.9;
        }
        iter += 1;
    }
    if iter > 0 {
        notes.push(format!(
            "제약 조정 {}회: ap {:.2} mm, ae {:.2} mm, fz 배율 {:.2}.",
            iter, ap, ae, fz_scale
        ));
    }
    Recommendation {
        purpose,
        predicted_wall_error_um: last.wall.deflection_um,
        predicted_tool_life_min: last.wear.tool_life_min,
        conditions: conds,
        fz_target_mm: fz_target,
        chip_thinning_factor: thinning,
        deflection_budget_um: budget_um,
        notes,
    }
}

pub fn flute_count_from_conditions(conds: &CuttingConditions) -> u32 {
    if conds.spindle_rpm == 0 || conds.feed_per_tooth_mm <= 0.0 {
        return 4;
    }
    (conds.feed_rate_mm_min / (conds.spindle_rpm as f64 * conds.feed_per_tooth_mm)).round().clamp(1.0, 12.0) as u32
}

