use crate::loadsim::{HeightmapView, LoadSimReport, WallErrorSample};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoldDescriptor {
    pub depth_hist: Vec<f32>,
    pub slope_hist: Vec<f32>,
    pub concave_hist: Vec<f32>,
    pub wall_dir_hist: Vec<f32>,
    pub max_depth_mm: f64,
    pub mean_depth_mm: f64,
    pub cavity_area_ratio: f64,
    pub steep_ratio: f64,
    pub flat_ratio: f64,
    pub min_concave_radius_mm: f64,
    pub rest_material_ratio: f64,
    pub removed_volume_cm3: f64,
    pub reach_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoldVector {
    pub namespace: String,
    pub dims: Vec<(String, usize)>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoldComparison {
    pub cosine: f32,
    pub block_distance: Vec<(String, f32)>,
}

fn valid(z: f32) -> bool {
    z.is_finite()
}

fn grad(h: &HeightmapView, i: usize, j: usize) -> Option<(f64, f64)> {
    let at = |ii: usize, jj: usize| -> Option<f64> {
        let v = h.z[jj * h.nx + ii];
        if valid(v) {
            Some(v as f64)
        } else {
            None
        }
    };
    let c = at(i, j)?;
    let xm = if i > 0 { at(i - 1, j) } else { None };
    let xp = if i + 1 < h.nx { at(i + 1, j) } else { None };
    let ym = if j > 0 { at(i, j - 1) } else { None };
    let yp = if j + 1 < h.ny { at(i, j + 1) } else { None };
    let gx = match (xm, xp) {
        (Some(a), Some(b)) => (b - a) / (2.0 * h.cell_mm),
        (Some(a), None) => (c - a) / h.cell_mm,
        (None, Some(b)) => (b - c) / h.cell_mm,
        _ => 0.0,
    };
    let gy = match (ym, yp) {
        (Some(a), Some(b)) => (b - a) / (2.0 * h.cell_mm),
        (Some(a), None) => (c - a) / h.cell_mm,
        (None, Some(b)) => (b - c) / h.cell_mm,
        _ => 0.0,
    };
    Some((gx, gy))
}

fn laplacian_curvature(h: &HeightmapView, i: usize, j: usize) -> Option<f64> {
    if i == 0 || j == 0 || i + 1 >= h.nx || j + 1 >= h.ny {
        return None;
    }
    let v = |ii: usize, jj: usize| h.z[jj * h.nx + ii];
    let c = v(i, j);
    let n = [v(i - 1, j), v(i + 1, j), v(i, j - 1), v(i, j + 1)];
    if !valid(c) || n.iter().any(|x| !valid(*x)) {
        return None;
    }
    let lap = (n.iter().map(|x| *x as f64).sum::<f64>() - 4.0 * c as f64) / (h.cell_mm * h.cell_mm);
    Some(lap / 2.0)
}

fn normalize(v: &mut [f32]) {
    let s: f32 = v.iter().sum();
    if s > 0.0 {
        for x in v.iter_mut() {
            *x /= s;
        }
    }
}

pub fn describe(h: &HeightmapView, tool_radius_mm: f64, stickout_mm: f64) -> MoldDescriptor {
    let mut depth_hist = vec![0f32; 16];
    let mut slope_hist = vec![0f32; 12];
    let mut concave_hist = vec![0f32; 6];
    let mut wall_dir_hist = vec![0f32; 8];
    let mut max_depth = 0.0f64;
    let mut depth_sum = 0.0;
    let mut n_mat = 0.0;
    let mut n_cav = 0.0;
    let mut steep = 0.0;
    let mut flat = 0.0;
    let mut min_r = f64::INFINITY;
    let mut rest = 0.0;
    let mut vol = 0.0;
    let full = (h.bottom as f64).abs().max(1e-6);
    for j in 0..h.ny {
        for i in 0..h.nx {
            let z = h.z[j * h.nx + i];
            if !valid(z) {
                continue;
            }
            n_mat += 1.0;
            let depth = (-(z as f64)).max(0.0);
            max_depth = max_depth.max(depth);
            depth_sum += depth;
            vol += depth * h.cell_mm * h.cell_mm;
            if depth > 1e-3 {
                n_cav += 1.0;
            }
            let b = ((depth / full) * 16.0).floor().clamp(0.0, 15.0) as usize;
            depth_hist[b] += 1.0;
            if let Some((gx, gy)) = grad(h, i, j) {
                let slope = (gx * gx + gy * gy).sqrt().atan().to_degrees();
                slope_hist[((slope / 7.5).floor() as usize).min(11)] += 1.0;
                if slope > 60.0 {
                    steep += 1.0;
                    let ang = gy.atan2(gx).to_degrees().rem_euclid(360.0);
                    wall_dir_hist[((ang / 45.0).floor() as usize).min(7)] += 1.0;
                }
                if slope < 5.0 {
                    flat += 1.0;
                }
            }
            if let Some(k) = laplacian_curvature(h, i, j) {
                if k > 1e-4 {
                    let r = 1.0 / k;
                    min_r = min_r.min(r);
                    let rel = r / tool_radius_mm.max(1e-6);
                    let bin = if rel < 0.5 {
                        0
                    } else if rel < 1.0 {
                        1
                    } else if rel < 2.0 {
                        2
                    } else if rel < 4.0 {
                        3
                    } else {
                        4
                    };
                    concave_hist[bin] += 1.0;
                    if rel < 1.0 {
                        rest += 1.0;
                    }
                } else {
                    concave_hist[5] += 1.0;
                }
            }
        }
    }
    normalize(&mut depth_hist);
    normalize(&mut slope_hist);
    normalize(&mut concave_hist);
    normalize(&mut wall_dir_hist);
    MoldDescriptor {
        depth_hist,
        slope_hist,
        concave_hist,
        wall_dir_hist,
        max_depth_mm: max_depth,
        mean_depth_mm: if n_mat > 0.0 { depth_sum / n_mat } else { 0.0 },
        cavity_area_ratio: if n_mat > 0.0 { n_cav / n_mat } else { 0.0 },
        steep_ratio: if n_mat > 0.0 { steep / n_mat } else { 0.0 },
        flat_ratio: if n_mat > 0.0 { flat / n_mat } else { 0.0 },
        min_concave_radius_mm: if min_r.is_finite() { min_r } else { 0.0 },
        rest_material_ratio: if n_mat > 0.0 { rest / n_mat } else { 0.0 },
        removed_volume_cm3: vol / 1000.0,
        reach_ratio: (max_depth + 2.0) / stickout_mm.max(1.0),
    }
}

pub fn mold_vector(d: &MoldDescriptor) -> MoldVector {
    let scalars = vec![
        (d.max_depth_mm / 50.0).min(2.0) as f32,
        (d.mean_depth_mm / 50.0).min(2.0) as f32,
        d.cavity_area_ratio as f32,
        d.steep_ratio as f32,
        d.flat_ratio as f32,
        d.rest_material_ratio as f32,
        (d.reach_ratio / 2.0).min(2.0) as f32,
    ];
    let blocks: Vec<(&str, Vec<f32>)> = vec![
        ("depth", d.depth_hist.clone()),
        ("slope", d.slope_hist.clone()),
        ("concave", d.concave_hist.clone()),
        ("wall_dir", d.wall_dir_hist.clone()),
        ("scalars", scalars),
    ];
    let mut values = Vec::new();
    let mut dims = Vec::new();
    for (name, mut b) in blocks.into_iter() {
        let n = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            for x in b.iter_mut() {
                *x /= n;
            }
        }
        dims.push((name.to_string(), b.len()));
        values.extend(b);
    }
    crate::ml::l2_normalize(&mut values);
    MoldVector {
        namespace: "mold.geom".into(),
        dims,
        values,
    }
}

pub fn compare(a: &MoldVector, b: &MoldVector) -> MoldComparison {
    let mut off = 0;
    let mut blocks = Vec::new();
    for (name, n) in a.dims.iter() {
        let ea = (off + n).min(a.values.len());
        let eb = (off + n).min(b.values.len());
        if ea <= off || eb <= off {
            break;
        }
        let d = 1.0 - crate::ml::cosine(&a.values[off..ea], &b.values[off..eb]);
        blocks.push((name.clone(), d));
        off += n;
    }
    MoldComparison {
        cosine: crate::ml::cosine(&a.values, &b.values),
        block_distance: blocks,
    }
}

pub fn heightmap_from_depth_image(bytes: &[u8], mm_per_px: f64, depth_range_mm: f64, invert: bool, origin: (f64, f64)) -> Result<HeightmapView, String> {
    let img = crate::toolimage::decode(bytes)?;
    let g16 = img.to_luma16();
    let (w, h) = (g16.width() as usize, g16.height() as usize);
    let max_side = 400usize;
    let f = ((w.max(h) as f64) / max_side as f64).ceil().max(1.0) as usize;
    let nx = (w + f - 1) / f;
    let ny = (h + f - 1) / f;
    let mut z = vec![f32::NAN; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let mut acc = 0.0f64;
            let mut n = 0.0f64;
            for jj in j * f..((j + 1) * f).min(h) {
                for ii in i * f..((i + 1) * f).min(w) {
                    acc += g16.get_pixel(ii as u32, (h - 1 - jj) as u32)[0] as f64;
                    n += 1.0;
                }
            }
            let v = acc / n.max(1.0) / 65535.0;
            let depth = if invert { v } else { 1.0 - v } * depth_range_mm;
            z[j * nx + i] = -(depth as f32);
        }
    }
    Ok(HeightmapView {
        x0: origin.0,
        y0: origin.1,
        cell_mm: mm_per_px * f as f64,
        nx,
        ny,
        z,
        bottom: -(depth_range_mm as f32),
    })
}

pub fn heightmap_from_csv(text: &str, cell_hint_mm: Option<f64>) -> Result<HeightmapView, String> {
    let rows: Vec<Vec<f64>> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| {
            l.split(|c| c == ',' || c == ';' || c == '\t' || c == ' ')
                .filter(|t| !t.trim().is_empty())
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .collect::<Vec<f64>>()
        })
        .filter(|r| !r.is_empty())
        .collect();
    if rows.is_empty() {
        return Err("높이맵 CSV 에 숫자 데이터가 없습니다".into());
    }
    if rows.iter().all(|r| r.len() == 3) && rows.len() > 8 {
        let xs: Vec<f64> = rows.iter().map(|r| r[0]).collect();
        let ys: Vec<f64> = rows.iter().map(|r| r[1]).collect();
        let (x0, x1) = (xs.iter().cloned().fold(f64::INFINITY, f64::min), xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
        let (y0, y1) = (ys.iter().cloned().fold(f64::INFINITY, f64::min), ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
        let area = ((x1 - x0) * (y1 - y0)).max(1e-6);
        let cell = cell_hint_mm.unwrap_or((area / rows.len() as f64).sqrt()).max(1e-3);
        let nx = ((x1 - x0) / cell).floor() as usize + 1;
        let ny = ((y1 - y0) / cell).floor() as usize + 1;
        if nx * ny > 4_000_000 {
            return Err("점군 격자가 너무 큽니다. 셀 크기를 키워 주세요".into());
        }
        let mut acc = vec![0f64; nx * ny];
        let mut cnt = vec![0f64; nx * ny];
        for r in rows.iter() {
            let i = (((r[0] - x0) / cell).round() as usize).min(nx - 1);
            let j = (((r[1] - y0) / cell).round() as usize).min(ny - 1);
            acc[j * nx + i] += r[2];
            cnt[j * nx + i] += 1.0;
        }
        let z: Vec<f32> = acc
            .iter()
            .zip(cnt.iter())
            .map(|(a, c)| if *c > 0.0 { (*a / *c) as f32 } else { f32::NAN })
            .collect();
        let bottom = z.iter().cloned().filter(|v| v.is_finite()).fold(0f32, f32::min);
        return Ok(HeightmapView { x0, y0, cell_mm: cell, nx, ny, z, bottom });
    }
    let nx = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let ny = rows.len();
    let cell = cell_hint_mm.unwrap_or(1.0);
    let mut z = vec![f32::NAN; nx * ny];
    for (j, r) in rows.iter().enumerate() {
        for (i, v) in r.iter().enumerate() {
            z[(ny - 1 - j) * nx + i] = *v as f32;
        }
    }
    let bottom = z.iter().cloned().filter(|v| v.is_finite()).fold(0f32, f32::min);
    Ok(HeightmapView { x0: 0.0, y0: 0.0, cell_mm: cell, nx, ny, z, bottom })
}

pub fn sample(h: &HeightmapView, x: f64, y: f64) -> Option<f64> {
    let fx = (x - h.x0) / h.cell_mm - 0.5;
    let fy = (y - h.y0) / h.cell_mm - 0.5;
    if fx < 0.0 || fy < 0.0 {
        return None;
    }
    let i0 = fx.floor() as usize;
    let j0 = fy.floor() as usize;
    if i0 + 1 >= h.nx || j0 + 1 >= h.ny {
        return None;
    }
    let (tx, ty) = (fx - i0 as f64, fy - j0 as f64);
    let v = |i: usize, j: usize| h.z[j * h.nx + i];
    let q = [v(i0, j0), v(i0 + 1, j0), v(i0, j0 + 1), v(i0 + 1, j0 + 1)];
    if q.iter().any(|z| !z.is_finite()) {
        return None;
    }
    let a = q[0] as f64 * (1.0 - tx) + q[1] as f64 * tx;
    let b = q[2] as f64 * (1.0 - tx) + q[3] as f64 * tx;
    Some(a * (1.0 - ty) + b * ty)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coef {
    pub name: String,
    pub label: String,
    pub value: f64,
    pub stderr: f64,
    pub identifiable: bool,
    pub predicted: Option<f64>,
    pub ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviationFit {
    pub points: usize,
    pub span_mm: f64,
    pub coefficients: Vec<Coef>,
    pub r2: f64,
    pub rms_dev_um: f64,
    pub rms_residual_um: f64,
    pub max_abs_dev_um: f64,
    pub residual_map: HeightmapView,
    pub notes: Vec<String>,
}

fn solve(a: &mut [Vec<f64>], b: &mut [f64]) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let piv = (c..n).max_by(|i, j| a[*i][c].abs().partial_cmp(&a[*j][c].abs()).unwrap_or(std::cmp::Ordering::Equal))?;
        if a[piv][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in 0..n {
            if r != c {
                let f = a[r][c] / a[c][c];
                for k in c..n {
                    a[r][k] -= f * a[c][k];
                }
                b[r] -= f * b[c];
            }
        }
    }
    Some((0..n).map(|i| b[i] / a[i][i]).collect())
}

fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut cols = Vec::with_capacity(n);
    for c in 0..n {
        let mut a = m.to_vec();
        let mut e = vec![0.0; n];
        e[c] = 1.0;
        cols.push(solve(&mut a, &mut e)?);
    }
    Some((0..n).map(|r| (0..n).map(|c| cols[c][r]).collect()).collect())
}

pub fn least_squares(x: &[Vec<f64>], y: &[f64]) -> Option<(Vec<f64>, Vec<f64>, f64, f64)> {
    let p = x.first()?.len();
    let n = y.len();
    if n <= p {
        return None;
    }
    let mut xtx = vec![vec![0.0; p]; p];
    let mut xty = vec![0.0; p];
    for (row, yy) in x.iter().zip(y.iter()) {
        for i in 0..p {
            xty[i] += row[i] * yy;
            for j in 0..p {
                xtx[i][j] += row[i] * row[j];
            }
        }
    }
    let scale = (0..p).map(|i| xtx[i][i]).fold(0.0, f64::max).max(1e-12);
    for (i, row) in xtx.iter_mut().enumerate() {
        row[i] += 1e-9 * scale;
    }
    let inv = invert(&xtx)?;
    let beta: Vec<f64> = (0..p).map(|i| (0..p).map(|j| inv[i][j] * xty[j]).sum()).collect();
    let mut sse = 0.0;
    let mean = y.iter().sum::<f64>() / n as f64;
    let mut sst = 0.0;
    for (row, yy) in x.iter().zip(y.iter()) {
        let pred: f64 = row.iter().zip(beta.iter()).map(|(a, b)| a * b).sum();
        sse += (yy - pred).powi(2);
        sst += (yy - mean).powi(2);
    }
    let sigma2 = sse / (n - p) as f64;
    let se: Vec<f64> = (0..p).map(|i| (sigma2 * inv[i][i]).max(0.0).sqrt()).collect();
    let r2 = if sst > 0.0 { 1.0 - sse / sst } else { 0.0 };
    Some((beta, se, r2, (sse / n as f64).sqrt()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MoldPrediction {
    pub radial_wear_um: f64,
    pub axial_offset_um: f64,
    pub deflection_um: f64,
    pub workpiece_rise_c: f64,
    pub expansion_per_k: f64,
}

impl MoldPrediction {
    pub fn from_sim(r: &LoadSimReport, expansion: f64) -> Self {
        let defl = if r.wall_errors.is_empty() {
            r.max_wall_defl_um
        } else {
            r.wall_errors.iter().map(|w| (w.deflection_um + w.workpiece_um).abs()).sum::<f64>() / r.wall_errors.len() as f64
        };
        let wear = if r.wall_errors.is_empty() {
            r.radial_loss_end_um
        } else {
            r.wall_errors.iter().map(|w| w.wear_um).sum::<f64>() / r.wall_errors.len() as f64
        };
        Self {
            radial_wear_um: wear,
            axial_offset_um: r.floor_error_mean_um,
            deflection_um: defl,
            workpiece_rise_c: r.wp_temp_end_c - 22.0,
            expansion_per_k: expansion,
        }
    }
}

pub fn deviation_fit(measured: &HeightmapView, nominal: &HeightmapView, pred: &MoldPrediction) -> Result<DeviationFit, String> {
    let mut xs: Vec<Vec<f64>> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    let mut cells: Vec<usize> = Vec::new();
    let datum = (nominal.x0, nominal.y0, 0.0f64);
    let xc = nominal.x0 + nominal.nx as f64 * nominal.cell_mm / 2.0;
    let yc = nominal.y0 + nominal.ny as f64 * nominal.cell_mm / 2.0;
    let mut slopes = 0usize;
    for j in 0..nominal.ny {
        for i in 0..nominal.nx {
            let k = j * nominal.nx + i;
            let zn = nominal.z[k];
            if !zn.is_finite() {
                continue;
            }
            let x = nominal.x0 + (i as f64 + 0.5) * nominal.cell_mm;
            let y = nominal.y0 + (j as f64 + 0.5) * nominal.cell_mm;
            let zm = match sample(measured, x, y) {
                Some(v) => v,
                None => continue,
            };
            let (gx, gy) = match grad(nominal, i, j) {
                Some(g) => g,
                None => continue,
            };
            let theta = (gx * gx + gy * gy).sqrt().atan();
            if theta.to_degrees() > 80.0 {
                continue;
            }
            if theta.to_degrees() > 5.0 {
                slopes += 1;
            }
            let dev_um = (zm - zn as f64) * 1000.0;
            let b_uniform = 1.0;
            let b_normal = 1.0 / theta.cos() - 1.0;
            let b_defl = theta.tan();
            let b_therm = -(((zn as f64) - datum.2) - (gx * (x - datum.0) + gy * (y - datum.1))) * pred.expansion_per_k * 1000.0;
            xs.push(vec![b_uniform, b_normal, b_defl, b_therm, x - xc, y - yc]);
            ys.push(dev_um);
            cells.push(k);
        }
    }
    if ys.len() < 20 {
        return Err("측정·공칭 높이맵의 겹치는 유효 셀이 20개 미만입니다. 원점/배율을 확인하세요".into());
    }
    let names = [
        ("axial_offset_um", "Z 오프셋 (길이마모·열·드리프트)"),
        ("radial_wear_um", "반경 마모 (법선 오프셋)"),
        ("deflection_um", "휨 (경사면 측방)"),
        ("workpiece_dT_c", "소재 열수축 ΔT"),
        ("tilt_x_um_per_mm", "셋업 기울기 X (µm/mm)"),
        ("tilt_y_um_per_mm", "셋업 기울기 Y (µm/mm)"),
    ];
    let mut active: Vec<usize> = Vec::new();
    let mut notes = Vec::new();
    for c in 0..names.len() {
        let norm: f64 = xs.iter().map(|r| r[c] * r[c]).sum::<f64>().sqrt();
        let base: f64 = (ys.len() as f64).sqrt();
        if c == 0 || norm > 0.05 * base {
            active.push(c);
        } else {
            notes.push(format!("{} 기저는 경사면/범위가 부족해 식별할 수 없습니다", names[c].1));
        }
    }
    let xa: Vec<Vec<f64>> = xs.iter().map(|r| active.iter().map(|c| r[*c]).collect()).collect();
    let (beta, se, r2, rms_res) = least_squares(&xa, &ys).ok_or_else(|| "편차 분해 연립방정식을 풀 수 없습니다".to_string())?;
    let predicted = [
        Some(pred.axial_offset_um),
        Some(pred.radial_wear_um),
        Some(pred.deflection_um),
        Some(pred.workpiece_rise_c),
        None,
        None,
    ];
    let mut coefficients = Vec::new();
    for c in 0..names.len() {
        let pos = active.iter().position(|a| *a == c);
        let value = pos.map(|p| beta[p]).unwrap_or(0.0);
        let stderr = pos.map(|p| se[p]).unwrap_or(f64::NAN);
        let pr = predicted[c];
        coefficients.push(Coef {
            name: names[c].0.into(),
            label: names[c].1.into(),
            value,
            stderr,
            identifiable: pos.is_some(),
            predicted: pr,
            ratio: match pr {
                Some(p) if p.abs() > 1e-6 && pos.is_some() => Some(value / p),
                _ => None,
            },
        });
    }
    let mut res_map = nominal.clone();
    for v in res_map.z.iter_mut() {
        *v = f32::NAN;
    }
    for (idx, k) in cells.iter().enumerate() {
        let pred_v: f64 = xa[idx].iter().zip(beta.iter()).map(|(a, b)| a * b).sum();
        res_map.z[*k] = (ys[idx] - pred_v) as f32;
    }
    if slopes < ys.len() / 20 {
        notes.push("경사면이 거의 없어(2.5D 바닥 위주) Z 오프셋만 신뢰할 수 있습니다. 벽 치수는 CMM 벽면 측정으로 분해하세요".into());
    }
    let rms_dev = (ys.iter().map(|v| v * v).sum::<f64>() / ys.len() as f64).sqrt();
    Ok(DeviationFit {
        points: ys.len(),
        span_mm: (nominal.nx.max(nominal.ny)) as f64 * nominal.cell_mm,
        coefficients,
        r2,
        rms_dev_um: rms_dev,
        rms_residual_um: rms_res,
        max_abs_dev_um: ys.iter().map(|v| v.abs()).fold(0.0, f64::max),
        residual_map: res_map,
        notes,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub deviation_um: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallFit {
    pub points: usize,
    pub matched: usize,
    pub coefficients: Vec<Coef>,
    pub r2: f64,
    pub rms_residual_um: f64,
    pub notes: Vec<String>,
}

pub fn parse_wall_points(text: &str) -> Vec<WallPoint> {
    text.lines()
        .filter_map(|l| {
            let v: Vec<f64> = l
                .split(|c| c == ',' || c == ';' || c == '\t')
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .collect();
            if v.len() >= 4 {
                Some(WallPoint {
                    x: v[0],
                    y: v[1],
                    z: v[2],
                    deviation_um: v[v.len() - 1],
                })
            } else {
                None
            }
        })
        .collect()
}

pub fn wall_fit(points: &[WallPoint], predicted: &[WallErrorSample], tol_mm: f64) -> Result<WallFit, String> {
    if predicted.is_empty() {
        return Err("시뮬레이션 벽면 예측값이 없습니다 (벽을 만드는 측면 가공이 없는 패턴)".into());
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for p in points.iter() {
        let best = predicted
            .iter()
            .map(|w| ((w.x - p.x).powi(2) + (w.y - p.y).powi(2) + 0.25 * (w.z_mid - p.z).powi(2), w))
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((d2, w)) = best {
            if d2.sqrt() <= tol_mm {
                xs.push(vec![1.0, w.deflection_um + w.workpiece_um, w.wear_um, w.thermal_tool_um + w.thermal_wp_um]);
                ys.push(p.deviation_um);
            }
        }
    }
    let mut notes = Vec::new();
    if ys.len() < 6 {
        return Err(format!("예측 벽면과 {:.1} mm 이내로 매칭된 측정점이 6개 미만입니다", tol_mm));
    }
    let labels = [
        ("setup_offset_um", "셋업/측정 오프셋"),
        ("deflection_gain", "휨 이득 (측정/예측)"),
        ("wear_gain", "마모 이득 (측정/예측)"),
        ("thermal_gain", "열 이득 (측정/예측)"),
    ];
    let mut active = vec![0usize];
    for c in 1..4 {
        let var = {
            let m = xs.iter().map(|r| r[c]).sum::<f64>() / xs.len() as f64;
            xs.iter().map(|r| (r[c] - m).powi(2)).sum::<f64>()
        };
        if var > 1e-6 {
            active.push(c);
        } else {
            notes.push(format!("{} 는 측정 구간에서 변화가 없어 분리할 수 없습니다", labels[c].1));
        }
    }
    let xa: Vec<Vec<f64>> = xs.iter().map(|r| active.iter().map(|c| r[*c]).collect()).collect();
    let (beta, se, r2, rms) = least_squares(&xa, &ys).ok_or_else(|| "벽면 분해 연립방정식을 풀 수 없습니다".to_string())?;
    let coefficients = (0..4)
        .map(|c| {
            let pos = active.iter().position(|a| *a == c);
            Coef {
                name: labels[c].0.into(),
                label: labels[c].1.into(),
                value: pos.map(|p| beta[p]).unwrap_or(if c == 0 { 0.0 } else { 1.0 }),
                stderr: pos.map(|p| se[p]).unwrap_or(f64::NAN),
                identifiable: pos.is_some(),
                predicted: if c == 0 { None } else { Some(1.0) },
                ratio: pos.map(|p| beta[p]).filter(|_| c > 0),
            }
        })
        .collect();
    Ok(WallFit {
        points: points.len(),
        matched: ys.len(),
        coefficients,
        r2,
        rms_residual_um: rms,
        notes,
    })
}