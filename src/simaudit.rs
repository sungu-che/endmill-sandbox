//! 시뮬레이션 감사 — 로그에 찍힌 수치가 서로 맞는지, 모델이 어디까지 믿을 만한지를 한 곳에 모은다.
//!
//! 저장되는 곳: `wear_logs/*.json` 의 `audit` 키, 테스트 로그의 `[AUDIT]` 줄 (`lines`).
//! 이 모듈은 값을 바꾸지 않는다. 이미 계산된 결과를 다시 읽어
//!  1) 공칭 해석(프리셋 ae)과 실제 경로 시뮬레이션(경로가 실제로 깎는 ae)의 조건 차이,
//!  2) 격자 이산화·결합 판독이 기하와 맞는지 (체적·시간·ae),
//!  3) VB 성장 중 길들이기 항과 정상 마모 항의 비율, 교체 개수 계산의 근거,
//!  4) 힘·열·동역학·휨 각각의 분해 값과 이 모델이 아직 다루지 않는 항,
//! 을 나란히 놓고, 어긋나는 곳을 `flags` 로 남긴다. 임계값은 상수로 두고 근거를 주석으로 적는다.
use crate::loadsim::{EngageAudit, LoadSimReport};
use crate::physics::{self, CutAnalysis, CutContext, Engagement, ThermalBody};
use crate::profile::{MachiningProfile, ToolNose};
use crate::wearcomp::CompOutcome;
use crate::wearlog::WearLog;
use serde::{Deserialize, Serialize};

pub const AUDIT_VERSION: u32 = 4;

/// 경로 ae 를 격자가 읽은 값과 기하 기대값(알 수 있는 패턴만)이 이 비율 이상 다르면 주의
pub const AE_GEOMETRY_TOL: f64 = 0.15;
/// 절삭 시간이 기하 기대(길이/이송)와 이 비율 이상 다르면 주의
pub const TIME_GEOMETRY_TOL: f64 = 0.06;
/// 모델에 들어간 ae(스텝별 판독)와 체적으로 읽은 평균 ae 가 이 비율 이상 다르면 주의
pub const AE_READ_TOL: f64 = 0.12;
/// 격자 셀이 실행 ae 의 이 배수를 넘으면 경계 셀 면적 가중 근사의 오차가 단위 시험 범위(±5%)를 벗어날 수 있다
pub const MAX_CELL_OVER_AE: f64 = 3.0;
/// VB 성장 중 길들이기 항 비율이 이 값을 넘으면 '마모 속도' 로그를 정상 마모율로 읽으면 안 된다
pub const BREAKIN_SHARE_WARN: f64 = 0.5;
/// 교체 개수(물리 궤적)와 작업 평균 마모율로 단순 환산한 개수가 이 배수 이상 다르면 설명이 필요하다
pub const PARTS_RATIO_WARN: f64 = 3.0;
/// ZOA(영차 근사) 채터 한계는 반경 절입이 이 비율(ae/D)보다 작고 날 통과 주파수가 고유진동수에 가까우면 flip(주기 2배) 로브를
/// 놓친다 (Altintas·Stépán·Merdol·Dombovari 2008, CIRP J. Manuf. Sci. Technol. 1:35 — ae > D/4 에서 충분)
pub const ZOA_MIN_IMMERSION: f64 = 0.25;
/// 비에너지(절삭동력/MRR) 허용 띠 — 금속 절삭 일반 범위 (J/mm³)
pub const SPECIFIC_ENERGY_BAND: (f64, f64) = (0.3, 8.0);
/// 평균 힘 대비 피크 힘이 이 값을 넘으면 날이 한 개씩만 물리는 단속이 강하다
pub const PEAK_OVER_MEAN_INFO: f64 = 5.0;
/// 평균 칩 두께가 이보다 얇으면 크기 효과(Kienzle h^-mc)와 날끝 밀림 몫 때문에 비에너지가 일반 범위 위로 올라가는 것이 정상이다 (µm)
pub const SMALL_CHIP_UM: f64 = 15.0;
/// 초경 엔드밀 길들이기(초기 마모) 시상수의 문헌 범위 (분). 시상수는 피삭재별 기준값(`WorkpieceProps::breakin_tau_min`)이다
pub const BREAKIN_TAU_BAND: (f64, f64) = (0.5, 5.0);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditFlag {
    pub id: String,
    /// 0 정보 · 1 주의 · 2 이 항목의 예측을 그대로 믿으면 안 됨
    pub severity: u8,
    pub value: f64,
    pub expected: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditInputs {
    pub profile: String,
    pub pattern: String,
    pub material: String,
    pub stock_mm: [f64; 3],
    pub allowance_mm: f64,
    pub tolerance_um: f64,
    pub d_mm: f64,
    pub flutes: u32,
    pub helix_deg: f64,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_mm: f64,
    pub stickout_mm: f64,
    pub stickout_over_d: f64,
    pub rake_deg: f64,
    pub clearance_deg: f64,
    pub edge_um: f64,
    pub core_ratio: f64,
    /// 입력된 런아웃 TIR (µm)
    pub runout_um: f64,
    /// 모델이 쓰는 편심 e = TIR/2 (µm) — 2날 공구의 날 사이 칩 두께 오프셋은 2e = TIR
    pub runout_ecc_um: f64,
    /// 이 소재의 반경비(kr) 하한
    pub kr_floor: f64,
    pub coating: String,
    pub coating_um: f64,
    pub rpm: f64,
    pub vc_m_min: f64,
    pub feed_mm_min: f64,
    pub fz_mm: f64,
    pub ap_mm: f64,
    pub ae_planned_mm: f64,
    pub holder_k_n_per_um: f64,
    pub damping_ratio: f64,
    pub holder_mass_kg: f64,
    pub max_power_kw: f64,
    pub frf_measured: bool,
    pub coolant: String,
    pub coolant_flow_l_min: f64,
    pub coolant_c: f64,
    pub ambient_c: f64,
    pub humidity_pct: f64,
    /// 적용된 보정 계수 [동력, 마모, 휨, 온도]
    pub calibration: [f64; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditEngagement {
    pub planned_ae_mm: f64,
    pub planned_ae_over_d: f64,
    pub executed_ae_mm: f64,
    pub executed_ae_by_volume_mm: f64,
    pub executed_ae_over_d: f64,
    pub executed_engage_deg: f64,
    /// 패턴에서 기하로 알 수 있을 때만 (현재 rect_profile)
    pub geometry_ae_mm: Option<f64>,
    pub geometry_cut_len_mm: Option<f64>,
    pub geometry_cut_time_s: Option<f64>,
    pub sim_cut_len_mm: f64,
    pub sim_cut_time_s: f64,
    pub sim_removed_mm3: f64,
    pub total_time_s: f64,
    /// 격자 셀 / 실행 ae — 1 보다 크면 ae 가 셀 한 칸보다 좁아 경계 셀 면적 가중(상자 필터) 근사에 의존한다
    pub cell_over_ae: f64,
    pub grid: EngageAudit,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForceView {
    pub ae_mm: f64,
    pub engage_deg: f64,
    pub f_mean_n: f64,
    pub f_peak_n: f64,
    pub peak_over_mean: f64,
    pub power_kw: f64,
    pub specific_energy_j_mm3: f64,
    pub h_mean_um: f64,
    pub h_max_um: f64,
    pub teeth_in_cut: f64,
    /// 날끝 문지름(kte)·플랭크 접촉이 접선력에서 차지하는 비율
    pub edge_share: f64,
    pub flank_share_radial: f64,
    pub ploughing_share: f64,
    /// 축방향(공구를 뽑아 올리는) 힘 평균·피크 (N)
    pub fa_mean_n: f64,
    pub fa_peak_n: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditForce {
    /// 프리셋 조건(ae 그대로)의 공칭 해석
    pub nominal: ForceView,
    /// 경로가 실제로 깎은 ae 로 같은 모델을 다시 돌린 값
    pub executed: ForceView,
    /// 경로 시뮬레이션 빈의 시간 가중 평균 힘 / 최대 피크
    pub path_mean_n: f64,
    pub path_peak_n: f64,
    pub kc11_eff: f64,
    pub mc: f64,
    pub kr: f64,
    pub kte_n_mm: f64,
    pub kre_n_mm: f64,
    pub mu: f64,
    pub shear_deg: f64,
    pub softening: f64,
    pub strain_rate_per_s: f64,
    /// 축방향 힘 항(kac_ratio, kae)을 계산하는가
    pub axial_force_modeled: bool,
    pub kac_ratio: f64,
    pub kae_n_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditThermal {
    pub interface_dry_c: f64,
    pub interface_wet_c: f64,
    pub executed_interface_c: f64,
    pub coolant_reduction_pct: f64,
    pub cooling_effectiveness: f64,
    pub h_film_w_m2k: f64,
    pub h_eff_w_m2k: f64,
    pub boiling: String,
    pub chip_conduction_w_m2k: f64,
    pub contact_length_mm: f64,
    pub transient_factor: f64,
    pub barrier_interface: f64,
    pub barrier_tool_heat: f64,
    pub tool_body_c: f64,
    pub tool_body_rise_k: f64,
    pub tool_radial_growth_um: f64,
    pub engagement_ms: f64,
    pub workpiece_heat_share: f64,
    pub path_temp_mean_c: f64,
    pub path_temp_max_c: f64,
    pub path_temp_first_bin_c: f64,
    pub path_temp_last_bin_c: f64,
    pub wp_end_c: f64,
    /// 공구 몸체 온도를 시간에 따라 적분하는가 (현재는 정상상태 한 값)
    pub tool_body_time_resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditWear {
    pub vb_start_mm: f64,
    pub vb_end_mm: f64,
    pub vb_limit_mm: f64,
    pub job_cut_min: f64,
    /// 작업 전체 평균 VB 속도 (정상 + 길들이기)
    pub job_rate_um_min: f64,
    pub steady_um_min: f64,
    pub breakin_avg_um_min: f64,
    pub breakin_share: f64,
    pub breakin_vb_mm: f64,
    pub breakin_tau_min: f64,
    /// 이 작업이 끝난 시점에 길들이기 항이 얼마나 소진됐는가 (0~1)
    pub breakin_done: f64,
    pub kappa: f64,
    pub nominal_rate_um_min: f64,
    pub nominal_life_min: f64,
    pub nominal_linear_life_min: f64,
    pub path_factor: f64,
    pub path_life_min: f64,
    pub chatter_wear_job: f64,
    pub chatter_wear_nominal: f64,
    pub dominant: String,
    pub mechanisms: Vec<(String, f64)>,
    pub parts_left_p50: Option<f64>,
    pub parts_left_p90: Option<f64>,
    pub parts_source: String,
    /// 교체 개수를 작업 평균 VB 속도로 단순 환산 (길들이기 항을 정상 마모로 오해한 값)
    pub parts_naive_total_rate: f64,
    /// 정상 마모율만으로 (길들이기 VB 를 이미 소진했다고 보고) 환산
    pub parts_steady_rate_only: f64,
    /// 경로 물리 궤적이 말하는 남은 절삭 시간(분)과 부품당 절삭 시간(분)
    pub path_remaining_min: f64,
    pub part_cut_min: f64,
    /// 반경 마모(VB·tanα/(1−tanα·tanγ), `ToolGeometry::radial_per_vb`)가 공차를 다 쓰는 VB (mm). VB 한계보다 작으면 치수 기준 수명이 먼저 온다
    pub vb_dim_mm: f64,
    /// 치수 기준(VB_dim)으로 환산한 교체까지 부품 수 — 경로 물리 궤적 사용
    pub parts_by_dimension: Option<f64>,
    /// 경로 평균 재료 제거율 (cm³/min), 새 공구 경로 수명 동안 깎는 체적 (cm³), 그때의 비마모 (µm/cm³)
    pub mrr_cm3_min: f64,
    pub path_volume_cm3: f64,
    pub specific_wear_um_cm3: f64,
    /// 공구 수명 불확실성: 공구 간 편차 σ, 모델 오차 σ, 보수(10 % 분위) 배수 — `pairref::life_spread`
    #[serde(default)]
    pub life_sigma_scatter: f64,
    #[serde(default)]
    pub life_sigma_model: f64,
    #[serde(default)]
    pub parts_p10_factor: f64,
    #[serde(default)]
    pub parts_p10_scatter_only: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditDynamics {
    pub fn_hz: f64,
    pub k_n_per_um: f64,
    pub zeta: f64,
    pub m_eff_g: f64,
    pub holder_share: f64,
    pub tooth_hz: f64,
    pub fn_over_tooth: f64,
    pub kt_n_mm2: f64,
    pub ap_mm: f64,
    pub ap_lim_mm: f64,
    pub ap_crit_mm: f64,
    pub margin_executed: f64,
    pub margin_nominal: f64,
    pub margin_path_min: f64,
    pub chatter_fraction: f64,
    pub chatter_probability: f64,
    pub frf_measured: bool,
    pub process_damping_gain: f64,
    pub immersion_over_d: f64,
    pub zoa_reliable: bool,
    pub forced_sle_um: f64,
    pub forced_amplitude_um: f64,
    /// 채터(margin<1)가 벽면 오차 중앙값에 반영되는가 — 떨림 자국은 산-골 자국이라 중앙값이 아니라 벽 샘플의 파상도 기대값(`waviness_um`)과 Ra 로 따로 남긴다
    pub chatter_in_wall_error: bool,
    /// margin<1 일 때 떨림 자국 산-골 대역 (µm): W ≈ h_max·ρ (ρ = ap/ap_lim = 1/여유), 대역 0.5~2배, 상한 3·h_max.
    /// 아임계 분기라 한계에서 바로 유한 진폭으로 뛰고, 날이 절삭에서 이탈하면 h/(2|sin(ε/2)|) 부근에서 포화 (날 이탈 비선형·밀링 분기 정리:
    /// Honeycutt·Schmitz 2018, JMSE 140:120801 — '아임계·포화'는 리뷰의 직접 표현이 아니라 그 내용을 이 모델에 맞춘 해석, R8)
    pub chatter_waviness_low_um: f64,
    pub chatter_waviness_high_um: f64,
    /// 스핀들·베어링 1차 굽힘 모드 (추정 FRF 일 때 공구 모드와 2자유도 사슬로 결합, 실측 FRF 면 0)
    #[serde(default)]
    pub spindle_fn_hz: f64,
    #[serde(default)]
    pub spindle_k_n_per_um: f64,
    #[serde(default)]
    pub spindle_zeta: f64,
    /// 이 회전수의 채터 한계를 스핀들 모드가 정하는가 (채터 주파수가 스핀들 모드 쪽)
    #[serde(default)]
    pub spindle_dominates: bool,
    /// 정적 공구 끝 강성 (공구·홀더 + 스핀들 직렬, N/µm)
    #[serde(default)]
    pub static_k_n_per_um: f64,
    /// ZOA flip 로브 보정 배수 (1 = 보정 없음)
    #[serde(default)]
    pub flip_factor: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditDeflection {
    pub tool_tip_stiffness_n_per_um: f64,
    pub beam_compliance_um_per_n: f64,
    pub holder_compliance_um_per_n: f64,
    pub holder_share: f64,
    pub spindle_taper_compliance_modeled: bool,
    pub load_point_from_holder_mm: f64,
    pub wall_defl_nominal_um: f64,
    pub wall_defl_top_um: f64,
    pub wall_defl_bottom_um: f64,
    pub wall_mean_path_um: f64,
    pub wall_max_path_um: f64,
    pub fixture_um: f64,
    pub thermal_wp_um: f64,
    pub force_at_wall_over_mean: f64,
    /// 같은 실행 ae·ap 로 업밀링/다운밀링일 때 벽이 만들어지는 순간의 법선 힘 (N)과 공구 끝 휨 (µm).
    /// 업밀링은 진입(칩 0)에서, 다운밀링은 이탈에서 벽이 만들어지며 헬릭스 지연 때문에 그 순간에도 일부 슬라이스가 물려 있다.
    /// 업밀링은 접선·반경 성분이 상쇄되어 작고 다운밀링은 더해져 10배 가까이 커진다 (프로브 시험: 알루미늄 Ø6 ae 0.5 → 1.4 vs 14.9 µm)
    pub wall_up_force_n: f64,
    pub wall_up_um: f64,
    pub wall_down_force_n: f64,
    pub wall_down_um: f64,
    /// 이 경로에서 다운밀링으로 깎은 벽 구간의 비율 (0 = 전부 업밀링)
    pub path_down_share: f64,
    /// 가장 긴 날이 벽을 만드는 반경 런아웃 과삭 (µm, 음수=과삭) = −0.85·e (e = TIR/2) — 벽면 오차 값에 들어간다.
    /// 회전 상태 레이저로 공구 반경을 쟀으면(`radius_measured_rotating`) 0 (Kline & DeVor 1983 IJMTDR 23:123, Schmitz 외 2007 IJMTM 47:841)
    pub runout_overcut_um: f64,
    /// 스핀들(베어링·테이퍼) 정적 컴플라이언스 (µm/N)
    #[serde(default)]
    pub spindle_compliance_um_per_n: f64,
    /// 반경을 회전 상태로 측정해 런아웃 과삭이 상쇄되었는가
    #[serde(default)]
    pub runout_compensated: bool,
}

/// 측벽 표면조도 — 날 궤적 하한 포락선 모델 (`surface::wall_roughness`)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditSurface {
    /// 실행 ae·공칭 fz, 새 공구에서의 Ra 기대값 / 안정 절삭 Ra / Rz (µm)
    pub ra_um: f64,
    pub ra_stable_um: f64,
    pub rz_um: f64,
    /// 이송 자국만의 Rt = fz²/8R 와 런아웃까지 넣은 날 궤적 포락선 Rt (µm)
    pub rt_feed_um: f64,
    pub rt_envelope_um: f64,
    /// 표면을 실제로 만드는 날 수 / 공구 날 수
    pub forming_teeth: u32,
    pub flutes: u32,
    /// Ra 의 소재 바닥값·최소 칩 밀림 항 (µm)
    pub floor_um: f64,
    pub hmin_term_um: f64,
    pub bue_factor: f64,
    /// 경로 끝 VB 에서의 마모 배수
    pub wear_factor_end: f64,
    pub chatter_wpv_um: f64,
    pub chatter_probability: f64,
    /// 경로(최종 벽) Ra 평균·90 % 분위·최대, 파상도 기대값 최대 (µm)
    pub path_ra_mean_um: f64,
    pub path_ra_p90_um: f64,
    pub path_ra_max_um: f64,
    pub path_waviness_max_um: f64,
}

/// 아직 실측으로 확정되지 않은 모델 항목과, 그 값을 확정하는 가장 싼 측정 (실측 장비가 생겼을 때의 우선순위)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MeasureNext {
    /// 확정되는 모델 파라미터·출력
    pub target: String,
    /// 지금 값의 출처 ("모델 추정" / "문헌 대표값" 등)
    pub basis: String,
    pub measurement: String,
}

/// 공구 수명 감시 요약 (`toollife` — 수명 판정·교체·3차 마모·파손·부하 감시)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditToolLife {
    pub policy: String,
    pub policy_label: String,
    pub state: String,
    pub end_fraction: f64,
    pub vb_limit_mm: f64,
    pub vb_accel_mm: f64,
    pub vb_collapse_mm: f64,
    pub accel_mult: f64,
    pub accel_factor_max: f64,
    pub life_used_max: f64,
    pub life_used_end: f64,
    pub tools_used: u32,
    pub tool_changes: u32,
    pub change_step_um: f64,
    pub failed: bool,
    pub failure_cause: String,
    pub unfinished: bool,
    pub completed_fraction: f64,
    pub uncut_feed_mm: f64,
    /// 멈춘 작업에서 남은 경로가 다시 깎을 미절삭 경계 벽 점 수 (최종 벽·품질 보고에서 뺌, `loadsim::LoadSimReport::uncut_boundary_count`)
    #[serde(default)]
    pub uncut_boundary_points: usize,
    pub expired: bool,
    /// 몸체 강도의 기준 모재 (`toollife::strength_of` — 코팅이 아니라 초경 등급·HSS)
    #[serde(default)]
    pub body_label: String,
    pub trs_mpa: f64,
    pub static_mpa: f64,
    pub fatigue_mpa: f64,
    pub peak_stress_mpa: f64,
    pub peak_stress_ratio: f64,
    pub critical_section: String,
    pub fatigue_damage: f64,
    pub chip_probability: f64,
    pub chip_factors: crate::toollife::ChipFactors,
    pub load_ratio_max: f64,
    pub load_ratio_at_life_end: Option<f64>,
    pub load_alarm_t_s: Option<f64>,
    /// 부하 감시가 치핑을 잡아 정지·교체한 횟수
    #[serde(default)]
    pub chip_detected: u32,
    pub events: Vec<String>,
    /// 손상 공구(수명 한계 넘김 · 치핑 위험 · 3차 마모)로 깎은 최종 벽 (`loadsim::quality_loss`)
    #[serde(default)]
    pub final_points: usize,
    #[serde(default)]
    pub degraded_points: usize,
    #[serde(default)]
    pub degraded_wall_max_um: f64,
    #[serde(default)]
    pub degraded_ra_max_um: f64,
    /// 수명 한계 수준을 넘는 반경 마모 최대 (µm) — 손상 공구 탓의 치수 손실
    #[serde(default)]
    pub degraded_extra_wear_um: f64,
    #[serde(default)]
    pub good_points: usize,
    #[serde(default)]
    pub good_wall_p90_um: f64,
    #[serde(default)]
    pub good_ra_mean_um: f64,
    /// 구간마다 한 줄 (원인 · 공구 · 위치 · 벽 오차 · Ra)
    #[serde(default)]
    pub degraded_zones: Vec<String>,
}

/// 측정 없이는 정할 수 없는 값 — 쓴 값, 그럴 법한 범위, 그것이 바꾸는 결과, 가장 싼 측정
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnknownItem {
    pub key: String,
    pub used: String,
    pub range: String,
    pub effect: String,
    pub resolve: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SimAudit {
    pub version: u32,
    pub inputs: AuditInputs,
    pub engagement: AuditEngagement,
    pub force: AuditForce,
    pub thermal: AuditThermal,
    pub wear: AuditWear,
    pub dynamics: AuditDynamics,
    pub deflection: AuditDeflection,
    #[serde(default)]
    pub surface: AuditSurface,
    pub flags: Vec<AuditFlag>,
    pub measure_next: Vec<MeasureNext>,
    /// 이 작업에 쓴 공구–피삭재 조합 기준값과 근거 (`pairref`)
    #[serde(default)]
    pub pair: crate::pairref::PairSummary,
    /// 공구 수명 감시 (수명 판정·교체·3차 마모·파손·부하 감시)
    #[serde(default)]
    pub tool_life: AuditToolLife,
    /// 공구 형상·날끝·모재의 용도 적합도 (`appfit`, 측면 가공 기준)
    #[serde(default)]
    pub fit: crate::appfit::AppFit,
    /// 측정 없이는 정할 수 없는 값과 그 영향 (`[UNKNOWN]` 줄)
    #[serde(default)]
    pub unknowns: Vec<UnknownItem>,
}

fn f(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

fn force_view(ctx: &CutContext, co: &physics::CuttingCoeffs, fz: f64, rpm: f64, ap: f64, ae: f64, feed_mm_min: f64) -> ForceView {
    let d = ctx.tool.diameter_mm;
    let eng = Engagement::side(ap, ae.clamp(1e-6, d), d, false);
    let r = physics::mechanistic_forces(&ctx.tool, co, fz, rpm, &eng, 72, 12);
    let mrr = ae * ap * feed_mm_min / 60.0;
    ForceView {
        ae_mm: f(ae),
        engage_deg: f(eng.span().to_degrees()),
        f_mean_n: f(r.f_res_mean),
        f_peak_n: f(r.f_res_peak),
        peak_over_mean: f(r.f_res_peak / r.f_res_mean.max(1e-9)),
        power_kw: f(r.cutting_power_kw),
        specific_energy_j_mm3: f(r.cutting_power_kw * 1000.0 / mrr.max(1e-9)),
        h_mean_um: f(r.h_mean_mm * 1000.0),
        h_max_um: f(r.h_max_mm * 1000.0),
        teeth_in_cut: f(r.teeth_in_cut),
        edge_share: f(r.rubbing_share),
        flank_share_radial: f(r.flank_share),
        ploughing_share: f(r.ploughing_share),
        fa_mean_n: f(r.fa_mean),
        fa_peak_n: f(r.fa_peak),
    }
}

fn nominal_view(nominal: &CutAnalysis, feed_mm_min: f64) -> ForceView {
    let fo = &nominal.forces;
    ForceView {
        ae_mm: f(nominal.ae_mm),
        engage_deg: f(nominal.engage_angle_deg),
        f_mean_n: f(fo.f_res_mean),
        f_peak_n: f(fo.f_res_peak),
        peak_over_mean: f(fo.f_res_peak / fo.f_res_mean.max(1e-9)),
        power_kw: f(fo.cutting_power_kw),
        specific_energy_j_mm3: f(fo.cutting_power_kw * 1000.0 / (nominal.ae_mm * nominal.ap_mm * feed_mm_min / 60.0).max(1e-9)),
        h_mean_um: f(fo.h_mean_mm * 1000.0),
        h_max_um: f(fo.h_max_mm * 1000.0),
        teeth_in_cut: f(fo.teeth_in_cut),
        edge_share: 0.0,
        flank_share_radial: 0.0,
        ploughing_share: f(nominal.ploughing_share),
        fa_mean_n: f(fo.fa_mean),
        fa_peak_n: f(fo.fa_peak),
    }
}

fn flag(flags: &mut Vec<AuditFlag>, id: &str, severity: u8, value: f64, expected: &str, message: String) {
    flags.push(AuditFlag {
        id: id.into(),
        severity,
        value: f(value),
        expected: expected.into(),
        message,
    });
}

/// 감사 결과를 만든다. `out` 이 없으면 보정 판정·교체 개수 항목은 비운다.
pub fn build(profile: &MachiningProfile, ctx: &CutContext, rep: &LoadSimReport, log: &WearLog, out: Option<&CompOutcome>, pattern_key: &str) -> SimAudit {
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let nominal = physics::analyze_cut(ctx, &profile.conditions, true, body);
    let c = &profile.conditions;
    let w = &profile.workpiece_setup;
    let t = &ctx.tool;
    let d = t.diameter_mm;
    let rpm = c.spindle_rpm as f64;
    let au = &rep.engage_audit;
    let mut flags: Vec<AuditFlag> = Vec::new();

    // ---- 입력 ----
    let inputs = AuditInputs {
        profile: profile.name.clone(),
        pattern: pattern_key.to_string(),
        material: w.effective_material().key(),
        stock_mm: [w.width_mm, w.height_mm, w.thickness_mm],
        allowance_mm: w.stock_allowance_mm,
        tolerance_um: ctx.tolerance_mm * 1000.0,
        d_mm: d,
        flutes: t.flutes,
        helix_deg: t.helix_deg,
        loc_mm: t.loc_mm,
        oal_mm: t.oal_mm,
        shank_mm: t.shank_mm,
        stickout_mm: t.stickout_mm,
        stickout_over_d: f(t.stickout_mm / d.max(1e-9)),
        rake_deg: t.rake_deg,
        clearance_deg: t.clearance_deg,
        edge_um: t.edge_radius_um,
        core_ratio: t.core_ratio,
        runout_um: t.runout_um,
        runout_ecc_um: t.runout_ecc_mm() * 1000.0,
        kr_floor: ctx.wp.kr_floor,
        coating: t.coating.name.clone(),
        coating_um: t.coating.thickness_um,
        rpm,
        vc_m_min: c.cutting_speed_m_min,
        feed_mm_min: c.feed_rate_mm_min,
        fz_mm: nominal.fz_mm,
        ap_mm: c.axial_doc_mm,
        ae_planned_mm: c.radial_doc_mm,
        holder_k_n_per_um: t.holder_stiffness_n_per_um,
        damping_ratio: t.damping_ratio,
        holder_mass_kg: t.holder_mass_kg,
        max_power_kw: ctx.machine.max_power_kw,
        frf_measured: t.measured_fn_hz.is_some() && t.measured_k_n_per_um.is_some(),
        coolant: ctx.coolant.method.label().to_string(),
        coolant_flow_l_min: ctx.coolant.flow_l_min,
        coolant_c: ctx.coolant.temperature_c,
        ambient_c: ctx.env.ambient_c,
        humidity_pct: ctx.env.humidity_pct,
        calibration: [ctx.calib.power, ctx.calib.wear, ctx.calib.deflection, ctx.calib.temperature],
    };

    // ---- 결합·격자 ----
    let ae_exec = if au.ae_model_mm > 1e-6 { au.ae_model_mm } else { c.radial_doc_mm };
    let ap_exec = if au.ap_mean_mm > 1e-6 { au.ap_mean_mm } else { nominal.ap_mm };
    // 볼 노즈는 선단 곡면 때문에 벽 근처에서 전체 깊이를 깎지 않는다(깎는 폭이 깊이마다 다름) — ae 기하 기대는 세우지 않고 길이·시간만 비교한다
    let ball_nose = t.nose == ToolNose::Ball;
    let (geo_ae, geo_len, geo_time) = if pattern_key == "rect_profile" {
        let ae = if ball_nose { None } else { Some(w.stock_allowance_mm.max(0.0).min(d / 2.0)) };
        let len = 2.0 * (w.width_mm + w.height_mm);
        (ae, Some(len), Some(len / (c.feed_rate_mm_min / 60.0).max(1e-9)))
    } else {
        (None, None, None)
    };
    let engagement = AuditEngagement {
        planned_ae_mm: c.radial_doc_mm,
        planned_ae_over_d: f(c.radial_doc_mm / d.max(1e-9)),
        executed_ae_mm: f(ae_exec),
        executed_ae_by_volume_mm: f(au.ae_volume_mm),
        executed_ae_over_d: f(ae_exec / d.max(1e-9)),
        executed_engage_deg: f(Engagement::side(ap_exec, ae_exec.clamp(1e-6, d), d, false).span().to_degrees()),
        geometry_ae_mm: geo_ae,
        geometry_cut_len_mm: geo_len,
        geometry_cut_time_s: geo_time,
        sim_cut_len_mm: f(au.cut_len_mm),
        sim_cut_time_s: f(au.cut_time_s),
        sim_removed_mm3: f(au.removed_mm3),
        total_time_s: f(rep.total_time_min * 60.0),
        cell_over_ae: f(au.cell_mm / ae_exec.max(1e-9)),
        grid: au.clone(),
    };
    if ball_nose && pattern_key == "rect_profile" {
        flag(
            &mut flags,
            "engage_geometry_ball_nose",
            0,
            ae_exec,
            "정보",
            format!(
                "볼 노즈는 선단 곡면 때문에 벽 근처에서 깎는 폭이 깊이마다 달라 ae 의 기하 기대값(스톡 여유 {:.2} mm)을 세우지 않았다 — 읽은 평균 ae {:.3} mm 는 격자가 깊이 방향으로 평균한 값. 길이·시간은 그대로 대조한다",
                w.stock_allowance_mm, ae_exec
            ),
        );
    }
    if let Some(g) = geo_ae {
        if g > 1e-9 && (ae_exec / g - 1.0).abs() > AE_GEOMETRY_TOL {
            flag(
                &mut flags,
                "engage_ae_vs_geometry",
                2,
                ae_exec / g,
                "1.00 ± 0.15",
                format!("경로가 실제로 깎는 ae 를 격자에서 {:.3} mm 로 읽었으나 기하 기대값은 {:.3} mm (비 {:.2}) — 결합각·힘·마모가 이 비율만큼 어긋남", ae_exec, g, ae_exec / g),
            );
        }
    }
    if let Some(gt) = geo_time {
        if gt > 1e-9 && (au.cut_time_s / gt - 1.0).abs() > TIME_GEOMETRY_TOL {
            flag(
                &mut flags,
                "engage_time_vs_geometry",
                2,
                au.cut_time_s / gt,
                "1.00 ± 0.06",
                format!("절삭 시간 {:.2} s 가 기하 기대 {:.2} s (둘레/이송) 와 다름 — 스텝을 '공중'으로 잘못 분류했거나 격자가 거칠다", au.cut_time_s, gt),
            );
        }
    }
    if au.ae_volume_mm > 1e-9 && (au.ae_model_mm / au.ae_volume_mm - 1.0).abs() > AE_READ_TOL {
        flag(
            &mut flags,
            "engage_read_mismatch",
            1,
            au.ae_model_mm / au.ae_volume_mm,
            "1.00 ± 0.12",
            format!("모델에 들어간 ae {:.3} mm 와 제거 체적으로 읽은 평균 ae {:.3} mm 가 다름 (윈도우 {:.2} mm / {:.0} 셀)", au.ae_model_mm, au.ae_volume_mm, au.window_len_mean_mm, au.window_cells_mean),
        );
    }
    if engagement.cell_over_ae > MAX_CELL_OVER_AE {
        flag(
            &mut flags,
            "engage_grid_coarse",
            if engagement.cell_over_ae > 2.0 * MAX_CELL_OVER_AE { 1 } else { 0 },
            engagement.cell_over_ae,
            "<= 3",
            format!(
                "격자 셀 {:.3} mm 가 실행 ae {:.3} mm 의 {:.1}배 (스톡 여유 {:.2} mm = {:.1}칸) — 경계 셀을 면적 가중으로 읽어 단위 시험은 셀/ae ≤ 3 에서 ±5% 이내지만 그보다 거칠면 ae·결합각 오차가 커진다",
                au.cell_mm, ae_exec, engagement.cell_over_ae, w.stock_allowance_mm, au.allowance_cells
            ),
        );
    }
    if au.steps_cutting_no_own_removal > 0 {
        flag(
            &mut flags,
            "engage_no_own_removal",
            1,
            au.no_own_removal_fraction,
            "0",
            format!("절삭으로 센 스텝 중 {:.1}% 가 그 스텝 자체의 제거량이 0 이었다 — 면적 가중 제거에서는 생기지 않아야 하는 값 (격자·스텝 설정 확인)", au.no_own_removal_fraction * 100.0),
        );
    }

    // ---- 힘 ----
    let co = &nominal.coeffs;
    let exec_view = force_view(ctx, co, nominal.fz_mm, rpm, ap_exec, ae_exec, c.feed_rate_mm_min);
    let bins = &rep.wear_log;
    let tw: f64 = bins.iter().map(|b| b.dt_s).sum::<f64>().max(1e-12);
    let path_mean_n = bins.iter().map(|b| b.force_n * b.dt_s).sum::<f64>() / tw;
    let path_peak_n = bins.iter().map(|b| b.force_peak_n).fold(0.0, f64::max);
    let path_temp_mean = bins.iter().map(|b| b.temp_c * b.dt_s).sum::<f64>() / tw;
    let force = AuditForce {
        nominal: nominal_view(&nominal, c.feed_rate_mm_min),
        executed: exec_view.clone(),
        path_mean_n: f(path_mean_n),
        path_peak_n: f(path_peak_n),
        kc11_eff: f(co.kc11),
        mc: f(co.mc),
        kr: f(co.kr),
        kte_n_mm: f(co.kte),
        kre_n_mm: f(co.kre),
        mu: f(co.mu),
        shear_deg: f(co.shear_angle_deg),
        softening: f(co.softening),
        strain_rate_per_s: f(co.strain_rate),
        axial_force_modeled: true,
        kac_ratio: f(co.kac_ratio),
        kae_n_mm: f(co.kae),
    };
    let se = exec_view.specific_energy_j_mm3;
    if se < SPECIFIC_ENERGY_BAND.0 || se > SPECIFIC_ENERGY_BAND.1 {
        // 얇은 칩(정삭·저절입)에서는 일반 범위 위로 올라가는 것이 크기 효과라 모델 오류 신호가 아니다
        let size_effect = se > SPECIFIC_ENERGY_BAND.1 && exec_view.h_mean_um < SMALL_CHIP_UM;
        flag(
            &mut flags,
            "force_specific_energy",
            if size_effect { 0 } else { 1 },
            se,
            "0.3 ~ 8 J/mm³",
            if size_effect {
                format!(
                    "비에너지 {:.1} J/mm³ 가 일반 범위(0.3~8)를 넘지만 평균 칩 두께가 {:.1} µm 로 얇다 — 크기 효과(Kienzle h^-mc)와 날끝 밀림 몫 {:.0}% 때문에 올라가는 값 (정삭·미세 절삭은 10 J/mm³ 이상이 보고됨)",
                    se,
                    exec_view.h_mean_um,
                    exec_view.edge_share * 100.0
                )
            } else {
                format!("비에너지 {:.2} J/mm³ 가 금속 절삭 일반 범위를 벗어남 (평균 칩 두께 {:.1} µm)", se, exec_view.h_mean_um)
            },
        );
    }
    if exec_view.peak_over_mean > PEAK_OVER_MEAN_INFO {
        flag(
            &mut flags,
            "force_peak_over_mean",
            0,
            exec_view.peak_over_mean,
            "< 5",
            format!("피크/평균 힘 {:.1} — 날이 한 개씩 짧게 물리는 단속 절삭, 평균 힘으로 휨을 읽으면 과소평가", exec_view.peak_over_mean),
        );
    }
    if path_mean_n > 1e-9 && exec_view.f_mean_n > 1e-9 && (path_mean_n / exec_view.f_mean_n - 1.0).abs() > 0.35 {
        flag(
            &mut flags,
            "force_path_vs_executed",
            0,
            path_mean_n / exec_view.f_mean_n,
            "1.0 안팎",
            format!("경로 평균 힘 {:.1} N 와 실행 ae 로 다시 돌린 평균 힘 {:.1} N 차이 — 코너·진입에서 ae 가 변한 구간이 길다", path_mean_n, exec_view.f_mean_n),
        );
    }

    // ---- 열 ----
    let vce = nominal.vc_effective_m_min;
    let eng_exec = Engagement::side(ap_exec, ae_exec.clamp(1e-6, d), d, false);
    let f_exec = physics::mechanistic_forces(t, co, nominal.fz_mm, rpm, &eng_exec, 72, 12);
    let th_exec = physics::thermal(ctx, co, &f_exec, vce, rpm, &eng_exec, body.mass_kg, body.area_m2);
    let tn = &nominal.thermal;
    let ambient = ctx.coolant.ambient_c();
    let thermal = AuditThermal {
        interface_dry_c: f(tn.interface_dry_c),
        interface_wet_c: f(tn.interface_c),
        executed_interface_c: f(th_exec.interface_c),
        coolant_reduction_pct: f(tn.coolant_reduction_pct),
        cooling_effectiveness: f(tn.cooling.effectiveness),
        h_film_w_m2k: f(tn.cooling.h_film),
        h_eff_w_m2k: f(tn.cooling.h_eff),
        boiling: tn.cooling.boiling_state.clone(),
        chip_conduction_w_m2k: f(tn.cooling.conduction),
        contact_length_mm: f(tn.cooling.contact_length_mm),
        transient_factor: f(tn.transient_factor),
        barrier_interface: f(tn.barrier.interface),
        barrier_tool_heat: f(tn.barrier.tool_heat),
        tool_body_c: f(tn.tool_body_c),
        tool_body_rise_k: f(tn.tool_body_c - ambient),
        tool_radial_growth_um: f(tn.tool_radial_growth_um),
        engagement_ms: f(tn.engagement_time_ms),
        workpiece_heat_share: f(tn.heat_to_workpiece_share),
        path_temp_mean_c: f(path_temp_mean),
        path_temp_max_c: f(rep.max_temp_c),
        path_temp_first_bin_c: f(bins.first().map(|b| b.temp_c).unwrap_or(0.0)),
        path_temp_last_bin_c: f(bins.last().map(|b| b.temp_c).unwrap_or(0.0)),
        wp_end_c: f(rep.wp_temp_end_c),
        tool_body_time_resolved: false,
    };
    if tn.cooling.effectiveness < 0.1 && ctx.coolant.is_liquid() {
        flag(
            &mut flags,
            "thermal_coolant_ineffective",
            0,
            tn.cooling.effectiveness,
            "정보",
            format!("{} 이지만 계면 냉각 효과 {:.0}% — 칩 전도({:.1e} W/m²K)가 막 대류({:.1e})보다 훨씬 커서 날끝 온도에는 거의 영향이 없고 공구 몸체 쪽만 식는다는 모델", ctx.coolant.method.label(), tn.cooling.effectiveness * 100.0, tn.cooling.conduction, tn.cooling.h_eff),
        );
    }
    if tn.interface_c - ambient > 100.0 && thermal.tool_body_rise_k < 20.0 {
        // 집중 열용량 추정(공구로 들어가는 열 η_t = 3~8% ÷ (대류 hA + 홀더 전도 G)): 플러드 1~5 K, 건식·공기 25~80 K — 플러드면 모델과 같은 자릿수
        let liquid = ctx.coolant.is_liquid();
        flag(
            &mut flags,
            "thermal_tool_body_flat",
            if liquid { 0 } else { 1 },
            thermal.tool_body_rise_k,
            if liquid { "집중 열용량 추정 1~5 K" } else { "집중 열용량 추정 25~80 K" },
            format!(
                "공구 몸체 온도 상승 {:.1} K 는 정상상태 한 값(시간 적분 없음)이다 — {}",
                thermal.tool_body_rise_k,
                if liquid { "플러드에서는 집중 열용량 추정(1~5 K)과 같은 자릿수라 열 변형 오차(0.1 µm 대)는 무시해도 된다" } else { "건식·공기·MQL 은 집중 열용량 추정(25~80 K)보다 낮을 수 있고 시상수가 6~30 s 라 짧은 작업은 정상상태에 못 미친다" }
            ),
        );
    }

    // ---- 마모 ----
    let tr = &nominal.trajectory;
    let job_min = rep.cut_time_min.max(1e-12);
    // 경로 중 공구를 바꿨으면 VB 성장 비율은 마지막 공구의 절삭 시간으로 센다 (`vb_start_mm`·`growth_*` 가 마지막 공구 값)
    let tool_min = if rep.tool_life.tool_changes > 0 { rep.tool_life.last_tool_cut_min.max(1e-12) } else { job_min };
    let growth_min = if rep.growth_cut_min > 1e-12 { rep.growth_cut_min } else { tool_min };
    let steady = rep.growth_calib_mm / growth_min * 1000.0;
    let breakin_avg = rep.growth_fixed_mm / growth_min * 1000.0;
    let total_growth = (rep.growth_calib_mm + rep.growth_fixed_mm).max(1e-12);
    let job_rate = (rep.vb_end_mm - rep.vb_start_mm) / tool_min * 1000.0;
    let age_end = rep.wear_age0_min + tool_min;
    let tau = tr.break_in_tau_min.max(1e-9);
    let limit = ctx.vb_limit_mm();
    let part_cut_min = rep.cut_time_min;
    let (path_factor, path_life, p50, p90, source, remaining) = match out {
        Some(o) => {
            let dr = &o.draft;
            let rem = dr.path_trajectory.minutes_between(o.vb_after_mm, dr.vb_limit_mm);
            (dr.path_factor, dr.path_trajectory.life_min, o.parts_left_p50, o.parts_left_p90, o.parts_source.clone(), rem)
        }
        None => (0.0, 0.0, None, None, String::new(), 0.0),
    };
    let naive = if job_rate > 1e-12 && part_cut_min > 1e-12 { (limit - rep.vb_end_mm).max(0.0) * 1000.0 / job_rate / part_cut_min } else { 0.0 };
    let steady_only = if steady > 1e-12 && part_cut_min > 1e-12 {
        (limit - rep.vb_end_mm.max(tr.break_in_vb_mm)).max(0.0) * 1000.0 / steady / part_cut_min
    } else {
        0.0
    };
    // 치수 기준 수명: 반경 마모 ΔR = VB·tan(여유각) 이 공차를 다 쓰는 VB. VB 한계(0.2/0.3 mm)보다 작으면 그쪽이 먼저 온다
    let tan_clear = t.radial_per_vb().max(1e-6);
    let vb_dim = ctx.tolerance_mm / tan_clear;
    let parts_dim = match out {
        Some(o) if vb_dim < limit && part_cut_min > 1e-9 => Some(o.draft.path_trajectory.minutes_between(o.vb_after_mm, vb_dim) / part_cut_min),
        _ => None,
    };
    let spread = crate::pairref::life_spread(ctx, rep.chatter_fraction, rep.min_chatter_margin, out.map(|o| o.draft.radial_source != "simulated").unwrap_or(false));
    let mrr_cm3_min = rep.removed_volume_cm3 / job_min;
    let path_volume = path_life * mrr_cm3_min;
    let specific_wear = if path_volume > 1e-9 { limit * 1000.0 / path_volume } else { 0.0 };
    let wear = AuditWear {
        vb_start_mm: f(rep.vb_start_mm),
        vb_end_mm: f(rep.vb_end_mm),
        vb_limit_mm: f(limit),
        job_cut_min: f(job_min),
        job_rate_um_min: f(job_rate),
        steady_um_min: f(steady),
        breakin_avg_um_min: f(breakin_avg),
        breakin_share: f(rep.growth_fixed_mm / total_growth),
        breakin_vb_mm: f(tr.break_in_vb_mm),
        breakin_tau_min: f(tr.break_in_tau_min),
        breakin_done: f(1.0 - (-age_end / tau).exp()),
        kappa: f(tr.kappa),
        nominal_rate_um_min: f(nominal.wear.vb_rate_mm_per_min * 1000.0),
        nominal_life_min: f(tr.life_min),
        nominal_linear_life_min: f(tr.linear_life_min),
        path_factor: f(path_factor),
        path_life_min: f(path_life),
        chatter_wear_job: f(rep.chatter_wear_factor),
        chatter_wear_nominal: f(nominal.wear.chatter_factor),
        dominant: nominal.wear.dominant.clone(),
        mechanisms: nominal.wear.mechanisms.iter().map(|m| (m.key.clone(), f(m.share))).collect(),
        parts_left_p50: p50,
        parts_left_p90: p90,
        parts_source: source,
        parts_naive_total_rate: f(naive),
        parts_steady_rate_only: f(steady_only),
        path_remaining_min: f(remaining),
        part_cut_min: f(part_cut_min),
        vb_dim_mm: f(vb_dim),
        parts_by_dimension: parts_dim.map(|v| f(v.floor())),
        mrr_cm3_min: f(mrr_cm3_min),
        path_volume_cm3: f(path_volume),
        specific_wear_um_cm3: f(specific_wear),
        life_sigma_scatter: f(spread.sigma_scatter),
        life_sigma_model: f(spread.sigma_model),
        parts_p10_factor: f(spread.p10_total),
        parts_p10_scatter_only: f(spread.p10_scatter_only),
    };
    if vb_dim < limit {
        flag(
            &mut flags,
            "wear_dimension_before_vb",
            1,
            vb_dim,
            &format!("VB 한계 {:.3} mm", limit),
            format!(
                "공차 {:.0} µm 를 반경 마모(VB·tanα/(1−tanα·tanγ), 여유 {:.0}° · 경사 {:.0}°)가 다 쓰는 VB 는 {:.3} mm 로 VB 한계 {:.2} mm 보다 작다 — 치수 기준으로는 교체까지 {}개 (VB 한계 기준 {})",
                ctx.tolerance_mm * 1000.0,
                t.clearance_deg,
                t.rake_deg,
                vb_dim,
                limit,
                parts_dim.map(|v| format!("{:.0}", v.floor())).unwrap_or_else(|| "-".into()),
                wear.parts_left_p50.map(|v| format!("{:.0}개", v)).unwrap_or_else(|| "-".into())
            ),
        );
    }
    // 알루미늄 Ø6~8 초경 엔드밀의 문헌 범위 (Grigoriev 외 2020 Coatings 10:1038 외 R3 조사): 정상 VB 속도 0.4~7 µm/min, VB 0.2 까지 30~200분,
    // 공구당 100~700 cm³, 비마모 0.1~2.3 µm/cm³. 측정은 대부분 건식 무코팅·작은 MRR 이고 VB 한계에 실제로 도달시키는 조건이라
    // 긴 수명(코팅·가벼운 정삭) 쪽은 자료가 드물다 — 그쪽으로 벗어난 값은 주의(노랑)까지만, 짧은 수명 쪽(10분 미만·400 µm/min 초과 등)은 신뢰불가(빨강)
    if ctx.wp.key == "aluminum" && d <= 8.5 && path_life > 0.0 {
        let band = |id: &str, v: f64, lo: f64, hi: f64, red_lo: f64, red_hi: f64, unit: &str, long_life_is_high: bool, flags: &mut Vec<AuditFlag>| {
            if v < lo || v > hi {
                let long_life_side = if long_life_is_high { v > hi } else { v < lo };
                flag(
                    flags,
                    id,
                    if v < red_lo || v > red_hi { 2 } else { 1 },
                    v,
                    &format!("{:.1} ~ {:.1} {}", lo, hi, unit),
                    format!(
                        "{:.2} {} 은 알루미늄 Ø≤8 mm 초경 엔드밀 문헌 범위 {:.1}~{:.1} {} 를 벗어난다 — {} (절대값이 아니라 자릿수 점검용)",
                        v,
                        unit,
                        lo,
                        hi,
                        unit,
                        if long_life_side {
                            "공구 수명을 길게 읽는 쪽: 문헌 시험은 대부분 건식·무코팅·저 MRR 로 VB 한계에 도달시키는 조건이라 이쪽은 자료가 드물다 (코팅·가벼운 정삭이면 가능하나 VB 계열 실측으로 확인)"
                        } else {
                            "공구 수명을 짧게 읽는 쪽: 같은 조건 문헌 실측보다 마모가 빠르다고 모델이 말한다"
                        }
                    ),
                );
            }
        };
        let never = f64::NEG_INFINITY;
        let always = f64::INFINITY;
        band("wear_band_steady_rate", steady, 0.4, 7.0, never, 15.0, "µm/min", false, &mut flags);
        band("wear_band_life_time", path_life, 30.0, 200.0, 10.0, always, "min", true, &mut flags);
        band("wear_band_volume", path_volume, 100.0, 700.0, 30.0, always, "cm³", true, &mut flags);
        band("wear_band_specific", specific_wear, 0.1, 2.3, never, 7.0, "µm/cm³", false, &mut flags);
    }
    if wear.breakin_share > BREAKIN_SHARE_WARN {
        flag(
            &mut flags,
            "wear_breakin_dominates",
            1,
            wear.breakin_share,
            "< 0.5",
            format!(
                "이번 작업 VB 성장 {:.2} µm 중 {:.0}% 가 길들이기 항(VB {:.0} µm, τ {:.1} min) — 로그의 'steady {:.2} µm/min' 은 정상 마모율이 아니라 정상 {:.2} + 길들이기 {:.2} µm/min 의 합",
                (rep.vb_end_mm - rep.vb_start_mm) * 1000.0,
                wear.breakin_share * 100.0,
                tr.break_in_vb_mm * 1000.0,
                tr.break_in_tau_min,
                job_rate,
                steady,
                breakin_avg
            ),
        );
    }
    // 수명의 반경 절입 의존: 일정 칩 두께에서 실측 수명은 T ∝ ae^−1.0 (Ti-6Al-4V TiAlN, Tatar 외 2020 Heliyon 의 맞춤 R² 0.97 — 판정은
    // 노치 마모 0.2 mm. Liu 외 2019 는 제거 체적이 ae 의 2차 함수). 이 모델은 결합 시간 비율(작은 ae 에서 ≈ ae^0.5)만큼만 바뀐다 — 열이 지배하는 소재에서 기준 ae 와 많이 다르면 남긴다
    {
        use crate::pairref::WpGroup;
        let g = WpGroup::of(&ctx.wp);
        let ae_ref = (ctx.wp.ae_ratio * d).max(1e-6);
        let ratio = ae_exec / ae_ref;
        if matches!(g, WpGroup::Titanium | WpGroup::Nickel | WpGroup::Stainless | WpGroup::Hardened) && (ratio < 0.5 || ratio > 2.0) {
            let span = |ae: f64| Engagement::side(ap_exec, ae.clamp(1e-6, d), d, false).span().max(1e-9);
            let model = span(ae_ref) / span(ae_exec);
            let measured = 1.0 / ratio.max(1e-9);
            let gap = measured / model;
            flag(
                &mut flags,
                "wear_ae_scaling_gap",
                0,
                gap,
                "1.0 (실측 T ∝ ae^−1)",
                format!(
                    "실행 ae {:.2} mm 는 기준 상태 ae {:.2} mm 의 {:.2}배 — 일정 칩 두께에서 실측 수명은 ae^−1 로 {:.2}배가 되는데 이 모델은 결합 시간 비율로 {:.2}배만 바뀌어, 수명을 실측 경향보다 {:.1}배 {} 읽을 수 있다 (Ti-6Al-4V 실측 근거, 다른 소재는 미확인 — 공구 열 누적이 결합각에 따라 커지는 효과가 모델에 없음)",
                    ae_exec,
                    ae_ref,
                    ratio,
                    measured,
                    model,
                    if gap >= 1.0 { gap } else { 1.0 / gap },
                    if gap >= 1.0 { "짧게" } else { "길게" }
                ),
            );
        }
    }
    if wear.breakin_tau_min > BREAKIN_TAU_BAND.1 || wear.breakin_tau_min < BREAKIN_TAU_BAND.0 {
        flag(
            &mut flags,
            "wear_breakin_tau_assumed",
            0,
            wear.breakin_tau_min,
            "0.5 ~ 5 min (문헌)",
            format!(
                "길들이기 시상수 {:.1} min 가 초경 엔드밀 길들이기 문헌 범위(0.5~5분)를 벗어난다 (피삭재 기준값 {:.1} min, 선형 수명 {:.0} min) — 절삭 0.5·2·5·10분의 VB 를 재서 확정",
                wear.breakin_tau_min, ctx.wp.breakin_tau_min, tr.linear_life_min
            ),
        );
    }
    if let Some(n) = wear.parts_left_p50 {
        if naive > 1e-9 && n > PARTS_RATIO_WARN * naive {
            flag(
                &mut flags,
                "wear_parts_vs_job_rate",
                0,
                n / naive,
                "설명 필요",
                format!("교체까지 {:.0}개는 작업 평균 VB 속도로 환산한 {:.0}개의 {:.1}배 — 평균 속도에 길들이기 항이 섞여 있고 물리 궤적은 정상 마모율만으로 {:.0}분을 더 본다", n, naive, n / naive, remaining),
            );
        }
    }
    if rep.vb_start_mm <= 0.0 && wear.job_cut_min < 0.2 * wear.breakin_tau_min {
        flag(
            &mut flags,
            "wear_job_short_vs_breakin",
            0,
            wear.job_cut_min / wear.breakin_tau_min.max(1e-9),
            "정보",
            format!("한 작업({:.2} min)이 길들이기 시상수({:.1} min)의 {:.0}% 뿐 — 이 작업 하나의 VB 기울기로는 정상 마모를 구분할 수 없다", wear.job_cut_min, wear.breakin_tau_min, 100.0 * wear.job_cut_min / wear.breakin_tau_min.max(1e-9)),
        );
    }

    // ---- 동역학 ----
    let modal = crate::dynamics::tool_modal(t, ap_exec, ctx.calib.deflection);
    let st = crate::dynamics::stability(t, co, &eng_exec, f_exec.h_mean_mm, rpm, ap_exec, ctx.calib.deflection, ctx.machine.max_rpm, vce, ctx.wp.process_damping);
    let tooth_hz = rpm / 60.0 * t.flutes as f64;
    let immersion = ae_exec / d.max(1e-9);
    let dynamics = AuditDynamics {
        fn_hz: f(modal.fn_hz),
        k_n_per_um: f(modal.k_n_per_um),
        zeta: f(modal.zeta),
        m_eff_g: f(modal.m_eff_kg * 1000.0),
        holder_share: f(modal.holder_share),
        tooth_hz: f(tooth_hz),
        fn_over_tooth: f(modal.fn_hz / tooth_hz.max(1e-9)),
        kt_n_mm2: f(st.kt_n_mm2),
        ap_mm: f(ap_exec),
        ap_lim_mm: f(st.ap_lim_mm),
        ap_crit_mm: f(st.ap_crit_mm),
        margin_executed: f(st.margin),
        margin_nominal: f(nominal.stability.margin),
        margin_path_min: f(rep.min_chatter_margin),
        chatter_fraction: f(rep.chatter_fraction),
        chatter_probability: f(rep.chatter_probability),
        frf_measured: st.measured,
        process_damping_gain: f(st.process_damping_gain),
        immersion_over_d: f(immersion),
        zoa_reliable: immersion >= ZOA_MIN_IMMERSION,
        forced_sle_um: f(nominal.forced.sle_um),
        forced_amplitude_um: f(nominal.forced.amplitude_um),
        chatter_in_wall_error: false,
        chatter_waviness_low_um: if st.margin > 0.0 && st.margin < 1.0 { f((0.5 * exec_view.h_max_um / st.margin).min(3.0 * exec_view.h_max_um)) } else { 0.0 },
        chatter_waviness_high_um: if st.margin > 0.0 && st.margin < 1.0 { f((2.0 * exec_view.h_max_um / st.margin).min(3.0 * exec_view.h_max_um)) } else { 0.0 },
        spindle_fn_hz: f(modal.spindle_fn_hz),
        spindle_k_n_per_um: f(modal.spindle_k_n_per_um),
        spindle_zeta: f(modal.spindle_zeta),
        spindle_dominates: st.spindle_dominates,
        static_k_n_per_um: f(modal.static_k_n_per_um()),
        flip_factor: f(st.flip_factor),
    };
    if rep.min_chatter_margin > 0.0 && rep.min_chatter_margin < 1.0 {
        flag(
            &mut flags,
            "dynamics_chatter_not_in_wall",
            if rep.min_chatter_margin < 0.5 { 2 } else { 1 },
            rep.min_chatter_margin,
            ">= 1",
            format!(
                "채터 여유 최소 {:.2} (경로의 {:.0}% 가 한계 밖) — 벽면 오차 중앙값 {:.1} µm 는 안정 절삭 기준이고, 떨림 자국은 산-골 {:.1}~{:.1} µm (W ≈ h_max·ap/ap_lim, 대역 0.5~2배, 상한 3·h_max) 로 따로 본다. 경로 벽 샘플의 채터 확률 가중 파상도 최대 {:.1} µm · 측벽 Ra 최대 {:.2} µm 에 반영됨. 아임계 분기라 한계 바로 아래(여유 0.8~1.0)에서도 충격 한 번이면 자국이 날 수 있다",
                rep.min_chatter_margin,
                rep.chatter_fraction * 100.0,
                rep.mean_wall_error_um,
                dynamics.chatter_waviness_low_um,
                dynamics.chatter_waviness_high_um,
                rep.wall_waviness_max_um,
                rep.wall_ra_max_um
            ),
        );
    }
    if !dynamics.zoa_reliable && dynamics.margin_executed > 0.0 {
        // 같은 조건(Ø6 2날 35° 헬릭스 알루미늄, kr 0.2~0.45)의 반정수화(SD) 시험 계산은 ae/D 0.08~0.17 에서 ZOA 의 0.86~1.13배.
        // kr 0.35 부근 ae/D 0.08 에서만 ZOA 가 0.68~0.81 로 과대(특이 봉우리), 이 소재는 kr 하한을 0.40 으로 둬 그 구간을 피한다.
        // 헬릭스가 ap 를 날 피치 길이(πD/(N·tanβ))보다 얕게 만들면 flip 불안정도 없다.
        flag(
            &mut flags,
            "dynamics_zoa_low_immersion",
            0,
            immersion,
            ">= 0.25 이면 ZOA 신뢰",
            format!(
                "반경 절입 ae/D = {:.2} 로 작아 ZOA 안정 한계의 오차는 ±15% 안팎(반정수화 시험 계산 0.86~1.13배)이다 — 여유 {:.2} 는 방향성과 자릿수로 읽을 것 (날 통과 {:.0} Hz, 고유진동수 {:.0} Hz, 날 통과/고유 {:.2}; {})",
                immersion,
                dynamics.margin_executed,
                tooth_hz,
                modal.fn_hz,
                tooth_hz / modal.fn_hz.max(1e-9),
                if st.flip_factor < 0.999 {
                    format!("flip 로브 위치 2/(2k+1) 근처라 한계에 {:.2} 를 곱했다 — 주기 2배 채터는 날 하나 건너 자국을 남긴다", st.flip_factor)
                } else if tooth_hz / modal.fn_hz.max(1e-9) <= 0.3 {
                    "날 통과가 고유진동수의 0.3배 이하라 flip 로브 영역 밖".to_string()
                } else {
                    "flip 로브 위치(2, 0.67, 0.4)에서 벗어났거나 헬릭스가 ap 를 덮어 보정 없음".to_string()
                }
            ),
        );
    }
    // 감쇠비: 사용자가 넣지 않았으면 돌출비 기본값 ζ ≈ 0.28·(L/D)^−1.38 (실측 L/D 3.75 → 0.05, 5.8 → 0.026, 12 → 0.009).
    // 같은 공구라도 재클램프·스핀들 예열로 ±10 % 이상 바뀌고(NIST 반복 시험), 가공 중에는 +30~60 % — 한계 깊이는 ζ 에 비례한다
    if !ctx.machine.damping_is_user() {
        let ld = t.stickout_mm / d.max(1e-9);
        flag(
            &mut flags,
            "dynamics_zeta_default",
            0,
            modal.zeta,
            "탭 테스트",
            format!(
                "감쇠비 {:.3} 는 돌출비 L/D {:.1} 의 공구 끝 모드 기본값(실측 3건 맞춤, 홀더 {}) — 정지 상태 충격 시험 기준이라 가공 중 값보다 보수적이고, 공구마다 ±50 % 는 다를 수 있다. 탭 테스트로 확정",
                modal.zeta,
                ld,
                profile.endmill_setting.holder.label()
            ),
        );
    }
    if !dynamics.frf_measured {
        flag(
            &mut flags,
            "dynamics_frf_estimated",
            0,
            modal.fn_hz,
            "탭 테스트",
            format!(
                "고유진동수 {:.0} Hz · 강성 {:.2} N/µm · 감쇠비 {:.3} 는 공구·홀더 모델 추정값 (홀더 {:.0} N/µm 가정) + 스핀들 모드 {:.0} Hz · {:.0} N/µm · ζ {:.3} (기계 설정 기본값, 문헌 500~1500 Hz · 20~60 N/µm){} — 채터 여유의 로그 표준편차 0.4 로 취급",
                modal.fn_hz,
                modal.k_n_per_um,
                modal.zeta,
                t.holder_stiffness_n_per_um,
                modal.spindle_fn_hz,
                modal.spindle_k_n_per_um,
                modal.spindle_zeta,
                if st.spindle_dominates { ", 이 회전수의 채터 한계는 스핀들 모드가 정함 (채터 주파수가 스핀들 쪽)" } else if modal.spindle_dominates() { ", 스핀들 모드의 k·ζ 가 공구 모드보다 작음" } else { "" }
            ),
        );
    }

    // ---- 휨 ----
    let comp = physics::tool_compliance(t, ap_exec, ctx.calib.deflection);
    let total_c = 1.0 / comp.stiffness_n_per_um.max(1e-9);
    let holder_c = ctx.calib.deflection / t.holder_stiffness_n_per_um.max(1e-9);
    let spindle_c = ctx.calib.deflection / t.spindle_stiffness_n_per_um.max(1e-9);
    let force_at_wall = rep.max_wall_defl_um.max(1e-12);
    // 벽이 만들어지는 순간의 법선 힘: 업밀링과 다운밀링을 같은 실행 ae·ap 로 나란히 계산한다
    let eng_up = Engagement::side(ap_exec, ae_exec.clamp(1e-6, d), d, false);
    let eng_dn = Engagement::side(ap_exec, ae_exec.clamp(1e-6, d), d, true);
    let f_up = physics::mechanistic_forces(t, co, nominal.fz_mm, rpm, &eng_up, 48, 10);
    let f_dn = physics::mechanistic_forces(t, co, nominal.fz_mm, rpm, &eng_dn, 48, 10);
    let (wall_up_um, wall_up_n) = crate::loadsim::wall_deflection(t, ctx.calib.deflection, &f_up, &eng_up).unwrap_or((0.0, 0.0));
    let (wall_dn_um, wall_dn_n) = crate::loadsim::wall_deflection(t, ctx.calib.deflection, &f_dn, &eng_dn).unwrap_or((0.0, 0.0));
    let (down_n, all_n) = rep.seg_walls.iter().fold((0.0f64, 0.0f64), |a, s| (a.0 + if s.down { s.n as f64 } else { 0.0 }, a.1 + s.n as f64));
    let down_share = if all_n > 0.0 { down_n / all_n } else { 0.0 };
    let deflection = AuditDeflection {
        tool_tip_stiffness_n_per_um: f(comp.stiffness_n_per_um),
        beam_compliance_um_per_n: f((total_c - holder_c - spindle_c).max(0.0)),
        holder_compliance_um_per_n: f(holder_c),
        holder_share: f((holder_c + spindle_c) / total_c.max(1e-12)),
        spindle_taper_compliance_modeled: true,
        load_point_from_holder_mm: f(comp.load_point_from_holder_mm),
        wall_defl_nominal_um: f(nominal.wall.deflection_um),
        wall_defl_top_um: f(nominal.wall.deflection_top_um),
        wall_defl_bottom_um: f(nominal.wall.deflection_bottom_um),
        wall_mean_path_um: f(rep.mean_wall_error_um),
        wall_max_path_um: f(rep.max_wall_error_um),
        fixture_um: f(nominal.fixture_deflection_um),
        thermal_wp_um: f(nominal.error_budget_um.thermal_workpiece_um),
        force_at_wall_over_mean: f(force_at_wall / (path_mean_n.max(1e-9) * total_c)),
        wall_up_force_n: f(wall_up_n),
        wall_up_um: f(wall_up_um),
        wall_down_force_n: f(wall_dn_n),
        wall_down_um: f(wall_dn_um),
        path_down_share: f(down_share),
        runout_overcut_um: f(-t.runout_overcut_um()),
        spindle_compliance_um_per_n: f(spindle_c),
        runout_compensated: t.runout_compensated,
    };
    {
        let (lo, hi) = (wall_up_um.abs().min(wall_dn_um.abs()), wall_up_um.abs().max(wall_dn_um.abs()));
        if hi > 2.0 && hi > 3.0 * lo.max(0.3) {
            flag(
                &mut flags,
                "deflection_mode_sensitivity",
                0,
                hi / lo.max(0.3),
                "정보",
                format!(
                    "이 경로는 다운밀링 구간 {:.0}% (나머지 업밀링). 같은 ae {:.2} mm 라도 벽 순간 법선 힘이 업밀링 {:.1} N → 휨 {:.1} µm, 다운밀링 {:.1} N → {:.1} µm 로 절삭 방향이 벽 휨을 {:.0}배 가른다 (업밀링은 진입에서 접선·반경 성분이 상쇄, 다운밀링은 이탈에서 더해짐)",
                    down_share * 100.0,
                    ae_exec,
                    wall_up_n,
                    wall_up_um,
                    wall_dn_n,
                    wall_dn_um,
                    hi / lo.max(0.3)
                ),
            );
        }
        if t.runout_um > 0.5 {
            flag(
                &mut flags,
                "deflection_runout_overcut",
                if !t.runout_compensated && t.runout_overcut_um() > 0.25 * ctx.tolerance_mm * 1000.0 { 1 } else { 0 },
                deflection.runout_overcut_um,
                "정보",
                if t.runout_compensated {
                    format!(
                        "런아웃 TIR {:.1} µm — 공구 반경을 회전 상태(레이저)로 재서 긴 날 반경이 이미 보정값에 들어 있어 과삭 0 으로 뒀다 (정지 측정이면 약 {:.1} µm 과삭, 공구 교체마다 테이퍼·스핀들 반복성 1~3 µm 는 남음)",
                        t.runout_um,
                        crate::surface::RUNOUT_EFFECTIVE * t.runout_ecc_mm() * 1000.0
                    )
                } else {
                    format!(
                        "런아웃 TIR {:.1} µm 면 가장 긴 날이 벽을 {:.1} µm 과삭한다 (0.85·e, e = TIR/2, 긴 날이 더 휘어 e 의 0.7~1.0) — 벽면 오차·2차 보정 바이어스에 넣었다. 공구 반경을 회전 상태 레이저로 재면(공구 설정 '반경 회전 측정') 0 이 된다",
                        t.runout_um,
                        t.runout_overcut_um()
                    )
                },
            );
        }
    }
    if deflection.holder_share < 0.05 {
        flag(
            &mut flags,
            "deflection_holder_negligible",
            0,
            deflection.holder_share,
            "정보",
            format!("공구 끝 컴플라이언스 {:.3} µm/N 중 홀더({:.0} N/µm)·스핀들({:.0} N/µm) 몫 {:.0}% — 공구 보 굽힘이 휨을 거의 다 정한다", total_c, t.holder_stiffness_n_per_um, t.spindle_stiffness_n_per_um, deflection.holder_share * 100.0),
        );
    }
    // 얇은 벽 항복: 벽에 걸린 힘이 뿌리를 항복시킨 최종 벽 — 벽 오차에는 전소성 하중까지의 탄성 복원만 들어간다 (`loadsim::wall_springback_um`)
    {
        let sigma_y = ctx.wp.flow_stress(0.0, 1.0, rep.bulk_c).max(1.0);
        let thin: Vec<&crate::loadsim::WallErrorSample> = rep.wall_errors.iter().filter(|w| w.final_wall && w.wall_k_n_mm > 0.0).collect();
        let worst = thin.iter().map(|w| w.wall_stress_mpa).fold(0.0, f64::max);
        if worst > sigma_y {
            let n = thin.iter().filter(|w| w.wall_stress_mpa > sigma_y).count();
            flag(
                &mut flags,
                "thin_wall_yield",
                1,
                worst / sigma_y,
                "< 1 (탄성)",
                format!(
                    "최종 얇은 벽 {}점에서 벽 뿌리 굽힘 응력이 항복 응력 {:.0} MPa 의 최대 {:.1}배 — 그 벽은 영구 변형(휨·꺾임)돼 치수 오차가 아니라 불량이고, 벽 오차에는 전소성 하중(1.5·σy)까지의 탄성 복원만 들어갔다. 벽 쪽 절입을 줄이고 단계 절입·지지를 남기는 순서로 바꿀 것",
                    n,
                    sigma_y,
                    worst / sigma_y
                ),
            );
        }
    }

    // ---- 표면조도 ----
    let group = crate::pairref::WpGroup::of(&ctx.wp);
    let chat_p_exec = crate::toolwear::chatter_probability(st.margin, st.measured);
    let rough = crate::surface::wall_roughness(
        0.5 * d,
        t.flutes,
        nominal.fz_mm,
        t.runout_ecc_mm(),
        co.h_min_mm,
        exec_view.h_max_um / 1000.0,
        0.0,
        group,
        nominal.thermal.bue_risk,
        st.margin,
        chat_p_exec,
    );
    let surface = AuditSurface {
        ra_um: f(rough.ra_um),
        ra_stable_um: f(rough.ra_stable_um),
        rz_um: f(rough.rz_um),
        rt_feed_um: f(rough.rt_feed_um),
        rt_envelope_um: f(rough.rt_envelope_um),
        forming_teeth: rough.forming_teeth,
        flutes: t.flutes,
        floor_um: f(rough.floor_um),
        hmin_term_um: f(rough.hmin_term_um),
        bue_factor: f(rough.bue_factor),
        wear_factor_end: f(crate::surface::wear_factor(group, rep.vb_end_mm)),
        chatter_wpv_um: f(rough.chatter_wpv_um),
        chatter_probability: f(rough.chatter_probability),
        path_ra_mean_um: f(rep.wall_ra_mean_um),
        path_ra_p90_um: f(rep.wall_ra_p90_um),
        path_ra_max_um: f(rep.wall_ra_max_um),
        path_waviness_max_um: f(rep.wall_waviness_max_um),
    };
    if t.flutes > 1 && rough.forming_teeth < t.flutes {
        flag(
            &mut flags,
            "surface_runout_single_tooth",
            0,
            rough.forming_teeth as f64,
            "날 수",
            format!(
                "런아웃 편심 {:.1} µm 가 이송 자국 깊이(fz²/8R = {:.3} µm)보다 커서 {}날 중 {}날만 최종 벽을 만든다 — 자국 간격이 날당이 아니라 회전당 이송이 되어 Rt {:.2} µm (런아웃 없으면 {:.3} µm). 실측 정삭 조도가 기하 이송 자국의 수십 배인 주된 이유 (Schmitz 외 2007, Wojciechowski 2011)",
                t.runout_ecc_mm() * 1000.0,
                rough.rt_feed_um,
                t.flutes,
                rough.forming_teeth,
                rough.rt_envelope_um,
                rough.rt_feed_um
            ),
        );
    }
    if surface.path_ra_max_um > 1.5 * surface.ra_um.max(1e-6) && surface.path_ra_max_um > 0.4 {
        flag(
            &mut flags,
            "surface_ra_rises_along_path",
            0,
            surface.path_ra_max_um / surface.ra_um.max(1e-6),
            "< 1.5",
            format!(
                "경로 최종 벽 Ra 평균 {:.2} · 90 % {:.2} · 최대 {:.2} µm 로 새 공구 공칭 {:.2} µm 의 {:.1}배 — 마모(경로 끝 배수 {:.2})·채터(파상도 기대 최대 {:.1} µm)·코너 이송 변화가 원인",
                surface.path_ra_mean_um,
                surface.path_ra_p90_um,
                surface.path_ra_max_um,
                surface.ra_um,
                surface.path_ra_max_um / surface.ra_um.max(1e-6),
                surface.wear_factor_end,
                surface.path_waviness_max_um
            ),
        );
    }
    // 기계(스핀들·주축대) 열변위: 예열·열보정이 된 기계라 XY 벽 오차에는 넣지 않았다 (실내 온도 변동 드리프트만 반영)
    flag(
        &mut flags,
        "machine_thermal_warmup_excluded",
        0,
        0.0,
        "예열 후 가공",
        "스핀들·주축대 열변위는 넣지 않았다 — 예열·열보정된 기계의 XY 잔여 2~5 µm (Z 3~10 µm). 냉간 시동 직후면 XY 3~15 µm · Z 15~50 µm 가 시상수 20~40 분으로 쌓이므로 정밀 벽은 20~30 분 예열 후 가공 (ISO 230-3 시험법, Uhlmann 외 2022 euspen 등 여러 열변위 자료를 모은 범위)".to_string(),
    );

    // ---- 공칭 vs 경로 조건 차이 ----
    if c.radial_doc_mm > 1e-9 && (ae_exec / c.radial_doc_mm - 1.0).abs() > 0.3 {
        flag(
            &mut flags,
            "nominal_vs_path_regime",
            0,
            ae_exec / c.radial_doc_mm,
            "정보",
            format!(
                "공칭 해석은 프리셋 ae {:.2} mm (결합 {:.0}°)로, 경로는 실제 ae {:.2} mm (결합 {:.0}°)로 깎음 — 공칭 수명 {:.0}분·채터 여유 {:.2} 와 경로 값은 서로 다른 조건의 값",
                c.radial_doc_mm, nominal.engage_angle_deg, ae_exec, engagement.executed_engage_deg, tr.life_min, nominal.stability.margin
            ),
        );
    }
    let _ = log;

    // ---- 다음 실측 ----
    let mut measure_next: Vec<MeasureNext> = Vec::new();
    let mut want = |target: &str, basis: &str, measurement: &str| {
        measure_next.push(MeasureNext { target: target.into(), basis: basis.into(), measurement: measurement.into() });
    };
    if !dynamics.frf_measured {
        want(
            "채터 여유·안정 로브 (fn, 강성, 감쇠비)",
            "공구·홀더 모델 추정 (홀더 스프링 + 집중질량, ζ 가정)",
            "탭 테스트(충격 해머 + 가속도계)로 공구 끝 FRF 실측 → 앱 '탭 테스트' 입력. 돌출 길이 1~2 가지로 반복하면 돌출 스케일링도 확인됨",
        );
    }
    want(
        "비절삭저항·날끝 힘 (kc11, mc, kte, kre, kr) · 평균/피크 힘",
        "문헌 대표값 + 마찰각 모델",
        "공구동력계(또는 스핀들 동력 로그)로 같은 조건의 평균 힘·동력 실측. 칩 두께를 3점(fz 변경) 바꾸면 kc11·mc 와 날끝 항이 분리됨",
    );
    want(
        "플랭크 마모 VB 성장 (κ, 길들이기 VB_b·τ, 코팅 효과)",
        "기준 상태 정규화 + 조합 기준 길들이기 (크기 µm·시상수 분, 실측이 적은 소재는 가정값)",
        "절삭 0.5·2·5·10분마다 공구 현미경으로 VB 측정 (휴대폰 파노라마 + 마스크도 가능) → 길들이기와 정상 마모를 분리해 입력",
    );
    want(
        "날끝/칩 온도 (thermal_factor, 냉각 효과)",
        "Loewen–Shaw 형 해석식 × 냉각 유효도",
        "IR 카메라(칩 배출 쪽) 또는 공구-칩 열전대로 건식/플러드 한 쌍 측정. 계면 온도가 해석식의 절반 이하면 thermal_factor 를 낮춤",
    );
    want(
        "벽면 치수 오차 분해 (휨 · 마모 · 열 · 런아웃 · 채터)",
        "정적 휨(스핀들 포함) + 반경 마모 + 열팽창 + 런아웃 과삭 0.85·e 합 (채터 떨림 자국은 파상도로 따로)",
        "CMM 또는 마이크로미터로 1차 벽 높이별 3점 측정, 같은 조건 안정(ap 절반) 가공과 비교하면 채터 몫이 분리됨. 다이얼 게이지로 공구 끝 TIR 을 회전 측정하면 런아웃 몫이 분리됨",
    );
    want(
        "측벽 조도 Ra·Rz (소재 바닥값, 최소 칩 밀림, 마모 배수)",
        "날 궤적 포락선 + 문헌 바닥값 (정삭 실측 7075 0.10~0.15 · 강 0.15~0.2 · SUS 0.3 µm)",
        "촉침식 조도계로 벽 3곳 이송 방향 측정 (λc 0.8 mm). 새 공구와 경로 끝 공구로 각각 재면 마모 배수가 분리됨",
    );
    if wear.breakin_share > BREAKIN_SHARE_WARN {
        want(
            "교체까지 개수",
            "물리 궤적(정상 마모율만으로 환산) — 한 작업이 길들이기 시상수보다 훨씬 짧음",
            "동일 조건으로 누적 10~30분 가공 후 VB 를 한 번 더 측정하면 정상 마모율이 직접 나옴 (SDS 원장에 '실측 VB' 로 입력)",
        );
    }
    drop(want);

    // ---- 조합 기준값 ----
    let pair = crate::pairref::summary(&profile.endmill_setting, ctx, nominal.vc_effective_m_min);
    if !pair.low_confidence.is_empty() {
        flag(
            &mut flags,
            "pair_reference_low_confidence",
            0,
            pair.low_confidence.len() as f64,
            "정보",
            format!("이 조합({} × {})의 기준값 중 실측 근거가 약한 항목: {}", pair.wp_group, pair.coat_group, pair.low_confidence.join(" / ")),
        );
    }

    // ---- 공구 수명 감시 ----
    let tlr = &rep.tool_life;
    let tool_life = AuditToolLife {
        policy: tlr.policy.key().into(),
        policy_label: crate::toollife::policy_label(tlr.policy).into(),
        state: tlr.state.clone(),
        end_fraction: f(tlr.end_fraction),
        vb_limit_mm: f(tlr.vb_limit_mm),
        vb_accel_mm: f(tlr.params.vb_accel_mm),
        vb_collapse_mm: f(tlr.params.vb_collapse_mm),
        accel_mult: f(tlr.params.accel_mult),
        accel_factor_max: f(tlr.accel_factor_max),
        life_used_max: f(tlr.life_used_max),
        life_used_end: f(tlr.life_used_end),
        tools_used: tlr.tools_used,
        tool_changes: tlr.tool_changes,
        change_step_um: f(crate::toollife::change_step_um(t)),
        failed: tlr.failed,
        failure_cause: tlr.failure_cause.clone(),
        unfinished: tlr.unfinished,
        completed_fraction: f(tlr.completed_fraction()),
        uncut_feed_mm: f(tlr.uncut_feed_mm),
        uncut_boundary_points: rep.uncut_boundary_count,
        expired: tlr.expired,
        body_label: tlr.strength.label.clone(),
        trs_mpa: f(tlr.strength.trs_mpa),
        static_mpa: f(tlr.strength.static_mpa()),
        fatigue_mpa: f(tlr.strength.fatigue_mpa()),
        peak_stress_mpa: f(tlr.peak_stress_mpa),
        peak_stress_ratio: f(tlr.peak_stress_ratio),
        critical_section: tlr.critical_section.clone(),
        fatigue_damage: f(tlr.fatigue_damage),
        chip_probability: f(tlr.chip_probability),
        chip_factors: tlr.chip_factors_mean,
        load_ratio_max: f(tlr.load_ratio_max),
        load_ratio_at_life_end: tlr.load_ratio_at_life_end.map(f),
        load_alarm_t_s: tlr.load_alarm_t_s.map(f),
        chip_detected: tlr.chip_detected,
        events: tlr.events.iter().map(|e| format!("{} #{} t {:.1} s (절삭 {:.1} s) seg {} — {}", e.kind, e.tool_no, e.t_s, e.t_cut_s, e.seg, e.message)).collect(),
        final_points: rep.quality_loss.final_points,
        degraded_points: rep.quality_loss.degraded_points,
        degraded_wall_max_um: f(rep.quality_loss.worst_wall_um),
        degraded_ra_max_um: f(rep.quality_loss.worst_ra_um),
        degraded_extra_wear_um: f(rep.quality_loss.worst_extra_wear_um),
        good_points: rep.quality_loss.good_points,
        good_wall_p90_um: f(rep.quality_loss.good_wall_p90_um),
        good_ra_mean_um: f(rep.quality_loss.good_ra_mean_um),
        degraded_zones: rep
            .quality_loss
            .zones
            .iter()
            .map(|z| {
                format!(
                    "{} · {} 쪽 벽 · 공구 #{} · seg {}–{} · t {:.1}–{:.1} s · X {:.1}~{:.1} Y {:.1}~{:.1} Z {:.2}~{:.2} mm · 벽 {:.1} mm · 수명 {:.0}% · 치핑 P {:.0}% · 한계 넘어 덜 깎임 +{:.1} µm (반경 마모 {:.1}) · Ra {:.2}/{:.2} µm (새 날 {:.2} · ×{:.2}) · 벽 위치 오차(전체) {:.1} µm",
                    z.label,
                    z.wall_dir,
                    z.tool_no,
                    z.seg_from,
                    z.seg_to,
                    z.t_from_s,
                    z.t_to_s,
                    z.x_min,
                    z.x_max,
                    z.y_min,
                    z.y_max,
                    z.z_min,
                    z.z_max,
                    z.len_mm,
                    z.life_used_max * 100.0,
                    z.chip_p_max * 100.0,
                    z.extra_wear_um,
                    z.wear_um_max,
                    z.ra_mean_um,
                    z.ra_max_um,
                    z.ra_fresh_mean_um,
                    z.ra_ratio,
                    z.wall_err_max_um
                )
            })
            .collect(),
    };
    let ql = &rep.quality_loss;
    if ql.degraded_points > 0 {
        flag(
            &mut flags,
            "quality_degraded",
            if ql.zones.iter().any(|z| z.severity >= 2) { 2 } else { 1 },
            ql.degraded_fraction,
            "0",
            format!(
                "최종 벽 {}점 중 {}점({:.0}%)을 손상 공구로 깎음 — 벽 {}곳 · 한계 넘어 덜 깎임 최대 +{:.1} µm · Ra 최대 {:.2} µm{}. 그 구간을 확대경·조도계로 확인",
                ql.final_points,
                ql.degraded_points,
                ql.degraded_fraction * 100.0,
                ql.zones.len(),
                ql.worst_extra_wear_um,
                ql.worst_ra_um,
                if ql.good_points > 0 { format!(" (정상 공구로 깎은 벽 평균 {:.2})", ql.good_ra_mean_um) } else { " (정상 공구로 깎은 최종 벽 없음)".to_string() }
            ),
        );
    }
    // 멈춘 작업의 마지막 벽 중 남은 경로가 다시 깎을 곳 — 부품 벽이 아니라 최종 벽 통계·품질 보고에서 뺐다
    let uncut_note = if rep.uncut_boundary_count > 0 {
        format!(" · 남은 경로가 다시 깎을 미절삭 경계 벽 {}점은 최종 벽·품질 보고에서 뺌", rep.uncut_boundary_count)
    } else {
        String::new()
    };
    if tlr.failed {
        flag(&mut flags, "tool_failure", 2, tlr.completed_fraction(), "파손 없음", format!("{} — 경로의 {:.0}% 에서 정지, 부품 검사 필요{}", tlr.stop_reason().unwrap_or_default(), tlr.completed_fraction() * 100.0, uncut_note));
    } else if tlr.unfinished {
        let chipped = tlr.events.iter().rev().find(|e| e.kind == "stop").map(|e| e.cause == "chipping").unwrap_or(false);
        flag(
            &mut flags,
            "path_unfinished",
            1,
            tlr.completed_fraction(),
            "1.0",
            format!(
                "{} 이송 경로 {:.0} / {:.0} mm 에서 정지 ({}) — 이 조건으로 이 경로를 끝내려면 공구 {}개 이상 또는 조건 완화 필요{}",
                if chipped { "치핑 감지로" } else { "공구 수명 판정으로" },
                tlr.total_feed_mm - tlr.uncut_feed_mm,
                tlr.total_feed_mm,
                tool_life.policy_label,
                (1.0 / tlr.completed_fraction().max(1e-3)).ceil(),
                uncut_note
            ),
        );
    }
    if tlr.tool_changes > 0 {
        flag(
            &mut flags,
            "tool_changed_mid_path",
            1,
            tlr.tool_changes as f64,
            "0",
            format!(
                "1차 중 공구 {}회 교체 (공구 {}개) — 교체 지점 앞뒤 벽은 다른 공구가 깎아 단차 ±{:.1} µm (재클램프 런아웃·반경 측정), 새 공구 길들이기 VB {:.0} µm 가 다시 생김",
                tlr.tool_changes,
                tlr.tools_used,
                tool_life.change_step_um,
                tr.break_in_vb_mm * 1000.0
            ),
        );
    }
    if tlr.expired {
        flag(&mut flags, "tool_life_expired_finish_part", 1, tlr.life_used_max, "≤ 판정", format!("공구 수명 관리로 작업 중 만료 — 프로그램은 끝까지 가공 (수명 사용 최대 {:.0}%), 다음 작업부터 예비 공구. 만료 뒤 깎은 벽은 마모 공구 품질", tlr.life_used_max * 100.0));
    }
    if tlr.peak_stress_ratio >= 0.6 && !tlr.failed {
        flag(
            &mut flags,
            "body_stress_high",
            1,
            tlr.peak_stress_ratio,
            "< 0.6",
            format!(
                "공구 몸체 최대 주응력 {:.0} MPa = 정적 유효 강도 {:.0} MPa 의 {:.0}% ({}) — 채터·런아웃·칩 막힘 같은 순간 하중 증가에 여유가 작다 (피로 한도 {:.0} MPa)",
                tlr.peak_stress_mpa,
                tlr.strength.static_mpa(),
                tlr.peak_stress_ratio * 100.0,
                tlr.critical_section,
                tlr.strength.fatigue_mpa()
            ),
        );
    }
    if tlr.fatigue_damage >= 0.3 && !tlr.failed {
        flag(&mut flags, "body_fatigue_high", 1, tlr.fatigue_damage, "< 0.3", format!("회전 굽힘 피로 누적 {:.2} (1 이면 파손) — 같은 공구로 이 작업을 {:.0}번 더 하면 피로 파손", tlr.fatigue_damage, ((1.0 - tlr.fatigue_damage) / tlr.fatigue_damage.max(1e-9)).floor()));
    }
    if tlr.chip_detected > 0 {
        let cf = &tlr.chip_factors_mean;
        flag(
            &mut flags,
            "chipping_detected",
            2,
            tlr.chip_detected as f64,
            "0",
            format!(
                "부하 감시가 치핑을 잡아 {} {}회 (누적 치핑 확률 {:.0}% 도달) — 이 조건은 VB 한계보다 치핑이 공구 수명을 정함, 치핑 지점 앞 벽·바닥 검사 필요. 평균 배수 채터 ×{:.1} · 응력 ×{:.1} · 경도 ×{:.1} · 적합도 ×{:.1} 중 큰 것을 낮추는 조건 변경",
                if tlr.policy == crate::profile::ToolLifePolicy::ChangeTool { "공구 교체" } else { "정지" },
                tlr.chip_detected,
                crate::toollife::CHIP_FAILURE_P * 100.0,
                cf.chatter,
                cf.stress,
                cf.hardness,
                cf.fit
            ),
        );
    }
    if tlr.chip_probability >= crate::toollife::CHIP_WARN_P && !tlr.failed {
        let cf = &tlr.chip_factors_mean;
        flag(
            &mut flags,
            "chipping_risk",
            1,
            tlr.chip_probability,
            "< 0.5",
            format!(
                "이 작업 동안 날 치핑 누적 확률 {:.0}% (평균 배수 채터 ×{:.1} · 응력 ×{:.1} · 경도 ×{:.1} · 열균열 ×{:.1} · 한계 초과 ×{:.1} · 적합도 ×{:.1}) — 2차 전 M00 에서 날 확인",
                tlr.chip_probability * 100.0,
                cf.chatter,
                cf.stress,
                cf.hardness,
                cf.thermal,
                cf.worn,
                cf.fit
            ),
        );
    }
    if let Some(r) = tlr.load_ratio_at_life_end {
        if r < crate::toollife::LOAD_ALARM_RATIO {
            flag(
                &mut flags,
                "load_monitor_blind",
                0,
                r,
                "≥ 1.2",
                format!(
                    "수명 판정 시점의 절삭 동력이 새 날의 ×{:.2} 뿐 — 부하 감시(학습 부하 1.2~1.25배 경보)로는 이 조건의 마모를 못 잡는다 (얇은 칩·정삭: 플랭크 마찰 몫이 동력에서 작음) → 시간·횟수 수명 관리나 공구 측정이 필요",
                    r
                ),
            );
        }
    }
    // 표면 무결성: 마모가 커지면 피삭재 쪽 손상이 생긴다 (리서치 정리 — 고경도 백층 VB 0.2 → 4.5 µm · 0.4 → 11.8 µm,
    // IN718 VB 0.3 에서 압축 잔류응력 층 ~120 µm, 노치 0.2 mm 에서 경화층 230 µm, 스테인리스 가공경화층 50~200 µm, Ti 변형층 10~50 µm,
    // 알루미늄 버 높이 수명 말기 2~3배, CFRP VB 0.1~0.15 위에서 박리·미절단 섬유)
    let vb_seen = tlr.retired.iter().map(|r| r.vb_band_mm).fold(rep.vb_end_mm, f64::max);
    let group = crate::pairref::WpGroup::of(&ctx.wp);
    let integrity: Option<(f64, &str)> = match group {
        crate::pairref::WpGroup::Hardened => Some((0.2, "백층(white layer) ≥ 4 µm · 열영향층 10~36 µm")),
        crate::pairref::WpGroup::Nickel => Some((0.25, "압축 잔류응력 층 ~120 µm · 노치 경화층 ~230 µm")),
        crate::pairref::WpGroup::Stainless => Some((0.2, "가공경화층 50~200 µm 가 깊어짐")),
        crate::pairref::WpGroup::Titanium => Some((0.2, "소성 변형층 10~50 µm")),
        crate::pairref::WpGroup::Cfrp => Some((0.15, "층간 박리·미절단 섬유")),
        crate::pairref::WpGroup::Aluminum => Some((0.2, "버 높이 2~3배")),
        _ => None,
    };
    if let Some((th_vb, what)) = integrity {
        if vb_seen >= th_vb {
            flag(&mut flags, "surface_integrity_risk", 1, vb_seen, &format!("VB < {:.2} mm", th_vb), format!("이 작업의 최대 VB {:.3} mm ≥ {:.2} mm — {} (치수·조도와 별개의 피삭재 손상, 단면 검사·잔류응력 측정으로 확인)", vb_seen, th_vb, what));
        }
    }
    // ---- 용도 적합도 ----
    let fit = crate::appfit::evaluate(t, &ctx.wp, false);
    if fit.verdict >= 1 {
        flag(&mut flags, "tool_application_fit", fit.verdict.min(2), fit.score, "적합", fit.summary());
    }
    if t.geometry_from_workpiece {
        flag(
            &mut flags,
            "tool_geometry_assumed",
            0,
            0.0,
            "공구 사양 입력",
            format!(
                "공구 경사각 {:.0}° · 여유각 {:.0}° · 날끝 반경 {:.1} µm 중 입력하지 않은 값은 {} 기본값 — 이 소재용으로 설계된 공구라고 가정했다 (용도 적합도가 실제보다 좋게 나올 수 있음)",
                t.rake_deg,
                t.clearance_deg,
                t.edge_radius_um,
                ctx.wp.label
            ),
        );
    }
    let unknowns = unknown_items(profile, ctx, rep, &nominal, &pair);

    SimAudit {
        version: AUDIT_VERSION,
        inputs,
        engagement,
        force,
        thermal,
        wear,
        dynamics,
        deflection,
        surface,
        flags,
        measure_next,
        pair,
        tool_life,
        fit,
        unknowns,
    }
}

/// 측정 없이는 정할 수 없는 값 — 이 작업에서 쓴 값, 그럴 법한 범위, 결과에 주는 영향, 가장 싼 측정.
/// 범위·영향의 근거: 홀더·측정기 제조사 사양, 공구 공차(e8/h10), 정밀가공 오차원 정리 (`docs/tool-life-and-pair-matrix.md`)
pub fn unknown_items(profile: &MachiningProfile, ctx: &CutContext, rep: &LoadSimReport, nominal: &CutAnalysis, pair: &crate::pairref::PairSummary) -> Vec<UnknownItem> {
    let t = &ctx.tool;
    let mut v: Vec<UnknownItem> = Vec::new();
    let mut add = |key: &str, used: String, range: &str, effect: String, resolve: &str| {
        v.push(UnknownItem { key: key.into(), used, range: range.into(), effect, resolve: resolve.into() });
    };
    let e_um = t.runout_ecc_mm() * 1000.0;
    add(
        "runout_tir_um",
        format!("TIR {:.1} µm ({})", t.runout_um, pair.runout_source),
        "열박음·유압 1.5 (P95 3) · ER UP 3 (6) · ER 표준 6 (12) µm, 클램프마다 달라짐",
        format!(
            "벽 과삭 0.85·e = {:.1} µm (TIR 3 → {:.1} · 12 → {:.1} µm), 표면 형성 날 수·Ra, 날별 칩 두께 차 = TIR",
            0.85 * e_um,
            0.85 * 1.5,
            0.85 * 6.0
        ),
        "다이얼 게이지로 1×D 에서 회전 TIR, 또는 회전 레이저 측정 (클램프할 때마다)",
    );
    if !t.runout_compensated {
        add(
            "tool_radius_eff_um",
            "공칭 반경 (회전 측정 없음)".into(),
            "Ø6~18 공차 e8 22~27 µm · h10 58~70 µm (반경 ±5~35 µm)",
            "벽 위치에 그대로 더해짐 — 공차 20 µm 이하 정삭은 이 값 하나로 공차를 넘을 수 있음".into(),
            "회전 상태 레이저 공구 측정 (공구 설정 '반경 회전 측정' 체크)",
        );
    }
    add(
        "tool_length_um",
        "길이 보정 오차 0 으로 가정".into(),
        "돌출 반복도 < 10 µm (재클램프), 측정기 ±0.5~2 µm",
        "바닥(Z) 높이 오차 — 벽 치수에는 영향 없음".into(),
        "레이저·접촉식 공구 길이 측정 (재클램프마다)",
    );
    let st = &nominal.stability;
    if !st.measured {
        add(
            "frf_fn_zeta_k",
            format!("fn {:.0} Hz · k {:.2} N/µm · ζ {:.3} (모델 추정)", st.fn_hz, st.k_n_per_um, t.damping_ratio),
            "ζ 0.01~0.05 (공구마다 ±50 %), fn 은 돌출 1 mm 당 약 5 % 이동",
            format!("채터 여유 공칭 {:.2} → ζ ×0.5~×1.5 이면 약 {:.2}~{:.2} (치핑 위험·마모 배수·떨림 자국이 따라 바뀜)", st.margin, st.margin * 0.5, st.margin * 1.5),
            "충격 해머 + 가속도계 탭 테스트 (공구당 약 15 분)",
        );
    }
    if st.chatter_hz > 0.0 && st.margin.is_finite() {
        let lam = nominal.vc_effective_m_min * 1000.0 / 60.0 / st.chatter_hz;
        add(
            "pd_model",
            format!("공정감쇠 이득 ×{:.2} (채터 {:.0} Hz · 파장 {:.2} mm — 모든 모드에 같은 이득, 상한 5)", st.process_damping_gain, st.chatter_hz, lam),
            "공정감쇠 계수 C 1.2~5.8 ×10⁵ N/m (피삭재·여유각·마모, Tyler·Schmitz 2014). 강 공구 모드 이득 2.5~3.5 (파장 0.58 mm, Mohammadi 외 2017), 700 Hz 아래 스핀들·홀더 모드는 거의 받지 못함",
            format!("채터 여유 {:.2} — 공정감쇠, 가장 약한 모드의 k·ζ (같은 지름·돌출에서도 약 20배 차), 가변 피치 패턴 때문에 실제 한계는 대략 ×0.3~×2", st.margin),
            "탭 테스트로 FRF 를 넣고, ap 를 두세 단계로 바꿔 시험 절삭 (소리·표면 자국)",
        );
    }
    if t.variable_pitch {
        add(
            "variable_pitch_gain",
            format!("가변 피치 이득 ×{:.1} 고정", st.pitch_gain),
            "높은 로브(채터/날 통과 ≳ 10)에서 ×1.2~1.6, 첫 로브 근처 ×2~5 — 설계 회전수에서 ±15~25 % 벗어나면 ×0.5 까지 떨어짐 (Budak 2003 JMSE 125:29, Comak 2013 Sabancı 석사논문, Iglesias 외 2019 Materials 12:112)",
            "채터 여유·추천 회전수 — 피치 각도와 설계 회전수를 모르면 이득을 계산할 수 없음".into(),
            "공구 제조사에 피치 각도 문의, 또는 회전수를 ±10 % 바꿔 가며 시험 절삭",
        );
    }
    add(
        "edge_radius_um",
        format!("{:.1} µm (호닝 + 코팅 {:.1} µm × 0.8 추정{})", t.edge_radius_um, t.coating.thickness_um, if t.geometry_from_workpiece { ", 피삭재 기본값" } else { "" }),
        "±30 % (호닝·코팅 배치마다 다름, 5~30 µm)",
        format!("최소 칩 두께 {:.1}±{:.1} µm · 날끝 힘 ±14 % · 용도 적합도(치핑)", ctx.wp.min_chip_ratio * t.edge_radius_um, 0.3 * ctx.wp.min_chip_ratio * t.edge_radius_um),
        "광학 날끝 측정 또는 레플리카 현미경",
    );
    if rep.vb_start_mm > 0.0 || rep.tool_life.retired.iter().any(|r| r.vb_start_mm > 0.0) {
        add(
            "vb_reused_tool",
            format!("VB {:.3} mm (공구 개체 누적 시뮬레이션)", rep.tool_life.retired.first().map(|r| r.vb_start_mm).unwrap_or(rep.vb_start_mm)),
            "시뮬레이션 누적은 실측 보정 전 σ_ln 0.5",
            "이어 쓰는 공구의 시작 마모 — 수명 판정·벽 오차의 출발점".into(),
            "공구 현미경 VB, 또는 레이저 반경 손실 ÷ (tanα/(1−tanα·tanγ))",
        );
    }
    if rep.quality_loss.degraded_points > 0 {
        add(
            "damaged_tool_surface",
            format!(
                "손상 공구로 깎은 최종 벽 {} 점 — 치수는 수명 한계를 넘는 반경 마모, Ra 는 마모 배수로만 계산 (치핑 자국은 수치 없음)",
                rep.quality_loss.degraded_points
            ),
            "치핑 크기·자리(날끝 모서리 · 절입 경계 노치)와 어느 날이 깨졌는지는 공구마다 다름 — 측정 전에는 정할 수 없음",
            "깨진 날끝이 남기는 찢김·번짐·버(burr)·가공 변질층과 바닥 자국 높이는 모델 밖 — 구간 위치와 치핑 확률로만 표시".into(),
            "의심 구간을 확대경·조도계로 확인하고, 공구는 날끝 현미경으로 치핑 크기·자리 확인",
        );
    }
    add(
        "machine_thermal_um",
        "예열·열보정된 기계로 가정 (0)".into(),
        "XY 2~10 µm · Z 5~40 µm (보정 없음), 시상수 20~60 분, 정지 5 분 뒤 다시 안정까지 6~7 분",
        "벽 위치·바닥 높이 드리프트 — 아침 첫 부품·긴 정지 뒤".into(),
        "작업 시작과 정지 뒤 기준구(datum sphere) 프로빙",
    );
    add(
        "part_temp_at_finish",
        format!("모델 소재 온도 {:.1}°C (덩어리 정상 상승)", rep.wp_temp_end_c),
        "플러드 +0.5~3 K (τ < 2 분) · 건식·MQL +5~30 K (τ 20~80 분)",
        format!("치수 오차 = α·L·ΔT ({:.1} µm/m·K × 크기)", ctx.wp.expansion * 1e6),
        "정삭 직전 소재 온도계, 또는 그 온도에서 프로브 측정",
    );
    add(
        "coolant_delivery",
        format!("{} 공칭 (노즐이 절삭점에 닿는다고 가정)", ctx.coolant.method.label()),
        "Ti·Ni 공구 수명 1.5~3배 차이",
        "계면 온도·확산 마모·칩 배출".into(),
        "노즐 방향 확인 + 스핀들 동력·온도 기록",
    );
    add(
        "fixture_wall_stiffness",
        format!("고정 강성 {:.1} N/µm ({:?})", ctx.fixture_stiffness_n_per_um, profile.workpiece_setup.clamping),
        "0.05~5 N/µm",
        format!("고정 휨 {:.2} µm · 얇은 벽 휨", nominal.fixture_deflection_um),
        "정삭 위치에서 탭·밀기 시험",
    );
    {
        let thin: Vec<&crate::loadsim::WallErrorSample> = rep.wall_errors.iter().filter(|w| w.final_wall && w.wall_k_n_mm > 0.0).collect();
        if !thin.is_empty() {
            let sigma_y = ctx.wp.flow_stress(0.0, 1.0, rep.bulk_c).max(1.0);
            let unloaded = thin.iter().filter(|w| w.wall_stress_mpa <= 0.0).count();
            let yielded = thin.iter().filter(|w| w.wall_stress_mpa > sigma_y).count();
            add(
                "thin_wall_load",
                format!(
                    "얇은 벽 최소 {:.2} mm (최종 벽 {}점): 칩이 붙은 소재가 벽 재료면 공구 법선 힘 전체, 벽 법선 방향으로 15 mm 넘게 이어진 덩어리면 0 ({}점) · 탄성 복원은 전소성 하중 1.5·σy (σy {:.0} MPa) 까지",
                    thin.iter().map(|w| w.wall_mm).fold(f64::INFINITY, f64::min),
                    thin.len(),
                    unloaded,
                    sigma_y
                ),
                "벽과 덩어리가 힘을 나눠 받는 몫 0~100 % (두 강성의 비 — 유한요소로만 정함, 미절삭 소재 1~5 mm 앞이면 벽 강성 ×4~115) · 판 폭 b = ap + 2a 는 강성 등가 폭과 ±15 % 이나 뿌리 응력은 Roark 판 해(응력 폭 1.6~2.0·a)로 1.3~1.5배 클 수 있음 · 벽 위끝 휨은 가운데 높이의 약 2.5배 (벽 오차는 가운데 높이) · 벽 자유 끝 강성 ×0.37 · 로트 σy ±10 % (IN718 시효재 0.2 % 내력 최소 1,034 MPa)",
                format!("얇은 벽 휨 최대 {:.1} µm · 항복 넘은 최종 벽 {}점 (그 벽은 영구 변형 — 치수가 아니라 불량)", rep.max_workpiece_defl_um, yielded),
                "정삭 전후 벽 두께를 마이크로미터로 재고, 다이얼 게이지를 대고 벽을 밀어 강성 확인 (또는 그 벽만 유한요소 해석)",
            );
        }
    }
    if rep.pattern == "pocket_zigzag" {
        let plan = crate::gcode::pocket_plan(profile);
        let entry = match plan.entry.as_str() {
            "helix" => format!("헬리컬 진입 (구멍 Ø{:.1} mm, 공구 중심 경사 {:.1}°)", plan.helix_bore_mm, plan.ramp_deg),
            "ramp" => format!("직선 램프 진입 ({:.1}°)", plan.ramp_deg),
            _ => "직선 플런지 진입".to_string(),
        };
        add(
            "pocket_path_strategy",
            format!(
                "합성 지그재그 포켓 — 단계 {} × {:.2} mm (슬롯 ap 상한 {}) · {} · 첫 줄 슬롯·줄 끝 피치 이동 fz ×{:.2} (칩 얇아짐 보정 뺌) · 벽 정삭 여유 {:.2} mm 를 남긴 뒤 전체 깊이 정삭 윤곽 ({}) · 벽 두께 {:.2} mm — {}",
                plan.levels,
                plan.level_ap_mm,
                plan.slot_ap_max_mm.map(|v| format!("{:.1} mm", v)).unwrap_or_else(|| "자료 없음".into()),
                entry,
                plan.slot_feed_ratio,
                plan.finish_allow_mm,
                if plan.up_milling { "상향" } else { "하향" },
                plan.wall_mm,
                plan.notes.join(" · ")
            ),
            "제조사 슬롯 ap 는 같은 소재에서도 갈림 (Ti 0.25~1.25·D · Ni 합금 0.25~0.75·D · 58 HRC 0.05~0.5·D) · 헬리컬 이송 = 슬롯 이송의 20~100 % (Seco ↔ IMCO·SGS) · 벽 정삭 여유 2~6 % D (Seco·Haas·Helical) · 모서리 반경 4/3·r 은 부품 설계값으로 가정 (도면이 더 작으면 모서리 결합이 커짐)",
            format!(
                "슬롯 ap 를 범위 끝으로 옮기면 단계 수·절삭 시간·공구 응력이 움직임 · 최종 벽은 정삭 윤곽이 만든 벽 (이 작업 최종 벽 공구 휨 최대 {:.0} µm, 모서리 반경 {:.1} mm)",
                rep.wall_errors.iter().filter(|w| w.final_wall).map(|w| w.deflection_um.abs()).fold(0.0, f64::max),
                plan.corner_radius_mm
            ),
            "실제 CAM G-code 를 불러와 시뮬레이션 — 슬롯 ap·헬리컬 이송은 쓰는 공구의 제조사 자료로",
        );
    }
    // 얕은 절입(헬리컬·램프 진입: 회전당 하강 0.3~0.5 mm 로 바닥 전폭)의 마모는 마모장의 공구 끝 칸에만 쌓이지만, 작업 안의 VB(수명 판정·
    // 반경 마모 보정)는 깊이를 가리지 않고 더한다. 새 날 길들이기도 공구 나이로만 세어 처음 물린 곳(진입이면 날끝)에 다 들어간다
    let band_ap = profile.conditions.axial_doc_mm;
    let dvb_all: f64 = rep.regimes.iter().map(|g| g.dvb_mm).sum();
    let dvb_shallow: f64 = if band_ap > 1e-9 { rep.regimes.iter().map(|g| g.dvb_mm * (1.0 - (g.ap_mm / band_ap).min(1.0))).sum() } else { 0.0 };
    if dvb_all > 1e-9 && dvb_shallow > 0.15 * dvb_all {
        add(
            "shallow_cut_wear_depth",
            format!(
                "VB 성장의 {:.0}% 가 ap {:.1} mm 띠보다 얕은 절입 (마모장 띠 평균 {:.3} mm · 깊이를 가리지 않은 VB {:.3} mm) — 작업 안 수명 판정·반경 마모는 깊이 무관 VB, 공구 장부는 띠 평균을 이어 씀 · 길들이기는 공구 나이로만 셈",
                100.0 * dvb_shallow / dvb_all,
                band_ap,
                rep.wear_field.band_vb(band_ap),
                rep.vb_end_mm - rep.vb_start_mm
            ),
            "날 위치별 길들이기·날끝(코너) 마모 배분 자료 없음 — 진입 마모는 날끝 마모로 가고, 측면 띠는 처음 물릴 때 따로 길들여질 수 있음",
            format!(
                "작업 안 VB·수명 판정·벽 마모 보정이 측면 띠 마모보다 최대 {:.0}% 크고, 다음 작업의 시작 VB(띠 평균)는 그만큼 작음 — 날끝 마모는 마모장 최댓값(국부 수명)으로만 남음",
                100.0 * dvb_shallow / dvb_all
            ),
            "진입이 긴 작업 하나 뒤 공구 현미경으로 날끝(코너)과 띠 중간 VB 를 따로 측정",
        );
    }
    add(
        "lot_hardness",
        format!("{} 대표 경도 {:.0} HRC", ctx.wp.label, ctx.wp.hardness_hrc),
        "로트마다 경도 ±5~10 %",
        "공구 수명 ±20~40 % (Taylor C 가 움직임)".into(),
        "로트마다 경도 시험",
    );
    add(
        "corner_feed_engagement",
        "합성 경로 — 제어기 코너 감속·코너 결합 증가는 원호 보간 이송 보정만".into(),
        "코너 이송 20~60 % 로 감속, 안쪽 코너 결합 2~3배",
        "코너의 힘·마모·채터·벽 오차".into(),
        "제어기 이송 기록(트레이스)",
    );
    let spread = crate::pairref::life_spread(ctx, rep.chatter_fraction, rep.min_chatter_margin, false);
    add(
        "tool_life_scatter",
        format!("σ_ln {:.2} (공구 간) · 모델 {:.2}", spread.sigma_scatter, spread.sigma_model),
        "생산 현장 CV 0.25~0.5 (Weibull β 2~4.5), 시험실 반복 β 6~9",
        format!("보수 교체 시점 = 중앙값 × {:.2} (P10) — 버리는 수명 23~47 %", spread.p10_total),
        "교체할 때마다 절삭 시간과 VB 기록 → 분포 맞춤",
    );
    let tlr = &rep.tool_life;
    if t.coating.thickness_um > 0.0 && !t.coating.chem.bare {
        let through = tlr.events.iter().find(|e| e.kind == "coating_through");
        add(
            "coating_life",
            format!(
                "{} {:.1} µm · 관통 랜드 {:.0} µm (두께 / sin 여유각){}",
                t.coating.family,
                t.coating.thickness_um,
                crate::toolwear::coating_strip_um(t),
                through
                    .map(|e| format!(" · 이번 경로에서 공구 #{} 이 작업 절삭 {:.1} s 에 관통", e.tool_no, e.t_cut_s))
                    .unwrap_or_else(|| " · 이번 경로에서는 관통 안 함".into())
            ),
            "PVD 1~5 µm · CVD 다이아몬드 6~20 µm. 박리(층간 균열)·CVD 전처리의 Co 결핍층은 모델에 없어 실제로는 더 일찍 벗겨질 수 있음",
            "관통 전에는 코팅 배수(화학 궁합), 뒤에는 드러난 몫만큼 무코팅 초경 마모율 — 맞지 않는 코팅(다이아몬드로 강·Ti)은 관통 시점이, 보호 코팅은 관통 뒤 가속이 수명을 크게 바꿈".into(),
            "처음 10 s · 1 min · 5 min 뒤 날을 공구 현미경으로 보고 코팅이 벗겨진 폭을 기록 (실측 VB 와 같이 입력)",
        );
    }
    use crate::tribology::WpChem;
    let chem = ctx.wp.chem;
    if t.coating.family == "Diamond" && matches!(chem, WpChem::Iron | WpChem::Nickel) {
        add(
            "diamond_ferrous_onset",
            format!("흑연화 개시 700 °C (국부 온도 기준) 를 접촉 평균 계면 {:.0} °C 에 적용, 그 아래는 화학 마모 바닥 몫 0.25", nominal.thermal.interface_c),
            "정적 시험 700~740 °C (공기 중 DTA 740 °C · Zou 외 2017, 약 970 K · Chen 외 2007). 경사면 최고 온도는 평균보다 수십~150 °C 높을 수 있고, 그 아래에서도 새 면 반응·산화로 화학 마모가 큼 (Thornton·Wilks 1978)",
            "코팅 관통 시점과 관통 전 마모율 — 평균 550~650 °C 구간에서는 실제 용해가 모델보다 빠를 수 있음 (확산 물질 이동 상한 추정 600 °C 30~55 · 700 °C 50~100 µm/min × 날 결합 시간 비, 리서치 추정값)".into(),
            "쓰지 않는 조합 — 꼭 써야 하면 처음 10 s · 1 min 뒤 날을 현미경으로 보고 코팅이 없어진 폭을 기록",
        );
    }
    if chem == WpChem::CarbonFiber {
        add(
            "cfrp_layup",
            format!("준등방 적층 평균 — 측벽 Ra 바닥 {:.1} µm, 절삭 계수는 섬유각 평균", crate::surface::ra_floor_um(crate::pairref::WpGroup::Cfrp)),
            "0°·45°·90° 층 Ra 0.7~1.5 µm · −45°(135°) 층 3~5 µm (새 CVD 공구 3~13 µm), 하향 절삭은 2~6배 거침. Ktc 420~1,240 N/mm² 로 섬유 절단각 약 120° 에서 최대 (Karpat 외 2012)",
            "측벽 Ra·Rz, 박리·미절단 섬유가 생기는 층, 회전당 힘 변동".into(),
            "적층 순서(도면) 확인, −45° 층만 따로 조도 측정, 가장자리를 확대해 박리·섬유 돌출 확인",
        );
        add(
            "cfrp_chatter",
            format!("재생 채터 여유 {:.2} (금속과 같은 0차 해석)", st.margin),
            "단단히 고정한 Ø6~12 CFRP 트리밍에서는 재생 채터 보고가 없음 — 0차 이론이 채터를 예측하는 슬롯 ap 3.6~10 mm 도 안정 (Hamedanianpour 2013, Karpat 외 2012, Delahaigue 외 2017, Ashworth 외 2022). 채터는 홀더 모드 600~700 Hz 같은 유연한 계에서 보고됨 (Rusinek·Lajmert 2020)",
            "채터 확률·떨림 자국·Ra — 여유 < 1 이어도 실제로는 안정일 수 있음 (빠진 감쇠 후보: 섬유 탄성 복원, 정량 자료 없음)".into(),
            "탭 테스트, 시험 절삭의 소리·가속도 스펙트럼에 날 통과 주파수 말고 다른 봉우리가 있는지 확인",
        );
        add(
            "cfrp_damage",
            "모델 없음 — VB·Ra 만 계산 (Ra 는 다이아몬드 코팅 공구에서 마모될수록 오히려 낮아짐)".into(),
            "박리·섬유 돌출 깊이 0.1~0.2 mm (마모된 날 r 90 µm, Hintze·Hartmann 2013), −45° 층 뽑힘 구멍 약 40 µm, 마모 공구 날끝 475 °C 에서 수지 탄화 (Delahaigue 2017)",
            "공구 교체 시점 — 실제로는 VB 0.2 mm 보다 박리·미절단 섬유·탄 자국이 먼저 오는 경우가 많음".into(),
            "N 개마다 가장자리 확대 사진(박리 길이·섬유 돌출)과 날끝 반경 측정",
        );
        if !t.coating.chem.bare && !t.coating.chem.carbon {
            add(
                "pvd_in_cfrp",
                format!("{} 코팅 배수를 수명 전체에 씀 (보호 코팅 규칙)", t.coating.family),
                "CFRP 에서 3 µm AlTiN 은 약 1 분(드릴 50 구멍) 안에 날끝에서 벗겨지고, 그 뒤로는 무코팅과 같거나 더 빨리 닳음 (Wang 외 2013 · Sturtevant 2013, Ozkan 외 2020)",
                "PVD 공구의 CFRP 수명 — 기준 공구 AlTiN 의 절대 수명은 실측 3~6 분에 맞췄지만, 무코팅 대비 이득(경도 몫)은 실측보다 큼".into(),
                "같은 조건에서 무코팅 공구와 VB 0.1 mm 까지의 절삭 길이 비교",
            );
        }
    }
    if chem == WpChem::Aluminum && t.coating.chem.carbon {
        add(
            "carbon_coating_al_benefit",
            format!("{} 응착 배수 0.45 (무코팅 0.9) — 미스트 프리셋에서 DLC/무코팅 수명비 약 1.6", t.coating.family),
            "건식 단조 Al 에서 DLC 수명은 무코팅의 2.2~3.5배 (Grigoriev 외 2019·2020, Huang 외 2021). 고Si 주조 Al 에서 10 µm 다이아몬드는 랜드 40~60 µm 에서 관통·박리가 수명을 정함 (US 6,436,204)",
            "탄소 코팅의 Al 수명 — 단조 Al 건식 실측보다 보수적, 고Si 주조 Al 은 박리를 모델이 다루지 않음".into(),
            "같은 조건에서 무코팅·DLC 공구 VB 곡선 비교, 고Si 합금은 코팅 박리 시점 기록",
        );
    }
    add(
        "tool_body_strength",
        format!("{} 항절력 {:.0} MPa → 정적 {:.0} · 피로 {:.0} MPa", tlr.strength.label, tlr.strength.trs_mpa, tlr.strength.static_mpa(), tlr.strength.fatigue_mpa()),
        "등급·연삭 상태·Weibull 계수(10~20)에 따라 ±15~20 %",
        format!("파손 판정 — 이 작업 최대 주응력 비 {:.2}, 피로 누적 {:.3}", tlr.peak_stress_ratio, tlr.fatigue_damage),
        "공구 제조사 등급 자료 (항절력·K_IC)",
    );
    add(
        "chipping_hazard",
        format!(
            "기본 위험 {:.0} % (수명까지) × 배수 (채터·응력·경도·열균열·한계 초과·적합도), 누적 {:.0} % 에서 치핑으로 봄 (감시 정책은 부하 감시가 이때 잡는다고 가정)",
            tlr.params.chip_base_p * 100.0,
            crate::toollife::CHIP_FAILURE_P * 100.0
        ),
        "검증된 모델 없음 (확신도 낮음) · 작은 치핑·얇은 칩 정삭은 부하 감시가 놓칠 수 있음",
        format!("치핑 누적 확률 {:.0} % · 치핑 감지 {}회", tlr.chip_probability * 100.0, tlr.chip_detected),
        "같은 조건 공구 10자루 이상의 수명·치핑 기록, 부하 감시 파손 경보 로그와 날 사진 대조",
    );
    v
}

/// 테스트·콘솔 로그용 사람이 읽는 줄
pub fn lines(a: &SimAudit) -> Vec<String> {
    let mut v = Vec::new();
    let i = &a.inputs;
    v.push(format!(
        "[AUDIT] {} · {} · {} Ø{}x{}F 헬릭스 {}° 돌출 {:.0}mm (L/D {:.1}) · {}  Vc {:.0} rpm {:.0} fz {:.4} ap {:.1} ae(설정) {:.2}",
        i.profile, i.pattern, i.material, i.d_mm, i.flutes, i.helix_deg, i.stickout_mm, i.stickout_over_d, i.coating, i.vc_m_min, i.rpm, i.fz_mm, i.ap_mm, i.ae_planned_mm
    ));
    let e = &a.engagement;
    v.push(format!(
        "[AUDIT] 결합: ae 설정 {:.2} → 경로 실행 {:.3} mm (체적 {:.3}, ae/D {:.2}, 결합각 {:.1}°) | 기하 기대 {} | 격자 셀 {:.3} 스텝 {:.3} 여유 {:.1}칸 (셀/ae {:.2}) | 절삭 {:.1} mm {:.2} s (전체 {:.1} s) 제거 {:.0} mm³ | 자체 제거 0 인 절삭 스텝 {:.0}%",
        e.planned_ae_mm,
        e.executed_ae_mm,
        e.executed_ae_by_volume_mm,
        e.executed_ae_over_d,
        e.executed_engage_deg,
        match (e.geometry_ae_mm, e.geometry_cut_len_mm, e.geometry_cut_time_s) {
            (Some(ae), Some(l), Some(t)) => format!("ae {:.3} · {:.0} mm · {:.2} s", ae, l, t),
            (None, Some(l), Some(t)) => format!("ae 기대 없음(볼 노즈) · {:.0} mm · {:.2} s", l, t),
            _ => "알 수 없음".to_string(),
        },
        e.grid.cell_mm,
        e.grid.step_mm,
        e.grid.allowance_cells,
        e.cell_over_ae,
        e.sim_cut_len_mm,
        e.sim_cut_time_s,
        e.total_time_s,
        e.sim_removed_mm3,
        e.grid.no_own_removal_fraction * 100.0
    ));
    let fo = &a.force;
    v.push(format!(
        "[AUDIT] 힘: 공칭(ae {:.2}) 평균 {:.1}/피크 {:.1} N {:.3} kW {:.2} J/mm³ h {:.1}/{:.1} µm | 실행(ae {:.2}) 평균 {:.1}/피크 {:.1} N (x{:.1}) {:.3} kW {:.2} J/mm³ h {:.1}/{:.1} µm 날끝비 {:.0}% | 경로 평균 {:.1} N 피크 {:.1} N | kc11 {:.0} kte {:.1} kre {:.1} N/mm μ {:.2} 축력 {} (실행 평균 {:.1}/피크 {:.1} N)",
        fo.nominal.ae_mm,
        fo.nominal.f_mean_n,
        fo.nominal.f_peak_n,
        fo.nominal.power_kw,
        fo.nominal.specific_energy_j_mm3,
        fo.nominal.h_mean_um,
        fo.nominal.h_max_um,
        fo.executed.ae_mm,
        fo.executed.f_mean_n,
        fo.executed.f_peak_n,
        fo.executed.peak_over_mean,
        fo.executed.power_kw,
        fo.executed.specific_energy_j_mm3,
        fo.executed.h_mean_um,
        fo.executed.h_max_um,
        fo.executed.edge_share * 100.0,
        fo.path_mean_n,
        fo.path_peak_n,
        fo.kc11_eff,
        fo.kte_n_mm,
        fo.kre_n_mm,
        fo.mu,
        if fo.axial_force_modeled { "계산" } else { "미계산" },
        fo.executed.fa_mean_n,
        fo.executed.fa_peak_n
    ));
    let th = &a.thermal;
    v.push(format!(
        "[AUDIT] 열: 계면 건식 {:.0} → 습식 {:.0}°C (저감 {:.1}%, 효과 {:.2}, {}) 실행 ae {:.0}°C | h_film {:.0} h_eff {:.0} 칩전도 {:.1e} W/m²K 접촉 {:.3} mm {:.2} ms 단속 열 ×{:.2} | 공구몸체 {:.1}°C (+{:.1} K, 시간적분 {}) | 경로 평균 {:.0} 최대 {:.0} 첫 빈 {:.0} 끝 빈 {:.0}°C 소재 {:.1}°C",
        th.interface_dry_c,
        th.interface_wet_c,
        th.coolant_reduction_pct,
        th.cooling_effectiveness,
        th.boiling,
        th.executed_interface_c,
        th.h_film_w_m2k,
        th.h_eff_w_m2k,
        th.chip_conduction_w_m2k,
        th.contact_length_mm,
        th.engagement_ms,
        th.transient_factor,
        th.tool_body_c,
        th.tool_body_rise_k,
        if th.tool_body_time_resolved { "예" } else { "아니오" },
        th.path_temp_mean_c,
        th.path_temp_max_c,
        th.path_temp_first_bin_c,
        th.path_temp_last_bin_c,
        th.wp_end_c
    ));
    let w = &a.wear;
    v.push(format!(
        "[AUDIT] 마모: VB {:.2}→{:.2} µm / {:.3} min = {:.2} µm/min (정상 {:.2} + 길들이기 {:.2}, 길들이기 {:.0}%) | 길들이기 VB {:.0} µm τ {:.1} min 소진 {:.1}% | κ {:.3} 공칭 {:.2} µm/min 수명 {:.0}/{:.0} min | 경로계수 k {:.3} 경로 수명 {:.0} min | 채터 배수 경로 x{:.2} 공칭 x{:.2} | 교체까지 p50 {} p90 {} (수명 편차 σ {:.2}·모델 σ {:.2} → 보수 ×{:.2}, 편차만 ×{:.2} · 단순 환산 {:.0} · 정상율만 {:.0} · 남은 {:.0} min / 부품 {:.3} min) | 치수 기준 VB_dim {:.0} µm → {} 개 | MRR {:.2} cm³/min 경로 수명 동안 {:.0} cm³ 비마모 {:.2} µm/cm³ | 지배 {}",
        w.vb_start_mm * 1000.0,
        w.vb_end_mm * 1000.0,
        w.job_cut_min,
        w.job_rate_um_min,
        w.steady_um_min,
        w.breakin_avg_um_min,
        w.breakin_share * 100.0,
        w.breakin_vb_mm * 1000.0,
        w.breakin_tau_min,
        w.breakin_done * 100.0,
        w.kappa,
        w.nominal_rate_um_min,
        w.nominal_life_min,
        w.nominal_linear_life_min,
        w.path_factor,
        w.path_life_min,
        w.chatter_wear_job,
        w.chatter_wear_nominal,
        w.parts_left_p50.map(|x| format!("{:.0}", x)).unwrap_or_else(|| "-".into()),
        w.parts_left_p90.map(|x| format!("{:.0}", x)).unwrap_or_else(|| "-".into()),
        w.life_sigma_scatter,
        w.life_sigma_model,
        w.parts_p10_factor,
        w.parts_p10_scatter_only,
        w.parts_naive_total_rate,
        w.parts_steady_rate_only,
        w.path_remaining_min,
        w.part_cut_min,
        w.vb_dim_mm * 1000.0,
        w.parts_by_dimension.map(|x| format!("{:.0}", x)).unwrap_or_else(|| "-".into()),
        w.mrr_cm3_min,
        w.path_volume_cm3,
        w.specific_wear_um_cm3,
        w.dominant
    ));
    let d = &a.dynamics;
    v.push(format!(
        "[AUDIT] 동역학: fn {:.0} Hz k {:.2} N/µm ζ {:.3} m_eff {:.1} g (홀더 몫 {:.0}%) 날통과 {:.0} Hz (fn/f {:.1}) kt {:.0} N/mm² | ap {:.1} → 한계 {:.2} (임계 {:.2}) 여유 실행 {:.2} 공칭 {:.2} 경로 최소 {:.2} 비율 {:.0}% 확률 {:.0}% | ae/D {:.2} ZOA {} flip x{:.2} FRF {} | 스핀들 모드 {:.0} Hz {:.0} N/µm ζ {:.3} (지배 {}) 정적 끝 강성 {:.2} N/µm | 강제진동 SLE {:.2} 진폭 {:.2} µm | 채터→벽 오차 {} (마진<1 일 때 떨림 자국 산-골 {:.1}~{:.1} µm)",
        d.fn_hz,
        d.k_n_per_um,
        d.zeta,
        d.m_eff_g,
        d.holder_share * 100.0,
        d.tooth_hz,
        d.fn_over_tooth,
        d.kt_n_mm2,
        d.ap_mm,
        d.ap_lim_mm,
        d.ap_crit_mm,
        d.margin_executed,
        d.margin_nominal,
        d.margin_path_min,
        d.chatter_fraction * 100.0,
        d.chatter_probability * 100.0,
        d.immersion_over_d,
        if d.zoa_reliable { "신뢰" } else { "저절입(부정확)" },
        d.flip_factor,
        if d.frf_measured { "실측" } else { "추정" },
        d.spindle_fn_hz,
        d.spindle_k_n_per_um,
        d.spindle_zeta,
        if d.spindle_dominates { "스핀들" } else { "공구" },
        d.static_k_n_per_um,
        d.forced_sle_um,
        d.forced_amplitude_um,
        if d.chatter_in_wall_error { "반영" } else { "미반영" },
        d.chatter_waviness_low_um,
        d.chatter_waviness_high_um
    ));
    let df = &a.deflection;
    v.push(format!(
        "[AUDIT] 휨: 공구 끝 강성 {:.2} N/µm (보 {:.3} + 홀더 {:.3} + 스핀들 {:.3} µm/N, 지지부 몫 {:.0}%, 스핀들 테이퍼 {}) 하중점 {:.1} mm | 벽 휨 공칭 {:.2} (상 {:.2}/하 {:.2}) 경로 평균 {:.2} 최대 {:.2} µm | 벽 순간 법선 힘 업 {:.1} N → {:.2} µm / 다운 {:.1} N → {:.2} µm (경로 다운밀링 {:.0}%) | 런아웃 과삭(반영{}) {:.1} µm | 고정 {:.2} 소재 열 {:.2} µm",
        df.tool_tip_stiffness_n_per_um,
        df.beam_compliance_um_per_n,
        df.holder_compliance_um_per_n,
        df.spindle_compliance_um_per_n,
        df.holder_share * 100.0,
        if df.spindle_taper_compliance_modeled { "모델" } else { "미모델" },
        df.load_point_from_holder_mm,
        df.wall_defl_nominal_um,
        df.wall_defl_top_um,
        df.wall_defl_bottom_um,
        df.wall_mean_path_um,
        df.wall_max_path_um,
        df.wall_up_force_n,
        df.wall_up_um,
        df.wall_down_force_n,
        df.wall_down_um,
        df.path_down_share * 100.0,
        if df.runout_compensated { ", 회전 측정 반경으로 상쇄" } else { "" },
        df.runout_overcut_um,
        df.fixture_um,
        df.thermal_wp_um
    ));
    let sf = &a.surface;
    v.push(format!(
        "[AUDIT] 조도: 측벽 Ra {:.2} (안정 {:.2}) Rz {:.1} µm | 포락선 Rt {:.2} (이송 자국만 {:.3}) µm · 표면 형성 날 {}/{} | 바닥 {:.2} + 최소 칩 {:.2} µm · 구성인선 x{:.2} · 경로 끝 마모 x{:.2} | 채터 파상도 {:.1} µm × 확률 {:.0}% | 경로 Ra 평균 {:.2} · 90% {:.2} · 최대 {:.2} µm · 파상도 기대 최대 {:.1} µm",
        sf.ra_um,
        sf.ra_stable_um,
        sf.rz_um,
        sf.rt_envelope_um,
        sf.rt_feed_um,
        sf.forming_teeth,
        sf.flutes,
        sf.floor_um,
        sf.hmin_term_um,
        sf.bue_factor,
        sf.wear_factor_end,
        sf.chatter_wpv_um,
        sf.chatter_probability * 100.0,
        sf.path_ra_mean_um,
        sf.path_ra_p90_um,
        sf.path_ra_max_um,
        sf.path_waviness_max_um
    ));
    let pr = &a.pair;
    v.push(format!(
        "[AUDIT] 조합 기준값: {} × {} · 홀더 {} | 절삭 μ {:.2} × 코팅 {:.2} · Kr 하한 {:.2} · Kte 배수 {:.2} · Kre/Kte {:.2} · h_min/r {:.2} | 여유각 {:.1}° ({}) 호닝 {:.1} µm 런아웃 {:.1} µm ({}) | ζ {:.3} ({}) 공정감쇠 {:.2} × 여유각 {:.2} | 열 계수 {:.2} | 길들이기 {:.0} µm τ {:.1} min | VB 한계 {:.2} mm — {}",
        pr.wp_group,
        pr.coat_group,
        pr.holder,
        pr.cut_mu_base,
        pr.coating_mu_factor,
        pr.kr_floor,
        pr.kte_factor,
        pr.kre_over_kte,
        pr.min_chip_ratio,
        pr.clearance_deg,
        pr.clearance_source,
        pr.edge_radius_um,
        pr.runout_um,
        pr.runout_source,
        pr.damping_ratio,
        pr.damping_source,
        pr.process_damping,
        pr.pd_clearance_factor,
        pr.thermal_factor,
        pr.breakin_vb_um,
        pr.breakin_tau_min,
        pr.vb_limit_mm,
        pr.vb_limit_basis
    ));
    let tl = &a.tool_life;
    v.push(format!(
        "[AUDIT] 수명: 정책 {} · 상태 {} · 공구 {}개 (교체 {}) · 경로 완료 {:.0}%{}{} | VB 한계 {:.2} · 3차 시작 {:.2} · 붕괴 {:.2} mm (배수 ×{:.1}, 최대 ×{:.2}) · 수명 사용 최대 {:.0}% 끝 {:.0}% | 몸체 {} 항절력 {:.0} → 정적 {:.0} · 피로 {:.0} MPa · 최대 주응력 {:.0} MPa (×{:.2}, {}) · 피로 누적 {:.3} | 치핑 P {:.0}% (채터 ×{:.1} 응력 ×{:.1} 경도 ×{:.1} 열 ×{:.1} 초과 ×{:.1} 적합 ×{:.1}) | 부하(새 날 대비 동력) 최대 ×{:.2} · 판정 시점 {} · 1.2배 경보 {}",
        tl.policy,
        tl.state,
        tl.tools_used,
        tl.tool_changes,
        tl.completed_fraction * 100.0,
        if tl.failed {
            format!(" · 파손({})", tl.failure_cause)
        } else if tl.chip_detected > 0 {
            format!(" · 치핑 감지 {}회", tl.chip_detected)
        } else if tl.expired {
            " · 수명 관리 만료".to_string()
        } else {
            String::new()
        },
        if tl.uncut_boundary_points > 0 { format!(" · 미절삭 경계 벽 {}점은 최종 벽에서 뺌", tl.uncut_boundary_points) } else { String::new() },
        tl.vb_limit_mm,
        tl.vb_accel_mm,
        tl.vb_collapse_mm,
        tl.accel_mult,
        tl.accel_factor_max,
        tl.life_used_max * 100.0,
        tl.life_used_end * 100.0,
        if tl.body_label.is_empty() { "-" } else { tl.body_label.as_str() },
        tl.trs_mpa,
        tl.static_mpa,
        tl.fatigue_mpa,
        tl.peak_stress_mpa,
        tl.peak_stress_ratio,
        if tl.critical_section.is_empty() { "-" } else { tl.critical_section.as_str() },
        tl.fatigue_damage,
        tl.chip_probability * 100.0,
        tl.chip_factors.chatter,
        tl.chip_factors.stress,
        tl.chip_factors.hardness,
        tl.chip_factors.thermal,
        tl.chip_factors.worn,
        tl.chip_factors.fit,
        tl.load_ratio_max,
        tl.load_ratio_at_life_end.map(|r| format!("×{:.2}", r)).unwrap_or_else(|| "-".into()),
        tl.load_alarm_t_s.map(|s| format!("{:.1} s", s)).unwrap_or_else(|| "없음".into())
    ));
    for e in tl.events.iter() {
        v.push(format!("[EOL] {}", e));
    }
    if tl.degraded_points > 0 {
        v.push(format!(
            "[AUDIT] 품질 저하: 최종 벽 {} 점 중 {} 점 ({:.0}%) 을 손상 공구로 깎음 · 벽 {}곳 · 한계 넘어 덜 깎임 최대 +{:.1} µm · Ra 최대 {:.2} µm · 벽 위치 오차(전체) 최대 {:.1} µm{}",
            tl.final_points,
            tl.degraded_points,
            tl.degraded_points as f64 / tl.final_points.max(1) as f64 * 100.0,
            tl.degraded_zones.len(),
            tl.degraded_extra_wear_um,
            tl.degraded_ra_max_um,
            tl.degraded_wall_max_um,
            if tl.good_points > 0 { format!(" | 정상 공구 벽 {} 점: Ra 평균 {:.2} µm · 벽 위치 오차 P90 {:.1} µm", tl.good_points, tl.good_ra_mean_um, tl.good_wall_p90_um) } else { " | 정상 공구로 깎은 최종 벽 없음".to_string() }
        ));
        for z in tl.degraded_zones.iter() {
            v.push(format!("[QUAL] {}", z));
        }
    }
    let fit = &a.fit;
    v.push(format!(
        "[AUDIT] 적합도: {} × 모재 {} — {} (점수 {:.2}, 약한 쪽 {:.2} · 무딘 쪽 {:.2} 단계) · 마모 ×{:.2} · 치핑 ×{:.1} · 구성인선 ×{:.2} · 조도 ×{:.2} · 칩 막힘 {:.0}%{}",
        fit.wp_group,
        fit.substrate,
        fit.verdict_label(),
        fit.score,
        fit.sum_weak,
        fit.sum_blunt,
        fit.wear_factor,
        fit.chip_factor,
        fit.bue_factor,
        fit.ra_factor,
        fit.evac_risk * 100.0,
        {
            let r = fit.reasons();
            if r.is_empty() { String::new() } else { format!(" | {}", r.join(" · ")) }
        }
    ));
    for fl in a.flags.iter() {
        v.push(format!("[AUDIT][{}] {} (값 {:.3}, 기대 {}) — {}", ["정보", "주의", "신뢰불가"][fl.severity.min(2) as usize], fl.id, fl.value, fl.expected, fl.message));
    }
    for u in a.unknowns.iter() {
        v.push(format!("[UNKNOWN] {} · 사용 {} · 범위 {} · 영향 {} → {}", u.key, u.used, u.range, u.effect, u.resolve));
    }
    for m in a.measure_next.iter() {
        v.push(format!("[AUDIT][다음 실측] {} · 현재 근거: {} → {}", m.target, m.basis, m.measurement));
    }
    v
}
