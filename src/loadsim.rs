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
}

#[derive(Clone)]
struct CacheEntry {
    f: ForceResult,
}

fn quant(v: f64, step: f64) -> i64 {
    (v / step).round() as i64
}

pub fn simulate(profile: &MachiningProfile, pattern: &ToolPathPattern, ctx: &CutContext) -> LoadSimReport {
    let segments = GCodeGenerator::generate_synthetic_segments_with_pattern(profile, pattern);
    simulate_segments(profile, &segments, pattern.key(), ctx)
}

pub fn simulate_segments(profile: &MachiningProfile, segments: &[ToolPathSegment], label: &str, ctx: &CutContext) -> LoadSimReport {
    let mut grid = StockGrid::from_setup(profile, 3_000_000);
    let mut floor_err = vec![f32::NAN; grid.nx * grid.ny];
    let tool = &ctx.tool;
    let r = tool.radius();
    let z = tool.flutes.max(1) as f64;
    let rpm = profile.conditions.spindle_rpm.max(1) as f64;
    let co = physics::cutting_coeffs(ctx);
    let reference = physics::reference_state(ctx);
    let body = ThermalBody::of_setup(&profile.workpiece_setup, &ctx.wp);
    let mut cache: HashMap<(i64, i64, i64, i64), CacheEntry> = HashMap::new();
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
            if cutting && dt > 0.0 {
                cut_time += dt;
                let mut wall: Option<(f64, (f64, f64))> = None;
                if plunge_like {
                    plunges += usize::from(is_plunge && first_cut_of_seg);
                    sample_force = physics::plunge_forces(tool, &co, fz_nom, rpm);
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
                    let key = (quant(ap_eff, 0.02), quant(eng.phi_st, 0.035), quant(eng.phi_ex, 0.035), quant(fz, fz_nom.max(1e-6) * 0.02));
                    let entry = cache.entry(key).or_insert_with(|| CacheEntry {
                        f: physics::mechanistic_forces(tool, &co, fz, rpm, &eng, 48, 10),
                    });
                    sample_force = entry.f.clone();
                    eng_deg = eng.span().to_degrees();
                    if let Some(wall_angle) = eng.wall_angle() {
                        let comp = physics::tool_compliance(tool, ap_eff, ctx.calib.deflection);
                        let holder = 1.0 / (tool.holder_stiffness_n_per_um * 1000.0) * ctx.calib.deflection;
                        let steps = sample_force.fy_series.len().max(1);
                        let tan_h = tool.helix_deg.to_radians().tan();
                        let mut acc = 0.0;
                        let slices = 4;
                        for k in 0..slices {
                            let zk = (k as f64 + 0.5) * ap_eff / slices as f64;
                            let theta = (wall_angle + zk * tan_h / r).rem_euclid(sample_force.pitch_rad.max(1e-9));
                            let idx = ((theta / sample_force.pitch_rad.max(1e-9)) * steps as f64).floor() as usize % steps;
                            let fy = sample_force.fy_series.get(idx).cloned().unwrap_or(0.0);
                            let away = if eng.mode == MillMode::Down { fy } else { -fy };
                            acc += physics::deflection_at(&comp, away, (tool.stickout_mm - zk).max(0.0), holder) / slices as f64;
                        }
                        defl_wall = acc;
                        let side = if eng.mode == MillMode::Up { (-dir.1, dir.0) } else { (dir.1, -dir.0) };
                        wall = Some((p.2 + ap_eff / 2.0, side));
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
                let th = physics::thermal(ctx, &co, &sample_force, vce, rpm, &eng_for_heat, body.mass_kg, body.area_m2);
                let wr = physics::wear(ctx, &sample_force, &th, vce, fz_nom.max(1e-6), &eng_for_heat, reference);
                vb += wr.vb_rate_mm_per_min * dt / 60.0;
                temp = th.interface_c;
                wp_rise += ((th.workpiece_heat_w - wp_rise * ha) / mc_wp.max(1e-6)) * dt;
                if let Some((z_mid, side)) = wall {
                    let wear_um = vb * tan_clear * 1000.0;
                    let th_tool = -th.tool_radial_growth_um;
                    let wx = p.0 + side.0 * r;
                    let wy = p.1 + side.1 * r;
                    let dist_datum = ((wx - datum.0) * side.0 + (wy - datum.1) * side.1).abs();
                    let thermal_wp = -ctx.wp.expansion * dist_datum * wp_rise * 1000.0;
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
                            total_um: defl_wall + wear_um + th_tool + thermal_wp,
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
                wp_temp_c: 22.0 + wp_rise,
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
        wp_temp_end_c: 22.0 + wp_rise,
        plunge_count: plunges,
        cell_mm: grid.cell,
        samples: samples_ds,
        warnings,
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