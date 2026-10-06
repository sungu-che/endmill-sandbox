use crate::physics::{tool_compliance, CuttingCoeffs, Engagement, ForceResult, MillMode, ToolGeometry};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    fn scale(self, k: f64) -> C {
        C::new(self.re * k, self.im * k)
    }
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
    fn abs(self) -> f64 {
        (self.re * self.re + self.im * self.im).sqrt()
    }
    fn sqrt(self) -> C {
        let r = self.abs();
        let re = ((r + self.re) / 2.0).max(0.0).sqrt();
        let im = ((r - self.re) / 2.0).max(0.0).sqrt();
        C::new(re, if self.im < 0.0 { -im } else { im })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolModal {
    pub k_n_per_um: f64,
    pub fn_hz: f64,
    pub zeta: f64,
    pub m_eff_kg: f64,
    pub holder_share: f64,
}

impl ToolModal {
    fn frf(&self, omega: f64) -> C {
        let k = self.k_n_per_um * 1000.0;
        let wn = 2.0 * PI * self.fn_hz;
        let r = omega / wn.max(1e-9);
        C::new(1.0, 0.0).div(C::new(k * (1.0 - r * r), k * 2.0 * self.zeta * r))
    }
}

pub fn estimated_modal(tool: &ToolGeometry, ap: f64, cal: f64) -> ToolModal {
    let comp = tool_compliance(tool, ap, cal);
    let k_total = comp.stiffness_n_per_um.max(1e-6);
    let holder_share = (k_total * cal / tool.holder_stiffness_n_per_um.max(1e-6)).clamp(0.0, 1.0);
    let d = tool.diameter_mm;
    let l = tool.stickout_mm.max(1.0);
    let lf = tool.loc_mm.min(l);
    let ls = (l - lf).max(0.0);
    let area_f = 0.65 * PI * d * d / 4.0;
    let area_s = PI * tool.shank_mm * tool.shank_mm / 4.0;
    let m_tool = tool.tool_density * (area_f * lf + area_s * ls) * 1e-9;
    let beam = 1.0 - holder_share;
    let m_eff = (0.236 * m_tool * beam * beam + (m_tool + tool.holder_mass_kg) * holder_share * holder_share).max(1e-5);
    let fn_hz = (k_total * 1e6 / m_eff).sqrt() / (2.0 * PI);
    ToolModal {
        k_n_per_um: k_total,
        fn_hz,
        zeta: tool.damping_ratio,
        m_eff_kg: m_eff,
        holder_share,
    }
}

pub fn tool_modal(tool: &ToolGeometry, ap: f64, cal: f64) -> ToolModal {
    let model = estimated_modal(tool, ap, cal);
    let (Some(fn_m), Some(k_m)) = (tool.measured_fn_hz, tool.measured_k_n_per_um) else {
        return model;
    };
    let (sf, sk) = match tool.measured_stickout_mm {
        Some(lm) if (lm - tool.stickout_mm).abs() > 0.5 => {
            let mut at = tool.clone();
            at.stickout_mm = lm;
            let ref_modal = estimated_modal(&at, ap, cal);
            (
                model.fn_hz / ref_modal.fn_hz.max(1e-9),
                model.k_n_per_um / ref_modal.k_n_per_um.max(1e-9),
            )
        }
        _ => (1.0, 1.0),
    };
    let fn_hz = fn_m * sf;
    let k = k_m * sk;
    let wn = 2.0 * PI * fn_hz;
    ToolModal {
        k_n_per_um: k,
        fn_hz,
        zeta: tool.damping_ratio,
        m_eff_kg: k * 1e6 / (wn * wn).max(1e-9),
        holder_share: model.holder_share,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LobePoint {
    pub rpm: f64,
    pub ap_lim_mm: f64,
    pub lobe: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stability {
    pub fn_hz: f64,
    pub k_n_per_um: f64,
    pub zeta: f64,
    pub kt_n_mm2: f64,
    pub kr: f64,
    pub ap_lim_mm: f64,
    pub ap_crit_mm: f64,
    pub margin: f64,
    pub chatter_hz: f64,
    pub stable: bool,
    pub process_damping_gain: f64,
    pub pitch_gain: f64,
    pub suggested_rpm: Vec<f64>,
    pub lobes: Vec<LobePoint>,
    pub notes: Vec<String>,
    #[serde(default)]
    pub measured: bool,
    #[serde(default)]
    pub envelope: Vec<LobePoint>,
    #[serde(default)]
    pub stickout_scaled: bool,
}

fn directional(eng: &Engagement, kr: f64) -> (f64, f64, f64, f64) {
    let f = |p: f64| {
        let (s2, c2) = (2.0 * p).sin_cos();
        (
            0.5 * (c2 - 2.0 * kr * p + kr * s2),
            0.5 * (-s2 - 2.0 * p + kr * c2),
            0.5 * (-s2 + 2.0 * p + kr * c2),
            0.5 * (-c2 - 2.0 * kr * p - kr * s2),
        )
    };
    let (a, b) = (f(eng.phi_ex), f(eng.phi_st));
    (a.0 - b.0, a.1 - b.1, a.2 - b.2, a.3 - b.3)
}

struct Branches {
    omega: Vec<f64>,
    a: [Vec<Option<(f64, f64)>>; 2],
}

fn scan(modal: &ToolModal, dir: (f64, f64, f64, f64), kt: f64, n_teeth: f64) -> Branches {
    let (axx, axy, ayx, ayy) = dir;
    let det_a = axx * ayy - axy * ayx;
    let wn = 2.0 * PI * modal.fn_hz;
    let steps = 900;
    let mut br = Branches {
        omega: Vec::with_capacity(steps),
        a: [Vec::with_capacity(steps), Vec::with_capacity(steps)],
    };
    for i in 0..steps {
        let omega = wn * (0.55 + 2.45 * i as f64 / (steps - 1) as f64);
        br.omega.push(omega);
        let g = modal.frf(omega);
        let a0 = g.mul(g).scale(det_a);
        let a1 = g.scale(axx + ayy);
        let disc = a1.mul(a1).sub(a0.scale(4.0)).sqrt();
        for (b, root) in [a1.add(disc), a1.sub(disc)].into_iter().enumerate() {
            let mut v = None;
            if a0.abs() > 1e-30 {
                let lam = root.div(a0.scale(2.0)).scale(-1.0);
                if lam.re < 0.0 && lam.re.is_finite() {
                    let kappa = lam.im / lam.re;
                    let a_lim = -2.0 * PI * lam.re * (1.0 + kappa * kappa) / (n_teeth * kt);
                    if a_lim.is_finite() && a_lim > 0.0 {
                        v = Some((a_lim, PI - 2.0 * kappa.atan()));
                    }
                }
            }
            br.a[b].push(v);
        }
    }
    br
}

const LOBES_MAX: u32 = 80;

fn lobe_rpm(omega: f64, eps: f64, k: u32, n_teeth: f64) -> f64 {
    60.0 * omega / (n_teeth * (eps + 2.0 * PI * k as f64))
}

fn limit_at(br: &Branches, rpm: f64, n_teeth: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for b in 0..2 {
        for k in 0..LOBES_MAX {
            let mut prev: Option<(f64, f64, f64)> = None;
            for (i, v) in br.a[b].iter().enumerate() {
                match v {
                    Some((a, eps)) => {
                        let om = br.omega[i];
                        let n = lobe_rpm(om, *eps, k, n_teeth);
                        if let Some((pn, pa, po)) = prev {
                            if (pn - rpm) * (n - rpm) <= 0.0 && (n - pn).abs() > 1e-12 {
                                let t = (rpm - pn) / (n - pn);
                                let a_i = pa + t * (a - pa);
                                if best.map(|x| a_i < x.0).unwrap_or(true) {
                                    best = Some((a_i, po + t * (om - po)));
                                }
                            }
                        }
                        prev = Some((n, *a, om));
                    }
                    None => prev = None,
                }
            }
        }
    }
    best
}

fn envelope(br: &Branches, n_teeth: f64, lo: f64, hi: f64, cells: usize) -> Vec<(f64, f64)> {
    let cells = cells.max(2);
    let step = ((hi - lo) / (cells - 1) as f64).max(1e-9);
    let mut env = vec![f64::INFINITY; cells];
    for b in 0..2 {
        for k in 0..LOBES_MAX {
            let mut prev: Option<(f64, f64)> = None;
            let mut reach = false;
            for (i, v) in br.a[b].iter().enumerate() {
                let Some((a, eps)) = *v else {
                    prev = None;
                    continue;
                };
                let n = lobe_rpm(br.omega[i], eps, k, n_teeth);
                if n >= lo {
                    reach = true;
                }
                if let Some((pn, pa)) = prev {
                    let (n0, a0, n1, a1) = if pn <= n { (pn, pa, n, a) } else { (n, a, pn, pa) };
                    if n1 - n0 > 1e-12 && n1 >= lo && n0 <= hi {
                        let j0 = ((n0 - lo) / step).ceil().max(0.0) as usize;
                        let j1 = ((n1 - lo) / step).floor().min((cells - 1) as f64);
                        if j1 >= 0.0 {
                            for (j, e) in env.iter_mut().enumerate().take(j1 as usize + 1).skip(j0) {
                                let x = lo + j as f64 * step;
                                let t = (x - n0) / (n1 - n0);
                                *e = e.min(a0 + t * (a1 - a0));
                            }
                        }
                    }
                }
                prev = Some((n, a));
            }
            if !reach {
                break;
            }
        }
    }
    env.into_iter().enumerate().map(|(j, a)| (lo + j as f64 * step, a)).collect()
}

fn robust_lobes(env: &[(f64, f64)], rpm: f64, ap: f64, gain: f64, max_rpm: f64) -> Vec<f64> {
    if env.len() < 3 {
        return Vec::new();
    }
    let step = (env[1].0 - env[0].0).max(1e-9);
    let w = ((0.02 * rpm) / step).ceil() as usize;
    let robust: Vec<f64> = (0..env.len())
        .map(|j| {
            env[j.saturating_sub(w)..=(j + w).min(env.len() - 1)]
                .iter()
                .map(|p| p.1)
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let mut cands: Vec<(f64, f64)> = Vec::new();
    for j in 0..env.len() {
        let r = robust[j];
        let n = env[j].0;
        let left = if j > 0 { robust[j - 1] } else { f64::NEG_INFINITY };
        let right = if j + 1 < env.len() { robust[j + 1] } else { f64::NEG_INFINITY };
        let ok = r.is_finite()
            && r >= left
            && r >= right
            && r * gain >= 1.2 * ap
            && n >= 0.5 * rpm
            && (max_rpm <= 0.0 || n <= max_rpm);
        if !ok {
            continue;
        }
        match cands.last_mut() {
            Some(c) if n - c.0 < 0.03 * rpm => {
                if r > c.1 {
                    *c = (n, r);
                }
            }
            _ => cands.push((n, r)),
        }
    }
    cands.sort_by(|a, b| (a.0 - rpm).abs().partial_cmp(&(b.0 - rpm).abs()).unwrap_or(std::cmp::Ordering::Equal));
    cands.iter().take(3).map(|c| c.0.round()).collect()
}

pub fn robust_peak(env: &[LobePoint], lo: f64, hi: f64, band: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for p in env.iter().filter(|p| p.rpm >= lo && p.rpm <= hi) {
        let r = env
            .iter()
            .filter(|q| (q.rpm - p.rpm).abs() <= band)
            .map(|q| q.ap_lim_mm)
            .fold(f64::INFINITY, f64::min);
        if r.is_finite() && best.map(|b| r > b.1).unwrap_or(true) {
            best = Some((p.rpm, r));
        }
    }
    best
}

fn critical(br: &Branches) -> f64 {
    br.a.iter()
        .flat_map(|v| v.iter().filter_map(|x| x.map(|p| p.0)))
        .fold(f64::INFINITY, f64::min)
}

pub fn process_damping_gain(vc_m_min: f64, chatter_hz: f64, flank_wear_mm: f64, material_factor: f64) -> f64 {
    if vc_m_min <= 0.0 || chatter_hz <= 0.0 {
        return 1.0;
    }
    let wavelength = vc_m_min * 1000.0 / 60.0 / chatter_hz;
    let critical = (0.15 + 2.0 * flank_wear_mm) * material_factor;
    (1.0 + (critical / wavelength.max(1e-6)).powi(2)).min(5.0)
}

#[allow(clippy::too_many_arguments)]
pub fn stability_margin(
    tool: &ToolGeometry,
    co: &CuttingCoeffs,
    eng: &Engagement,
    h_mean_mm: f64,
    rpm: f64,
    ap: f64,
    cal_defl: f64,
    vc_m_min: f64,
    pd_material: f64,
) -> f64 {
    if ap <= 0.0 || rpm <= 0.0 || eng.span() <= 0.0 {
        return f64::INFINITY;
    }
    let modal = tool_modal(tool, ap, cal_defl);
    let n_teeth = tool.flutes.max(1) as f64;
    let kt = ((1.0 - co.mc) * co.kc11 * h_mean_mm.max(1e-4).powf(-co.mc)).max(1.0);
    let br = scan(&modal, directional(eng, co.kr), kt, n_teeth);
    let crit = critical(&br);
    if !crit.is_finite() {
        return f64::INFINITY;
    }
    let (a, om) = limit_at(&br, rpm, n_teeth).unwrap_or((crit, 2.0 * PI * modal.fn_hz));
    let gain = if tool.variable_pitch { 1.4 } else { 1.0 } * process_damping_gain(vc_m_min, om / (2.0 * PI), tool.flank_wear_mm, pd_material);
    a * gain / ap
}

#[allow(clippy::too_many_arguments)]
pub fn stability(
    tool: &ToolGeometry,
    co: &CuttingCoeffs,
    eng: &Engagement,
    h_mean_mm: f64,
    rpm: f64,
    ap: f64,
    cal_defl: f64,
    max_rpm: f64,
    vc_m_min: f64,
    pd_material: f64,
) -> Stability {
    let modal = tool_modal(tool, ap, cal_defl);
    let n_teeth = tool.flutes.max(1) as f64;
    let kt = ((1.0 - co.mc) * co.kc11 * h_mean_mm.max(1e-4).powf(-co.mc)).max(1.0);
    let measured = tool.measured_fn_hz.is_some() && tool.measured_k_n_per_um.is_some();
    let stickout_scaled = measured && tool.measured_stickout_mm.map(|l| (l - tool.stickout_mm).abs() > 0.5).unwrap_or(false);
    let mut out = Stability {
        fn_hz: modal.fn_hz,
        k_n_per_um: modal.k_n_per_um,
        zeta: modal.zeta,
        kt_n_mm2: kt,
        kr: co.kr,
        pitch_gain: if tool.variable_pitch { 1.4 } else { 1.0 },
        process_damping_gain: 1.0,
        margin: f64::INFINITY,
        stable: true,
        measured,
        stickout_scaled,
        ..Default::default()
    };
    if ap <= 0.0 || rpm <= 0.0 || eng.span() <= 0.0 {
        return out;
    }
    let br = scan(&modal, directional(eng, co.kr), kt, n_teeth);
    let crit = critical(&br);
    if !crit.is_finite() {
        out.notes.push("방향 계수가 작아 재생 채터 한계를 계산할 수 없습니다 (매우 작은 접촉각)".into());
        return out;
    }
    let gain_static = out.pitch_gain;
    let (a_raw, omega_c) = limit_at(&br, rpm, n_teeth).unwrap_or((crit, 2.0 * PI * modal.fn_hz));
    let chatter_hz = omega_c / (2.0 * PI);
    let pd = process_damping_gain(vc_m_min, chatter_hz, tool.flank_wear_mm, pd_material);
    out.process_damping_gain = pd;
    out.chatter_hz = chatter_hz;
    out.ap_crit_mm = crit * gain_static;
    out.ap_lim_mm = a_raw * gain_static * pd;
    out.margin = out.ap_lim_mm / ap;
    out.stable = out.margin >= 1.0;
    let lo_view = 0.3 * rpm;
    let hi_view = if max_rpm > 0.0 { (2.0 * rpm).min(max_rpm) } else { 2.0 * rpm };
    for b in 0..2 {
        for k in 0..LOBES_MAX {
            let mut in_view = 0usize;
            let mut below = true;
            for (i, v) in br.a[b].iter().enumerate() {
                if let Some((a, eps)) = v {
                    let n = lobe_rpm(br.omega[i], *eps, k, n_teeth);
                    if n >= 0.25 * rpm {
                        below = false;
                    }
                    if n >= lo_view && n <= hi_view {
                        if in_view % 30 == 0 {
                            out.lobes.push(LobePoint {
                                rpm: n,
                                ap_lim_mm: a * gain_static,
                                lobe: k,
                            });
                        }
                        in_view += 1;
                    }
                }
            }
            if below {
                break;
            }
        }
    }
    out.lobes.sort_by(|a, b| a.rpm.partial_cmp(&b.rpm).unwrap_or(std::cmp::Ordering::Equal));
    let env = envelope(&br, n_teeth, lo_view, hi_view, 240);
    out.envelope = env
        .iter()
        .filter(|p| p.1.is_finite())
        .map(|p| LobePoint {
            rpm: p.0,
            ap_lim_mm: p.1 * gain_static,
            lobe: 0,
        })
        .collect();
    out.suggested_rpm = robust_lobes(&env, rpm, ap, gain_static, max_rpm);
    if !out.stable {
        out.notes.push(format!(
            "재생 채터 한계 ap {:.2} mm < 현재 {:.2} mm (여유 {:.2}배, 고유진동수 {:.0} Hz, {})",
            out.ap_lim_mm,
            ap,
            out.margin,
            modal.fn_hz,
            if stickout_scaled {
                "실측 FRF · 돌출 길이 보정"
            } else if measured {
                "실측 FRF"
            } else {
                "공구·홀더 모델 추정 FRF"
            }
        ));
        if let Some(n) = out.suggested_rpm.first() {
            out.notes.push(format!(
                "안정 로브 S{:.0} 부근은 ±2% 회전수 오차에도 한계 ap 가 현재의 1.2배 이상입니다",
                n
            ));
        } else {
            out.notes.push(format!("가까운 안정 로브가 없어 ap 를 {:.2} mm 이하로 낮추는 것이 안전합니다", out.ap_lim_mm * 0.9));
        }
    }
    if pd > 1.2 {
        out.notes.push(format!("저속 공정감쇠로 한계가 {:.1}배 높아졌습니다 (파장 짧음)", pd));
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForcedVibration {
    pub tooth_hz: f64,
    pub f1_n: f64,
    pub amplitude_um: f64,
    pub magnification: f64,
    pub sle_um: f64,
    pub near_resonance: bool,
}

fn harmonic(series: &[f64], h: usize) -> C {
    let n = series.len().max(1);
    let mut acc = C::new(0.0, 0.0);
    for (s, v) in series.iter().enumerate() {
        let ang = -2.0 * PI * (h * s) as f64 / n as f64;
        acc = acc.add(C::new(ang.cos(), ang.sin()).scale(*v));
    }
    acc.scale(1.0 / n as f64)
}

pub fn forced_vibration(f: &ForceResult, modal: &ToolModal, rpm: f64, flutes: u32, eng: &Engagement) -> ForcedVibration {
    let mut out = ForcedVibration::default();
    if f.fy_series.len() < 8 || rpm <= 0.0 || modal.fn_hz <= 0.0 {
        return out;
    }
    let tooth_hz = rpm / 60.0 * flutes.max(1) as f64;
    out.tooth_hz = tooth_hz;
    let n = f.fy_series.len();
    let mut ys: Vec<C> = Vec::new();
    let mut f1 = 0.0;
    for h in 1..=3usize {
        let fx = harmonic(&f.fx_series, h);
        let fy = harmonic(&f.fy_series, h);
        if h == 1 {
            f1 = 2.0 * (fx.abs().powi(2) + fy.abs().powi(2)).sqrt();
        }
        let g = modal.frf(2.0 * PI * tooth_hz * h as f64).sub(modal.frf(0.0));
        ys.push(g.mul(fy));
        let ratio = tooth_hz * h as f64 / modal.fn_hz;
        if (ratio - 1.0).abs() < 0.15 {
            out.near_resonance = true;
        }
    }
    out.f1_n = f1;
    out.magnification = modal.frf(2.0 * PI * tooth_hz).abs() / modal.frf(0.0).abs().max(1e-30);
    let disp = |theta: f64| -> f64 {
        let mut y = 0.0;
        for (i, yh) in ys.iter().enumerate() {
            let h = (i + 1) as f64;
            let ang = 2.0 * PI * h * theta / f.pitch_rad.max(1e-9);
            y += 2.0 * (yh.re * ang.cos() - yh.im * ang.sin());
        }
        y * 1000.0
    };
    let mut amp: f64 = 0.0;
    for s in 0..n {
        amp = amp.max(disp(f.pitch_rad * s as f64 / n as f64).abs());
    }
    out.amplitude_um = amp;
    if let Some(wall) = eng.wall_angle() {
        let theta = wall.rem_euclid(f.pitch_rad.max(1e-9));
        let y = disp(theta);
        out.sle_um = if eng.mode == MillMode::Down { y } else { -y };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{cutting_coeffs, mechanistic_forces, CutContext};
    use crate::profile::{MachiningPreset, MachiningProfile};

    fn ctx() -> (CutContext, MachiningProfile) {
        let p = MachiningProfile::from_preset(&MachiningPreset::default_steel_general(), "t");
        (CutContext::from_profile(&p), p)
    }

    #[test]
    fn modal_frequency_is_realistic_and_drops_with_stickout() {
        let (mut c, _) = ctx();
        c.tool.stickout_mm = 35.0;
        let short = tool_modal(&c.tool, 5.0, 1.0);
        assert!(short.fn_hz > 800.0 && short.fn_hz < 12000.0, "fn {}", short.fn_hz);
        c.tool.stickout_mm = 60.0;
        let long = tool_modal(&c.tool, 5.0, 1.0);
        assert!(long.fn_hz < short.fn_hz);
        assert!(long.k_n_per_um < short.k_n_per_um);
    }

    #[test]
    fn longer_tools_chatter_earlier() {
        let (mut c, p) = ctx();
        let co = cutting_coeffs(&c);
        let d = c.tool.diameter_mm;
        let eng = Engagement::side(10.0, 0.5 * d, d, true);
        let rpm = p.conditions.spindle_rpm as f64;
        let s1 = stability(&c.tool, &co, &eng, 0.05, rpm, 10.0, 1.0, 12000.0, 180.0, 1.0);
        c.tool.stickout_mm += 25.0;
        let s2 = stability(&c.tool, &co, &eng, 0.05, rpm, 10.0, 1.0, 12000.0, 180.0, 1.0);
        assert!(s1.ap_crit_mm > s2.ap_crit_mm);
        assert!(s1.ap_lim_mm >= s1.ap_crit_mm * 0.99);
        c.tool.variable_pitch = !c.tool.variable_pitch;
        let s3 = stability(&c.tool, &co, &eng, 0.05, rpm, 10.0, 1.0, 12000.0, 180.0, 1.0);
        assert!((s3.ap_crit_mm - s2.ap_crit_mm).abs() > 1e-9);
        assert!(!s1.lobes.is_empty());
    }

    #[test]
    fn low_speed_process_damping_raises_limit() {
        assert!(process_damping_gain(20.0, 2500.0, 0.0, 1.3) > process_damping_gain(200.0, 2500.0, 0.0, 1.3));
        assert!(process_damping_gain(40.0, 2500.0, 0.3, 1.0) > process_damping_gain(40.0, 2500.0, 0.0, 1.0));
    }

    #[test]
    fn measured_frf_overrides_and_scales_with_stickout() {
        let (mut c, _) = ctx();
        c.tool.stickout_mm = 40.0;
        c.tool.measured_fn_hz = Some(2500.0);
        c.tool.measured_k_n_per_um = Some(8.0);
        c.tool.measured_stickout_mm = Some(40.0);
        let m = tool_modal(&c.tool, 5.0, 1.0);
        assert!((m.fn_hz - 2500.0).abs() < 1e-9 && (m.k_n_per_um - 8.0).abs() < 1e-9);
        c.tool.stickout_mm = 55.0;
        let longer = tool_modal(&c.tool, 5.0, 1.0);
        assert!(longer.fn_hz < 2500.0 && longer.k_n_per_um < 8.0, "{:?}", longer);
    }

    #[test]
    fn suggested_lobes_stay_stable_under_speed_error() {
        let (mut c, _) = ctx();
        c.tool.measured_fn_hz = Some(1800.0);
        c.tool.measured_k_n_per_um = Some(10.0);
        let co = cutting_coeffs(&c);
        let d = c.tool.diameter_mm;
        let eng = Engagement::side(4.0, 0.5 * d, d, true);
        let rpm = 20000.0;
        let probe = stability(&c.tool, &co, &eng, 0.05, rpm, 1.0, 1.0, 40000.0, 0.0, 1.0);
        let ap = 2.0 * probe.ap_crit_mm;
        let s = stability(&c.tool, &co, &eng, 0.05, rpm, ap, 1.0, 40000.0, 0.0, 1.0);
        assert!(!s.envelope.is_empty());
        assert!(!s.suggested_rpm.is_empty(), "no robust lobe for ap {}", ap);
        for n in &s.suggested_rpm {
            for dn in [-0.02 * rpm, 0.0, 0.02 * rpm] {
                let m = stability_margin(&c.tool, &co, &eng, 0.05, n + dn, ap, 1.0, 0.0, 1.0);
                assert!(m >= 1.1, "S{} {:+.0}: margin {}", n, dn, m);
            }
        }
    }

    #[test]
    fn forced_response_grows_near_resonance() {
        let (c, p) = ctx();
        let co = cutting_coeffs(&c);
        let d = c.tool.diameter_mm;
        let eng = Engagement::side(5.0, 0.3 * d, d, true);
        let rpm = p.conditions.spindle_rpm as f64;
        let f = mechanistic_forces(&c.tool, &co, 0.05, rpm, &eng, 72, 8);
        let m = tool_modal(&c.tool, 5.0, 1.0);
        let far = forced_vibration(&f, &m, rpm, c.tool.flutes, &eng);
        let res_rpm = m.fn_hz * 60.0 / c.tool.flutes as f64;
        let near = forced_vibration(&f, &m, res_rpm, c.tool.flutes, &eng);
        assert!(near.magnification > far.magnification);
        assert!(near.near_resonance);
    }
}