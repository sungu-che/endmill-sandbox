use image::{DynamicImage, ImageDecoder, ImageReader, Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolView {
    Side,
    End,
}

impl ToolView {
    pub fn from_key(k: &str) -> Self {
        match k.trim().to_lowercase().as_str() {
            "end" | "bottom" | "저면" | "날끝" | "끝면" => ToolView::End,
            _ => ToolView::Side,
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            ToolView::Side => "side",
            ToolView::End => "end",
        }
    }
}

pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Gray {
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    pub fn sample(&self, x: f64, y: f64) -> f32 {
        if self.w == 0 || self.h == 0 {
            return 0.0;
        }
        let xf = x.clamp(0.0, (self.w - 1) as f64);
        let yf = y.clamp(0.0, (self.h - 1) as f64);
        let x0 = xf.floor() as usize;
        let y0 = yf.floor() as usize;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let fx = (xf - x0 as f64) as f32;
        let fy = (yf - y0 as f64) as f32;
        let a = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let b = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        a * (1.0 - fy) + b * fy
    }
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("이미지 형식 판별 실패: {}", e))?;
    let mut decoder = reader.into_decoder().map_err(|e| format!("이미지 디코더 생성 실패: {}", e))?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| format!("이미지 디코딩 실패: {}", e))?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img)
}

pub fn limit_size(img: DynamicImage, max_side: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max_side {
        return img;
    }
    let s = max_side as f64 / w.max(h) as f64;
    img.resize_exact(
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
        image::imageops::FilterType::Triangle,
    )
}

pub fn to_gray(img: &RgbImage) -> Gray {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut v = vec![0f32; w * h];
    for (x, y, p) in img.enumerate_pixels() {
        v[y as usize * w + x as usize] = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    }
    Gray { w, h, v }
}

pub fn otsu(g: &Gray) -> f32 {
    let mut hist = [0f64; 256];
    for v in g.v.iter() {
        hist[(*v as usize).min(255)] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let sum_all: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * c).sum();
    let mut w_b = 0.0;
    let mut sum_b = 0.0;
    let mut best = 0.0;
    let mut thr = 127.0;
    for (i, c) in hist.iter().enumerate() {
        w_b += c;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += i as f64 * c;
        let m_b = sum_b / w_b;
        let m_f = (sum_all - sum_b) / w_f;
        let between = w_b * w_f * (m_b - m_f).powi(2);
        if between > best {
            best = between;
            thr = i as f64 + 0.5;
        }
    }
    thr as f32
}

#[derive(Clone)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub m: Vec<bool>,
}

impl Mask {
    pub fn get(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.m[y as usize * self.w + x as usize]
    }

    pub fn area(&self) -> usize {
        self.m.iter().filter(|b| **b).count()
    }

    pub fn border_ratio(&self) -> f64 {
        let mut touch = 0usize;
        let mut total = 0usize;
        for x in 0..self.w {
            for y in [0, self.h - 1] {
                total += 1;
                if self.m[y * self.w + x] {
                    touch += 1;
                }
            }
        }
        for y in 0..self.h {
            for x in [0, self.w - 1] {
                total += 1;
                if self.m[y * self.w + x] {
                    touch += 1;
                }
            }
        }
        touch as f64 / total.max(1) as f64
    }
}

fn largest_component(mask: &Mask) -> Mask {
    let mut label = vec![0u32; mask.w * mask.h];
    let mut best_id = 0u32;
    let mut best_n = 0usize;
    let mut next = 1u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..mask.m.len() {
        if !mask.m[start] || label[start] != 0 {
            continue;
        }
        let id = next;
        next += 1;
        let mut n = 0usize;
        stack.push(start);
        label[start] = id;
        while let Some(k) = stack.pop() {
            n += 1;
            let x = k % mask.w;
            let y = k / mask.w;
            let mut push = |nx: usize, ny: usize| {
                let kk = ny * mask.w + nx;
                if mask.m[kk] && label[kk] == 0 {
                    label[kk] = id;
                    stack.push(kk);
                }
            };
            if x > 0 {
                push(x - 1, y);
            }
            if x + 1 < mask.w {
                push(x + 1, y);
            }
            if y > 0 {
                push(x, y - 1);
            }
            if y + 1 < mask.h {
                push(x, y + 1);
            }
        }
        if n > best_n {
            best_n = n;
            best_id = id;
        }
    }
    Mask {
        w: mask.w,
        h: mask.h,
        m: label.iter().map(|l| *l == best_id && best_id != 0).collect(),
    }
}

fn fill_holes(mask: &Mask) -> Mask {
    let (w, h) = (mask.w, mask.h);
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let k = y * w + x;
            if !mask.m[k] && !outside[k] {
                outside[k] = true;
                stack.push(k);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let k = y * w + x;
            if !mask.m[k] && !outside[k] {
                outside[k] = true;
                stack.push(k);
            }
        }
    }
    while let Some(k) = stack.pop() {
        let x = k % w;
        let y = k / w;
        let mut visit = |kk: usize| {
            if !mask.m[kk] && !outside[kk] {
                outside[kk] = true;
                stack.push(kk);
            }
        };
        if x > 0 {
            visit(k - 1);
        }
        if x + 1 < w {
            visit(k + 1);
        }
        if y > 0 {
            visit(k - w);
        }
        if y + 1 < h {
            visit(k + w);
        }
    }
    Mask {
        w,
        h,
        m: outside.iter().map(|o| !o).collect(),
    }
}

pub struct Silhouette {
    pub mask: Mask,
    pub threshold: f32,
    pub dark_object: bool,
}

pub fn silhouette(g: &Gray) -> Result<Silhouette, String> {
    let thr = otsu(g);
    let mut best: Option<(f64, Silhouette)> = None;
    for dark in [true, false] {
        let raw = Mask {
            w: g.w,
            h: g.h,
            m: g.v.iter().map(|v| if dark { *v < thr } else { *v >= thr }).collect(),
        };
        let comp = fill_holes(&largest_component(&raw));
        let area = comp.area() as f64 / (g.w * g.h).max(1) as f64;
        if !(0.01..=0.9).contains(&area) {
            continue;
        }
        let score = comp.border_ratio() + (area - 0.25).abs() * 0.1;
        if best.as_ref().map(|b| score < b.0).unwrap_or(true) {
            best = Some((
                score,
                Silhouette {
                    mask: comp,
                    threshold: thr,
                    dark_object: dark,
                },
            ));
        }
    }
    best.map(|b| b.1)
        .ok_or_else(|| "공구 실루엣을 분리하지 못했습니다. 단색 배경에서 촬영해 주세요.".into())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Axis {
    pub cx: f64,
    pub cy: f64,
    pub angle: f64,
    pub major: f64,
    pub minor: f64,
}

pub fn principal_axis(mask: &Mask) -> Axis {
    let mut n = 0.0f64;
    let mut sx = 0.0f64;
    let mut sy = 0.0f64;
    for y in 0..mask.h {
        for x in 0..mask.w {
            if mask.m[y * mask.w + x] {
                n += 1.0;
                sx += x as f64;
                sy += y as f64;
            }
        }
    }
    let cx = sx / n.max(1.0);
    let cy = sy / n.max(1.0);
    let mut sxx = 0.0f64;
    let mut syy = 0.0f64;
    let mut sxy = 0.0f64;
    for y in 0..mask.h {
        for x in 0..mask.w {
            if mask.m[y * mask.w + x] {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                sxx += dx * dx;
                syy += dy * dy;
                sxy += dx * dy;
            }
        }
    }
    sxx /= n.max(1.0);
    syy /= n.max(1.0);
    sxy /= n.max(1.0);
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let tr = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    Axis {
        cx,
        cy,
        angle,
        major: 4.0 * (tr / 2.0 + disc).sqrt(),
        minor: 4.0 * (tr / 2.0 - disc).max(0.0).sqrt(),
    }
}

fn rotate_rgb(img: &RgbImage, cx: f64, cy: f64, theta: f64, out_w: usize, out_h: usize, ocx: f64, ocy: f64) -> (RgbImage, Vec<bool>) {
    let (c, s) = (theta.cos(), theta.sin());
    let mut out = RgbImage::new(out_w as u32, out_h as u32);
    let mut valid = vec![false; out_w * out_h];
    let (w, h) = (img.width() as f64, img.height() as f64);
    for oy in 0..out_h {
        for ox in 0..out_w {
            let dx = ox as f64 - ocx;
            let dy = oy as f64 - ocy;
            let sxp = cx + c * dx - s * dy;
            let syp = cy + s * dx + c * dy;
            if sxp < 0.0 || syp < 0.0 || sxp > w - 1.0 || syp > h - 1.0 {
                continue;
            }
            valid[oy * out_w + ox] = true;
            let x0 = sxp.floor() as u32;
            let y0 = syp.floor() as u32;
            let x1 = (x0 + 1).min(img.width() - 1);
            let y1 = (y0 + 1).min(img.height() - 1);
            let fx = sxp - x0 as f64;
            let fy = syp - y0 as f64;
            let p00 = img.get_pixel(x0, y0);
            let p10 = img.get_pixel(x1, y0);
            let p01 = img.get_pixel(x0, y1);
            let p11 = img.get_pixel(x1, y1);
            let mut px = [0u8; 3];
            for k in 0..3 {
                let v = p00[k] as f64 * (1.0 - fx) * (1.0 - fy)
                    + p10[k] as f64 * fx * (1.0 - fy)
                    + p01[k] as f64 * (1.0 - fx) * fy
                    + p11[k] as f64 * fx * fy;
                px[k] = v.round().clamp(0.0, 255.0) as u8;
            }
            out.put_pixel(ox as u32, oy as u32, Rgb(px));
        }
    }
    (out, valid)
}

fn background_fill(img: &mut RgbImage, valid: &[bool], mask: &Mask) {
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let step = ((img.width() as usize * img.height() as usize) / 20000).max(1);
    for (k, p) in img.pixels().enumerate() {
        if k % step != 0 || !valid[k] || mask.m[k] {
            continue;
        }
        for c in 0..3 {
            ch[c].push(p[c]);
        }
    }
    if ch[0].is_empty() {
        return;
    }
    let mut bg = [0u8; 3];
    for c in 0..3 {
        ch[c].sort_unstable();
        bg[c] = ch[c][ch[c].len() / 2];
    }
    for (k, p) in img.pixels_mut().enumerate() {
        if !valid[k] {
            *p = Rgb(bg);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideProfile {
    pub px_per_mm: f64,
    pub calibration: String,
    pub tip_row: f64,
    pub axis_x: f64,
    pub z_mm: Vec<f64>,
    pub width_mm: Vec<f64>,
    pub left_mm: Vec<f64>,
    pub right_mm: Vec<f64>,
    pub corner_r_left_mm: f64,
    pub corner_r_right_mm: f64,
    pub edge_roughness_um: f64,
    pub visible_length_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndProfile {
    pub px_per_mm: f64,
    pub calibration: String,
    pub radius_mm: Vec<f64>,
    pub center: (f64, f64),
}

pub struct Prepared {
    pub view: ToolView,
    pub aligned: RgbImage,
    pub crop_px: f64,
    pub side: Option<SideProfile>,
    pub end: Option<EndProfile>,
    pub axis: Axis,
    pub notes: Vec<String>,
    pub zone_of_patch: Vec<String>,
}

pub struct ToolDims {
    pub diameter_mm: f64,
    pub shank_mm: f64,
    pub loc_mm: f64,
    pub corner_r_mm: f64,
}

fn subpixel_edge(g: &Gray, y: usize, x_in: usize, step_out: i64, thr: f32) -> f64 {
    let xo = x_in as i64 + step_out;
    if xo < 0 || xo as usize >= g.w {
        return x_in as f64;
    }
    let a = g.at(x_in, y);
    let b = g.at(xo as usize, y);
    if (a - b).abs() < 1e-3 {
        return x_in as f64 + 0.5 * step_out as f64;
    }
    let t = ((thr - a) / (b - a)).clamp(0.0, 1.0) as f64;
    x_in as f64 + t * step_out as f64
}

pub fn prepare(bytes: &[u8], view: ToolView, dims: &ToolDims, size: usize, px_per_mm_hint: Option<f64>, tip_hint: Option<&str>) -> Result<Prepared, String> {
    let img = limit_size(decode(bytes)?, 2000).to_rgb8();
    let g = to_gray(&img);
    let sil = silhouette(&g)?;
    let axis = principal_axis(&sil.mask);
    let mut notes = Vec::new();
    match view {
        ToolView::Side => prepare_side(&img, &g, &sil, axis, dims, size, px_per_mm_hint, tip_hint, &mut notes),
        ToolView::End => prepare_end(&img, &sil, axis, dims, size, px_per_mm_hint, &mut notes),
    }
    .map(|mut p| {
        p.notes.splice(0..0, notes);
        p
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_side(img: &RgbImage, _g: &Gray, sil: &Silhouette, axis: Axis, dims: &ToolDims, size: usize, px_hint: Option<f64>, tip_hint: Option<&str>, notes: &mut Vec<String>) -> Result<Prepared, String> {
    let theta = axis.angle - std::f64::consts::FRAC_PI_2;
    let (ca, sa) = (axis.angle.cos(), axis.angle.sin());
    let (ct, st) = (theta.cos(), theta.sin());
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    let mut t_min = f64::INFINITY;
    let mut t_max = f64::NEG_INFINITY;
    let mut bx = (f64::INFINITY, f64::NEG_INFINITY);
    let mut by = (f64::INFINITY, f64::NEG_INFINITY);
    for y in 0..ih {
        for x in 0..iw {
            if !sil.mask.m[y * iw + x] {
                continue;
            }
            let dx = x as f64 - axis.cx;
            let dy = y as f64 - axis.cy;
            let t = dx * ca + dy * sa;
            t_min = t_min.min(t);
            t_max = t_max.max(t);
            let rx = ct * dx + st * dy;
            let ry = -st * dx + ct * dy;
            bx = (bx.0.min(rx), bx.1.max(rx));
            by = (by.0.min(ry), by.1.max(ry));
        }
    }
    let len = (t_max - t_min).max(1.0);
    let mut touch_min = false;
    let mut touch_max = false;
    for y in 0..ih {
        for x in 0..iw {
            if !sil.mask.m[y * iw + x] || !(x == 0 || y == 0 || x + 1 == iw || y + 1 == ih) {
                continue;
            }
            let t = (x as f64 - axis.cx) * ca + (y as f64 - axis.cy) * sa;
            if t < t_min + 0.05 * len {
                touch_min = true;
            }
            if t > t_max - 0.05 * len {
                touch_max = true;
            }
        }
    }
    let margin = (axis.minor * 1.2 + 24.0).ceil();
    let out_w = ((bx.1 - bx.0) + 2.0 * margin).ceil() as usize;
    let out_h = ((by.1 - by.0) + 2.0 * margin).ceil() as usize;
    let ocx = -bx.0 + margin;
    let ocy = -by.0 + margin;
    let (mut rot, valid) = rotate_rgb(img, axis.cx, axis.cy, theta, out_w, out_h, ocx, ocy);
    let rg = to_gray(&rot);
    let mut rmask = Mask {
        w: out_w,
        h: out_h,
        m: rg
            .v
            .iter()
            .zip(valid.iter())
            .map(|(v, ok)| *ok && if sil.dark_object { *v < sil.threshold } else { *v >= sil.threshold })
            .collect(),
    };
    rmask = fill_holes(&largest_component(&rmask));
    background_fill(&mut rot, &valid, &rmask);
    let rows: Vec<usize> = (0..out_h).filter(|y| (0..out_w).any(|x| rmask.m[y * out_w + x])).collect();
    if rows.len() < 10 {
        return Err("회전 정렬 후 공구 영역이 너무 작습니다".into());
    }
    let (top, bottom) = (*rows.first().unwrap(), *rows.last().unwrap());
    let third = (bottom - top) / 3;
    let energy = |a: usize, b: usize| -> f64 {
        let mut e = 0.0f64;
        let mut n = 0.0f64;
        for y in a..b {
            for x in 1..out_w - 1 {
                if rmask.m[y * out_w + x] {
                    e += (rg.at(x + 1, y) - rg.at(x - 1, y)).abs() as f64;
                    n += 1.0;
                }
            }
        }
        e / n.max(1.0)
    };
    let tip_at_bottom = match tip_hint.map(|s| s.to_lowercase()) {
        Some(ref s) if s == "top" => false,
        Some(ref s) if s == "bottom" => true,
        _ => {
            if touch_min != touch_max {
                touch_min
            } else {
                energy(bottom - third, bottom) >= energy(top, top + third)
            }
        }
    };
    let (rot, rg, rmask) = if tip_at_bottom {
        (rot, rg, rmask)
    } else {
        let flipped = image::imageops::rotate180(&rot);
        let fg = to_gray(&flipped);
        let fm = Mask {
            w: out_w,
            h: out_h,
            m: (0..out_w * out_h).map(|k| rmask.m[out_w * out_h - 1 - k]).collect(),
        };
        (flipped, fg, fm)
    };
    let rows: Vec<usize> = (0..out_h).filter(|y| (0..out_w).any(|x| rmask.m[y * out_w + x])).collect();
    let (top, bottom) = (*rows.first().unwrap(), *rows.last().unwrap());
    let mut lefts = Vec::with_capacity(bottom - top + 1);
    let mut rights = Vec::with_capacity(bottom - top + 1);
    let mut ys = Vec::with_capacity(bottom - top + 1);
    for y in (top..=bottom).rev() {
        let xs: Vec<usize> = (0..out_w).filter(|x| rmask.m[y * out_w + x]).collect();
        if xs.is_empty() {
            continue;
        }
        lefts.push(subpixel_edge(&rg, y, xs[0], -1, sil.threshold));
        rights.push(subpixel_edge(&rg, y, *xs.last().unwrap(), 1, sil.threshold));
        ys.push(y as f64);
    }
    let widths: Vec<f64> = lefts.iter().zip(rights.iter()).map(|(l, r)| r - l).collect();
    let tip_row = bottom as f64 + 0.5;
    let n = widths.len();
    let flute_rows = ((n as f64) * 0.4).ceil() as usize;
    let mut env: Vec<f64> = widths[..flute_rows.max(1).min(n)].to_vec();
    env.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let s0 = env[..(env.len() / 20).max(1)].iter().sum::<f64>() / (env.len() / 20).max(1) as f64 / dims.diameter_mm.max(0.1);
    let (px_per_mm, calibration) = match px_hint {
        Some(p) if p > 0.0 => (p, "사용자 지정".to_string()),
        _ => {
            let z_lo = dims.loc_mm + 0.3 * dims.diameter_mm;
            let z_hi = z_lo + 1.5 * dims.shank_mm;
            let shank: Vec<f64> = ys
                .iter()
                .zip(widths.iter())
                .filter(|(y, _)| {
                    let z = (tip_row - **y) / s0.max(1e-6);
                    z >= z_lo && z <= z_hi
                })
                .map(|(_, w)| *w)
                .collect();
            if shank.len() >= 10 {
                (median(&shank) / dims.shank_mm.max(0.1), "샹크 직경 기준".to_string())
            } else {
                notes.push("샹크가 충분히 보이지 않아 날부 최대 폭을 공구 직경으로 보정했습니다 (정밀도 저하).".into());
                (s0, "날부 최대 폭 기준".to_string())
            }
        }
    };
    let axis_x = median(&lefts.iter().zip(rights.iter()).map(|(l, r)| (l + r) / 2.0).collect::<Vec<_>>());
    let z_mm: Vec<f64> = ys.iter().map(|y| (tip_row - y) / px_per_mm).collect();
    let width_mm: Vec<f64> = widths.iter().map(|w| w / px_per_mm).collect();
    let left_mm: Vec<f64> = lefts.iter().map(|l| (axis_x - l) / px_per_mm).collect();
    let right_mm: Vec<f64> = rights.iter().map(|r| (r - axis_x) / px_per_mm).collect();
    let r_nom = dims.diameter_mm / 2.0;
    let corner_window = (0.25 * dims.diameter_mm).max(dims.corner_r_mm * 1.5);
    let corner = |side: &Vec<f64>| -> f64 {
        let above: Vec<f64> = z_mm
            .iter()
            .enumerate()
            .filter(|(_, z)| **z > corner_window && **z <= 2.0 * corner_window)
            .map(|(i, _)| side[i])
            .collect();
        let r_ref = if above.len() >= 5 { median(&above).min(r_nom) } else { r_nom };
        let mut deficit = 0.0;
        let dz = 1.0 / px_per_mm;
        for (i, z) in z_mm.iter().enumerate() {
            if *z > corner_window {
                continue;
            }
            let ext = side[i].min(r_ref);
            deficit += (r_ref - ext).clamp(0.0, corner_window) * dz;
        }
        (deficit / (1.0 - std::f64::consts::PI / 4.0)).max(0.0).sqrt()
    };
    let rough = {
        let loc_rows: Vec<usize> = (0..n).filter(|i| z_mm[*i] > 0.3 * dims.diameter_mm && z_mm[*i] < dims.loc_mm).collect();
        let seg: Vec<f64> = loc_rows.iter().map(|i| left_mm[*i] + right_mm[*i]).collect();
        if seg.len() > 5 {
            let mut res = Vec::with_capacity(seg.len());
            for i in 2..seg.len() - 2 {
                let mut win = seg[i - 2..=i + 2].to_vec();
                win.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                res.push((seg[i] - win[2]).abs());
            }
            median(&res) * 1000.0
        } else {
            0.0
        }
    };
    let side = SideProfile {
        px_per_mm,
        calibration,
        tip_row,
        axis_x,
        visible_length_mm: (tip_row - top as f64) / px_per_mm,
        corner_r_left_mm: corner(&left_mm),
        corner_r_right_mm: corner(&right_mm),
        edge_roughness_um: rough,
        z_mm,
        width_mm,
        left_mm,
        right_mm,
    };
    let d_px = dims.diameter_mm * px_per_mm;
    let focus = dims.loc_mm.min(3.0 * dims.diameter_mm) * px_per_mm;
    let crop = (d_px / 0.3).max(focus / 0.8).max(32.0);
    let ox = axis_x - crop / 2.0;
    let oy = tip_row - 0.88 * crop;
    let (mut aligned, avalid) = crop_resize(&rot, ox, oy, crop, size);
    let amask = mask_crop(&rmask, ox, oy, crop, size);
    background_fill(&mut aligned, &avalid, &amask);
    let grid = size / 16;
    let zone_of_patch = (0..grid * grid)
        .map(|k| {
            let (pi, pj) = (k % grid, k / grid);
            let x = ox + (pi as f64 + 0.5) * crop / grid as f64;
            let y = oy + (pj as f64 + 0.5) * crop / grid as f64;
            let u = (x - axis_x) / (d_px / 2.0);
            let v = (tip_row - y) / d_px;
            side_zone(u, v, dims.loc_mm / dims.diameter_mm.max(0.1)).to_string()
        })
        .collect();
    Ok(Prepared {
        view: ToolView::Side,
        aligned,
        crop_px: crop,
        side: Some(side),
        end: None,
        axis,
        notes: Vec::new(),
        zone_of_patch,
    })
}

pub fn side_zone(u: f64, v: f64, loc_d: f64) -> &'static str {
    let au = u.abs();
    if au > 1.6 || v < -0.25 || v > loc_d + 0.3 {
        return if au > 1.6 { "background" } else { "outside" };
    }
    if v <= 0.35 && au >= 0.45 && au <= 1.35 {
        return if u < 0.0 { "corner_left" } else { "corner_right" };
    }
    if v <= 0.2 && au < 0.45 {
        return "end_edge";
    }
    if au >= 0.6 && au <= 1.35 && v > 0.35 && v <= 1.2 {
        return if u < 0.0 { "flank_left" } else { "flank_right" };
    }
    if au >= 0.6 && au <= 1.35 && v > 1.2 && v <= loc_d {
        return "flank_upper";
    }
    if au < 0.6 && v > 0.2 && v <= loc_d {
        return "flute_body";
    }
    if v > loc_d && au <= 1.35 {
        return "shank";
    }
    "transition"
}

fn prepare_end(img: &RgbImage, sil: &Silhouette, axis: Axis, dims: &ToolDims, size: usize, px_hint: Option<f64>, notes: &mut Vec<String>) -> Result<Prepared, String> {
    let g = to_gray(img);
    let (cx, cy) = (axis.cx, axis.cy);
    let mut radius_px = Vec::with_capacity(360);
    let maxr = (img.width().max(img.height())) as f64;
    for k in 0..360 {
        let a = (k as f64).to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let mut r = 0.0;
        let mut last_in = 0.0;
        while r < maxr {
            let x = cx + dx * r;
            let y = cy + dy * r;
            if !sil.mask.get(x.round() as i64, y.round() as i64) {
                break;
            }
            last_in = r;
            r += 0.5;
        }
        let a_in = g.sample(cx + dx * last_in, cy + dy * last_in);
        let a_out = g.sample(cx + dx * (last_in + 1.0), cy + dy * (last_in + 1.0));
        let t = if (a_out - a_in).abs() > 1e-3 {
            ((sil.threshold - a_in) / (a_out - a_in)).clamp(0.0, 1.0) as f64
        } else {
            0.5
        };
        radius_px.push(last_in + t);
    }
    let rmax = radius_px.iter().cloned().fold(0.0, f64::max);
    let (px_per_mm, calibration) = match px_hint {
        Some(p) if p > 0.0 => (p, "사용자 지정".to_string()),
        _ => {
            let mut s = radius_px.clone();
            s.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let top = s[..10].iter().sum::<f64>() / 10.0;
            notes.push("저면 이미지는 외곽 최대 반경을 공구 반경으로 보정했습니다. 동일 촬영 거리일 때 기준 이미지의 배율을 사용하세요.".into());
            (top / (dims.diameter_mm / 2.0).max(0.05), "최대 반경 기준".to_string())
        }
    };
    let crop = (2.4 * rmax).max(32.0);
    let ox = cx - crop / 2.0;
    let oy = cy - crop / 2.0;
    let (mut aligned, avalid) = crop_resize(img, ox, oy, crop, size);
    let amask = mask_crop(&sil.mask, ox, oy, crop, size);
    background_fill(&mut aligned, &avalid, &amask);
    let grid = size / 16;
    let r_px = dims.diameter_mm / 2.0 * px_per_mm;
    let zone_of_patch = (0..grid * grid)
        .map(|k| {
            let (pi, pj) = (k % grid, k / grid);
            let x = ox + (pi as f64 + 0.5) * crop / grid as f64;
            let y = oy + (pj as f64 + 0.5) * crop / grid as f64;
            let rr = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / r_px.max(1e-6);
            if rr < 0.3 {
                "center"
            } else if rr < 0.8 {
                "end_mid"
            } else if rr <= 1.15 {
                "outer_ring"
            } else {
                "background"
            }
            .to_string()
        })
        .collect();
    Ok(Prepared {
        view: ToolView::End,
        aligned,
        crop_px: crop,
        side: None,
        end: Some(EndProfile {
            px_per_mm,
            calibration,
            radius_mm: radius_px.iter().map(|r| r / px_per_mm).collect(),
            center: (cx, cy),
        }),
        axis,
        notes: Vec::new(),
        zone_of_patch,
    })
}

fn crop_resize(img: &RgbImage, ox: f64, oy: f64, crop: f64, size: usize) -> (RgbImage, Vec<bool>) {
    let mut out = RgbImage::new(size as u32, size as u32);
    let mut valid = vec![false; size * size];
    let scale = crop / size as f64;
    let (w, h) = (img.width() as f64, img.height() as f64);
    let taps = scale.ceil().max(1.0) as i64;
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0f64; 3];
            let mut n = 0.0;
            for ty in 0..taps {
                for tx in 0..taps {
                    let sx = ox + (x as f64 + (tx as f64 + 0.5) / taps as f64) * scale;
                    let sy = oy + (y as f64 + (ty as f64 + 0.5) / taps as f64) * scale;
                    if sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
                        continue;
                    }
                    let p = img.get_pixel(sx as u32, sy as u32);
                    for k in 0..3 {
                        acc[k] += p[k] as f64;
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                out.put_pixel(x as u32, y as u32, Rgb([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]));
                valid[y * size + x] = true;
            }
        }
    }
    (out, valid)
}

fn mask_crop(mask: &Mask, ox: f64, oy: f64, crop: f64, size: usize) -> Mask {
    let scale = crop / size as f64;
    let mut m = vec![false; size * size];
    for y in 0..size {
        for x in 0..size {
            let sx = (ox + (x as f64 + 0.5) * scale).floor() as i64;
            let sy = (oy + (y as f64 + 0.5) * scale).floor() as i64;
            m[y * size + x] = mask.get(sx, sy);
        }
    }
    Mask { w: size, h: size, m }
}

pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 2]
}

pub fn center_square(bytes: &[u8], size: usize) -> Result<RgbImage, String> {
    let img = limit_size(decode(bytes)?, 2000).to_rgb8();
    let side = img.width().min(img.height()) as f64;
    let ox = (img.width() as f64 - side) / 2.0;
    let oy = (img.height() as f64 - side) / 2.0;
    Ok(crop_resize(&img, ox, oy, side, size).0)
}

pub fn png_base64(img: &RgbImage) -> String {
    use base64::Engine;
    let mut buf = Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .is_err()
    {
        return String::new();
    }
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}