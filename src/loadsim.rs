use crate::gcode::{GCodeGenerator, ToolPathPattern, ToolPathSegment};
use crate::physics::{self, CutContext, Engagement, ForceResult, MillMode, ThermalBody};
use crate::profile::{MachiningProfile, ToolNose};
use crate::timeseries::split_episodes;
use crate::workpiece_setup::StockShape;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const NO_MATERIAL: f32 = f32::NEG_INFINITY;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockGrid {
    pub x0: f64,
    pub y0: f64,
    pub cell: f64,
    pub nx: usize,
    pub ny: usize,
    pub h: Vec<f32>,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Removal {
    volume: f64,
    lateral: f64,
    forward: f64,
    max_depth: f64,
    cells: usize,
}

impl StockGrid {
    pub fn from_setup(p: &MachiningProfile, max_cells: usize) -> Self {
        let w = p.workpiece_setup.width_mm.max(1.0);
        let hgt = p.workpiece_setup.height_mm.max(1.0);
        let d = p.endmill_setting.diameter_mm.max(0.1);
        let allowance = p.workpiece_setup.stock_allowance_mm.max(0.05);
        let margin = d * 1.5 + 2.0;
        let mut cell = (d / 20.0).min(allowance / 3.0).clamp(0.05, 0.5);
        let span_x = w + 2.0 * margin;
        let span_y = hgt + 2.0 * margin;
        while (span_x / cell) * (span_y / cell) > max_cells as f64 {
            cell *= 1.25;
        }
        let nx = (span_x / cell).ceil() as usize;
        let ny = (span_y / cell).ceil() as usize;
        let x0 = -margin;
        let y0 = -margin;
        let bottom = -(p.workpiece_setup.thickness_mm.max(0.1)) as f32;
        let mut h = vec![NO_MATERIAL; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let x = x0 + (i as f64 + 0.5) * cell;
                let y = y0 + (j as f64 + 0.5) * cell;
                let inside = match &p.workpiece_setup.shape {
                    StockShape::Cylindrical { diameter_mm } => {
                        let r = diameter_mm / 2.0;
                        (x - w / 2.0).powi(2) + (y - hgt / 2.0).powi(2) <= r * r
                    }
                    _ => x >= 0.0 && x <= w && y >= 0.0 && y <= hgt,
                };
                if inside {
                    h[j * nx + i] = 0.0;
                }
            }
        }
        Self {
            x0,
            y0,
            cell,
            nx,
            ny,
            h,
            top: 0.0,
            bottom,
        }
    }

    pub fn height_at(&self, x: f64, y: f64) -> f32 {
        let i = ((x - self.x0) / self.cell).floor();
        let j = ((y - self.y0) / self.cell).floor();
        if i < 0.0 || j < 0.0 || i as usize >= self.nx || j as usize >= self.ny {
            return NO_MATERIAL;
        }
        self.h[j as usize * self.nx + i as usize]
    }

    fn cut(&mut self, cx: f64, cy: f64, z_tip: f64, r: f64, nose: ToolNose, dir: (f64, f64), front_only: bool, floor_err: &mut [f32], floor_val: f32) -> Removal {
        let rc = nose.corner_radius(2.0 * r);
        let mut out = Removal::default();
        let i0 = (((cx - r) - self.x0) / self.cell).floor().max(0.0) as usize;
        let i1 = ((((cx + r) - self.x0) / self.cell).ceil() as usize).min(self.nx);
        let j0 = (((cy - r) - self.y0) / self.cell).floor().max(0.0) as usize;
        let j1 = ((((cy + r) - self.y0) / self.cell).ceil() as usize).min(self.ny);
        let area = self.cell * self.cell;
        let (ux, uy) = dir;
        let (nx_, ny_) = (-uy, ux);
        for j in j0..j1 {
            let y = self.y0 + (j as f64 + 0.5) * self.cell;
            for i in i0..i1 {
                let x = self.x0 + (i as f64 + 0.5) * self.cell;
                let dx = x - cx;
                let dy = y - cy;
                let rr = dx * dx + dy * dy;
                if rr > r * r {
                    continue;
                }
                let fwd = dx * ux + dy * uy;
                if front_only && fwd < -self.cell {
                    continue;
                }
                let k = j * self.nx + i;
                let old = self.h[k];
                if old == NO_MATERIAL {
                    continue;
                }
                let rd = rr.sqrt();
                let prof = if rc <= 0.0 || rd <= r - rc {
                    z_tip
                } else {
                    let q = rd - (r - rc);
                    z_tip + rc - (rc * rc - q * q).max(0.0).sqrt()
                };
                let new = prof.max(self.bottom as f64) as f32;
                if (old as f64) > prof + 1e-6 {
                    let removed = old as f64 - new as f64;
                    if removed > 0.0 {
                        let v = removed * area;
                        out.volume += v;
                        out.lateral += v * (dx * nx_ + dy * ny_);
                        out.forward += v * fwd;
                        out.max_depth = out.max_depth.max(old as f64 - z_tip);
                        out.cells += 1;
                        floor_err[k] = floor_val;
                    }
                    self.h[k] = if prof <= self.bottom as f64 { NO_MATERIAL } else { new };
                }
            }
        }
        out
    }

    pub fn downsample(&self, max_side: usize) -> HeightmapView {
        let f = ((self.nx.max(self.ny) as f64) / max_side as f64).ceil().max(1.0) as usize;
        let w = (self.nx + f - 1) / f;
        let h = (self.ny + f - 1) / f;
        let mut z = vec![f32::NAN; w * h];
        for j in 0..h {
            for i in 0..w {
                let mut acc = 0.0f64;
                let mut n = 0.0;
                for jj in j * f..((j + 1) * f).min(self.ny) {
                    for ii in i * f..((i + 1) * f).min(self.nx) {
                        let v = self.h[jj * self.nx + ii];
                        if v != NO_MATERIAL {
                            acc += v as f64;
                            n += 1.0;
                        }
                    }
                }
                if n > 0.0 {
                    z[j * w + i] = (acc / n) as f32;
                }
            }
        }
        HeightmapView {
            x0: self.x0,
            y0: self.y0,
            cell_mm: self.cell * f as f64,
            nx: w,
            ny: h,
            z,
            bottom: self.bottom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeightmapView {
    pub x0: f64,
    pub y0: f64,
    pub cell_mm: f64,
    pub nx: usize,
    pub ny: usize,
    pub z: Vec<f32>,
    pub bottom: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSample {
    pub t_s: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub seg: usize,
    pub kind: String,
    pub feed: f64,
    pub ae_mm: f64,
    pub ap_mm: f64,
    pub engage_deg: f64,
    pub mode: String,
    pub force_n: f64,
    pub force_peak_n: f64,
    pub torque_nm: f64,
    pub power_kw: f64,
    pub wall_defl_um: f64,
    pub temp_c: f64,
    pub vb_mm: f64,
    pub radial_loss_um: f64,
    pub wp_temp_c: f64,
    #[serde(default)]
    pub chatter_margin: f64,
    #[serde(default)]
    pub workpiece_um: f64,
    #[serde(default)]
    pub wp_local_c: f64,
    #[serde(default)]
    pub wp_heat_w: f64,
    #[serde(default)]
    pub preheat_c: f64,
    #[serde(default)]
    pub contact_dx: f64,
    #[serde(default)]
    pub contact_dy: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub start: usize,
    pub end: usize,
    pub t_start_s: f64,
    pub t_end_s: f64,
    pub seg_from: usize,
    pub seg_to: usize,
    pub mean_power_kw: f64,
    pub peak_force_n: f64,
    pub peak_defl_um: f64,
    pub mean_temp_c: f64,
    pub mean_engage_deg: f64,
    pub feed_scale: f64,
    pub limiting: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallErrorSample {
    pub x: f64,
    pub y: f64,
    pub nx: f64,
    pub ny: f64,
    pub z_mid: f64,
    pub t_s: f64,
    pub deflection_um: f64,
    pub wear_um: f64,
    pub thermal_tool_um: f64,
    pub thermal_wp_um: f64,
    pub total_um: f64,
    #[serde(default)]
    pub workpiece_um: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadSimReport {
    pub pattern: String,
    pub samples: Vec<SimSample>,
    pub episodes: Vec<Episode>,
    pub wall_errors: Vec<WallErrorSample>,
    pub heightmap: HeightmapView,
    pub floor_error: HeightmapView,
    pub total_time_min: f64,
    pub cut_time_min: f64,
    pub rapid_time_min: f64,
    pub removed_volume_cm3: f64,
    pub vb_end_mm: f64,
    pub radial_loss_end_um: f64,
    pub max_wall_defl_um: f64,
    pub mean_wall_error_um: f64,
    pub max_wall_error_um: f64,
    pub floor_error_mean_um: f64,
    pub max_power_kw: f64,
    pub max_force_n: f64,
    pub max_temp_c: f64,
    pub wp_temp_end_c: f64,
    pub plunge_count: usize,
    pub cell_mm: f64,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub max_workpiece_defl_um: f64,
    #[serde(default)]
    pub min_chatter_margin: f64,
    #[serde(default)]
    pub chatter_fraction: f64,
    #[serde(default)]
    pub thin_wall_mm: Option<f64>,
    #[serde(default)]
    pub heat_packets: Vec<HeatPacket>,
    #[serde(default)]
    pub heat: HeatFieldProps,
    #[serde(default)]
    pub segment_end_s: Vec<f64>,
    #[serde(default)]
    pub max_preheat_c: f64,
    #[serde(default)]
    pub bulk_c: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HeatPacket {
    pub t_s: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub q_j: f64,
    pub tau0_s: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct HeatFieldProps {
    pub rho_c: f64,
    pub kappa_m2_s: f64,
    pub effusivity: f64,
    pub h_surface: f64,
    pub bulk_c: f64,
    pub prune_k: f64,
}

impl HeatFieldProps {
    pub fn of(ctx: &CutContext) -> Self {
        let bulk = ctx.bulk_c();
        let rho_c = ctx.wp.rho_c_at(bulk).max(1.0e5);
        let kappa = ctx.wp.diffusivity_at(bulk).max(1.0e-8);
        Self {
            rho_c,
            kappa_m2_s: kappa,
            effusivity: rho_c * kappa.sqrt(),
            h_surface: ctx.coolant.workpiece_h(),
            bulk_c: bulk,
            prune_k: 0.02,
        }
    }

    pub fn convective_decay(&self, age_s: f64) -> f64 {
        (-2.0 * self.h_surface * age_s.max(0.0).sqrt() / (std::f64::consts::PI.sqrt() * self.effusivity.max(1.0))).exp()
    }

    pub fn amplitude(&self, p: &HeatPacket, age_s: f64) -> f64 {
        if age_s < 0.0 {
            return 0.0;
        }
        let tau = age_s + p.tau0_s.max(1e-4);
        let four_kt = 4.0 * self.kappa_m2_s * tau;
        2.0 * p.q_j / (self.rho_c * (std::f64::consts::PI * four_kt).powf(1.5)) * self.convective_decay(age_s)
    }

    pub fn excess_at(&self, p: &HeatPacket, x: f64, y: f64, z: f64, t_s: f64) -> f64 {
        let age = t_s - p.t_s;
        if age < 0.0 {
            return 0.0;
        }
        let tau = age + p.tau0_s.max(1e-4);
        let four_kt = 4.0 * self.kappa_m2_s * tau;
        let r2 = ((x - p.x).powi(2) + (y - p.y).powi(2) + (z - p.z).powi(2)) * 1e-6;
        let e = r2 / four_kt;
        if e > 30.0 {
            return 0.0;
        }
        2.0 * p.q_j / (self.rho_c * (std::f64::consts::PI * four_kt).powf(1.5)) * (-e).exp() * self.convective_decay(age)
    }

    pub fn life_s(&self, p: &HeatPacket, threshold_k: f64) -> f64 {
        let tau = (2.0 * p.q_j / (self.rho_c * threshold_k.max(1e-6))).powf(2.0 / 3.0) / (4.0 * std::f64::consts::PI * self.kappa_m2_s);
        (tau - p.tau0_s).max(0.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingHeat {
    q: f64,
    sx: f64,
    sy: f64,
    sz: f64,
    st: f64,
    t0: f64,
    x0: f64,
    y0: f64,
    tau0: f64,
}

struct HeatTracker {
    props: HeatFieldProps,
    alive: Vec<HeatPacket>,
    all: Vec<HeatPacket>,
    pending: Option<PendingHeat>,
    merge_dt: f64,
    merge_len: f64,
}

impl HeatTracker {
    fn new(props: HeatFieldProps, tool_d: f64) -> Self {
        Self {
            props,
            alive: Vec::new(),
            all: Vec::new(),
            pending: None,
            merge_dt: 0.1,
            merge_len: (0.1 * tool_d).max(0.5),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn deposit(&mut self, x: f64, y: f64, z: f64, t: f64, q: f64, tau0: f64) {
        if q <= 0.0 {
            return;
        }
        if let Some(p) = self.pending {
            let far = ((x - p.x0).powi(2) + (y - p.y0).powi(2)).sqrt() > self.merge_len;
            if t - p.t0 > self.merge_dt || far {
                self.flush();
            }
        }
        let p = self.pending.get_or_insert(PendingHeat {
            q: 0.0,
            sx: 0.0,
            sy: 0.0,
            sz: 0.0,
            st: 0.0,
            t0: t,
            x0: x,
            y0: y,
            tau0,
        });
        p.q += q;
        p.sx += q * x;
        p.sy += q * y;
        p.sz += q * z;
        p.st += q * t;
        p.tau0 = p.tau0.max(tau0);
    }

    fn flush(&mut self) {
        if let Some(p) = self.pending.take() {
            if p.q > 0.0 {
                let pk = HeatPacket {
                    t_s: p.st / p.q,
                    x: p.sx / p.q,
                    y: p.sy / p.q,
                    z: p.sz / p.q,
                    q_j: p.q,
                    tau0_s: p.tau0,
                };
                self.alive.push(pk);
                self.all.push(pk);
            }
        }
    }

    fn excess(&self, x: f64, y: f64, z: f64, t: f64, older_than: f64) -> f64 {
        self.alive
            .iter()
            .filter(|p| p.t_s <= older_than)
            .map(|p| self.props.excess_at(p, x, y, z, t))
            .sum()
    }

    fn prune(&mut self, t: f64) {
        let props = self.props;
        self.alive.retain(|p| props.amplitude(p, t - p.t_s) >= props.prune_k);
    }

    fn finish(mut self, cap: usize) -> (Vec<HeatPacket>, HeatFieldProps) {
        self.flush();
        let near = 4.0 * self.merge_len;
        let mut v = self.all;
        while v.len() > cap {
            let before = v.len();
            let mut merged = Vec::with_capacity(v.len() / 2 + 1);
            for pair in v.chunks(2) {
                if pair.len() == 1 {
                    merged.push(pair[0]);
                    continue;
                }
                let (a, b) = (pair[0], pair[1]);
                let q = a.q_j + b.q_j;
                if q <= 0.0 {
                    continue;
                }
                if ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt() > near {
                    merged.push(a);
                    merged.push(b);
                    continue;
                }
                merged.push(HeatPacket {
                    t_s: (a.t_s * a.q_j + b.t_s * b.q_j) / q,
                    x: (a.x * a.q_j + b.x * b.q_j) / q,
                    y: (a.y * a.q_j + b.y * b.q_j) / q,
                    z: (a.z * a.q_j + b.z * b.q_j) / q,
                    q_j: q,
                    tau0_s: a.tau0_s.max(b.tau0_s) + ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)) * 1e-6 / (16.0 * self.props.kappa_m2_s.max(1e-9)),
                });
            }
            v = merged;
            if v.len() >= before {
                break;
            }
        }
        (v, self.props)
    }
}

#[derive(Clone)]
struct CacheEntry {
    f: ForceResult,
}

fn quant(v: f64, step: f64) -> i64 {
    (v / step).round() as i64
}

#[allow(clippy::too_many_arguments)]
fn thin_wall_um(grid: &StockGrid, wx: f64, wy: f64, side: (f64, f64), z_tip: f64, z_mid: f64, ap: f64, force_n: f64, e_n_mm2: f64) -> Option<(f64, f64)> {
    if force_n.abs() < 1e-9 || ap <= 0.0 {
        return None;
    }
    let step = grid.cell.max(0.02);
    let mut t = 0.5 * step;
    while t <= 15.0 {
        let h = grid.height_at(wx + side.0 * t, wy + side.1 * t);
        if h == NO_MATERIAL || (h as f64) <= z_tip + 0.5 * step {
            let other = if h == NO_MATERIAL { grid.bottom as f64 } else { h as f64 };
            let root = z_tip.max(other);
            let a = z_mid - root;
            if a <= 0.05 {
                return None;
            }
            let l_eff = (ap + 2.0 * a).max(1.0);
            let k = e_n_mm2 * l_eff * t.powi(3) / (4.0 * a.powi(3));
            return Some((force_n / k.max(1e-9) * 1000.0, t));
        }
        t += step;
    }
    None
}

pub fn simulate(profile: &MachiningProfile, pattern: &ToolPathPattern, ctx: &CutContext) -> LoadSimReport {
    let segments = GCodeGenerator::generate_synthetic_segments_with_pattern(profile, pattern);
    simulate_segments(profile, &segments, pattern.key(), ctx)
}

pub fn simulate_segments(profile: &MachiningProfile, segments: &[ToolPathSegment], label: &str, ctx: &CutContext) -> LoadSimReport {
    simulate_segments_res(profile, segments, label, ctx, 3_000_000)
}

pub fn simulate_segments_res(profile: &MachiningProfile, segments: &[ToolPathSegment], label: &str, ctx: &CutContext, max_cells: usize) -> LoadSimReport {
    let mut grid = StockGrid::from_setup(profile, max_cells.max(10_000));
    let mut floor_err = vec![f32::NAN; grid.nx * grid.ny];
    let tool = &ctx.tool;
    let r = tool.radius();
    let z = tool.flutes.max(1) as f64;
    let rpm = profile.conditions.spindle_rpm.max(1) as f64;
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let nominal = physics::analyze_cut(ctx, &profile.conditions, true, body);
    let co = physics::cutting_coeffs_at(ctx, nominal.vc_effective_m_min, (nominal.fz_mm * 0.64).max(1e-4));
    let kappa = nominal.trajectory.kappa.max(1e-6);
    let vb_break = nominal.trajectory.break_in_vb_mm;
    let tau_break = nominal.trajectory.break_in_tau_min.max(1e-6);
    let e_wp = ctx.wp.elastic_gpa * 1000.0 / (1.0 - ctx.wp.poisson.powi(2)).max(0.5);
    let reference = physics::reference_state(ctx);
    let heat_props = HeatFieldProps::of(ctx);
    let bulk = heat_props.bulk_c;
    let mut heat = HeatTracker::new(heat_props, 2.0 * r);
    let tau0 = ((0.5 * r).clamp(0.3, 5.0) * 1e-3).powi(2) / (4.0 * heat_props.kappa_m2_s);
    let mut seg_start_s: Vec<f64> = vec![0.0; segments.len()];
    let mut preheat = 0.0f64;
    let mut max_preheat = 0.0f64;
    let mut last_eval_t = f64::NEG_INFINITY;
    let mut last_eval_xy = (f64::NAN, f64::NAN);
    let mut last_prune_t = 0.0f64;
    let mut wtool = tool.clone();
    let mut cache: HashMap<(i64, i64, i64, i64, i64), CacheEntry> = HashMap::new();
    let mut chatter_cache: HashMap<(i64, i64, i64), f64> = HashMap::new();
    let mut min_margin = f64::INFINITY;
    let mut chatter_samples = 0usize;
    let mut cut_samples = 0usize;
    let mut max_wp_defl = 0.0f64;
    let mut thin_wall: Option<f64> = None;
    let step = (grid.cell * 0.5).max(0.025);
    let total_len: f64 = {
        let mut prev = (0.0, 0.0, 0.0);
        let mut acc = 0.0;
        for s in segments {
            acc += s.length_from(prev);
            prev = s.end_point();
        }
        acc
    };
    let step = step.max(total_len / 250_000.0);
    let mut samples: Vec<SimSample> = Vec::new();
    let mut wall_errors: Vec<WallErrorSample> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut t = 0.0f64;
    let mut cut_time = 0.0f64;
    let mut rapid_time = 0.0f64;
    let mut vb = 0.0f64;
    let mut wp_rise = 0.0f64;
    let mut removed = 0.0f64;
    let mut ema_ae = 0.0f64;
    let mut max_power = 0.0f64;
    let mut max_force = 0.0f64;
    let mut max_temp = 0.0f64;
    let mut max_defl = 0.0f64;
    let mut plunges = 0usize;
    let mut prev = (0.0, 0.0, profile.endmill_setting.loc_mm + 10.0);
    let ha = ctx.coolant.workpiece_h() * body.area_m2;
    let mc_wp = body.mass_kg * ctx.wp.specific_heat;
    let tan_clear = tool.clearance_deg.to_radians().tan();
    let drift_um_per_s = profile.machine.spindle_drift_um_per_hr / 3600.0;
    let lead_tan = 7f64.to_radians().tan();
    let datum = (0.0f64, 0.0f64);
    for (si, seg) in segments.iter().enumerate() {
        seg_start_s[si] = t;
        let start = prev;
        let end = seg.end_point();
        let seg_len = seg.length_from(start);
        prev = end;
        if seg_len <= 1e-9 {
            continue;
        }
        match seg.feed() {
            None => {
                let dtr = seg_len / profile.machine.rapid_mm_min.max(1.0) * 60.0;
                t += dtr;
                rapid_time += dtr;
                let pts = seg.polyline_from(start, (grid.cell * 2.0).max(0.5));
                for p in pts.iter() {
                    let hgt = grid.height_at(p.0, p.1);
                    if hgt != NO_MATERIAL && (hgt as f64) > p.2 - 0.01 {
                        let msg = format!("급이송(G00) 경로가 소재와 간섭합니다 (세그먼트 {})", si);
                        if !warnings.contains(&msg) {
                            warnings.push(msg);
                        }
                        break;
                    }
                }
                wp_rise += (-wp_rise * ha / mc_wp.max(1e-6)) * dtr;
                continue;
            }
            Some(feed) if feed <= 0.0 => continue,
            Some(_) => {}
        }
        let feed = seg.feed().unwrap_or(0.0);
        let is_plunge = matches!(seg, ToolPathSegment::Plunge { .. });
        let arc = seg.arc_sweep(start);
        let pts = seg.polyline_from(start, step);
        let mut last = start;
        let mut first = true;
        let mut first_cut_of_seg = true;
        for p in pts.iter() {
            let dx = p.0 - last.0;
            let dy = p.1 - last.1;
            let dzv = p.2 - last.2;
            let dl = (dx * dx + dy * dy + dzv * dzv).sqrt();
            let lateral = (dx * dx + dy * dy).sqrt();
            let dir = if lateral > 1e-9 { (dx / lateral, dy / lateral) } else { (1.0, 0.0) };
            let dt = if feed > 0.0 { dl / feed * 60.0 } else { 0.0 };
            let floor_val = {
                let wear_len = vb * lead_tan * 0.5 * 1000.0;
                let drift = drift_um_per_s * t;
                (wear_len - drift) as f32
            };
            let plunge_like = is_plunge || lateral < 0.2 * dl;
            let rem = grid.cut(p.0, p.1, p.2, r, tool.nose, dir, !plunge_like && !first && tool.nose == ToolNose::Square, &mut floor_err, floor_val);
            first = false;
            removed += rem.volume;
            t += dt;
            let cutting = rem.volume > 1e-9;
            let fz_nom = feed / (rpm * z);
            let mut sample_force = ForceResult::default();
            let mut eng_deg = 0.0;
            let mut mode = "air";
            let mut ae_eff = 0.0;
            let mut ap_eff = 0.0;
            let mut defl_wall = 0.0;
            let mut temp = ctx.coolant.temperature_c;
            let mut margin = f64::INFINITY;
            let mut wp_defl = 0.0;
            let mut wp_heat = 0.0;
            let mut pre_here = 0.0;
            let mut cdir = (0.0, 0.0);
            if cutting && dt > 0.0 {
                cut_time += dt;
                cut_samples += 1;
                let vbq = (vb / 0.01).round() * 0.01;
                if (wtool.flank_wear_mm - vbq).abs() > 1e-9 {
                    wtool.flank_wear_mm = vbq;
                }
                let mut wall: Option<(f64, (f64, f64), f64)> = None;
                if plunge_like {
                    plunges += usize::from(is_plunge && first_cut_of_seg);
                    sample_force = physics::plunge_forces(&wtool, &co, fz_nom, rpm);
                    mode = "plunge";
                    eng_deg = 360.0;
                    ap_eff = rem.max_depth.min(tool.loc_mm);
                    ae_eff = 2.0 * r;
                } else {
                    ap_eff = rem.max_depth.clamp(0.0, tool.loc_mm);
                    let raw_ae = if ap_eff > 1e-6 { rem.volume / (ap_eff * lateral.max(1e-9)) } else { 0.0 };
                    ema_ae = if ema_ae <= 0.0 { raw_ae } else { 0.7 * ema_ae + 0.3 * raw_ae };
                    ae_eff = ema_ae.clamp(0.0, 2.0 * r);
                    let lat_mean = rem.lateral / rem.volume.max(1e-12);
                    let eng = if ae_eff >= 1.9 * r {
                        Engagement::side(ap_eff, 2.0 * r, 2.0 * r, true)
                    } else if lat_mean > 0.15 * r {
                        Engagement::side(ap_eff, ae_eff, 2.0 * r, false)
                    } else if lat_mean < -0.15 * r {
                        Engagement::side(ap_eff, ae_eff, 2.0 * r, true)
                    } else {
                        Engagement::centered(ap_eff, ae_eff, 2.0 * r)
                    };
                    mode = match eng.mode {
                        MillMode::Up => "up",
                        MillMode::Down => "down",
                        MillMode::Slot => "slot",
                        MillMode::Center => "center",
                    };
                    let mut fz = fz_nom;
                    if let Some((cx, cy, ra, _)) = arc {
                        if ra > 1e-6 && eng.mode != MillMode::Slot {
                            let to_c = ((cx - p.0) / ra, (cy - p.1) / ra);
                            let side = if eng.mode == MillMode::Up { (-dir.1, dir.0) } else { (dir.1, -dir.0) };
                            let concave = to_c.0 * side.0 + to_c.1 * side.1 < 0.0;
                            fz *= if concave { (ra + r) / ra } else { ((ra - r) / ra).max(0.2) };
                        }
                    }
                    let key = (
                        quant(ap_eff, 0.02),
                        quant(eng.phi_st, 0.035),
                        quant(eng.phi_ex, 0.035),
                        quant(fz, fz_nom.max(1e-6) * 0.02),
                        quant(vbq, 0.01),
                    );
                    let entry = cache.entry(key).or_insert_with(|| CacheEntry {
                        f: physics::mechanistic_forces(&wtool, &co, fz, rpm, &eng, 48, 10),
                    });
                    sample_force = entry.f.clone();
                    eng_deg = eng.span().to_degrees();
                    let ckey = (quant(ap_eff, 0.1), quant(eng.phi_st, 0.07), quant(eng.phi_ex, 0.07));
                    let h_mean = sample_force.h_mean_mm;
                    margin = *chatter_cache.entry(ckey).or_insert_with(|| {
                        crate::dynamics::stability_margin(
                            tool,
                            &co,
                            &eng,
                            h_mean,
                            rpm,
                            ap_eff,
                            ctx.calib.deflection,
                            nominal.vc_effective_m_min,
                            ctx.wp.process_damping,
                        )
                    });
                    if margin.is_finite() {
                        min_margin = min_margin.min(margin);
                        if margin < 1.0 {
                            chatter_samples += 1;
                        }
                    }
                    if let Some(wall_angle) = eng.wall_angle() {
                        let comp = physics::tool_compliance(tool, ap_eff, ctx.calib.deflection);
                        let holder = 1.0 / (tool.holder_stiffness_n_per_um * 1000.0) * ctx.calib.deflection;
                        let steps = sample_force.fy_series.len().max(1);
                        let tan_h = tool.helix_deg.to_radians().tan();
                        let mut acc = 0.0;
                        let mut force_acc = 0.0;
                        let slices = 4;
                        for k in 0..slices {
                            let zk = (k as f64 + 0.5) * ap_eff / slices as f64;
                            let theta = (wall_angle + zk * tan_h / r).rem_euclid(sample_force.pitch_rad.max(1e-9));
                            let idx = ((theta / sample_force.pitch_rad.max(1e-9)) * steps as f64).floor() as usize % steps;
                            let fy = sample_force.fy_series.get(idx).cloned().unwrap_or(0.0);
                            let away = if eng.mode == MillMode::Down { fy } else { -fy };
                            acc += physics::deflection_at(&comp, away, (tool.stickout_mm - zk).max(0.0), holder) / slices as f64;
                            force_acc += away / slices as f64;
                        }
                        defl_wall = acc;
                        let side = if eng.mode == MillMode::Up { (-dir.1, dir.0) } else { (dir.1, -dir.0) };
                        wall = Some((p.2 + ap_eff / 2.0, side, force_acc));
                    }
                }
                let vce = physics::effective_vc(tool, rpm, ap_eff);
                let eng_for_heat = Engagement {
                    ap_mm: ap_eff,
                    ae_mm: ae_eff,
                    phi_st: 0.0,
                    phi_ex: eng_deg.to_radians().max(1e-3),
                    mode: MillMode::Center,
                };
                let vol = rem.volume.max(1e-12);
                let (lat_c, fwd_c) = (rem.lateral / vol, rem.forward / vol);
                let (odx, ody) = if plunge_like { (0.0, 0.0) } else { (fwd_c * dir.0 - lat_c * dir.1, fwd_c * dir.1 + lat_c * dir.0) };
                let on = (odx * odx + ody * ody).sqrt();
                if on > 0.05 * r {
                    cdir = (odx / on, ody / on);
                }
                let (hx, hy, hz) = (p.0 + odx, p.1 + ody, p.2 + 0.25 * ap_eff);
                let self_window = (2.0 * r / (feed / 60.0).max(1e-3)).clamp(0.05, 10.0);
                let moved = ((hx - last_eval_xy.0).powi(2) + (hy - last_eval_xy.1).powi(2)).sqrt();
                if !(t - last_eval_t < 0.05 && moved < 0.5) {
                    preheat = heat.excess(hx, hy, hz, t, t - self_window).max(0.0);
                    last_eval_t = t;
                    last_eval_xy = (hx, hy);
                }
                pre_here = preheat;
                max_preheat = max_preheat.max(preheat);
                let th = physics::thermal_preheated(ctx, &co, &sample_force, vce, rpm, &eng_for_heat, body.mass_kg, body.area_m2, preheat + wp_rise.max(0.0));
                wp_heat = th.workpiece_heat_w;
                heat.deposit(hx, hy, hz, t - 0.5 * dt, th.workpiece_heat_w * dt, tau0);
                if t - last_prune_t > 1.0 {
                    heat.prune(t);
                    last_prune_t = t;
                }
                let wr = physics::wear(ctx, &sample_force, &th, vce, fz_nom.max(1e-6), &eng_for_heat, reference);
                let t_cut_min = cut_time / 60.0;
                let rate = kappa * wr.vb_rate_mm_per_min + vb_break / tau_break * (-t_cut_min / tau_break).exp();
                vb = (vb + rate * dt / 60.0).min(2.0 * ctx.vb_limit_mm());
                temp = th.interface_c;
                wp_rise += ((th.workpiece_heat_w - wp_rise * ha) / mc_wp.max(1e-6)) * dt;
                if let Some((z_mid, side, away_force)) = wall {
                    let wear_um = vb * tan_clear * 1000.0;
                    let th_tool = -th.tool_radial_growth_um;
                    let wx = p.0 + side.0 * r;
                    let wy = p.1 + side.1 * r;
                    let dist_datum = ((wx - datum.0) * side.0 + (wy - datum.1) * side.1).abs();
                    let thermal_wp = -ctx.wp.expansion * dist_datum * wp_rise * 1000.0;
                    if let Some((um, thick)) = thin_wall_um(&grid, wx, wy, side, p.2, z_mid, ap_eff, away_force, e_wp) {
                        wp_defl = um;
                        max_wp_defl = max_wp_defl.max(um.abs());
                        if um.abs() > 1.0 {
                            thin_wall = Some(thin_wall.map(|t: f64| t.min(thick)).unwrap_or(thick));
                        }
                    }
                    if wall_errors.len() < 200_000 {
                        wall_errors.push(WallErrorSample {
                            x: wx,
                            y: wy,
                            nx: side.0,
                            ny: side.1,
                            z_mid,
                            t_s: t,
                            deflection_um: defl_wall,
                            wear_um,
                            thermal_tool_um: th_tool,
                            thermal_wp_um: thermal_wp,
                            total_um: defl_wall + wp_defl + wear_um + th_tool + thermal_wp,
                            workpiece_um: wp_defl,
                        });
                    }
                }
                first_cut_of_seg = false;
            } else {
                wp_rise += (-wp_rise * ha / mc_wp.max(1e-6)) * dt;
            }
            let power = sample_force.cutting_power_kw / profile.machine.efficiency.max(0.1);
            max_power = max_power.max(power);
            max_force = max_force.max(sample_force.f_res_peak);
            max_temp = max_temp.max(temp);
            max_defl = max_defl.max(defl_wall.abs());
            samples.push(SimSample {
                t_s: t,
                x: p.0,
                y: p.1,
                z: p.2,
                seg: si,
                kind: seg.kind().into(),
                feed,
                ae_mm: ae_eff,
                ap_mm: ap_eff,
                engage_deg: eng_deg,
                mode: mode.into(),
                force_n: sample_force.f_res_mean,
                force_peak_n: sample_force.f_res_peak,
                torque_nm: sample_force.torque_mean_nm,
                power_kw: power,
                wall_defl_um: defl_wall,
                temp_c: temp,
                vb_mm: vb,
                radial_loss_um: vb * tan_clear * 1000.0,
                wp_temp_c: bulk + wp_rise,
                chatter_margin: if margin.is_finite() { margin } else { 0.0 },
                workpiece_um: wp_defl,
                wp_local_c: bulk + wp_rise + pre_here,
                wp_heat_w: wp_heat,
                preheat_c: pre_here,
                contact_dx: cdir.0,
                contact_dy: cdir.1,
            });
            last = *p;
        }
    }
    if max_power > profile.machine.available_power_kw(rpm) {
        warnings.push(format!(
            "최대 필요 동력 {:.2} kW 가 가용 동력 {:.2} kW 를 초과합니다",
            max_power,
            profile.machine.available_power_kw(rpm)
        ));
    }
    if plunges > 0 && tool.nose == ToolNose::Square {
        warnings.push(format!("수직 플런지 {}회: 램핑/헬리컬 진입으로 바꾸면 날끝 부하와 치핑 위험이 줄어듭니다", plunges));
    }
    let chatter_fraction = if cut_samples > 0 { chatter_samples as f64 / cut_samples as f64 } else { 0.0 };
    if chatter_samples > 0 {
        warnings.push(format!(
            "경로의 {:.0}% 구간이 재생 채터 한계를 넘습니다 (최소 여유 {:.2}배, {}) — 회전수를 안정 로브로 옮기거나 ap 를 낮추세요",
            chatter_fraction * 100.0,
            min_margin,
            if tool.measured_fn_hz.is_some() && tool.measured_k_n_per_um.is_some() { "실측 FRF" } else { "모델 추정 FRF · 탭 테스트로 확정" }
        ));
    }
    if let Some(tw) = thin_wall {
        warnings.push(format!(
            "얇은 벽(두께 약 {:.1} mm)이 절삭력으로 최대 {:.1} µm 휘어 벽면 오차에 더해집니다",
            tw, max_wp_defl
        ));
    }
    if max_preheat > 15.0 {
        warnings.push(format!(
            "이전 패스의 잔류열이 다 빠지기 전에 다음 패스가 지나가 절삭 지점 소재가 최대 {:.0} K 예열됩니다 (열확산율 {:.1} mm²/s) — 패스 순서·간격 조정 또는 냉각 강화를 검토하세요",
            max_preheat,
            heat_props.kappa_m2_s * 1e6
        ));
    }
    let mut segment_end_s: Vec<f64> = seg_start_s.iter().skip(1).cloned().collect();
    if !segments.is_empty() {
        segment_end_s.push(t);
    }
    let (heat_packets, heat_field) = heat.finish(6000);
    let episodes = build_episodes(&samples, ctx, profile);
    let (mean_wall, max_wall) = if wall_errors.is_empty() {
        (0.0, 0.0)
    } else {
        (
            wall_errors.iter().map(|w| w.total_um).sum::<f64>() / wall_errors.len() as f64,
            wall_errors.iter().map(|w| w.total_um.abs()).fold(0.0, f64::max),
        )
    };
    let floor_vals: Vec<f32> = floor_err.iter().cloned().filter(|v| v.is_finite()).collect();
    let floor_mean = if floor_vals.is_empty() {
        0.0
    } else {
        floor_vals.iter().map(|v| *v as f64).sum::<f64>() / floor_vals.len() as f64
    };
    let floor_grid = StockGrid {
        x0: grid.x0,
        y0: grid.y0,
        cell: grid.cell,
        nx: grid.nx,
        ny: grid.ny,
        h: floor_err.iter().map(|v| if v.is_finite() { *v } else { NO_MATERIAL }).collect(),
        top: 0.0,
        bottom: grid.bottom,
    };
    let stride = (samples.len() / 4000).max(1);
    let samples_ds: Vec<SimSample> = samples
        .iter()
        .enumerate()
        .filter(|(i, _)| i % stride == 0 || *i + 1 == samples.len())
        .map(|(_, s)| s.clone())
        .collect();
    let wstride = (wall_errors.len() / 3000).max(1);
    let walls_ds: Vec<WallErrorSample> = wall_errors.iter().step_by(wstride).cloned().collect();
    LoadSimReport {
        pattern: label.into(),
        episodes,
        wall_errors: walls_ds,
        heightmap: grid.downsample(256),
        floor_error: floor_grid.downsample(128),
        total_time_min: t / 60.0,
        cut_time_min: cut_time / 60.0,
        rapid_time_min: rapid_time / 60.0,
        removed_volume_cm3: removed / 1000.0,
        vb_end_mm: vb,
        radial_loss_end_um: vb * tan_clear * 1000.0,
        max_wall_defl_um: max_defl,
        mean_wall_error_um: mean_wall,
        max_wall_error_um: max_wall,
        floor_error_mean_um: floor_mean,
        max_power_kw: max_power,
        max_force_n: max_force,
        max_temp_c: max_temp,
        wp_temp_end_c: bulk + wp_rise,
        plunge_count: plunges,
        cell_mm: grid.cell,
        samples: samples_ds,
        warnings,
        max_workpiece_defl_um: max_wp_defl,
        min_chatter_margin: if min_margin.is_finite() { min_margin } else { 0.0 },
        chatter_fraction,
        thin_wall_mm: thin_wall,
        heat_packets,
        heat: heat_field,
        segment_end_s,
        max_preheat_c: max_preheat,
        bulk_c: bulk,
    }
}

fn build_episodes(samples: &[SimSample], ctx: &CutContext, profile: &MachiningProfile) -> Vec<Episode> {
    let idx: Vec<usize> = samples.iter().enumerate().filter(|(_, s)| s.force_n > 0.0).map(|(i, _)| i).collect();
    if idx.is_empty() {
        return Vec::new();
    }
    let feats: Vec<Vec<f64>> = idx
        .iter()
        .map(|i| {
            let s = &samples[*i];
            vec![s.power_kw, s.force_peak_n, s.wall_defl_um, s.temp_c, s.engage_deg]
        })
        .collect();
    let times: Vec<f64> = idx.iter().map(|i| samples[*i].t_s).collect();
    let parts = split_episodes(&feats, &times);
    let budget = (0.3 * ctx.tolerance_mm * 1000.0).max(2.0).max(0.5 * ctx.allowance_mm * 1000.0);
    let rpm = profile.conditions.spindle_rpm.max(1) as f64;
    let avail = profile.machine.available_power_kw(rpm) * 0.9;
    let t_lim = 0.85 * ctx.tool.coating.max_temp_c;
    let expo = 1.0 / (1.0 - ctx.wp.mc).max(0.5);
    let raw: Vec<Episode> = parts
        .into_iter()
        .map(|(a, b)| {
            let ss: Vec<&SimSample> = idx[a..b].iter().map(|i| &samples[*i]).collect();
            let n = ss.len().max(1) as f64;
            let mean_power = ss.iter().map(|s| s.power_kw).sum::<f64>() / n;
            let peak_power = ss.iter().map(|s| s.power_kw).fold(0.0, f64::max);
            let peak_force = ss.iter().map(|s| s.force_peak_n).fold(0.0, f64::max);
            let peak_defl = ss.iter().map(|s| s.wall_defl_um.abs()).fold(0.0, f64::max);
            let mean_temp = ss.iter().map(|s| s.temp_c).sum::<f64>() / n;
            let max_temp = ss.iter().map(|s| s.temp_c).fold(0.0, f64::max);
            let mean_eng = ss.iter().map(|s| s.engage_deg).sum::<f64>() / n;
            let mut scale: f64 = 1.0;
            let mut limiting = "없음".to_string();
            if peak_defl > budget {
                let s = (budget / peak_defl).powf(expo);
                if s < scale {
                    scale = s;
                    limiting = format!("휨 {:.1}µm > 예산 {:.1}µm", peak_defl, budget);
                }
            }
            if peak_power > avail && peak_power > 0.0 {
                let s = (avail / peak_power).powf(expo);
                if s < scale {
                    scale = s;
                    limiting = format!("동력 {:.2}kW > {:.2}kW", peak_power, avail);
                }
            }
            if max_temp > t_lim {
                let t0 = ctx.coolant.temperature_c;
                let s = ((t_lim - t0) / (max_temp - t0).max(1.0)).powi(3);
                if s < scale {
                    scale = s;
                    limiting = format!("날끝 {:.0}°C > {:.0}°C", max_temp, t_lim);
                }
            }
            Episode {
                start: idx[a],
                end: idx[b - 1],
                t_start_s: ss.first().map(|s| s.t_s).unwrap_or(0.0),
                t_end_s: ss.last().map(|s| s.t_s).unwrap_or(0.0),
                seg_from: ss.first().map(|s| s.seg).unwrap_or(0),
                seg_to: ss.last().map(|s| s.seg).unwrap_or(0),
                mean_power_kw: mean_power,
                peak_force_n: peak_force,
                peak_defl_um: peak_defl,
                mean_temp_c: mean_temp,
                mean_engage_deg: mean_eng,
                feed_scale: scale.clamp(0.3, 1.0),
                limiting,
            }
        })
        .collect();
    merge_episodes(raw, 60)
}

fn similar(a: &Episode, b: &Episode) -> bool {
    let rel = |x: f64, y: f64| (x - y).abs() / x.abs().max(y.abs()).max(1e-6);
    rel(a.mean_power_kw, b.mean_power_kw) < 0.15
        && rel(a.peak_force_n, b.peak_force_n) < 0.15
        && (a.feed_scale - b.feed_scale).abs() < 0.05
        && (a.mean_engage_deg - b.mean_engage_deg).abs() < 25.0
}

fn absorb(a: &mut Episode, b: &Episode) {
    let da = (a.t_end_s - a.t_start_s).max(1e-6);
    let db = (b.t_end_s - b.t_start_s).max(1e-6);
    let w = da / (da + db);
    a.mean_power_kw = a.mean_power_kw * w + b.mean_power_kw * (1.0 - w);
    a.mean_temp_c = a.mean_temp_c * w + b.mean_temp_c * (1.0 - w);
    a.mean_engage_deg = a.mean_engage_deg * w + b.mean_engage_deg * (1.0 - w);
    a.peak_force_n = a.peak_force_n.max(b.peak_force_n);
    a.peak_defl_um = a.peak_defl_um.max(b.peak_defl_um);
    if b.feed_scale < a.feed_scale {
        a.feed_scale = b.feed_scale;
        a.limiting = b.limiting.clone();
    }
    a.end = b.end;
    a.t_end_s = b.t_end_s;
    a.seg_to = b.seg_to;
}

fn merge_episodes(raw: Vec<Episode>, cap: usize) -> Vec<Episode> {
    let mut out: Vec<Episode> = Vec::with_capacity(raw.len());
    for e in raw.into_iter() {
        match out.last_mut() {
            Some(last) if similar(last, &e) => absorb(last, &e),
            _ => out.push(e),
        }
    }
    while out.len() > cap {
        let mut best = 0usize;
        let mut best_d = f64::INFINITY;
        for i in 0..out.len() - 1 {
            let d = (out[i].t_end_s - out[i].t_start_s) + (out[i + 1].t_end_s - out[i + 1].t_start_s);
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        let next = out.remove(best + 1);
        absorb(&mut out[best], &next);
    }
    out
}

pub fn adaptive_segments(segments: &[ToolPathSegment], episodes: &[Episode]) -> Vec<ToolPathSegment> {
    let mut scale_of: HashMap<usize, f64> = HashMap::new();
    for e in episodes.iter() {
        for s in e.seg_from..=e.seg_to {
            let v = scale_of.entry(s).or_insert(1.0);
            *v = v.min(e.feed_scale);
        }
    }
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let k = *scale_of.get(&i).unwrap_or(&1.0);
            if (k - 1.0).abs() < 1e-6 {
                return s.clone();
            }
            match s.clone() {
                ToolPathSegment::Linear { x, y, z, feed } => ToolPathSegment::Linear { x, y, z, feed: feed * k },
                ToolPathSegment::ArcCW { x, y, z, i, j, feed } => ToolPathSegment::ArcCW { x, y, z, i, j, feed: feed * k },
                ToolPathSegment::ArcCCW { x, y, z, i, j, feed } => ToolPathSegment::ArcCCW { x, y, z, i, j, feed: feed * k },
                ToolPathSegment::Plunge { x, y, z_start, z_end, feed } => ToolPathSegment::Plunge { x, y, z_start, z_end, feed: feed * k },
                other => other,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_wall_compliance_grows_as_wall_thins() {
        let mut g = StockGrid {
            x0: 0.0,
            y0: 0.0,
            cell: 0.1,
            nx: 200,
            ny: 10,
            h: vec![0.0; 2000],
            top: 0.0,
            bottom: -20.0,
        };
        for j in 0..10 {
            for i in 60..200 {
                g.h[j * 200 + i] = -10.0;
            }
        }
        let thick = thin_wall_um(&g, 3.0, 0.5, (1.0, 0.0), -10.0, -5.0, 10.0, 100.0, 69000.0).unwrap();
        let thin = thin_wall_um(&g, 5.0, 0.5, (1.0, 0.0), -10.0, -5.0, 10.0, 100.0, 69000.0).unwrap();
        assert!(thin.0 > 10.0 * thick.0, "{:?} {:?}", thin, thick);
        assert!((thick.1 - 3.0).abs() < 0.2);
        let mut solid = g.clone();
        for v in solid.h.iter_mut() {
            *v = 0.0;
        }
        assert!(thin_wall_um(&solid, 3.0, 0.5, (1.0, 0.0), -10.0, -5.0, 10.0, 100.0, 69000.0).is_none());
    }
}
