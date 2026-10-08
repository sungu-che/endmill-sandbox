use crate::gcode::{GCodeLine, GCodeProgram, ToolPathSegment};
use crate::loadsim::{self, HeightmapView, LoadSimReport, SegWallStat, SimSample};
use crate::ml::laya::{AdvisorAnswer, LayaAdvisor};
use crate::physics::{self, CutAnalysis, CutContext, Engagement, ThermalBody, ToolGeometry, WearReference};
use crate::profile::{CoolantMethod, MachiningProfile};
use crate::sds::{decay_shape, DecayShape, Scope, SdsStore, Track};
use crate::wear::{WearComparison, WearState};
use crate::wearlog::WearLog;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const COMP_AGREEMENT_AXIS: &str = "comp_rule_vs_laya";
pub const SPLIT_FIRST: f64 = 0.7;
pub const REDUCED_FEED: f64 = 0.6;
pub const MAX_FEED_BOOST: f64 = 1.5;
pub const CLEAR_Z: f64 = 2.0;
pub const MODEL_DEFL_ERR: f64 = 0.3;
pub const THERMAL_MODEL_ERR: f64 = 0.5;
pub const SHORT_TIME_POWER: f64 = 1.5;
pub const SEVERE_CHATTER_MARGIN: f64 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompOptions {
    #[serde(default = "default_target")]
    pub target_fraction: f64,
    #[serde(default)]
    pub checkpoint: bool,
    #[serde(default = "default_air")]
    pub air_blast_code: String,
    #[serde(default = "default_air_off")]
    pub air_off_code: String,
    #[serde(default)]
    pub cooldown_air: bool,
    #[serde(default = "default_slow_rpm")]
    pub slow_rpm: u32,
    #[serde(default)]
    pub use_orient: bool,
    #[serde(default = "default_ramp")]
    pub ramp_ms: u32,
    #[serde(default = "default_blow")]
    pub blow_ms: u32,
    #[serde(default = "default_max_cooldown")]
    pub max_cooldown_s: f64,
    #[serde(default = "yes")]
    pub use_laya: bool,
    #[serde(default = "yes")]
    pub use_ttm: bool,
    #[serde(default)]
    pub measured_radial_um: Option<f64>,
}

fn yes() -> bool {
    true
}
fn default_target() -> f64 {
    0.15
}
fn default_air() -> String {
    "M83".into()
}
fn default_air_off() -> String {
    "M84".into()
}
fn default_slow_rpm() -> u32 {
    60
}
fn default_ramp() -> u32 {
    2000
}
fn default_blow() -> u32 {
    1500
}
fn default_max_cooldown() -> f64 {
    600.0
}

impl Default for CompOptions {
    fn default() -> Self {
        Self {
            target_fraction: default_target(),
            checkpoint: false,
            air_blast_code: default_air(),
            air_off_code: default_air_off(),
            cooldown_air: false,
            slow_rpm: default_slow_rpm(),
            use_orient: false,
            ramp_ms: default_ramp(),
            blow_ms: default_blow(),
            max_cooldown_s: default_max_cooldown(),
            use_laya: true,
            use_ttm: true,
            measured_radial_um: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompAction {
    Execute,
    ExecuteReduced,
    Split,
    Skip,
    ToolChange,
    Hold,
}

impl CompAction {
    pub fn all() -> [CompAction; 6] {
        [
            CompAction::Execute,
            CompAction::ExecuteReduced,
            CompAction::Split,
            CompAction::Skip,
            CompAction::ToolChange,
            CompAction::Hold,
        ]
    }

    pub fn key(&self) -> &'static str {
        match self {
            CompAction::Execute => "execute",
            CompAction::ExecuteReduced => "execute_reduced",
            CompAction::Split => "split",
            CompAction::Skip => "skip",
            CompAction::ToolChange => "tool_change",
            CompAction::Hold => "hold",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CompAction::Execute => "2차 보정 1회 실행",
            CompAction::ExecuteReduced => "감속 2차 보정",
            CompAction::Split => "분할 보정 (보정 + 스프링 패스)",
            CompAction::Skip => "보정 안 함",
            CompAction::ToolChange => "공구 교체 후 보정",
            CompAction::Hold => "보류 (측정 후 재계획)",
        }
    }

    pub fn english(&self) -> &'static str {
        match self {
            CompAction::Execute => "run one light compensation pass at the planned offset and feed",
            CompAction::ExecuteReduced => "run one compensation pass at reduced feed to limit cutting force and deflection",
            CompAction::Split => "split the compensation into an offset pass and a final light spring pass",
            CompAction::Skip => "skip the compensation pass and accept the current wall size",
            CompAction::ToolChange => "replace the worn end mill first, then run the compensation pass",
            CompAction::Hold => "stop and measure the part and the tool before any further cutting",
        }
    }

    pub fn rank(&self) -> u8 {
        match self {
            CompAction::Execute => 0,
            CompAction::ExecuteReduced => 1,
            CompAction::Split => 2,
            CompAction::Skip | CompAction::ToolChange => 3,
            CompAction::Hold => 4,
        }
    }

    pub fn cuts(&self) -> bool {
        matches!(self, CompAction::Execute | CompAction::ExecuteReduced | CompAction::Split | CompAction::ToolChange)
    }

    pub fn from_key(k: &str) -> Option<CompAction> {
        CompAction::all().into_iter().find(|a| a.key() == k)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompGate {
    pub id: String,
    pub label: String,
    pub value: f64,
    pub limit: f64,
    pub unit: String,
    pub severity: u8,
    pub blocks: Vec<CompAction>,
    pub sole: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct PassEval {
    pub ae_um: f64,
    pub fz_mm: f64,
    pub force_n: f64,
    #[serde(default)]
    pub force_peak_n: f64,
    pub defl_um: f64,
    pub wall_um: f64,
    pub chatter: f64,
    pub temp_c: f64,
    pub h_max_um: f64,
    pub vb_rate_mm_min: f64,
    pub engage_deg: f64,
    pub power_kw: f64,
    #[serde(default)]
    pub wall_stress_mpa: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompPiece {
    pub f0: f64,
    pub f1: f64,
    pub n: usize,
    pub p50_um: f64,
    pub min_um: f64,
    pub max_um: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompZone {
    pub seg: usize,
    pub len_mm: f64,
    pub nx: f64,
    pub ny: f64,
    pub ap_mm: f64,
    pub z_mm: f64,
    pub down: bool,
    pub n: usize,
    pub residual_p10_um: f64,
    pub residual_p50_um: f64,
    pub residual_p90_um: f64,
    pub residual_max_um: f64,
    pub residual_min_um: f64,
    pub wall_mm: f64,
    pub needs: bool,
    pub excluded: bool,
    #[serde(default)]
    pub thin: bool,
    pub localized: bool,
    pub n_eff: f64,
    pub offset_um: f64,
    pub offset_reduced_um: f64,
    pub offset_split_um: f64,
    pub offset_fresh_um: f64,
    pub execute: PassEval,
    pub reduced: PassEval,
    pub split_first: PassEval,
    pub split_final: PassEval,
    pub fresh: PassEval,
    pub tol_um: f64,
    pub pieces: Vec<CompPiece>,
    pub raw_offsets: [f64; 4],
    #[serde(default)]
    pub wear_bias_um: f64,
    #[serde(default)]
    pub thermal_bias_um: f64,
    #[serde(default)]
    pub cause_defl_um: f64,
    #[serde(default)]
    pub cause_wall_um: f64,
    #[serde(default)]
    pub cause_wear_um: f64,
    #[serde(default)]
    pub cause_thermal_um: f64,
}

impl CompZone {
    pub fn bias_for(&self, a: CompAction) -> f64 {
        self.thermal_bias_um + if a == CompAction::ToolChange { 0.0 } else { self.wear_bias_um }
    }

    pub fn path_offsets_um(&self, a: CompAction, scale: f64) -> Vec<f64> {
        if self.pieces.is_empty() {
            return vec![self.offset_for(a) * scale - self.residual_p50_um];
        }
        self.piece_offsets(a).iter().zip(self.pieces.iter()).map(|(o, p)| o * scale - p.p50_um).collect()
    }

    pub fn path_offset_for(&self, a: CompAction) -> f64 {
        if !self.needs {
            return 0.0;
        }
        let v = self.path_offsets_um(a, 1.0);
        v.iter().sum::<f64>() / v.len().max(1) as f64
    }

    pub fn raw_offset_for(&self, a: CompAction) -> f64 {
        match a {
            CompAction::ExecuteReduced => self.raw_offsets[1],
            CompAction::Split => self.raw_offsets[2],
            CompAction::ToolChange => self.raw_offsets[3],
            _ => self.raw_offsets[0],
        }
    }

    pub fn piece_offsets(&self, a: CompAction) -> Vec<f64> {
        if !self.needs {
            return vec![0.0; self.pieces.len()];
        }
        let base = self.raw_offset_for(a);
        let e = self.eval_for(a, 1);
        let e2 = e.defl_um + e.wall_um;
        let bias = self.bias_for(a);
        let gain = if base > 1.0 && e2 < 0.8 * base { 1.0 / (1.0 - e2 / base) } else { 1.0 };
        let k = if base > 1e-6 { (e2 / base).clamp(0.0, 0.8) } else { 0.0 };
        self.pieces
            .iter()
            .map(|p| {
                let raw = base + (p.p50_um - self.residual_p50_um) * gain;
                let cap = p.p50_um + (p.min_um * k + bias + 0.8 * self.tol_um) / (1.0 - k);
                raw.min(cap).max(0.0)
            })
            .collect()
    }

    pub fn capped_pieces(&self, a: CompAction) -> usize {
        if !self.needs {
            return 0;
        }
        let base = self.raw_offset_for(a);
        let e = self.eval_for(a, 1);
        let e2 = e.defl_um + e.wall_um;
        let gain = if base > 1.0 && e2 < 0.8 * base { 1.0 / (1.0 - e2 / base) } else { 1.0 };
        self.pieces
            .iter()
            .zip(self.piece_offsets(a).iter())
            .filter(|(p, o)| base + (p.p50_um - self.residual_p50_um) * gain > **o + 0.5)
            .count()
    }

    pub fn offset_for(&self, a: CompAction) -> f64 {
        match a {
            CompAction::ExecuteReduced => self.offset_reduced_um,
            CompAction::Split => self.offset_split_um,
            CompAction::ToolChange => self.offset_fresh_um,
            _ => self.offset_um,
        }
    }

    pub fn eval_for(&self, a: CompAction, pass: usize) -> PassEval {
        match a {
            CompAction::ExecuteReduced => self.reduced,
            CompAction::Split => {
                if pass == 0 {
                    self.split_first
                } else {
                    self.split_final
                }
            }
            CompAction::ToolChange => self.fresh,
            _ => self.execute,
        }
    }

    pub fn after_for(&self, a: CompAction) -> (f64, f64, f64) {
        if !self.needs || !a.cuts() {
            return (self.residual_p50_um, self.residual_min_um, self.residual_max_um);
        }
        let e = self.eval_for(a, 1);
        let e2 = e.defl_um + e.wall_um;
        let bias = self.bias_for(a);
        let o = self.raw_offset_for(a).max(1e-6);
        let fin = |path: f64, r: f64| {
            let d = (path + r).max(0.0);
            r - (d - e2 * d / o - bias).max(0.0)
        };
        if self.pieces.is_empty() {
            let path = self.offset_for(a) - self.residual_p50_um;
            return (fin(path, self.residual_p50_um), fin(path, self.residual_min_um), fin(path, self.residual_max_um));
        }
        let paths = self.path_offsets_um(a, 1.0);
        let mut p50 = 0.0;
        let mut mn = f64::INFINITY;
        let mut mx = f64::NEG_INFINITY;
        let mut wsum = 0.0;
        for (p, path) in self.pieces.iter().zip(paths.iter()) {
            let w = p.n.max(1) as f64;
            p50 += fin(*path, p.p50_um) * w;
            wsum += w;
            mn = mn.min(fin(*path, p.min_um));
            mx = mx.max(fin(*path, p.max_um));
        }
        (p50 / wsum.max(1.0), mn, mx)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResidualSummary {
    pub n: usize,
    pub zones: usize,
    pub tolerance_um: f64,
    pub mean_um: f64,
    pub p10_um: f64,
    pub p50_um: f64,
    pub p90_um: f64,
    pub max_um: f64,
    pub min_um: f64,
    pub z_max: f64,
    pub n_eff: f64,
    pub wear_scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompDecision {
    pub need: bool,
    pub allowed: Vec<CompAction>,
    pub rule_action: CompAction,
    pub laya: Option<AdvisorAnswer>,
    pub laya_action: Option<CompAction>,
    pub laya_shape: Option<DecayShape>,
    pub laya_error: Option<String>,
    pub final_action: CompAction,
    pub decided_by: String,
    pub operator_confirm: bool,
    pub agreement: Option<bool>,
    pub agreement_rate: Option<(f64, u64)>,
    pub state_text: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckpointPlan {
    pub lines: Vec<String>,
    pub time_s: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PassPlan {
    pub name: String,
    pub offset_scale: f64,
    pub feed_scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompDraft {
    pub gates: Vec<CompGate>,
    pub zones: Vec<CompZone>,
    pub residual: ResidualSummary,
    pub need: bool,
    pub allowed: Vec<CompAction>,
    pub rule_action: CompAction,
    pub reasons: Vec<String>,
    pub state_text: String,
    pub vb_end1_mm: f64,
    pub radial_end1_um: f64,
    pub radial_used_um: f64,
    pub radial_source: String,
    pub vb_end2_p50_mm: f64,
    pub vb_end2_p90_mm: f64,
    pub vb_limit_mm: f64,
    pub cooldown_s: f64,
    pub bulk_excess_k: f64,
    pub thermal_err_um: f64,
    pub thermal_err_after_um: f64,
    pub h_min_um: f64,
    pub fz_nominal_mm: f64,
    pub rpm: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompOutcome {
    pub draft: CompDraft,
    pub decision: CompDecision,
    pub passes: Vec<PassPlan>,
    pub checkpoint: Option<CheckpointPlan>,
    pub program_name: String,
    pub program_text: String,
    pub segments: Vec<ToolPathSegment>,
    pub samples: Vec<SimSample>,
    pub segment_end_s: Vec<f64>,
    pub pre_s: f64,
    pub time_s: f64,
    pub cut_time_s: f64,
    pub vb_after_mm: f64,
    pub after_p50_um: f64,
    pub after_min_um: f64,
    pub after_max_um: f64,
    pub parts_left_p50: Option<f64>,
    pub parts_left_p90: Option<f64>,
    pub report_line: String,
    pub notes: Vec<String>,
}

pub fn wear_scope(p: &MachiningProfile) -> Scope {
    Scope::new(
        Track::Wear,
        &p.endmill_setting.signature(),
        &crate::pipeline::material_scope_key(&p.workpiece_setup.effective_material()),
    )
}

struct Env<'a> {
    profile: &'a MachiningProfile,
    ctx: &'a CutContext,
    nominal: &'a CutAnalysis,
    body: ThermalBody,
    reference: WearReference,
    rpm: f64,
    flutes: f64,
    bulk_excess: f64,
}

fn sin_max(ae_mm: f64, d: f64) -> f64 {
    let x = (ae_mm / d.max(1e-6)).clamp(1e-9, 1.0);
    if x >= 0.5 {
        1.0
    } else {
        2.0 * (x * (1.0 - x)).sqrt()
    }
}

fn machine_fz_cap(profile: &MachiningProfile, rpm: f64, flutes: f64) -> Option<f64> {
    let m = profile.machine.max_feed_mm_min;
    if m > 0.0 && rpm > 0.0 {
        Some(m / (rpm * flutes.max(1.0)))
    } else {
        None
    }
}

fn eval_pass(env: &Env, tool: &ToolGeometry, zone: &SegWallStat, ae_mm: f64, fz: f64) -> PassEval {
    let ctx = env.ctx;
    let d = tool.diameter_mm;
    let ap = zone.ap_mm.max(0.05).min(tool.loc_mm);
    let ae = ae_mm.clamp(1e-4, d);
    let eng = Engagement::side(ap, ae, d, zone.down);
    let vce = physics::effective_vc(tool, env.rpm, ap);
    let h_nom = fz * eng.max_sin();
    let h_est = (h_nom * 0.64).max(1e-5);
    let co = physics::cutting_coeffs_at(ctx, vce, h_est);
    let pitch = std::f64::consts::TAU / env.flutes.max(1.0);
    let steps = ((6.0 * pitch / eng.span().max(1e-6)).ceil() as usize).clamp(48, 360);
    let f = physics::mechanistic_forces(tool, &co, fz, env.rpm, &eng, steps, 10);
    let (defl, away) = loadsim::wall_deflection(tool, ctx.calib.deflection, &f, &eng).unwrap_or((0.0, 0.0));
    let wall = if zone.wall_k_n_mm > 0.0 {
        (away / zone.wall_k_n_mm * 1000.0).clamp(-500.0 * zone.wall_mm.max(0.01), 500.0 * zone.wall_mm.max(0.01))
    } else {
        0.0
    };
    let chatter = crate::dynamics::stability_margin(tool, &co, &eng, f.h_mean_mm, env.rpm, ap, ctx.calib.deflection, vce, ctx.wp.process_damping);
    let th = physics::thermal_preheated(ctx, &co, &f, vce, env.rpm, &eng, env.body.mass_kg, env.body.area_m2, env.bulk_excess);
    let wr = physics::wear(ctx, &f, &th, vce, fz.max(1e-6), &eng, env.reference);
    PassEval {
        ae_um: ae * 1000.0,
        fz_mm: fz,
        force_n: f.f_res_mean,
        force_peak_n: f.f_res_peak,
        defl_um: defl,
        wall_um: wall,
        chatter: if chatter.is_finite() { chatter } else { 99.0 },
        temp_c: th.interface_c,
        h_max_um: h_nom * 1000.0,
        vb_rate_mm_min: wr.vb_rate_mm_per_min * env.nominal.trajectory.kappa.max(1e-6),
        engage_deg: eng.span().to_degrees(),
        power_kw: f.cutting_power_kw / env.profile.machine.efficiency.max(0.1),
        wall_stress_mpa: away.abs() * zone.wall_sigma_per_n,
    }
}

fn chip_feed(env: &Env, ae_mm: f64, h_min_mm: f64) -> f64 {
    let d = env.ctx.tool.diameter_mm.max(0.1);
    let sinmax = sin_max(ae_mm, d);
    let fz_nom = env.nominal.fz_mm.max(1e-5);
    let h1 = (fz_nom * sin_max(env.profile.conditions.radial_doc_mm, d)).max(fz_nom * 0.2);
    let need = 1.5 * h_min_mm / sinmax;
    let cap = (fz_nom * MAX_FEED_BOOST).min(h1 / sinmax).max(fz_nom);
    let fz = need.clamp(fz_nom, cap);
    match machine_fz_cap(env.profile, env.rpm, env.flutes) {
        Some(m) => fz.min(m).max(1e-5),
        None => fz,
    }
}

fn min_cut_ae_um(d: f64, h_min_mm: f64, fz_max: f64) -> f64 {
    let c = (h_min_mm / (2.0 * fz_max.max(1e-9))).powi(2);
    if c >= 0.25 {
        return 0.5 * d * 1000.0;
    }
    0.5 * (1.0 - (1.0 - 4.0 * c).sqrt()) * d * 1000.0
}

#[allow(clippy::too_many_arguments)]
fn solve_offset(env: &Env, tool: &ToolGeometry, zone: &SegWallStat, r50: f64, target: f64, fz_scale: f64, h_min_mm: f64, bias_um: f64) -> (f64, PassEval) {
    let mut delta = (r50 - target + bias_um).max(0.0);
    let mut ev = PassEval::default();
    for _ in 0..8 {
        let ae = ((delta - bias_um) / 1000.0).max(0.001);
        let fz = chip_feed(env, ae, h_min_mm) * fz_scale;
        ev = eval_pass(env, tool, zone, ae, fz);
        let e2 = ev.defl_um + ev.wall_um;
        let next = (r50 - target + e2 + bias_um).max(0.0);
        if (next - delta).abs() < 0.3 {
            delta = next;
            break;
        }
        delta = 0.5 * delta + 0.5 * next;
    }
    (delta, ev)
}

#[allow(clippy::too_many_arguments)]
fn solve_split(env: &Env, tool: &ToolGeometry, zone: &SegWallStat, r50: f64, target: f64, h_min_mm: f64, start: f64, bias_um: f64) -> (f64, PassEval, PassEval) {
    let mut o = start.max(0.0);
    let mut first = PassEval::default();
    let mut last = PassEval::default();
    for _ in 0..8 {
        let ae_a = ((SPLIT_FIRST * o - bias_um) / 1000.0).max(0.001);
        first = eval_pass(env, tool, zone, ae_a, chip_feed(env, ae_a, h_min_mm));
        let ae_b = (((1.0 - SPLIT_FIRST) * o + first.defl_um + first.wall_um).max(1.0)) / 1000.0;
        last = eval_pass(env, tool, zone, ae_b, chip_feed(env, ae_b, h_min_mm));
        let next = (r50 - target + last.defl_um + last.wall_um + bias_um).max(0.0);
        if (next - o).abs() < 0.3 {
            o = next;
            break;
        }
        o = 0.5 * o + 0.5 * next;
    }
    (o, first, last)
}

fn heights(h: &HeightmapView, x: f64, y: f64) -> Option<f64> {
    if h.nx == 0 || h.ny == 0 || h.cell_mm <= 0.0 {
        return None;
    }
    let i = ((x - h.x0) / h.cell_mm).floor();
    let j = ((y - h.y0) / h.cell_mm).floor();
    if i < 0.0 || j < 0.0 || i as usize >= h.nx || j as usize >= h.ny {
        return None;
    }
    let v = h.z[j as usize * h.nx + i as usize];
    if v.is_finite() {
        Some(v as f64)
    } else {
        None
    }
}

fn clear_at(h: &HeightmapView, x: f64, y: f64, r: f64, z: f64) -> bool {
    let tol = (0.5 * h.cell_mm).max(0.05);
    let mut pts = vec![(x, y)];
    for k in 0..12 {
        let a = k as f64 / 12.0 * std::f64::consts::TAU;
        pts.push((x + 0.95 * r * a.cos(), y + 0.95 * r * a.sin()));
    }
    pts.iter().all(|(px, py)| heights(h, *px, *py).map(|v| v <= z + tol).unwrap_or(true))
}

fn utc_parts(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32)
}

pub fn utc_stamp(ms: i64) -> String {
    let (y, m, d, hh, mm, ss) = utc_parts(ms);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}Z", y, m, d, hh, mm, ss)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let i = pos.floor() as usize;
    let j = (i + 1).min(sorted.len() - 1);
    sorted[i] + (sorted[j] - sorted[i]) * (pos - i as f64)
}

pub fn evaluate(
    profile: &MachiningProfile,
    ctx: &CutContext,
    rep: &LoadSimReport,
    log: &WearLog,
    opts: &CompOptions,
    tool_image: Option<&WearComparison>,
) -> CompDraft {
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let nominal = physics::analyze_cut(ctx, &profile.conditions, true, body);
    let reference = physics::reference_state(ctx);
    let tol_um = (ctx.tolerance_mm * 1000.0).max(1.0);
    let target = opts.target_fraction.clamp(-0.5, 0.8) * tol_um;
    let vb_limit = ctx.vb_limit_mm();
    let tan_clear = ctx.tool.clearance_deg.to_radians().tan().max(1e-6);
    let vb1 = rep.vb_end_mm;
    let radial_sim = rep.radial_loss_end_um;
    let mut notes: Vec<String> = Vec::new();
    let image_radial = tool_image.and_then(|w| w.geometry.as_ref()).map(|g| g.mean_loss_tip_um).filter(|v| v.is_finite() && *v > 0.0);
    let (radial_used, radial_source) = match (opts.measured_radial_um.filter(|v| v.is_finite() && *v >= 0.0), image_radial) {
        (Some(m), _) => (m, "measured".to_string()),
        (None, Some(i)) => (i, "image".to_string()),
        _ => (radial_sim, "simulated".to_string()),
    };
    let wear_scale = if radial_sim > 1e-6 { (radial_used / radial_sim).clamp(0.0, 20.0) } else { 1.0 };
    if radial_source != "simulated" {
        notes.push(format!(
            "공구 반경 마모를 {} 값 {:.1} µm 로 대체 (시뮬레이션 {:.1} µm, 배율 ×{:.2}) — 벽면 잔여량의 마모 성분에 반영",
            if radial_source == "measured" { "실측" } else { "이미지 측정" },
            radial_used,
            radial_sim,
            wear_scale
        ));
    }
    let vb_now = if radial_source == "simulated" { vb1 } else { radial_used / 1000.0 / tan_clear };
    let ambient = ctx.env.ambient_c;
    let bulk_excess = (rep.wp_temp_end_c - ambient.min(rep.bulk_c)).max(0.0);
    let dwell_air = opts.cooldown_air && air_codes(profile, opts).is_some();
    let h_dwell = if dwell_air { physics::CoolantState::workpiece_h_of(&CoolantMethod::AirBlast, 0.0) } else { ctx.coolant.idle_workpiece_h() };
    let tau = body.mass_kg * ctx.wp.specific_heat / (h_dwell * body.area_m2).max(1e-6);
    let thermal_err = ctx.wp.expansion * body.size_mm * bulk_excess * 1000.0;
    let allowed_err = 0.25 * tol_um;
    let comp_ceiling = allowed_err / THERMAL_MODEL_ERR;
    let cooldown = if thermal_err > comp_ceiling { (tau * (thermal_err / comp_ceiling).ln()).max(0.0) } else { 0.0 };
    let cooldown_s = cooldown.min(opts.max_cooldown_s.max(0.0)).ceil();
    let decay = (-cooldown_s / tau.max(1e-6)).exp();
    let thermal_after = thermal_err * decay;
    let excess_after = bulk_excess * decay;
    let datum = profile.workpiece_setup.zero_point;
    let env = Env {
        profile,
        ctx,
        nominal: &nominal,
        body,
        reference,
        rpm: profile.conditions.spindle_rpm.max(1) as f64,
        flutes: ctx.tool.flutes.max(1) as f64,
        bulk_excess: excess_after,
    };
    let h_min_mm = ctx.wp.min_chip_ratio * ctx.tool.edge_radius_um / 1000.0;
    let mut worn = ctx.tool.clone();
    worn.flank_wear_mm = vb_now.max(0.0);
    let mut fresh = ctx.tool.clone();
    fresh.flank_wear_mm = 0.0;
    let mut all_res: Vec<f64> = Vec::new();
    for w in rep.wall_errors.iter().filter(|w| w.final_wall) {
        all_res.push(w.total_um - w.wear_um + w.wear_um * wear_scale);
    }
    let mut zones: Vec<CompZone> = Vec::new();
    for s in rep.seg_walls.iter().filter(|s| s.n >= 3 && s.len_mm > 0.2) {
        let wadj = s.wear_um * (wear_scale - 1.0);
        let (p10, p50, p90, mx, mn) = (s.p10_um + wadj, s.p50_um + wadj, s.p90_um + wadj, s.max_um + wadj, s.min_um + wadj);
        let excluded = mn < -tol_um;
        let wants = !excluded && (p90 > tol_um || p50 > 0.6 * tol_um);
        let zmax = if s.sd_um > 1e-9 { (mx - s.mean_um - wadj) / s.sd_um } else { 0.0 };
        let n_eff = (0.5 * zmax * zmax).min(20.0).exp();
        let localized = s.n >= 8 && n_eff > 10.0 * s.n as f64 && mx > tol_um;
        let thermal_bias = ctx.wp.expansion * excess_after * ((s.x_mm - datum.0) * s.nx + (s.y_mm - datum.1) * s.ny) * 1000.0;
        let bias_worn = radial_used + thermal_bias;
        let bias_fresh = thermal_bias;
        let (delta_e, ev_e) = solve_offset(&env, &worn, s, p50, target, 1.0, h_min_mm, bias_worn);
        let (delta_r, ev_r) = solve_offset(&env, &worn, s, p50, target, REDUCED_FEED, h_min_mm, bias_worn);
        let (delta_f, ev_f) = solve_offset(&env, &fresh, s, p50, target, 1.0, h_min_mm, bias_fresh);
        let (delta_s, ev_a, ev_b) = solve_split(&env, &worn, s, p50, target, h_min_mm, delta_e, bias_worn);
        let thin = wants && ev_e.h_max_um.min(ev_f.h_max_um) < h_min_mm * 1000.0;
        let needs = wants && !thin;
        let cap = |d: f64, e: &PassEval, b: f64| {
            let k = if d > 1e-6 { ((e.defl_um + e.wall_um) / d).clamp(0.0, 0.8) } else { 0.0 };
            (p50 + (mn * k + b + 0.8 * tol_um) / (1.0 - k)).max(0.0)
        };
        let limit = |d: f64, e: &PassEval, b: f64| if needs { d.min(cap(d, e, b)) } else { 0.0 };
        let delta = limit(delta_e, &ev_e, bias_worn);
        zones.push(CompZone {
            seg: s.seg,
            len_mm: s.len_mm,
            nx: s.nx,
            ny: s.ny,
            ap_mm: s.ap_mm,
            z_mm: s.z_mid - 0.5 * s.ap_mm,
            down: s.down,
            n: s.n,
            residual_p10_um: p10,
            residual_p50_um: p50,
            residual_p90_um: p90,
            residual_max_um: mx,
            residual_min_um: mn,
            wall_mm: s.wall_mm,
            needs,
            excluded,
            thin,
            localized,
            n_eff,
            offset_um: delta,
            offset_reduced_um: limit(delta_r, &ev_r, bias_worn),
            offset_split_um: limit(delta_s, &ev_b, bias_worn),
            offset_fresh_um: limit(delta_f, &ev_f, bias_fresh),
            execute: ev_e,
            reduced: ev_r,
            split_first: ev_a,
            split_final: ev_b,
            fresh: ev_f,
            tol_um,
            raw_offsets: if needs { [delta_e, delta_r, delta_s, delta_f] } else { [0.0; 4] },
            pieces: s
                .pieces
                .iter()
                .map(|pc| CompPiece {
                    f0: pc.f0,
                    f1: pc.f1,
                    n: pc.n,
                    p50_um: pc.p50_um + wadj,
                    min_um: pc.min_um + wadj,
                    max_um: pc.max_um + wadj,
                })
                .collect(),
            wear_bias_um: radial_used,
            thermal_bias_um: thermal_bias,
            cause_defl_um: s.defl_um,
            cause_wall_um: s.workpiece_um,
            cause_wear_um: s.wear_um + wadj,
            cause_thermal_um: s.thermal_um,
        });
    }
    all_res.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n_all = all_res.len();
    let mean_all = if n_all > 0 { all_res.iter().sum::<f64>() / n_all as f64 } else { 0.0 };
    let sd_all = if n_all > 1 { (all_res.iter().map(|v| (v - mean_all).powi(2)).sum::<f64>() / (n_all - 1) as f64).sqrt() } else { 0.0 };
    let z_max = if sd_all > 1e-9 { (all_res.last().cloned().unwrap_or(0.0) - mean_all) / sd_all } else { 0.0 };
    let residual = ResidualSummary {
        n: n_all,
        zones: zones.len(),
        tolerance_um: tol_um,
        mean_um: mean_all,
        p10_um: quantile(&all_res, 0.1),
        p50_um: quantile(&all_res, 0.5),
        p90_um: quantile(&all_res, 0.9),
        max_um: all_res.last().cloned().unwrap_or(0.0),
        min_um: all_res.first().cloned().unwrap_or(0.0),
        z_max,
        n_eff: (0.5 * z_max * z_max).min(20.0).exp(),
        wear_scale,
    };
    let need = zones.iter().any(|z| z.needs);
    let active: Vec<&CompZone> = zones.iter().filter(|z| z.needs).collect();
    let t2 = active
        .iter()
        .map(|z| z.len_mm / (z.execute.fz_mm * env.rpm * env.flutes).max(1e-6) * 60.0)
        .sum::<f64>();
    let rate2 = active.iter().map(|z| z.execute.vb_rate_mm_min).fold(0.0, f64::max);
    let dvb2 = rate2 * t2 / 60.0;
    let vb2_p50 = vb_now + dvb2;
    let vb2_p90 = vb_now + dvb2 * log.p90_growth_factor() + log.backtests.iter().map(|b| b.weight * b.rmse_um).sum::<f64>() / 1000.0 * 1.2816;
    let mut gates: Vec<CompGate> = Vec::new();
    let cutting = [CompAction::Execute, CompAction::ExecuteReduced, CompAction::Split];
    let worst = |f: &dyn Fn(&CompZone) -> f64| active.iter().map(|z| f(z)).fold(f64::NEG_INFINITY, f64::max);
    let least = |f: &dyn Fn(&CompZone) -> f64| active.iter().map(|z| f(z)).fold(f64::INFINITY, f64::min);
    let p90_max = zones.iter().filter(|z| !z.excluded).map(|z| z.residual_p90_um).fold(0.0, f64::max);
    let thin_zones: Vec<usize> = zones.iter().filter(|z| z.thin).map(|z| z.seg).collect();
    let n_excluded = zones.iter().filter(|z| z.excluded).count();
    gates.push(CompGate {
        id: "residual_need".into(),
        label: "최종 벽면 잔여량".into(),
        value: p90_max,
        limit: tol_um,
        unit: "µm".into(),
        severity: if need || !thin_zones.is_empty() || n_excluded > 0 { 1 } else { 0 },
        blocks: vec![],
        sole: false,
        message: if need {
            format!(
                "보정이 필요한 구간 {}개 (상위 10% 잔여량 최대 {:.1} µm > 공차 {:.0} µm 또는 중앙값 > 공차의 60%){}",
                active.len(),
                p90_max,
                tol_um,
                if thin_zones.is_empty() { String::new() } else { format!(" · 최소 절입 미만이라 빠진 구간 {}개", thin_zones.len()) }
            )
        } else if !thin_zones.is_empty() {
            format!("보정 대상 구간 {}개가 모두 최소 절삭 절입보다 얇아 2차로 깎을 대상이 없습니다 (상위 10% 최대 {:.1} µm, 공차 {:.0} µm)", thin_zones.len(), p90_max, tol_um)
        } else if zones.is_empty() {
            "최종 벽면이 남는 구간이 없어 (바닥·슬롯 위주 패턴) 측면 보정 대상이 없습니다".into()
        } else if n_excluded > 0 {
            format!("보정할 수 있는 구간은 모두 공차 안 (상위 10% 최대 {:.1} µm ≤ {:.0} µm) · 1차에서 −공차 밖으로 과삭된 구간 {}개는 2차로 복구 불가", p90_max, tol_um, n_excluded)
        } else {
            format!("모든 구간이 공차 안 (상위 10% 최대 {:.1} µm ≤ {:.0} µm)", p90_max, tol_um)
        },
    });
    if !thin_zones.is_empty() {
        let fz_nom = nominal.fz_mm.max(1e-5);
        let boost = (fz_nom * MAX_FEED_BOOST).max(fz_nom);
        let fz_max = match machine_fz_cap(profile, env.rpm, env.flutes) {
            Some(m) => boost.min(m),
            None => boost,
        };
        let ae_min_um = min_cut_ae_um(ctx.tool.diameter_mm, h_min_mm, fz_max);
        let thin_ae = zones.iter().filter(|z| z.thin).map(|z| z.execute.ae_um.min(z.fresh.ae_um)).fold(0.0, f64::max);
        let thin_p90 = zones.iter().filter(|z| z.thin).map(|z| z.residual_p90_um).fold(0.0, f64::max);
        gates.push(CompGate {
            id: "min_engagement".into(),
            label: "최소 반경 절입 (날끝 반경)".into(),
            value: thin_ae,
            limit: ae_min_um,
            unit: "µm".into(),
            severity: if thin_p90 > tol_um { 2 } else { 1 },
            blocks: vec![],
            sole: false,
            message: format!(
                "구간 {:?} 은 2차 반경 절입(구간 중앙값 기준 최대 {:.1} µm)이 최소 절삭 절입 {:.1} µm 보다 작아 2차에서 깎지 않습니다. 이송 상한 {:.4} mm/날로도 최대 칩이 {:.2} µm (= {:.2} × 날끝 반경 {:.1} µm) 에 못 미쳐 잘리지 않고 문질러지기 때문입니다{} · 이 구간에 남는 상위 10% 잔여량 최대 {:.1} µm{}",
                thin_zones,
                thin_ae,
                ae_min_um,
                fz_max,
                h_min_mm * 1000.0,
                ctx.wp.min_chip_ratio,
                ctx.tool.edge_radius_um,
                if ctx.wp.work_hardening >= 0.1 { " (가공경화 소재라 문지르면 표면이 경화)" } else { " (문지르면 열과 버니싱만 생김)" },
                thin_p90,
                if thin_p90 > tol_um { format!(" > 공차 {:.0} µm → 측정 후 재계획 (날끝 반경이 작은 공구나 다른 공정 검토)", tol_um) } else { String::new() }
            ),
        });
    }
    let excluded: Vec<usize> = zones.iter().filter(|z| z.excluded).map(|z| z.seg).collect();
    let excluded_up = zones.iter().filter(|z| z.excluded && !z.down).count();
        gates.push(CompGate {
        id: "overcut_pass1".into(),
        label: "1차 과삭 (이미 공차 밖)".into(),
        value: residual.min_um,
        limit: -tol_um,
        unit: "µm".into(),
        severity: if excluded.is_empty() { 0 } else { 2 },
        blocks: vec![],
        sole: false,
        message: if excluded.is_empty() {
            "1차에서 공차 밖으로 과삭된 구간 없음".into()
        } else {
            format!(
                "구간 {:?} 은 1차에서 이미 −공차 밖으로 과삭 → 보정으로 복구 불가, 보정 대상에서 제외하고 작업자 확인 필요{}",
                excluded,
                if excluded_up > 0 {
                    format!(" · 그중 {}개는 상향 절삭 구간(공구가 벽 쪽으로 끌림) — 지그재그 대신 한 방향 하향 경로를 쓰면 1차 과삭을 피할 수 있습니다", excluded_up)
                } else {
                    String::new()
                }
            )
        },
    });
    let avail_kw = profile.machine.available_power_kw(env.rpm);
    let power_ratio = if avail_kw > 1e-6 { rep.max_power_kw / avail_kw } else { 0.0 };
    let sigma_y = ctx.wp.flow_stress(0.0, 1.0, ambient + bulk_excess).max(1.0);
    let yield_ratio = rep.seg_walls.iter().map(|s| s.wall_stress_mpa / sigma_y).fold(0.0, f64::max);
    let yielded: Vec<usize> = rep.seg_walls.iter().filter(|s| s.wall_stress_mpa > sigma_y).map(|s| s.seg).collect();
    let chatter_severe = rep.chatter_fraction > 0.3 && rep.min_chatter_margin > 0.0 && rep.min_chatter_margin < SEVERE_CHATTER_MARGIN;
    let frf_measured = ctx.tool.measured_fn_hz.is_some() && ctx.tool.measured_k_n_per_um.is_some();
    let chatter_note = format!(
        "재생 채터 — 경로의 {:.0}% 가 한계 밖, 최소 여유 {:.2}배(한계 절입의 약 {:.0}배로 절삭, {}): 실제 벽면 파형·날 치핑 가능, 채터를 반영하지 않은 마모·잔여량 예측은 신뢰할 수 없음{}",
        rep.chatter_fraction * 100.0,
        rep.min_chatter_margin,
        1.0 / rep.min_chatter_margin.max(1e-3),
        if frf_measured { "실측 FRF" } else { "모델 추정 FRF" },
        if radial_source == "simulated" { " (공구 반경·치핑 실측 필요)" } else { "" }
    );
    let mut invalid: Vec<String> = Vec::new();
    if chatter_severe && frf_measured {
        invalid.push(chatter_note.clone());
    }
    if yield_ratio > 1.0 {
        invalid.push(format!(
            "얇은 벽 굽힘 응력 최대 {:.0} MPa > 항복 {:.0} MPa (×{:.1}, 구간 {:?}): 탄성 복원 가정이 깨져 벽이 영구 변형됐을 수 있고, 이 잔여량으로 만든 보정량은 무효",
            yield_ratio * sigma_y,
            sigma_y,
            yield_ratio,
            yielded
        ));
    }
    if power_ratio > SHORT_TIME_POWER {
        invalid.push(format!(
            "1차 최대 동력 {:.2} kW = 가용 {:.2} kW 의 {:.0}% (단시간 정격 추정 {:.0}% 초과): 과부하 정지·회전 저하로 실제 경로가 시뮬레이션과 다름",
            rep.max_power_kw,
            avail_kw,
            power_ratio * 100.0,
            SHORT_TIME_POWER * 100.0
        ));
    }
    let pass1_invalid = !invalid.is_empty();
    let chatter_unverified = chatter_severe && !frf_measured;
    let chatter_score = if rep.chatter_fraction > 0.3 && rep.min_chatter_margin > 0.0 { SEVERE_CHATTER_MARGIN / rep.min_chatter_margin } else { 0.0 };
    gates.push(CompGate {
        id: "pass1_validity".into(),
        label: "1차 결과의 물리적 유효성".into(),
        value: chatter_score.max(yield_ratio).max(power_ratio / SHORT_TIME_POWER),
        limit: 1.0,
        unit: "배".into(),
        severity: if !need {
            u8::from(pass1_invalid || chatter_unverified)
        } else if pass1_invalid {
            3
        } else if chatter_unverified {
            2
        } else if power_ratio > 1.0 || rep.chatter_fraction > 0.3 || yield_ratio > 0.8 {
            1
        } else {
            0
        },
        blocks: if !need {
            vec![]
        } else if pass1_invalid {
            cutting.to_vec()
        } else if chatter_unverified {
            vec![CompAction::Execute, CompAction::ExecuteReduced]
        } else {
            vec![]
        },
        sole: false,
        message: if pass1_invalid {
            format!(
                "{}{} → 보정 전에 공구·벽을 실측하고 1차 조건을 고쳐 다시 계산 (축방향 절입 분할, 안정 로브 회전수·탭 테스트, 마무리 윤곽 패스, 램핑 진입, 한 방향 하향)",
                invalid.join(" · "),
                if chatter_unverified { format!(" · {}", chatter_note) } else { String::new() }
            )
        } else if chatter_unverified {
            format!(
                "{} → 현재 공구로 한 번에 전량을 깎는 보정은 막고 분할 보정(또는 새 공구 교체 후 보정)만 허용, 2차 시작 전 M00 에서 날 치핑·벽 떨림 자국 확인 (탭 테스트로 FRF 를 실측하면 이 판단이 확정됨)",
                chatter_note
            )
        } else {
            format!(
                "1차 동력 ×{:.2} · 채터 최소 여유 {:.2}배 (구간 비율 {:.0}%) · 벽 응력/항복 ×{:.2} — 보정 계산의 전제(탄성 복원·안정 절삭) 범위 안",
                power_ratio,
                rep.min_chatter_margin,
                rep.chatter_fraction * 100.0,
                yield_ratio
            )
        },
    });
    let vb_sev = if vb_now >= vb_limit { 3 } else if vb_now >= 0.8 * vb_limit { 1 } else { 0 };
    gates.push(CompGate {
        id: "vb_now".into(),
        label: "현재 플랭크 마모".into(),
        value: vb_now,
        limit: vb_limit,
        unit: "mm".into(),
        severity: vb_sev,
        blocks: if vb_sev >= 3 { cutting.to_vec() } else { vec![] },
        sole: false,
        message: format!(
            "VB {:.3} mm / 한계 {:.2} mm ({}){}",
            vb_now,
            vb_limit,
            match radial_source.as_str() {
                "measured" => "실측 반경 마모 환산",
                "image" => "이미지 반경 마모 환산",
                _ => "1차 시뮬레이션",
            },
            if vb_sev >= 3 { " — 마모된 날로 얇게 깎으면 미끄럼·버니싱·발열이 생겨 보정 대신 공구 교체" } else { "" }
        ),
    });
    let vbf_sev = if !need { 0 } else if vb2_p90 >= vb_limit { 2 } else if vb2_p90 >= 0.9 * vb_limit { 1 } else { 0 };
    gates.push(CompGate {
        id: "vb_forecast".into(),
        label: "보정 후 마모 예측 (상위 분위)".into(),
        value: vb2_p90,
        limit: vb_limit,
        unit: "mm".into(),
        severity: vbf_sev,
        blocks: if vbf_sev >= 2 { cutting.to_vec() } else { vec![] },
        sole: false,
        message: format!(
            "2차 동안 마모 증가 {:.4} mm (물리, {:.1} s) · 중앙 {:.3} / 상위 {:.3} mm ({})",
            dvb2,
            t2,
            vb2_p50,
            vb2_p90,
            if log.ttm_used { "TTM-R3·Holt·물리 융합 분위" } else { "Holt·물리 융합 분위" }
        ),
    });
    if let Some(img) = tool_image {
        let sev = match img.state {
            WearState::Replace => 3,
            WearState::Alert => 2,
            WearState::Watch => 1,
            WearState::Normal => 0,
        };
        gates.push(CompGate {
            id: "image_wear".into(),
            label: "공구 이미지 판정".into(),
            value: img.severity_index as f64,
            limit: 1.0,
            unit: "".into(),
            severity: sev,
            blocks: match sev {
                3 => cutting.to_vec(),
                2 => vec![CompAction::Execute, CompAction::ExecuteReduced],
                _ => vec![],
            },
            sole: false,
            message: format!("이미지 상태 {} · 유형 {}", img.state.label(), img.wear_type_label),
        });
    }
    let defl_of = |e: &PassEval| e.defl_um.abs() + e.wall_um.abs();
    let half_tol = 0.5 * tol_um;
    if need {
        let d_exec = worst(&|z| defl_of(&z.execute));
        let d_red = worst(&|z| defl_of(&z.reduced));
        let d_split = worst(&|z| defl_of(&z.split_final));
        let unc = |e: &PassEval, varying_um: f64| -> f64 {
            let d = defl_of(e);
            let rel = varying_um / e.ae_um.max(1.0);
            ((MODEL_DEFL_ERR * d).powi(2) + (d * rel).powi(2)).sqrt()
        };
        let u_exec = worst(&|z| unc(&z.execute, 0.5 * (z.residual_p90_um - z.residual_p10_um)));
        let u_red = worst(&|z| unc(&z.reduced, 0.5 * (z.residual_p90_um - z.residual_p10_um)));
        let u_split = worst(&|z| {
            let carry = defl_of(&z.split_first) * (0.5 * (z.residual_p90_um - z.residual_p10_um) / z.split_first.ae_um.max(1.0)) + MODEL_DEFL_ERR * defl_of(&z.split_first);
            unc(&z.split_final, carry)
        });
        let mut blocks = Vec::new();
        if u_exec > half_tol {
            blocks.push(CompAction::Execute);
        }
        if u_red > half_tol {
            blocks.push(CompAction::ExecuteReduced);
        }
        if u_split > half_tol {
            blocks.push(CompAction::Split);
        }
        let sev = if blocks.len() == 3 { 3 } else if !blocks.is_empty() { 2 } else if d_exec > half_tol || u_exec > 0.3 * tol_um { 1 } else { 0 };
        gates.push(CompGate {
            id: "pass2_deflection".into(),
            label: "2차 휨 보정 불확도".into(),
            value: u_exec,
            limit: half_tol,
            unit: "µm".into(),
            severity: sev,
            blocks,
            sole: false,
            message: format!(
                "보정량에 예측 휨을 더해 상쇄하되 남는 불확도(모델 오차 {:.0}% + 잔여량 산포에 따른 절입 변동)로 판단: 1회 휨 {:.1}→불확도 {:.1} · 감속 {:.1}→{:.1} · 분할 마지막 {:.1}→{:.1} µm (한계 공차의 50% = {:.1} µm)",
                MODEL_DEFL_ERR * 100.0,
                d_exec,
                u_exec,
                d_red,
                u_red,
                d_split,
                u_split,
                half_tol
            ),
        });
        let c_exec = least(&|z| z.execute.chatter);
        let c_red = least(&|z| z.reduced.chatter);
        let c_split = least(&|z| z.split_final.chatter);
        let mut cb = Vec::new();
        if c_exec < 1.0 {
            cb.push(CompAction::Execute);
        }
        if c_red < 1.0 {
            cb.push(CompAction::ExecuteReduced);
        }
        if c_split < 1.0 {
            cb.push(CompAction::Split);
        }
        let csev = if cb.len() == 3 { 3 } else if !cb.is_empty() { 2 } else if c_exec < 1.3 { 1 } else { 0 };
        gates.push(CompGate {
            id: "pass2_chatter".into(),
            label: "2차 채터 여유".into(),
            value: c_exec,
            limit: 1.0,
            unit: "배".into(),
            severity: csev,
            blocks: cb,
            sole: false,
            message: format!("작은 반경 절입에서의 안정 한계: 1회 {:.2} · 감속 {:.2} · 분할 마지막 {:.2} 배", c_exec, c_red, c_split),
        });
        let h_min_um = h_min_mm * 1000.0;
        let hard = ctx.wp.work_hardening >= 0.1;
        let h_exec = least(&|z| z.execute.h_max_um);
        let h_red = least(&|z| z.reduced.h_max_um);
        let h_split = least(&|z| z.split_final.h_max_um);
        let mut hb = Vec::new();
        if h_exec < h_min_um {
            hb.push(CompAction::Execute);
        }
        if h_red < h_min_um {
            hb.push(CompAction::ExecuteReduced);
        }
        if h_split < h_min_um {
            hb.push(CompAction::Split);
        }
        let hsev = if hb.len() == 3 { if hard { 3 } else { 2 } } else if !hb.is_empty() { 2 } else if h_exec < 1.5 * h_min_um { 1 } else { 0 };
        if hsev == 2 && hb.len() == 3 && !hard {
            hb.clear();
            hb.push(CompAction::ExecuteReduced);
        }
        gates.push(CompGate {
            id: "min_chip".into(),
            label: "최소 칩 두께 (미끄럼·가공경화)".into(),
            value: h_exec,
            limit: h_min_um,
            unit: "µm".into(),
            severity: hsev,
            blocks: hb,
            sole: false,
            message: format!(
                "칩 얇아짐 보상 이송 적용 후 최대 칩 두께: 1회 {:.2} · 감속 {:.2} · 분할 마지막 {:.2} µm (최소 {:.2} µm = {:.2} × 날끝 반경 {:.1} µm){}",
                h_exec,
                h_red,
                h_split,
                h_min_um,
                ctx.wp.min_chip_ratio,
                ctx.tool.edge_radius_um,
                if hard { " · 가공경화 소재라 미끄럼이 표면을 경화시켜 다음 공정을 악화" } else { "" }
            ),
        });
        let max_feed_need = active
            .iter()
            .map(|z| z.execute.fz_mm * env.rpm * env.flutes)
            .fold(0.0, f64::max);
        if max_feed_need > profile.machine.max_feed_mm_min {
            gates.push(CompGate {
                id: "machine_feed".into(),
                label: "장비 최대 이송".into(),
                value: max_feed_need,
                limit: profile.machine.max_feed_mm_min,
                unit: "mm/min".into(),
                severity: 2,
                blocks: vec![CompAction::Execute],
                sole: false,
                message: "칩 얇아짐 보상 이송이 장비 최대 이송을 넘습니다".into(),
            });
        }
        let slender = active.iter().filter(|z| z.wall_mm > 0.0 && z.ap_mm / z.wall_mm.max(1e-3) > 15.0).count();
        let thin_min = active.iter().filter(|z| z.wall_mm > 0.0).map(|z| z.wall_mm).fold(f64::INFINITY, f64::min);
        let y_of = |e: &PassEval| e.wall_stress_mpa / sigma_y;
        let y_exec = worst(&|z| y_of(&z.execute));
        let y_red = worst(&|z| y_of(&z.reduced));
        let y_split = worst(&|z| y_of(&z.split_first).max(y_of(&z.split_final)));
        let mut tb: Vec<CompAction> = if slender > 0 { vec![CompAction::Execute, CompAction::ExecuteReduced] } else { vec![] };
        for (a, r) in [(CompAction::Execute, y_exec), (CompAction::ExecuteReduced, y_red), (CompAction::Split, y_split)] {
            if r > 0.8 && !tb.contains(&a) {
                tb.push(a);
            }
        }
        let tsev = if tb.len() == 3 { 3 } else if !tb.is_empty() { 2 } else if thin_min.is_finite() { 1 } else { 0 };
        gates.push(CompGate {
            id: "thin_wall".into(),
            label: "얇은 벽 세장비·응력".into(),
            value: if thin_min.is_finite() { thin_min } else { 0.0 },
            limit: 0.0,
            unit: "mm".into(),
            severity: tsev,
            blocks: tb,
            sole: false,
            message: format!(
                "{} · 2차 벽 굽힘 응력/항복({:.0} MPa): 1회 {:.2} · 감속 {:.2} · 분할 {:.2} (0.8 초과 선택지는 영구 변형 위험으로 차단)",
                if slender > 0 {
                    format!("높이/두께 > 15 인 얇은 벽 {}개 구간: 한 번에 깎으면 벽 진동·휨 → 분할 권장", slender)
                } else if thin_min.is_finite() {
                    format!("얇은 벽 최소 두께 {:.2} mm (2차 휨은 위 휨 게이트에 포함)", thin_min)
                } else {
                    "얇은 벽 없음".to_string()
                },
                sigma_y,
                y_exec,
                y_red,
                y_split
            ),
        });
    }
    let u_th = THERMAL_MODEL_ERR * thermal_after;
    let th_sev = if u_th > half_tol { 3 } else if u_th > 0.25 * tol_um { 1 } else { 0 };
    let th_bias_max = zones.iter().filter(|z| z.needs).map(|z| z.thermal_bias_um.abs()).fold(0.0, f64::max);
    gates.push(CompGate {
        id: "thermal_state".into(),
        label: "소재 잔열 열변위 (보정 후 불확도)".into(),
        value: u_th,
        limit: half_tol,
        unit: "µm".into(),
        severity: if need { th_sev } else { 0 },
        blocks: if need && th_sev >= 3 { cutting.to_vec() } else { vec![] },
        sole: false,
        message: format!(
            "덩어리 과열 {:.1} K × 팽창 {:.1} µm/m·K × {:.0} mm = {:.1} µm → 냉각 대기 {:.0} s ({} h {:.0} W/m²K · τ {:.0} s) 후 {:.1} µm · 구간 벽 위치별로 2차 경로에 최대 {:.1} µm 반영, 남는 불확도 ±{:.1} µm (열모델 오차 {:.0}%) — 체크포인트에서 소재 온도 실측 권장",
            bulk_excess,
            ctx.wp.expansion * 1e6,
            body.size_mm,
            thermal_err,
            cooldown_s,
            if dwell_air { "에어 블로우 냉각" } else { "절삭유 정지·자연 대류" },
            h_dwell,
            tau,
            thermal_after,
            th_bias_max,
            u_th,
            THERMAL_MODEL_ERR * 100.0
        ),
    });
    gates.push(CompGate {
        id: "chatter_pass1".into(),
        label: "1차 채터 구간 비율".into(),
        value: rep.chatter_fraction,
        limit: 0.3,
        unit: "".into(),
        severity: if rep.chatter_fraction > 0.3 { 1 } else { 0 },
        blocks: vec![],
        sole: false,
        message: format!(
            "1차 경로의 {:.0}% 가 채터 한계 밖 (최소 {:.2}배) — 1차 벽면 떨림 자국 깊이는 모델에 없으므로 2차 반경 절입보다 깊으면 남음, 측정으로 확인",
            rep.chatter_fraction * 100.0,
            rep.min_chatter_margin
        ),
    });
    let spread = zones.iter().filter(|z| z.needs).map(|z| z.residual_p90_um - z.residual_p10_um).fold(0.0, f64::max);
    let localized: Vec<usize> = zones.iter().filter(|z| z.localized).map(|z| z.seg).collect();
    gates.push(CompGate {
        id: "residual_spread".into(),
        label: "구간 내 잔여량 산포".into(),
        value: spread,
        limit: tol_um,
        unit: "µm".into(),
        severity: if need && (spread > tol_um || !localized.is_empty()) { 1 } else { 0 },
        blocks: vec![],
        sole: false,
        message: format!(
            "구간별 보정량은 구간 중앙값 기준 · 구간 내 상하위 산포 최대 {:.1} µm{}",
            spread,
            if localized.is_empty() {
                String::new()
            } else {
                format!(" · 극값 검사(N_eff = exp(z²/2))에서 국부 돌출 구간 {:?} → 코너·얇은 벽 국부 원인 점검", localized)
            }
        ),
    });
    let blocked_by = |gs: &[CompGate], a: CompAction| -> Vec<usize> { gs.iter().enumerate().filter(|(_, g)| g.blocks.contains(&a)).map(|(i, _)| i).collect() };
    for a in cutting.iter() {
        let b = blocked_by(&gates, *a);
        if b.len() == 1 {
            gates[b[0]].sole = true;
        }
    }
    let tool_gate = gates
        .iter()
        .filter(|g| g.id == "vb_now" || g.id == "vb_forecast" || g.id == "image_wear")
        .map(|g| g.severity)
        .max()
        .unwrap_or(0);
    let max_sev = gates.iter().map(|g| g.severity).max().unwrap_or(0);
    let unfixable: Vec<usize> = zones.iter().filter(|z| z.excluded || (z.thin && z.residual_p90_um > tol_um)).map(|z| z.seg).collect();
    let mut fresh_slender = false;
    let mut allowed: Vec<CompAction> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    if need {
        for a in cutting.iter() {
            if blocked_by(&gates, *a).is_empty() {
                allowed.push(*a);
            }
        }
        if tool_gate >= 1 {
            let fresh_ok = active.iter().all(|z| defl_of(&z.fresh) <= half_tol && z.fresh.chatter >= 1.0 && z.fresh.h_max_um >= h_min_mm * 1000.0 && z.fresh.wall_stress_mpa <= 0.8 * sigma_y)
                && !gates.iter().any(|g| g.id == "thermal_state" && g.severity >= 3)
                && !pass1_invalid;
            let slender_any = active.iter().any(|z| z.wall_mm > 0.0 && z.ap_mm / z.wall_mm.max(1e-3) > 15.0);
            if fresh_ok && !slender_any {
                allowed.push(CompAction::ToolChange);
            } else if fresh_ok {
                fresh_slender = true;
            }
        }
        allowed.push(CompAction::Skip);
        if max_sev >= 2 || !excluded.is_empty() {
            allowed.push(CompAction::Hold);
        }
    } else {
        allowed.push(CompAction::Skip);
        if !unfixable.is_empty() {
            allowed.push(CompAction::Hold);
        }
    }
    let watch_force = gates.iter().any(|g| (g.id == "pass2_deflection" || g.id == "pass2_chatter" || g.id == "thin_wall") && g.severity == 1);
    let rule_action = if !need && !unfixable.is_empty() {
        reasons.push(format!(
            "2차로 고칠 수 없는 공차 밖 구간 {:?} (1차 과삭, 또는 최소 절삭 절입보다 얇은 공차 밖 잔여) → 보정 안 함 대신 보류: 측정 후 재계획",
            unfixable
        ));
        CompAction::Hold
    } else if !need {
        reasons.push(if thin_zones.is_empty() {
            "잔여량이 공차 안이라 2차 보정 불필요".into()
        } else {
            "보정 대상 구간이 모두 공차 안이고 최소 절삭 절입보다 얇아 2차 보정으로 깎지 않음 (문지름 방지)".into()
        });
        CompAction::Skip
    } else if tool_gate >= 2 && allowed.contains(&CompAction::ToolChange) {
        reasons.push("공구 마모 게이트 초과 → 새 공구로 교체한 뒤 보정".into());
        CompAction::ToolChange
    } else if tool_gate >= 2 && fresh_slender {
        reasons.push("공구 마모 게이트 초과지만 새 공구 보정은 한 번에 전량을 깎는 패스라 높이/두께 > 15 인 얇은 벽에는 쓸 수 없음 → 보류: 공구를 교체한 뒤 다시 계획하면 새 공구로 분할 보정을 고를 수 있음".into());
        CompAction::Hold
    } else if let Some(first) = allowed.iter().cloned().find(|a| matches!(a, CompAction::Execute | CompAction::ExecuteReduced | CompAction::Split)) {
        if first == CompAction::Execute && watch_force {
            if allowed.contains(&CompAction::ExecuteReduced) {
                reasons.push("휨·채터·얇은 벽이 관찰 수준이라 감속 보정을 우선".into());
                CompAction::ExecuteReduced
            } else if allowed.contains(&CompAction::Split) {
                reasons.push("휨·채터·얇은 벽이 관찰 수준이고 감속이 막혀 분할 보정을 우선".into());
                CompAction::Split
            } else {
                CompAction::Execute
            }
        } else {
            reasons.push(format!("모든 안전 게이트를 통과한 가장 생산적인 선택: {}", first.label()));
            first
        }
    } else if allowed.contains(&CompAction::ToolChange) {
        reasons.push("현재 공구로는 절삭 선택지가 모두 차단되지만 새 공구 조건은 게이트를 통과 → 공구 교체 후 보정".into());
        CompAction::ToolChange
    } else if max_sev >= 3 {
        let critical: Vec<String> = gates.iter().filter(|g| g.severity >= 3).map(|g| g.label.clone()).collect();
        reasons.push(format!("절삭 선택지가 모두 차단되고 위험 등급 게이트({})가 있어 보류", critical.join(", ")));
        CompAction::Hold
    } else if allowed.contains(&CompAction::Hold) {
        reasons.push("보정이 필요한데 절삭 선택지가 모두 차단되어 보류 — 공차 밖 벽을 그대로 넘기지 않고 측정 후 재계획".into());
        CompAction::Hold
    } else {
        reasons.push("절삭 선택지가 모두 차단되어 2차 보정을 하지 않음".into());
        CompAction::Skip
    };
    let state_text = state_text(profile, ctx, &residual, &zones, &gates, vb_now, vb_limit, vb2_p50, vb2_p90, log, bulk_excess, thermal_err, cooldown_s, h_min_mm);
    CompDraft {
        gates,
        zones,
        residual,
        need,
        allowed,
        rule_action,
        reasons,
        state_text,
        vb_end1_mm: vb1,
        radial_end1_um: radial_sim,
        radial_used_um: radial_used,
        radial_source,
        vb_end2_p50_mm: vb2_p50,
        vb_end2_p90_mm: vb2_p90,
        vb_limit_mm: vb_limit,
        cooldown_s,
        bulk_excess_k: bulk_excess,
        thermal_err_um: thermal_err,
        thermal_err_after_um: thermal_after,
        h_min_um: h_min_mm * 1000.0,
        fz_nominal_mm: nominal.fz_mm,
        rpm: env.rpm,
        notes,
    }
}

#[allow(clippy::too_many_arguments)]
fn state_text(
    profile: &MachiningProfile,
    ctx: &CutContext,
    r: &ResidualSummary,
    zones: &[CompZone],
    gates: &[CompGate],
    vb_now: f64,
    vb_limit: f64,
    vb2_p50: f64,
    vb2_p90: f64,
    log: &WearLog,
    bulk_excess: f64,
    thermal_err: f64,
    cooldown_s: f64,
    h_min_mm: f64,
) -> String {
    let active: Vec<&CompZone> = zones.iter().filter(|z| z.needs).collect();
    let off = active.iter().map(|z| z.offset_um).fold(0.0, f64::max);
    let defl = active.iter().map(|z| z.execute.defl_um.abs() + z.execute.wall_um.abs()).fold(0.0, f64::max);
    let chat = active.iter().map(|z| z.execute.chatter).fold(f64::INFINITY, f64::min);
    let h = active.iter().map(|z| z.execute.h_max_um).fold(f64::INFINITY, f64::min);
    let thin = active.iter().filter(|z| z.wall_mm > 0.0).map(|z| z.wall_mm).fold(f64::INFINITY, f64::min);
    let mut s = format!(
        "Second compensation pass decision after the first end mill pass. Workpiece {}. Tool diameter {:.1} mm, {} flutes, coating {}. Coolant {}. Tolerance plus minus {:.0} um. ",
        profile.workpiece_setup.material_label(),
        profile.endmill_setting.diameter_mm,
        profile.endmill_setting.flute_count,
        profile.endmill_setting.coating_name.clone().unwrap_or_else(|| "none".into()),
        profile.coolant_config.method.key(),
        ctx.tolerance_mm * 1000.0
    );
    s.push_str(&format!(
        "Residual stock on final walls after pass one: median {:.1} um, 90th percentile {:.1} um, maximum {:.1} um, minimum {:.1} um, over {} samples in {} zones, {} zones need correction. ",
        r.p50_um,
        r.p90_um,
        r.max_um,
        r.min_um,
        r.n,
        zones.len(),
        active.len()
    ));
    s.push_str(&format!(
        "Flank wear now {:.3} mm, limit {:.2} mm, predicted after compensation {:.3} mm, 90th percentile {:.3} mm, forecast by {}. ",
        vb_now,
        vb_limit,
        vb2_p50,
        vb2_p90,
        if log.ttm_used { "TTM-R3 fused with Holt and physics" } else { "Holt and physics" }
    ));
    if !active.is_empty() {
        s.push_str(&format!(
            "Planned radial offset up to {:.1} um. Pass two deflection {:.1} um, chatter margin {:.2}, chip thickness {:.2} um against minimum {:.2} um. ",
            off,
            defl,
            if chat.is_finite() { chat } else { 99.0 },
            if h.is_finite() { h } else { 0.0 },
            h_min_mm * 1000.0
        ));
    }
    s.push_str(&format!("Workpiece warm by {:.1} K giving {:.1} um thermal error, cool down dwell {:.0} s. ", bulk_excess, thermal_err, cooldown_s));
    if thin.is_finite() {
        s.push_str(&format!("Thin wall {:.2} mm. ", thin));
    }
    for g in gates.iter().filter(|g| g.severity > 0) {
        let status = match g.severity {
            1 => "watch",
            2 => "over limit",
            _ => "critical",
        };
        s.push_str(&format!("{} {}. ", g.id.replace('_', " "), status));
    }
    s
}

pub fn consult(draft: &CompDraft, laya: Option<&LayaAdvisor>, agreement_rate: Option<(f64, u64)>, use_laya: bool) -> CompDecision {
    let mut d = CompDecision {
        need: draft.need,
        allowed: draft.allowed.clone(),
        rule_action: draft.rule_action,
        laya: None,
        laya_action: None,
        laya_shape: None,
        laya_error: None,
        final_action: draft.rule_action,
        decided_by: "rule".into(),
        operator_confirm: draft.gates.iter().any(|g| (g.id == "overcut_pass1" || g.id == "pass1_validity" || g.id == "min_engagement") && g.severity >= 2),
        agreement: None,
        agreement_rate,
        state_text: draft.state_text.clone(),
        reasons: draft.reasons.clone(),
    };
    if !draft.need {
        d.reasons.push(if draft.rule_action == CompAction::Hold { "2차로 깎을 구간이 없어 laya 를 호출하지 않음 (보류 유지)" } else { "보정이 필요 없어 laya 를 호출하지 않음" }.into());
        return d;
    }
    if draft.allowed.len() < 2 {
        d.reasons.push("허용된 선택지가 하나뿐이라 laya 를 호출하지 않음".into());
        return d;
    }
    if !use_laya {
        d.laya_error = Some("설정에서 laya 판단을 끔".into());
        return d;
    }
    let adv = match laya {
        Some(a) => a,
        None => {
            d.laya_error = Some("laya-typed-decisions 모델이 로드되지 않아 결정적 게이트 판단만 사용".into());
            return d;
        }
    };
    let options: Vec<(String, String)> = draft.allowed.iter().map(|a| (a.key().to_string(), a.english().to_string())).collect();
    let instruction = "Choose the safest way to run the optional second compensation pass that corrects the wall size left by tool wear and deflection after the first pass, without damaging the workpiece or the end mill.";
    match adv.ask_choice(instruction, &options, &draft.state_text) {
        Ok(ans) => apply_laya(&mut d, draft, ans),
        Err(e) => d.laya_error = Some(e),
    }
    d
}

pub fn apply_laya(d: &mut CompDecision, draft: &CompDraft, ans: AdvisorAnswer) {
    d.laya_error = None;
    let act = CompAction::from_key(&ans.top).filter(|a| draft.allowed.contains(a));
    let shape = decay_shape(&ans.probabilities);
    d.laya_shape = shape;
    d.laya_action = act;
    d.agreement = act.map(|a| a == draft.rule_action);
    if ans.act_probability < 0.5 {
        d.operator_confirm = true;
        d.reasons.push(format!("laya 행동 헤드가 사람 확인을 요청 (행동 확률 {:.2}) → 2차 시작 전 M00 정지", ans.act_probability));
    }
    if let Some(a) = act {
        let margin = shape.map(|s| s.margin).unwrap_or(0.0);
        let history_ok = matches!(d.agreement_rate, Some((r, n)) if n >= 10 && r >= 0.6);
        let n_opts = ans.probabilities.len().max(1) as f32;
        let risky = draft.gates.iter().any(|g| g.severity >= 2);
        let escalate_min: f32 = if risky { (1.25 / n_opts).min(0.35) } else { 0.35 };
        let critical: Vec<String> = draft.gates.iter().filter(|g| g.severity >= 3).map(|g| g.label.clone()).collect();
        let hold_locked = draft.rule_action == CompAction::Hold;
        if a == draft.rule_action {
            d.decided_by = "rule+laya".into();
            d.reasons.push(format!("laya 1위 {} ({:.0}%) 가 규칙 판단과 일치", a.label(), ans.top_probability * 100.0));
        } else if a.rank() > draft.rule_action.rank() && ans.top_probability >= escalate_min && ans.act_probability >= 0.5 {
            d.final_action = a;
            d.decided_by = "laya-escalate".into();
            d.reasons.push(format!(
                "laya 가 더 보수적인 {} 을 {:.0}% 로 선택 (보수 방향 채택 기준 {:.0}%{}) → 허용 범위 안의 보수적 상향 채택",
                a.label(),
                ans.top_probability * 100.0,
                escalate_min * 100.0,
                if risky { " · 초과 이상 게이트가 있어 균등 확률의 1.25배로 완화" } else { "" }
            ));
        } else if hold_locked {
            d.reasons.push(format!(
                "laya 1위 {} ({:.0}%) 는 규칙이 정한 보류{}를 낮추는 선택이라 채택하지 않음 — 보정 안 함은 측정 없이 부품을 끝내므로 측정 후 재계획 유지",
                a.label(),
                ans.top_probability * 100.0,
                if critical.is_empty() { String::new() } else { format!("(위험 등급 게이트: {})", critical.join(", ")) }
            ));
        } else if a.rank() < draft.rule_action.rank() && ans.top_probability >= 0.6 && margin >= 0.2 && ans.act_probability >= 0.5 && history_ok {
            d.final_action = a;
            d.decided_by = "laya-confident".into();
            d.reasons.push(format!(
                "laya 가 {} 을 {:.0}% (1·2위 차 {:.2}) 로 선택했고 이 스코프 일치율 이력이 충분해 허용 범위 안에서 채택",
                a.label(),
                ans.top_probability * 100.0,
                margin
            ));
        } else {
            d.reasons.push(format!(
                "laya 1위 {} ({:.0}%) 는 확신·이력 조건 미달 또는 덜 보수적이라 규칙 판단 {} 유지",
                a.label(),
                ans.top_probability * 100.0,
                draft.rule_action.label()
            ));
        }
    }
    d.laya = Some(ans);
}

#[derive(Debug, Clone, Copy)]
struct Move {
    seg: ToolPathSegmentKind,
    x: f64,
    y: f64,
    z: f64,
    i: f64,
    j: f64,
    feed: f64,
    zone: Option<usize>,
    pass: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ToolPathSegmentKind {
    Rapid,
    Linear,
    Cw,
    Ccw,
}

fn left_of(u: (f64, f64), n: (f64, f64)) -> bool {
    u.0 * n.1 - u.1 * n.0 > 0.0
}

fn norm2(v: (f64, f64)) -> (f64, f64) {
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    if l < 1e-12 {
        (1.0, 0.0)
    } else {
        (v.0 / l, v.1 / l)
    }
}

struct OffsetSeg {
    a: (f64, f64),
    b: (f64, f64),
    arc: Option<((f64, f64), bool)>,
    u0: (f64, f64),
    u1: (f64, f64),
    n0: (f64, f64),
    n1: (f64, f64),
    z: f64,
    zone: usize,
}

fn offset_segment(segments: &[ToolPathSegment], zone: &CompZone, zi: usize, offsets_mm: &[f64]) -> Vec<OffsetSeg> {
    let mut out = Vec::new();
    let seg = match segments.get(zone.seg) {
        Some(s) => s,
        None => return out,
    };
    let start = if zone.seg == 0 { (0.0, 0.0, 0.0) } else { segments[zone.seg - 1].end_point() };
    let end = seg.end_point();
    let n = norm2((zone.nx, zone.ny));
    let ranges: Vec<(f64, f64, f64)> = if zone.pieces.len() == offsets_mm.len() && !zone.pieces.is_empty() {
        zone.pieces.iter().zip(offsets_mm.iter()).map(|(p, o)| (p.f0, p.f1, *o)).collect()
    } else {
        vec![(0.0, 1.0, offsets_mm.first().cloned().unwrap_or(0.0))]
    };
    match seg {
        ToolPathSegment::Linear { .. } => {
            let u = norm2((end.0 - start.0, end.1 - start.1));
            let mut knots: Vec<(f64, f64)> = Vec::with_capacity(ranges.len() + 2);
            knots.push((0.0, ranges[0].2));
            for (f0, f1, off) in ranges.iter() {
                knots.push((0.5 * (f0 + f1), *off));
            }
            knots.push((1.0, ranges[ranges.len() - 1].2));
            knots.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-9);
            let at = |f: f64, off: f64| (start.0 + (end.0 - start.0) * f + n.0 * off, start.1 + (end.1 - start.1) * f + n.1 * off);
            for w in knots.windows(2) {
                if w[1].0 <= w[0].0 + 1e-9 {
                    continue;
                }
                out.push(OffsetSeg {
                    a: at(w[0].0, w[0].1),
                    b: at(w[1].0, w[1].1),
                    arc: None,
                    u0: u,
                    u1: u,
                    n0: n,
                    n1: n,
                    z: end.2,
                    zone: zi,
                });
            }
        }
        ToolPathSegment::ArcCW { .. } | ToolPathSegment::ArcCCW { .. } => {
            let (cx, cy, cw) = match seg.arc_center(start) {
                Some(c) => c,
                None => return out,
            };
            let (_, _, r, sweep) = match seg.arc_sweep(start) {
                Some(v) => v,
                None => return out,
            };
            let a0 = (start.1 - cy).atan2(start.0 - cx);
            let am = a0 + 0.5 * sweep;
            let s = if am.cos() * n.0 + am.sin() * n.1 >= 0.0 { 1.0 } else { -1.0 };
            let tan = |rv: (f64, f64)| if cw { (rv.1, -rv.0) } else { (-rv.1, rv.0) };
            for (f0, f1, off) in ranges.into_iter().filter(|r| r.1 > r.0 + 1e-9) {
                let r2 = (r + s * off).max(1e-3);
                let (b0, b1) = (a0 + sweep * f0, a0 + sweep * f1);
                let ra = (b0.cos(), b0.sin());
                let rb = (b1.cos(), b1.sin());
                out.push(OffsetSeg {
                    a: (cx + ra.0 * r2, cy + ra.1 * r2),
                    b: (cx + rb.0 * r2, cy + rb.1 * r2),
                    arc: Some(((cx, cy), cw)),
                    u0: tan(ra),
                    u1: tan(rb),
                    n0: (ra.0 * s, ra.1 * s),
                    n1: (rb.0 * s, rb.1 * s),
                    z: end.2,
                    zone: zi,
                });
            }
        }
        _ => {}
    }
    out
}

fn miter(a: &OffsetSeg, b: &OffsetSeg) -> Option<(f64, f64)> {
    if a.arc.is_some() || b.arc.is_some() || a.zone == b.zone {
        return None;
    }
    let (p, r) = (a.a, (a.b.0 - a.a.0, a.b.1 - a.a.1));
    let (q, s) = (b.a, (b.b.0 - b.a.0, b.b.1 - b.a.1));
    let (lr, ls) = ((r.0 * r.0 + r.1 * r.1).sqrt(), (s.0 * s.0 + s.1 * s.1).sqrt());
    if lr < 1e-9 || ls < 1e-9 {
        return None;
    }
    let den = r.0 * s.1 - r.1 * s.0;
    if (den / (lr * ls)).abs() < 0.05 {
        return None;
    }
    let t = ((q.0 - p.0) * s.1 - (q.1 - p.1) * s.0) / den;
    let x = (p.0 + t * r.0, p.1 + t * r.1);
    let gap = ((a.b.0 - b.a.0).powi(2) + (a.b.1 - b.a.1).powi(2)).sqrt();
    let reach = ((x.0 - a.b.0).powi(2) + (x.1 - a.b.1).powi(2)).sqrt();
    if reach > 5.0 * gap.max(0.01) {
        return None;
    }
    Some(x)
}

type Entry = (f64, f64, Option<(f64, f64, bool)>);

#[allow(clippy::too_many_arguments)]
fn build_moves(
    segments: &[ToolPathSegment],
    zones: &[CompZone],
    rep: &LoadSimReport,
    tool_r: f64,
    action: CompAction,
    passes: &[(f64, f64)],
    feed_of: &dyn Fn(&CompZone, usize) -> f64,
    plunge_feed: f64,
    notes: &mut Vec<String>,
) -> Vec<Move> {
    let mut moves: Vec<Move> = Vec::new();
    let active: Vec<(usize, &CompZone)> = zones.iter().enumerate().filter(|(_, z)| z.needs && z.offset_for(action) > 0.0).collect();
    if active.is_empty() {
        return moves;
    }
    let mut chains: Vec<Vec<(usize, &CompZone)>> = Vec::new();
    for (zi, z) in active.iter().cloned() {
        match chains.last_mut() {
            Some(c) if c.last().map(|(_, p)| p.seg + 1 == z.seg).unwrap_or(false) => c.push((zi, z)),
            _ => chains.push(vec![(zi, z)]),
        }
    }
    let lead = tool_r.min(4.0);
    let mut straight_entries = 0usize;
    let mut direct_entries = 0usize;
    for (pi, (off_scale, _)) in passes.iter().enumerate() {
        for chain in chains.iter() {
            let segs: Vec<OffsetSeg> = chain
                .iter()
                .flat_map(|(zi, z)| {
                    let offs: Vec<f64> = z.path_offsets_um(action, *off_scale).iter().map(|o| o / 1000.0).collect();
                    offset_segment(segments, z, *zi, &offs)
                })
                .collect();
            if segs.is_empty() {
                continue;
            }
            let mut starts: Vec<(f64, f64)> = segs.iter().map(|s| s.a).collect();
            let mut ends: Vec<(f64, f64)> = segs.iter().map(|s| s.b).collect();
            for k in 0..segs.len().saturating_sub(1) {
                if let Some(x) = miter(&segs[k], &segs[k + 1]) {
                    ends[k] = x;
                    starts[k + 1] = x;
                }
            }
            let z = segs[0].z;
            let p0 = starts[0];
            let (u, n) = (segs[0].u0, segs[0].n0);
            let mut entry: Option<Entry> = None;
            for rl in [lead, 0.5 * lead] {
                let c = (p0.0 - n.0 * rl, p0.1 - n.1 * rl);
                let s = (c.0 - u.0 * rl, c.1 - u.1 * rl);
                let m = norm2((n.0 - u.0, n.1 - u.1));
                let mid = (c.0 + m.0 * rl, c.1 + m.1 * rl);
                if clear_at(&rep.heightmap, s.0, s.1, tool_r, z) && clear_at(&rep.heightmap, mid.0, mid.1, tool_r, z) {
                    entry = Some((s.0, s.1, Some((c.0, c.1, left_of(u, n)))));
                    break;
                }
            }
            if entry.is_none() {
                let l = tool_r.max(1.0);
                let tangent = (p0.0 - u.0 * l, p0.1 - u.1 * l);
                let inward = (p0.0 - n.0 * l, p0.1 - n.1 * l);
                let standoff = zones[segs[0].zone].offset_for(action) * off_scale / 1000.0 + 0.05;
                let backed = (p0.0 - n.0 * standoff, p0.1 - n.1 * standoff);
                if clear_at(&rep.heightmap, tangent.0, tangent.1, tool_r, z) {
                    entry = Some((tangent.0, tangent.1, None));
                    straight_entries += 1;
                } else if clear_at(&rep.heightmap, inward.0, inward.1, tool_r, z) {
                    entry = Some((inward.0, inward.1, None));
                    straight_entries += 1;
                } else if clear_at(&rep.heightmap, backed.0, backed.1, tool_r, z) {
                    entry = Some((backed.0, backed.1, None));
                    straight_entries += 1;
                } else {
                    entry = Some((p0.0, p0.1, None));
                    direct_entries += 1;
                }
            }
            let (sx, sy, arc) = entry.unwrap_or((p0.0, p0.1, None));
            let f0 = feed_of(&zones[segs[0].zone], pi);
            moves.push(Move { seg: ToolPathSegmentKind::Rapid, x: sx, y: sy, z: CLEAR_Z, i: 0.0, j: 0.0, feed: 0.0, zone: None, pass: pi });
            moves.push(Move { seg: ToolPathSegmentKind::Linear, x: sx, y: sy, z, i: 0.0, j: 0.0, feed: plunge_feed, zone: None, pass: pi });
            if (sx - p0.0).abs() > 1e-6 || (sy - p0.1).abs() > 1e-6 {
                match arc {
                    Some((cx, cy, cw)) => moves.push(Move {
                        seg: if cw { ToolPathSegmentKind::Cw } else { ToolPathSegmentKind::Ccw },
                        x: p0.0,
                        y: p0.1,
                        z,
                        i: cx - sx,
                        j: cy - sy,
                        feed: f0,
                        zone: None,
                        pass: pi,
                    }),
                    None => moves.push(Move { seg: ToolPathSegmentKind::Linear, x: p0.0, y: p0.1, z, i: 0.0, j: 0.0, feed: f0, zone: None, pass: pi }),
                }
            }
            let mut cur = p0;
            for (k, s) in segs.iter().enumerate() {
                let fz = feed_of(&zones[s.zone], pi);
                if (starts[k].0 - cur.0).abs() > 1e-6 || (starts[k].1 - cur.1).abs() > 1e-6 {
                    moves.push(Move { seg: ToolPathSegmentKind::Linear, x: starts[k].0, y: starts[k].1, z: s.z, i: 0.0, j: 0.0, feed: fz, zone: Some(s.zone), pass: pi });
                }
                match s.arc {
                    Some(((cx, cy), cw)) => moves.push(Move {
                        seg: if cw { ToolPathSegmentKind::Cw } else { ToolPathSegmentKind::Ccw },
                        x: ends[k].0,
                        y: ends[k].1,
                        z: s.z,
                        i: cx - starts[k].0,
                        j: cy - starts[k].1,
                        feed: fz,
                        zone: Some(s.zone),
                        pass: pi,
                    }),
                    None => moves.push(Move { seg: ToolPathSegmentKind::Linear, x: ends[k].0, y: ends[k].1, z: s.z, i: 0.0, j: 0.0, feed: fz, zone: Some(s.zone), pass: pi }),
                }
                cur = ends[k];
            }
            if let Some(last) = segs.last() {
                let (ue, ne) = (last.u1, last.n1);
                let c = (cur.0 - ne.0 * lead, cur.1 - ne.1 * lead);
                let f = (c.0 + ue.0 * lead, c.1 + ue.1 * lead);
                let fl = feed_of(&zones[last.zone], pi);
                if clear_at(&rep.heightmap, f.0, f.1, tool_r, last.z) {
                    moves.push(Move {
                        seg: if left_of(ue, ne) { ToolPathSegmentKind::Cw } else { ToolPathSegmentKind::Ccw },
                        x: f.0,
                        y: f.1,
                        z: last.z,
                        i: c.0 - cur.0,
                        j: c.1 - cur.1,
                        feed: fl,
                        zone: None,
                        pass: pi,
                    });
                    cur = f;
                } else {
                    let away = (cur.0 - ne.0 * lead, cur.1 - ne.1 * lead);
                    moves.push(Move { seg: ToolPathSegmentKind::Linear, x: away.0, y: away.1, z: last.z, i: 0.0, j: 0.0, feed: fl, zone: None, pass: pi });
                    cur = away;
                }
            }
            moves.push(Move { seg: ToolPathSegmentKind::Rapid, x: cur.0, y: cur.1, z: CLEAR_Z, i: 0.0, j: 0.0, feed: 0.0, zone: None, pass: pi });
        }
    }
    if straight_entries > 0 {
        notes.push(format!("원호 진입 공간이 부족한 {}곳은 접선/직선 진입으로 대체", straight_entries));
    }
    if direct_entries > 0 {
        notes.push(format!("진입 공간이 없는 {}곳은 보정 경로 시작점에서 바로 하강 (반경 절입은 보정량뿐)", direct_entries));
    }
    moves
}

fn segs_of(moves: &[Move]) -> Vec<ToolPathSegment> {
    moves
        .iter()
        .map(|m| match m.seg {
            ToolPathSegmentKind::Rapid => ToolPathSegment::Rapid { x: m.x, y: m.y, z: m.z },
            ToolPathSegmentKind::Linear => ToolPathSegment::Linear { x: m.x, y: m.y, z: m.z, feed: m.feed },
            ToolPathSegmentKind::Cw => ToolPathSegment::ArcCW { x: m.x, y: m.y, z: m.z, i: m.i, j: m.j, feed: m.feed },
            ToolPathSegmentKind::Ccw => ToolPathSegment::ArcCCW { x: m.x, y: m.y, z: m.z, i: m.i, j: m.j, feed: m.feed },
        })
        .collect()
}

struct Lines {
    n: u32,
    v: Vec<GCodeLine>,
}

impl Lines {
    fn new() -> Self {
        Self { n: 10, v: Vec::new() }
    }
    fn push(&mut self, code: String, comment: Option<String>) {
        self.v.push(GCodeLine { line_number: self.n, code, comment });
        self.n += 10;
    }
}

fn cause_shares(zones: &[CompZone]) -> Option<[f64; 3]> {
    let mut s = [0.0f64; 3];
    for z in zones.iter().filter(|z| z.needs) {
        let w = z.n.max(1) as f64;
        s[0] += (z.cause_defl_um + z.cause_wall_um).abs() * w;
        s[1] += z.cause_wear_um.abs() * w;
        s[2] += z.cause_thermal_um.abs() * w;
    }
    let total: f64 = s.iter().sum();
    if total <= 1e-9 {
        None
    } else {
        Some([s[0] / total, s[1] / total, s[2] / total])
    }
}

pub fn m_number(code: &str) -> Option<u32> {
    let c = code.trim().to_uppercase();
    let digits = c.strip_prefix('M')?;
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub fn air_code_problem(on: &str, off: &str) -> Option<String> {
    let (a, b) = match (m_number(on), m_number(off)) {
        (Some(a), Some(b)) => (a, b),
        _ => return Some("에어 블로우 ON/OFF 코드는 M 과 숫자로 입력하세요 (예: M83 / M84)".into()),
    };
    let on_forbidden = [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9, 19, 30, 88, 89, 98, 99];
    let off_forbidden = [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 19, 30, 88, 98, 99];
    if a == b {
        Some("에어 블로우 ON 과 OFF 코드가 같습니다".into())
    } else if on_forbidden.contains(&a) {
        Some(format!("M{:02} 은 정지·주축·절삭유 계열 코드라 에어 블로우 ON 으로 쓸 수 없습니다 (M07·M08·M88 은 측정 직전 미스트·절삭유 분사)", a))
    } else if off_forbidden.contains(&b) {
        Some(format!("M{:02} 은 정지·주축·절삭유 ON 계열 코드라 에어 블로우 OFF 로 쓸 수 없습니다", b))
    } else {
        None
    }
}

fn air_codes(profile: &MachiningProfile, opts: &CompOptions) -> Option<(String, String)> {
    if air_code_problem(&opts.air_blast_code, &opts.air_off_code).is_some() {
        return None;
    }
    let on = m_number(&opts.air_blast_code)?;
    let off = m_number(&opts.air_off_code)?;
    let method = &profile.coolant_config.method;
    let coolant_on = m_number(method.gcode_m_code());
    if !matches!(method, CoolantMethod::AirBlast | CoolantMethod::Dry) && (coolant_on == Some(on) || coolant_on == Some(off)) {
        return None;
    }
    Some((format!("M{:02}", on), format!("M{:02}", off)))
}

fn checkpoint_plan(profile: &MachiningProfile, opts: &CompOptions, operator_stop: bool) -> CheckpointPlan {
    let rpm = profile.conditions.spindle_rpm;
    let air = air_codes(profile, opts);
    let mut lines = vec![format!("{} (냉각 정지)", profile.coolant_config.method.gcode_off_code())];
    let mut notes: Vec<String> = vec!["비절삭 구간(공중)에서만 실행되며 1차 프로그램은 그대로 둡니다".into()];
    match air.as_ref() {
        Some((on, off)) => {
            lines.push(format!("{} (에어 블로우 ON · 장비별 코드)", on));
            lines.push(format!("G04 P{} (칩·절삭유 제거)", opts.blow_ms));
            lines.push(format!("{} (에어 블로우 OFF)", off));
            notes.push("저속·정지 중에는 원심력이 없어 칩·절삭유가 날에 남기 쉬워 에어 블로우를 먼저 실행합니다".into());
        }
        None => notes.push(format!(
            "에어 코드 '{}'/'{}' 를 출력하지 않음 — {}. 장비의 에어 전용 ON/OFF M코드를 설정하세요",
            opts.air_blast_code.trim(),
            opts.air_off_code.trim(),
            air_code_problem(&opts.air_blast_code, &opts.air_off_code).unwrap_or_else(|| "현재 절삭유 ON 코드와 겹침 (측정 직전에 미스트·절삭유가 분사됨)".into())
        )),
    }
    if opts.use_orient {
        lines.push("M19 (주축 정위치 · 정지 촬영)".into());
        lines.push(format!("G04 P{} (정위치 안정 대기)", opts.ramp_ms));
    } else {
        lines.push(format!("S{} M03 (저속 회전 · 롤링셔터 왜곡 억제)", opts.slow_rpm.max(1)));
        lines.push(format!("G04 P{} (회전 안정 · 문 닫힌 상태 자동 촬영 구간)", opts.ramp_ms));
        lines.push("M05 (주축 정지 · 작업자 접근 전)".into());
        notes.push("저속 회전은 문을 닫은 자동 촬영 구간에서만 쓰고, 작업자가 측정·접근하는 M00/M01 전에는 M05 로 주축을 세웁니다".into());
    }
    lines.push(format!("{} (측정·촬영 — Plan B 체크포인트 · 주축 정지)", if operator_stop { "M00" } else { "M01" }));
    lines.push(format!("S{} M03 (원래 회전수 복귀)", rpm));
    lines.push(format!("G04 P{} (가속 안정 대기)", opts.ramp_ms));
    let blow = if air.is_some() { opts.blow_ms } else { 0 };
    let time_s = (blow + 2 * opts.ramp_ms) as f64 / 1000.0 + 2.0;
    CheckpointPlan { lines, time_s, notes }
}
#[allow(clippy::too_many_arguments)]
pub fn finalize(
    profile: &MachiningProfile,
    ctx: &CutContext,
    rep: &LoadSimReport,
    segments: &[ToolPathSegment],
    log: &WearLog,
    draft: &CompDraft,
    decision: &CompDecision,
    opts: &CompOptions,
) -> CompOutcome {
    let mut notes: Vec<String> = draft.notes.clone();
    let action = decision.final_action;
    let tool_r = ctx.tool.radius();
    let z = ctx.tool.flutes.max(1) as f64;
    let rpm = draft.rpm;
    let passes: Vec<(f64, f64)> = match action {
        CompAction::Execute | CompAction::ToolChange => vec![(1.0, 1.0)],
        CompAction::ExecuteReduced => vec![(1.0, REDUCED_FEED)],
        CompAction::Split => vec![(SPLIT_FIRST, 1.0), (1.0, 1.0)],
        _ => vec![],
    };
    let feed_cap = profile.machine.max_feed_mm_min;
    let feed_of = |zone: &CompZone, pass: usize| -> f64 {
        let f = zone.eval_for(action, pass).fz_mm.max(1e-5) * rpm * z;
        (if feed_cap > 0.0 { f.min(feed_cap) } else { f }).round().max(1.0)
    };
    let plunge_feed = (profile.conditions.feed_rate_mm_min * 0.4).round().max(1.0);
    let safe = profile.endmill_setting.loc_mm + 10.0 - profile.conditions.axial_doc_mm;
    let mut moves = if passes.is_empty() {
        Vec::new()
    } else {
        build_moves(segments, &draft.zones, rep, tool_r, action, &passes, &feed_of, plunge_feed, &mut notes)
    };
    if let Some(first) = moves.first().copied() {
        if first.seg == ToolPathSegmentKind::Rapid && first.z < safe {
            moves.insert(0, Move { z: safe, ..first });
        }
    }
    if action.cuts() {
        let active: Vec<&CompZone> = draft.zones.iter().filter(|z| z.needs && z.offset_for(action) > 0.0).collect();
        let paths: Vec<f64> = active.iter().flat_map(|z| z.path_offsets_um(action, 1.0)).collect();
        if !paths.is_empty() {
            let (pmin, pmax) = paths.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(*v), a.1.max(*v)));
            let (dmin, dmax) = active.iter().map(|z| z.offset_for(action)).fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(v), a.1.max(v)));
            let (tmin, tmax) = active.iter().map(|z| z.thermal_bias_um).fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(v), a.1.max(v)));
            notes.push(format!(
                "2차 경로는 1차 경로 대비 {:+.1}~{:+.1} µm 이동 (현재 표면 기준 반경 절입 {:.1}~{:.1} µm = 1차 잔여 − 목표 + 2차 휨 + 공구 반경 마모 {:.1} + 잔열 {:+.1}~{:+.1} µm)",
                pmin,
                pmax,
                dmin,
                dmax,
                if action == CompAction::ToolChange { 0.0 } else { draft.radial_used_um },
                tmin,
                tmax
            ));
        }
    }
    let pass_plans: Vec<PassPlan> = passes
        .iter()
        .enumerate()
        .map(|(i, (o, f))| PassPlan {
            name: if passes.len() == 2 {
                if i == 0 { "보정 패스 (오프셋 ×0.7)".into() } else { "스프링 패스 (최종 오프셋)".into() }
            } else {
                "보정 패스".into()
            },
            offset_scale: *o,
            feed_scale: *f,
        })
        .collect();
    let checkpoint = if !moves.is_empty() && (opts.checkpoint || decision.operator_confirm) {
        Some(checkpoint_plan(profile, opts, decision.operator_confirm))
    } else {
        None
    };
    let name = format!("{}_COMP2", profile.name.replace(' ', "_"));
    let mut header = Lines::new();
    let mut body = Lines::new();
    let mut footer = Lines::new();
    let mut program_text = String::new();
    let mut pre_s = 0.0;
    if !moves.is_empty() {
        header.push("G90 G94 G17 G21".into(), Some("절대좌표, mm, XY평면".into()));
        header.push("G54".into(), Some("워크 좌표계".into()));
        if action == CompAction::ToolChange {
            header.push("M00".into(), Some("마모 공구 교체 · 새 공구 길이 보정(H1) 측정 후 재개".into()));
        }
        header.push(
            "T1 M06".into(),
            Some(format!(
                "공구: {} (Ø{}mm, {}날){}",
                profile.endmill_setting.model,
                profile.endmill_setting.diameter_mm,
                profile.endmill_setting.flute_count,
                if action == CompAction::ToolChange { " · 새 공구" } else { " · 1차와 같은 공구" }
            )),
        );
        header.push("G43 H1".into(), Some("공구 길이 보정".into()));
        header.push(format!("G00 X0.000 Y0.000 Z{:.3}", safe), Some("안전 높이 · 주축 정지 상태".into()));
        if draft.cooldown_s > 0.0 {
            let air = air_codes(profile, opts).filter(|_| opts.cooldown_air);
            if let Some((on, _)) = air.as_ref() {
                header.push(on.clone(), Some("에어 블로우 ON · 소재 강제 대류 냉각".into()));
            }
            header.push(
                format!("G04 P{}", (draft.cooldown_s * 1000.0).round() as u64),
                Some(format!(
                    "잔열 냉각 대기 {:.0}s · 주축 정지 · 열변위 {:.1}→{:.1}µm, 남는 값은 구간별 경로에 반영",
                    draft.cooldown_s, draft.thermal_err_um, draft.thermal_err_after_um
                )),
            );
            if let Some((_, off)) = air.as_ref() {
                header.push(off.clone(), Some("에어 블로우 OFF".into()));
            }
            pre_s += draft.cooldown_s;
        }
        if let Some(cp) = checkpoint.as_ref() {
            for l in cp.lines.iter() {
                let (code, comment) = match l.find(" (") {
                    Some(i) => (l[..i].to_string(), Some(l[i + 2..].trim_end_matches(')').to_string())),
                    None => (l.clone(), None),
                };
                header.push(code, comment);
            }
            pre_s += cp.time_s;
        } else {
            if decision.operator_confirm {
                header.push("M00".into(), Some("작업자 확인 후 2차 보정 시작 · 주축 정지 상태".into()));
            }
            header.push(format!("S{} M03", profile.conditions.spindle_rpm), Some(format!("스핀들 {} RPM", profile.conditions.spindle_rpm)));
            header.push(format!("G04 P{}", opts.ramp_ms), Some("가속 안정 대기".into()));
            pre_s += opts.ramp_ms as f64 / 1000.0;
        }
        let coolant = profile.coolant_config.method.gcode_m_code();
        header.push(coolant.to_string(), Some(format!("냉각: {}", profile.coolant_config.method.label())));
        let mut last_pass = usize::MAX;
        for m in moves.iter() {
            let comment = if m.pass != last_pass {
                last_pass = m.pass;
                pass_plans.get(m.pass).map(|p| p.name.clone())
            } else {
                None
            };
            let code = match m.seg {
                ToolPathSegmentKind::Rapid => format!("G00 X{:.3} Y{:.3} Z{:.3}", m.x, m.y, m.z),
                ToolPathSegmentKind::Linear => format!("G01 X{:.3} Y{:.3} Z{:.3} F{:.0}", m.x, m.y, m.z, m.feed),
                ToolPathSegmentKind::Cw => format!("G02 X{:.3} Y{:.3} Z{:.3} I{:.3} J{:.3} F{:.0}", m.x, m.y, m.z, m.i, m.j, m.feed),
                ToolPathSegmentKind::Ccw => format!("G03 X{:.3} Y{:.3} Z{:.3} I{:.3} J{:.3} F{:.0}", m.x, m.y, m.z, m.i, m.j, m.feed),
            };
            body.push(code, comment);
        }
        footer.push(profile.coolant_config.method.gcode_off_code().to_string(), Some("냉각 정지".into()));
        footer.push("M05".into(), Some("스핀들 정지".into()));
        footer.push("G91 G28 Z0.".into(), Some("Z 원점".into()));
        footer.push("G91 G28 X0. Y0.".into(), Some("XY 원점".into()));
        footer.push("M30".into(), Some("종료".into()));
        let mut n = 10u32;
        let mut renum = |v: &mut Vec<GCodeLine>| {
            for l in v.iter_mut() {
                l.line_number = n;
                n += 10;
            }
        };
        renum(&mut header.v);
        renum(&mut body.v);
        renum(&mut footer.v);
        let prog = GCodeProgram {
            program_name: name.clone(),
            header: header.v,
            body: body.v,
            footer: footer.v,
        };
        program_text = prog.to_text();
    }
    let segs = segs_of(&moves);
    let (samples, segment_end_s, time_s, cut_time_s, vb_after) = pass_samples(profile, ctx, rep, &moves, &segs, draft, action);
    let afters: Vec<(f64, f64, f64)> = draft.zones.iter().filter(|z| !z.excluded).map(|z| z.after_for(action)).collect();
    let caps: usize = if action.cuts() { draft.zones.iter().map(|z| z.capped_pieces(action)).sum() } else { 0 };
    if caps > 0 {
        notes.push(format!(
            "벽면 조각 {}개는 고정 경로에서 조각 최소 잔여 지점이 공차의 80% 이상 과삭되지 않도록 보정량을 줄여 일부 잔여량이 남습니다",
            caps
        ));
    }
    let pieces_total: usize = draft.zones.iter().filter(|z| z.needs).map(|z| z.pieces.len().max(1)).sum();
    if action.cuts() && pieces_total > 0 {
        notes.push(format!(
            "보정량은 구간마다 벽면 조각 {}개(구간당 최대 12개)의 잔여량 분포를 따라 선형으로 바뀝니다 — 1차 휨이 경로를 따라 변하는 만큼 보정 경로도 따라감",
            pieces_total
        ));
    }
    let after_p50_um = if afters.is_empty() { draft.residual.p50_um } else { afters.iter().map(|a| a.0).fold(f64::NEG_INFINITY, f64::max) };
    let after_min_um = if afters.is_empty() { draft.residual.min_um } else { afters.iter().map(|a| a.1).fold(f64::INFINITY, f64::min) };
    let after_max_um = if afters.is_empty() { draft.residual.max_um } else { afters.iter().map(|a| a.2).fold(f64::NEG_INFINITY, f64::max) };
    let parts = |per: Option<f64>| -> Option<f64> {
        per.filter(|v| *v > 1e-9).map(|v| ((draft.vb_limit_mm - vb_after).max(0.0) / v).floor())
    };
    let parts_left_p50 = parts(log.per_part_vb_p50_mm);
    let parts_left_p90 = parts(log.per_part_vb_p90_mm);
    let causes = cause_shares(&draft.zones);
    let report_line = format!(
        "[COMP] {} · 잔여 P50 {:.1}/P90 {:.1} µm (공차 {:.0}) · VB {:.3}→{:.3} mm · 결정 {} ({}){}{}",
        profile.name,
        draft.residual.p50_um,
        draft.residual.p90_um,
        draft.residual.tolerance_um,
        draft.vb_end1_mm,
        vb_after,
        action.label(),
        decision.decided_by,
        match causes {
            Some(c) => format!(
                " · {} (보정 대상 잔여 원인 휨 {:.0}% · 마모 {:.0}% · 열 {:.0}%)",
                if c[0] >= c[1] && c[0] >= c[2] {
                    "휨 잔여 제거형"
                } else if c[1] >= c[2] {
                    "마모 보정형"
                } else {
                    "열변위 보정형"
                },
                c[0] * 100.0,
                c[1] * 100.0,
                c[2] * 100.0
            ),
            None => String::new(),
        },
        match (parts_left_p50, parts_left_p90) {
            (Some(a), Some(b)) => format!(" · 교체까지 약 {:.0}개 (보수 {:.0}개)", a, b),
            _ => String::new(),
        }
    );
    if action == CompAction::Hold {
        let pass1 = draft.gates.iter().any(|g| g.id == "pass1_validity" && g.severity >= 3);
        let tol = draft.residual.tolerance_um;
        let unfixable = !draft.need && draft.zones.iter().any(|z| z.excluded || (z.thin && z.residual_p90_um > tol));
        notes.push(if pass1 {
            "보류: 1차 결과가 보정 모델의 전제(탄성 복원·안정 절삭) 밖이라 2차 프로그램을 만들지 않았습니다. 공구 반경·치핑과 벽 두께·변형을 실측하고 1차 조건을 고쳐 다시 계산하세요".into()
        } else if unfixable {
            "보류: 2차로 고칠 수 없는 공차 밖 벽(1차 과삭, 또는 최소 절삭 절입보다 얇은 잔여)이 있어 2차 프로그램을 만들지 않았습니다. 해당 벽을 측정해 합격 여부를 판정하고, 필요하면 날끝 반경이 작은 공구나 다른 공정으로 다시 계획하세요".into()
        } else {
            "보류: 2차 프로그램을 만들지 않았습니다. 부품·공구를 측정해 실측 반경 마모를 입력하면 재계획합니다".into()
        });
    } else if action == CompAction::Skip {
        notes.push("2차 보정 안 함: 1차 프로그램만으로 마칩니다".into());
    }
    CompOutcome {
        draft: draft.clone(),
        decision: decision.clone(),
        passes: pass_plans,
        checkpoint,
        program_name: if program_text.is_empty() { String::new() } else { name },
        program_text,
        segments: segs,
        samples,
        segment_end_s,
        pre_s,
        time_s,
        cut_time_s,
        vb_after_mm: vb_after,
        after_p50_um,
        after_min_um,
        after_max_um,
        parts_left_p50,
        parts_left_p90,
        report_line,
        notes,
    }
}
fn pass_samples(
    profile: &MachiningProfile,
    ctx: &CutContext,
    rep: &LoadSimReport,
    moves: &[Move],
    segs: &[ToolPathSegment],
    draft: &CompDraft,
    action: CompAction,
) -> (Vec<SimSample>, Vec<f64>, f64, f64, f64) {
    let mut samples = Vec::new();
    let mut ends = Vec::with_capacity(segs.len());
    let mut t = 0.0;
    let mut cut_t = 0.0;
    let mut vb = draft.vb_end1_mm;
    let tan_clear = ctx.tool.clearance_deg.to_radians().tan();
    let bulk = rep.bulk_c + draft.bulk_excess_k * if draft.thermal_err_um > 1e-9 { draft.thermal_err_after_um / draft.thermal_err_um } else { 1.0 };
    let mut prev = (0.0, 0.0, profile.endmill_setting.loc_mm + 10.0 - profile.conditions.axial_doc_mm);
    let total_len: f64 = {
        let mut p = prev;
        let mut acc = 0.0;
        for s in segs.iter() {
            acc += s.length_from(p);
            p = s.end_point();
        }
        acc
    };
    let step = (total_len / 1500.0).max(0.5);
    for (k, (s, m)) in segs.iter().zip(moves.iter()).enumerate() {
        let len = s.length_from(prev);
        let feed = s.feed().unwrap_or(profile.machine.rapid_mm_min);
        let dt = if len > 0.0 { len / feed.max(1.0) * 60.0 } else { 0.0 };
        let zone = m.zone.and_then(|zi| draft.zones.get(zi));
        let ev = zone.map(|z| z.eval_for(action, m.pass));
        let pts = s.polyline_from(prev, step);
        let np = pts.len().max(1) as f64;
        for (i, p) in pts.iter().enumerate() {
            let tt = t + dt * (i + 1) as f64 / np;
            let (force, peak, defl, temp, chat, wall, eng, power) = match (zone, ev) {
                (Some(_), Some(e)) => (e.force_n, e.force_peak_n.max(e.force_n), e.defl_um, e.temp_c, e.chatter, e.wall_um, e.engage_deg, e.power_kw),
                _ => (0.0, 0.0, 0.0, ctx.coolant.temperature_c, 0.0, 0.0, 0.0, 0.0),
            };
            if let (Some(_), Some(e)) = (zone, ev) {
                vb += e.vb_rate_mm_min * (dt / np) / 60.0;
            }
            samples.push(SimSample {
                t_s: tt,
                x: p.0,
                y: p.1,
                z: p.2,
                seg: k,
                kind: s.kind().into(),
                feed: s.feed().unwrap_or(0.0),
                ae_mm: ev.map(|e| e.ae_um / 1000.0).unwrap_or(0.0),
                ap_mm: zone.map(|z| z.ap_mm).unwrap_or(0.0),
                engage_deg: eng,
                mode: match zone {
                    Some(z) if z.down => "down".into(),
                    Some(_) => "up".into(),
                    None => "air".into(),
                },
                force_n: force,
                force_peak_n: peak,
                torque_nm: 0.0,
                power_kw: power,
                wall_defl_um: defl,
                temp_c: temp,
                vb_mm: vb,
                radial_loss_um: vb * tan_clear * 1000.0,
                wp_temp_c: bulk,
                chatter_margin: chat,
                workpiece_um: wall,
                wp_local_c: bulk,
                wp_heat_w: 0.0,
                preheat_c: 0.0,
                contact_dx: zone.map(|z| z.nx).unwrap_or(0.0),
                contact_dy: zone.map(|z| z.ny).unwrap_or(0.0),
            });
        }
        if zone.is_some() {
            cut_t += dt;
        }
        t += dt;
        ends.push(t);
        prev = s.end_point();
    }
    (samples, ends, t, cut_t, vb)
}

pub fn record(sds: &mut SdsStore, profile: &MachiningProfile, log: &WearLog, out: &CompOutcome, prev_action: Option<&str>) -> Vec<String> {
    let scope = wear_scope(profile);
    let mut lines = Vec::new();
    let d = &out.draft;
    for (axis, v) in [
        ("residual_p50_um", d.residual.p50_um),
        ("residual_p90_um", d.residual.p90_um),
        ("vb_end_mm", d.vb_end1_mm),
        ("specific_wear_um_cm3", log.specific_wear_um_cm3),
        ("cooldown_s", d.cooldown_s),
    ] {
        if d.residual.n > 0 || axis == "vb_end_mm" || axis == "specific_wear_um_cm3" {
            sds.record_baseline(&scope, axis, v);
        }
    }
    let active: Vec<&CompZone> = d.zones.iter().filter(|z| z.needs).collect();
    if !active.is_empty() {
        sds.record_baseline(&scope, "offset_um", active.iter().map(|z| z.offset_um).fold(0.0, f64::max));
        sds.record_baseline(&scope, "pass2_defl_um", active.iter().map(|z| z.execute.defl_um.abs() + z.execute.wall_um.abs()).fold(0.0, f64::max));
    }
    for b in log.backtests.iter() {
        sds.record_baseline(&scope, &format!("fusion_mse:{}", b.method), b.rmse_um * b.rmse_um);
    }
    let gates: Vec<(String, u8, bool, bool)> = d.gates.iter().map(|g| (g.id.clone(), g.severity, !g.blocks.is_empty(), g.sole)).collect();
    sds.record_gates(&scope, &gates);
    lines.push(format!("wear 원장: 잔여량·마모·융합 오차 축과 게이트 {}개 기록 ({})", gates.len(), scope.key_secondary()));
    let dec = &out.decision;
    if let (Some(agree), Some(la)) = (dec.agreement, dec.laya_action) {
        sds.record_agreement(&scope, COMP_AGREEMENT_AXIS, agree);
        if !agree {
            let margin = dec.laya.as_ref().map(|a| a.top_probability as f64).unwrap_or(0.0);
            sds.record_confusion(&scope, dec.final_action.key(), if la == dec.final_action { dec.rule_action.key() } else { la.key() }, margin);
        }
        if let Some(a) = dec.laya.as_ref() {
            sds.record_decay(&scope, "comp_laya", &a.probabilities);
        }
        lines.push(format!("laya 일치 {} 기록", if agree { "예" } else { "아니오" }));
    }
    if let Some(prev) = prev_action {
        sds.record_transition(&scope, prev, dec.final_action.key());
    }
    if d.radial_source == "measured" && d.radial_end1_um > 1e-6 {
        let ps = crate::pipeline::process_scope(profile);
        sds.record_calibration(&ps, "wear", d.radial_used_um, d.radial_end1_um);
        lines.push(format!("실측 반경 마모 {:.1} µm / 예측 {:.1} µm → 공정 마모 보정 계수 학습", d.radial_used_um, d.radial_end1_um));
    }
    lines
}

pub fn save_job_files(dir: &Path, stamp: &str, profile_name: &str, bins_csv: &str, summary: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if std::fs::create_dir_all(dir).is_err() {
        return out;
    }
    let safe: String = profile_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .chars()
        .take(40)
        .collect();
    let base = format!("{}_{}", stamp, safe.trim_matches('_'));
    let csv = dir.join(format!("{}.csv", base));
    if std::fs::write(&csv, bins_csv).is_ok() {
        out.push(csv.display().to_string());
    }
    let js = dir.join(format!("{}.json", base));
    if let Ok(txt) = serde_json::to_string_pretty(summary) {
        if std::fs::write(&js, txt).is_ok() {
            out.push(js.display().to_string());
        }
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    files.sort();
    if files.len() > 400 {
        let excess = files.len() - 400;
        for f in files.iter().take(excess) {
            let _ = std::fs::remove_file(f);
        }
    }
    out
}

pub fn analyze_job(profile: &MachiningProfile, ctx: &CutContext, report: &LoadSimReport, ttm: Option<&crate::ml::ttm::TtmForecaster>, sds: &SdsStore) -> WearLog {
    let scope = wear_scope(profile);
    let history: Vec<(String, f64, u64)> = ["ttm", "holt", "physics"]
        .iter()
        .filter_map(|m| sds.fusion_mse(&scope, m).map(|(mse, n)| (m.to_string(), mse, n)))
        .collect();
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let nominal = physics::analyze_cut(ctx, &profile.conditions, true, body);
    let residuals: Vec<f64> = report.wall_errors.iter().filter(|w| w.final_wall).map(|w| w.total_um).collect();
    crate::wearlog::analyze(&report.wear_log, &residuals, Some(&nominal.trajectory), ttm, &history, ctx.vb_limit_mm())
}

pub struct PipelineResult {
    pub view: crate::viewport::ViewportSim,
    pub report: LoadSimReport,
    pub segments: Vec<ToolPathSegment>,
    pub log: WearLog,
    pub outcome: CompOutcome,
    pub sds_lines: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    profile: &MachiningProfile,
    pattern_key: &str,
    sds: &mut SdsStore,
    ttm: Option<&crate::ml::ttm::TtmForecaster>,
    laya: Option<&LayaAdvisor>,
    opts: &CompOptions,
    tool_image: Option<&WearComparison>,
    prev_action: Option<&str>,
    progress: &mut dyn FnMut(f64, &str) -> bool,
) -> Result<PipelineResult, String> {
    let cancelled = || "작업을 취소했습니다".to_string();
    let (ctx, _) = crate::pipeline::calibrated_context(profile, Some(sds));
    if !progress(0.02, "context") {
        return Err(cancelled());
    }
    let (view, report, segments) = crate::viewport::build_report_prog(profile, pattern_key, &ctx, &mut |f| progress(0.05 + 0.6 * f, "sim"))?;
    if !progress(0.66, "wearlog") {
        return Err(cancelled());
    }
    let log = analyze_job(profile, &ctx, &report, if opts.use_ttm { ttm } else { None }, sds);
    if !progress(0.75, "plan") {
        return Err(cancelled());
    }
    let draft = evaluate(profile, &ctx, &report, &log, opts, tool_image);
    let rate = sds.agreement_rate(&wear_scope(profile), COMP_AGREEMENT_AXIS);
    if !progress(0.8, "decide") {
        return Err(cancelled());
    }
    let decision = consult(&draft, laya, rate, opts.use_laya);
    if !progress(0.9, "gcode") {
        return Err(cancelled());
    }
    let outcome = finalize(profile, &ctx, &report, &segments, &log, &draft, &decision, opts);
    let sds_lines = record(sds, profile, &log, &outcome, prev_action);
    progress(1.0, "done");
    Ok(PipelineResult {
        view,
        report,
        segments,
        log,
        outcome,
        sds_lines,
    })
}
