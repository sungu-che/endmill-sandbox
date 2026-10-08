use crate::physics::{CutContext, ToolGeometry};
use crate::profile::ToolNose;
use serde::{Deserialize, Serialize};

pub const WEAR_BINS: usize = 80;
pub const LOCAL_LIMIT_RATIO: f64 = 5.0 / 3.0;
pub const CHATTER_LIFE_LOSS: f64 = 46.0 / 15.0 - 1.0;
pub const MEASURED_FRF_LOG_SD: f64 = 0.26;
pub const ESTIMATED_FRF_LOG_SD: f64 = 0.4;
pub const ESTIMATED_FRF_CREDIBILITY: f64 = 0.5;
pub const TF_REF_SEVERITY: f64 = 550.0;
pub const TF_REF_CYCLES: f64 = 4.0e4;
pub const TF_EXPONENT: f64 = 4.0;
pub const TF_THRESHOLD: f64 = 100.0;
pub const TF_IDLE_TAU_S: f64 = 0.010;
pub const TF_WATCH: f64 = 0.7;
pub const PROGRESSIVE_COATING_MAX_UM: f64 = 5.0;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WearField {
    pub loc_mm: f64,
    pub flutes: usize,
    pub nz: usize,
    pub dz_mm: f64,
    pub vb_mm: Vec<f64>,
}

impl WearField {
    pub fn new(loc_mm: f64, flutes: usize) -> Self {
        let loc = loc_mm.max(0.5);
        let z = flutes.max(1);
        Self {
            loc_mm: loc,
            flutes: z,
            nz: WEAR_BINS,
            dz_mm: loc / WEAR_BINS as f64,
            vb_mm: vec![0.0; z * WEAR_BINS],
        }
    }

    pub fn for_tool(tool: &ToolGeometry) -> Self {
        Self::new(tool.loc_mm, tool.flutes.max(1) as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.vb_mm.is_empty() || self.vb_mm.iter().all(|v| *v <= 0.0)
    }

    pub fn compatible(&self, loc_mm: f64, flutes: usize) -> bool {
        self.nz == WEAR_BINS && self.flutes == flutes.max(1) && (self.loc_mm - loc_mm.max(0.5)).abs() < 1e-6 && self.vb_mm.len() == self.flutes * self.nz
    }

    fn same_grid(&self, other: &WearField) -> bool {
        self.compatible(other.loc_mm, other.flutes) && other.nz == self.nz && (other.dz_mm - self.dz_mm).abs() < 1e-9 && other.vb_mm.len() == self.vb_mm.len()
    }

    pub fn z_mid(&self, k: usize) -> f64 {
        (k as f64 + 0.5) * self.dz_mm
    }

    pub fn at(&self, flute: usize, k: usize) -> f64 {
        self.vb_mm.get(flute * self.nz + k).cloned().unwrap_or(0.0)
    }

    pub fn value_at_z(&self, flute: usize, z_mm: f64) -> f64 {
        if self.nz == 0 || self.dz_mm <= 0.0 || flute >= self.flutes || z_mm < 0.0 || z_mm > self.loc_mm {
            return 0.0;
        }
        let u = (z_mm / self.dz_mm - 0.5).clamp(0.0, (self.nz - 1) as f64);
        let k = (u.floor() as usize).min(self.nz - 1);
        let k1 = (k + 1).min(self.nz - 1);
        let w = u - k as f64;
        self.at(flute, k) * (1.0 - w) + self.at(flute, k1) * w
    }

    pub fn resampled(&self, loc_mm: f64, flutes: usize) -> Option<WearField> {
        if self.flutes != flutes.max(1) || self.nz == 0 || self.vb_mm.len() != self.flutes * self.nz {
            return None;
        }
        let mut out = WearField::new(loc_mm, flutes);
        for f in 0..out.flutes {
            for k in 0..out.nz {
                let z = out.z_mid(k);
                out.vb_mm[f * out.nz + k] = self.value_at_z(f, z);
            }
        }
        Some(out)
    }

    fn band_weights(&self, ap_mm: f64) -> Vec<(usize, f64)> {
        let ap = ap_mm.clamp(0.0, self.loc_mm);
        let mut out = Vec::new();
        if ap <= 0.0 || self.dz_mm <= 0.0 {
            return out;
        }
        for k in 0..self.nz {
            let a = k as f64 * self.dz_mm;
            if a >= ap {
                break;
            }
            let b = ((k + 1) as f64 * self.dz_mm).min(ap);
            out.push((k, (b - a) / self.dz_mm));
        }
        out
    }

    fn covered_mean(&self, k: usize, ap: f64, axial: &AxialProfile) -> f64 {
        let a = k as f64 * self.dz_mm;
        let b = ((k + 1) as f64 * self.dz_mm).min(ap);
        if b <= a {
            return axial.raw(a.min(ap), ap);
        }
        let n = 8;
        (0..n).map(|i| axial.raw(a + (b - a) * (i as f64 + 0.5) / n as f64, ap)).sum::<f64>() / n as f64
    }

    pub fn deposit(&mut self, ap_mm: f64, dvb_mm: f64, axial: &AxialProfile, shares: &[f64]) {
        if !(dvb_mm > 0.0) || self.nz == 0 {
            return;
        }
        let band = self.band_weights(ap_mm);
        if band.is_empty() {
            return;
        }
        let ap = ap_mm.clamp(0.0, self.loc_mm);
        let raw: Vec<f64> = band.iter().map(|(k, _)| self.covered_mean(*k, ap, axial)).collect();
        let len: f64 = band.iter().map(|(_, f)| f).sum::<f64>().max(1e-12);
        let mean = band.iter().zip(raw.iter()).map(|((_, f), w)| f * w).sum::<f64>() / len;
        let norm = if mean > 1e-12 { 1.0 / mean } else { 1.0 };
        for f in 0..self.flutes {
            let s = shares.get(f).cloned().unwrap_or(1.0).clamp(0.0, 1.0);
            if s <= 0.0 {
                continue;
            }
            for ((k, _), w) in band.iter().zip(raw.iter()) {
                self.vb_mm[f * self.nz + k] += dvb_mm * w * norm * s;
            }
        }
    }

    pub fn flute_band_vb(&self, flute: usize, ap_mm: f64) -> f64 {
        let band = self.band_weights(ap_mm);
        let len: f64 = band.iter().map(|(_, f)| f).sum();
        if len <= 1e-12 {
            return 0.0;
        }
        band.iter().map(|(k, f)| self.at(flute, *k) * f).sum::<f64>() / len
    }

    pub fn band_vb(&self, ap_mm: f64) -> f64 {
        (0..self.flutes).map(|f| self.flute_band_vb(f, ap_mm)).fold(0.0, f64::max)
    }

    pub fn max_vb(&self) -> f64 {
        self.vb_mm.iter().cloned().fold(0.0, f64::max)
    }

    pub fn worst_flute(&self, ap_mm: f64) -> usize {
        (0..self.flutes)
            .map(|f| (f, self.flute_band_vb(f, ap_mm)))
            .fold((0usize, f64::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a })
            .0
    }

    pub fn add(&mut self, other: &WearField) -> bool {
        if !self.same_grid(other) {
            return false;
        }
        for (a, b) in self.vb_mm.iter_mut().zip(other.vb_mm.iter()) {
            *a = (*a + b).max(0.0);
        }
        true
    }

    pub fn add_any(&mut self, other: &WearField) -> bool {
        if self.add(other) {
            return true;
        }
        match other.resampled(self.loc_mm, self.flutes) {
            Some(r) => self.add(&r),
            None => false,
        }
    }

    pub fn subtract(&mut self, other: &WearField) -> bool {
        let o = if self.same_grid(other) {
            other.clone()
        } else {
            match other.resampled(self.loc_mm, self.flutes) {
                Some(r) => r,
                None => return false,
            }
        };
        for (a, b) in self.vb_mm.iter_mut().zip(o.vb_mm.iter()) {
            *a = (*a - b).max(0.0);
        }
        true
    }

    pub fn anchor_band(&mut self, ap_mm: f64, target_vb_mm: f64, axial: &AxialProfile, shares: &[f64]) {
        let target = target_vb_mm.max(0.0);
        let now = self.band_vb(ap_mm);
        if now > 1e-9 {
            let k = target / now;
            let band: Vec<usize> = self.band_weights(ap_mm).iter().map(|(k, _)| *k).collect();
            for f in 0..self.flutes {
                for k2 in band.iter() {
                    let v = &mut self.vb_mm[f * self.nz + k2];
                    *v *= k;
                }
            }
        } else if target > 0.0 {
            self.deposit(ap_mm, target, axial, shares);
        }
    }

    pub fn radial_um(&self, clearance_deg: f64) -> Vec<f64> {
        let t = clearance_deg.clamp(2.0, 30.0).to_radians().tan();
        self.vb_mm.iter().map(|v| v * t * 1000.0).collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AxialProfile {
    pub notch: f64,
    pub notch_sigma_mm: f64,
    pub corner: f64,
    pub corner_z_mm: f64,
    pub corner_sigma_mm: f64,
}

impl Default for AxialProfile {
    fn default() -> Self {
        Self {
            notch: 0.0,
            notch_sigma_mm: 0.3,
            corner: 0.0,
            corner_z_mm: 0.0,
            corner_sigma_mm: 0.2,
        }
    }
}

impl AxialProfile {
    pub fn of(ctx: &CutContext) -> Self {
        let d = ctx.tool.diameter_mm.max(0.1);
        let ox_share = crate::tribology::normalized_shares(&ctx.wp.mech_shares)[3];
        let notch = (1.6 * ctx.wp.work_hardening.max(0.0) + 0.4 * ox_share).clamp(0.0, 0.8);
        let rc = ctx.tool.nose.corner_radius(d);
        let (corner, corner_z) = match ctx.tool.nose {
            ToolNose::Square => (0.3, 0.0),
            ToolNose::Ball => (0.0, 0.0),
            _ => (0.15, rc),
        };
        Self {
            notch,
            notch_sigma_mm: (0.05 * d).max(0.3),
            corner,
            corner_z_mm: corner_z,
            corner_sigma_mm: (0.03 * d).max(0.2),
        }
    }

    pub fn raw(&self, z_mm: f64, ap_mm: f64) -> f64 {
        let g = |u: f64| (-u * u).exp();
        1.0 + self.notch * g((z_mm - ap_mm) / self.notch_sigma_mm.max(1e-3)) + self.corner * g((z_mm - self.corner_z_mm) / self.corner_sigma_mm.max(1e-3))
    }
}

pub fn flute_shares(tool: &ToolGeometry, h_mean_mm: f64, mc: f64) -> Vec<f64> {
    let z = tool.flutes.max(1) as usize;
    if z == 1 || tool.runout_um.abs() < 1e-9 {
        return vec![1.0; z];
    }
    let r = tool.runout_um / 1000.0;
    let h = h_mean_mm.max(1e-5);
    let e = (1.0 - mc).clamp(0.3, 1.0);
    let loads: Vec<f64> = (0..z)
        .map(|j| {
            let cur = (std::f64::consts::TAU * j as f64 / z as f64).cos();
            let prev = (std::f64::consts::TAU * ((j + z - 1) % z) as f64 / z as f64).cos();
            (h + r * (cur - prev)).max(0.2 * h).powf(e)
        })
        .collect();
    let mx = loads.iter().cloned().fold(0.0, f64::max).max(1e-12);
    loads.iter().map(|l| l / mx).collect()
}

fn erf(x: f64) -> f64 {
    let s = x.signum();
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * (-a * a).exp();
    s * y
}

pub fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

pub fn frf_log_sd(measured_frf: bool) -> f64 {
    if measured_frf {
        MEASURED_FRF_LOG_SD
    } else {
        ESTIMATED_FRF_LOG_SD
    }
}

pub fn frf_credibility(margin: f64, measured_frf: bool) -> f64 {
    if measured_frf {
        1.0
    } else {
        ESTIMATED_FRF_CREDIBILITY + (1.0 - ESTIMATED_FRF_CREDIBILITY) * crate::tribology::smoothstep((margin - 0.3) / 0.7)
    }
}

pub fn chatter_probability(margin: f64, measured_frf: bool) -> f64 {
    if !(margin.is_finite() && margin > 0.0) {
        return 0.0;
    }
    frf_credibility(margin, measured_frf) * normal_cdf(-margin.ln() / frf_log_sd(measured_frf))
}

pub fn chatter_severity_at(true_margin: f64) -> f64 {
    if !(true_margin > 0.0 && true_margin < 1.0) {
        return 0.0;
    }
    let q = 1.0 / true_margin;
    0.15 + 0.85 * (1.0 - (-(q - 1.0) / 2.0).exp())
}

pub fn expected_chatter_severity(margin: f64, measured_frf: bool) -> f64 {
    if !(margin.is_finite() && margin > 0.0) {
        return 0.0;
    }
    let sd = frf_log_sd(measured_frf);
    let zc = -margin.ln() / sd;
    let lo = -8.0;
    if zc <= lo {
        return 0.0;
    }
    let hi = zc.min(8.0);
    let n = 64;
    let h = (hi - lo) / n as f64;
    let pdf = |z: f64| (-0.5 * z * z).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let f = |z: f64| pdf(z) * chatter_severity_at((margin * (sd * z).exp()).min(1.0 - 1e-12));
    let mut s = f(lo) + f(hi);
    for i in 1..n {
        let z = lo + i as f64 * h;
        s += if i % 2 == 1 { 4.0 } else { 2.0 } * f(z);
    }
    (s * h / 3.0).max(0.0)
}

pub fn chatter_wear_factor(margin: f64, measured_frf: bool) -> f64 {
    1.0 + CHATTER_LIFE_LOSS * frf_credibility(margin, measured_frf) * expected_chatter_severity(margin, measured_frf)
}

pub fn coating_strip_um(tool: &ToolGeometry) -> f64 {
    let c = &tool.coating;
    if c.thickness_um <= 0.0 || c.chem.bare {
        return 0.0;
    }
    c.thickness_um / tool.clearance_deg.clamp(2.0, 30.0).to_radians().sin()
}

pub fn coating_exposure(tool: &ToolGeometry, vb_mm: f64) -> f64 {
    let strip = coating_strip_um(tool);
    let land = vb_mm.max(0.0) * 1000.0;
    if strip <= 0.0 || land <= strip {
        0.0
    } else {
        1.0 - strip / land
    }
}

pub fn progressive_coating(tool: &ToolGeometry) -> bool {
    let c = &tool.coating;
    !c.chem.bare && c.thickness_um > 0.0 && c.thickness_um <= PROGRESSIVE_COATING_MAX_UM
}

pub fn exposed_wear_rate(coated: f64, bare: f64, exposure: f64) -> f64 {
    let e = exposure.clamp(0.0, 1.0);
    if e <= 0.0 || !(coated > bare) || bare <= 0.0 {
        return coated;
    }
    1.0 / ((1.0 - e) / coated.max(1e-12) + e / bare)
}

pub fn idle_factor(span_rad: f64, rpm: f64) -> f64 {
    if rpm <= 0.0 || !(span_rad > 0.0) || span_rad >= 1.9 * std::f64::consts::PI {
        return 0.0;
    }
    let t_idle = (std::f64::consts::TAU - span_rad).max(0.0) / std::f64::consts::TAU * 60.0 / rpm;
    1.0 - (-t_idle / TF_IDLE_TAU_S).exp()
}

pub fn cycles_to_crack(severity_eff: f64) -> f64 {
    if !(severity_eff > TF_THRESHOLD) {
        return f64::INFINITY;
    }
    TF_REF_CYCLES * (TF_REF_SEVERITY / severity_eff).powf(TF_EXPONENT)
}

pub fn thermal_fatigue_rate(severity: f64, span_rad: f64, rpm: f64) -> f64 {
    let s = severity.max(0.0) * idle_factor(span_rad, rpm);
    let n = cycles_to_crack(s);
    if n.is_finite() && n > 0.0 {
        rpm / 60.0 / n
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolStart {
    pub instance_id: Option<i64>,
    pub generation: i64,
    pub label: String,
    pub vb_band_mm: f64,
    pub vb_max_mm: f64,
    pub cut_min: f64,
    pub removed_cm3: f64,
    pub jobs: i64,
    pub field: WearField,
    pub anchor_um: Option<f64>,
    pub pred_base_since_anchor_um: f64,
    pub history: Vec<(f64, f64)>,
    pub last_measured_um: Option<f64>,
    #[serde(default)]
    pub thermal_damage: f64,
    #[serde(default)]
    pub life_used_max: f64,
}

impl ToolStart {
    pub fn fresh(tool: &ToolGeometry) -> Self {
        Self {
            field: WearField::for_tool(tool),
            ..Default::default()
        }
    }

    pub fn is_fresh(&self) -> bool {
        self.vb_band_mm <= 1e-9 && self.cut_min <= 1e-9 && self.thermal_damage <= 1e-9
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WearSummary {
    pub vb_band_mm: f64,
    pub vb_max_mm: f64,
    pub vb_limit_mm: f64,
    pub local_limit_mm: f64,
    pub worst_flute: usize,
    pub flute_band_vb_mm: Vec<f64>,
    pub coating_strip_um: f64,
    pub coating_exposed: f64,
    pub radial_max_um: f64,
    pub life_used: f64,
    pub notes: Vec<String>,
    #[serde(default)]
    pub thermal_damage: f64,
    #[serde(default)]
    pub life_used_max: f64,
}

pub fn summarize(field: &WearField, tool: &ToolGeometry, ap_mm: f64, vb_limit_mm: f64) -> WearSummary {
    let band = field.band_vb(ap_mm);
    let mx = field.max_vb();
    let local = vb_limit_mm * LOCAL_LIMIT_RATIO;
    let strip = coating_strip_um(tool);
    let per: Vec<f64> = (0..field.flutes).map(|f| field.flute_band_vb(f, ap_mm)).collect();
    let mut notes = Vec::new();
    if strip > 0.0 && band * 1000.0 > strip {
        notes.push(format!(
            "플랭크 마모 폭 {:.0} µm 가 코팅 관통 폭 {:.1} µm (코팅 {:.1} µm / sin 여유각 {:.0}°) 를 넘어 마모 랜드의 {:.0}% 에서 모재가 드러남",
            band * 1000.0,
            strip,
            tool.coating.thickness_um,
            tool.clearance_deg,
            coating_exposure(tool, band) * 100.0
        ));
    }
    if mx > 1.15 * band && mx > 0.02 {
        notes.push(format!("국부 최대 VB {:.3} mm (절입 경계 노치·코너 집중) — 국부 한계 {:.2} mm", mx, local));
    }
    let life_used = (band / vb_limit_mm.max(1e-6)).max(mx / local.max(1e-6));
    WearSummary {
        vb_band_mm: band,
        vb_max_mm: mx,
        vb_limit_mm,
        local_limit_mm: local,
        worst_flute: field.worst_flute(ap_mm),
        flute_band_vb_mm: per,
        coating_strip_um: strip,
        coating_exposed: coating_exposure(tool, band),
        radial_max_um: mx * tool.clearance_deg.clamp(2.0, 30.0).to_radians().tan() * 1000.0,
        life_used,
        notes,
        thermal_damage: 0.0,
        life_used_max: life_used,
    }
}

pub fn summarize_state(field: &WearField, tool: &ToolGeometry, ap_mm: f64, vb_limit_mm: f64, thermal_damage: f64, life_used_max: f64) -> WearSummary {
    let mut s = summarize(field, tool, ap_mm, vb_limit_mm);
    s.thermal_damage = thermal_damage.max(0.0);
    s.life_used_max = life_used_max.max(s.life_used).max(s.thermal_damage);
    if s.thermal_damage >= 1.0 {
        s.notes.push(format!(
            "열피로(빗살 균열) 누적 손상 {:.2} — 단속 절삭의 가열·냉각 반복으로 균열이 시작됐을 가능성이 높음: 정삭 전 날 확인 또는 교체",
            s.thermal_damage
        ));
    } else if s.thermal_damage >= TF_WATCH {
        s.notes.push(format!("열피로(빗살 균열) 누적 손상 {:.2} — 균열 개시(1.0) 근접", s.thermal_damage));
    }
    s
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefCut {
    pub record_id: i64,
    pub job_id: i64,
    pub pass: i64,
    pub similarity: f64,
    pub mode: String,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub phi_st_deg: f64,
    pub phi_ex_deg: f64,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub vb_rate_um_min: f64,
    pub chatter_min: f64,
    pub measured_ratio: Option<f64>,
    pub tool_label: String,
    pub material_key: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefSummary {
    pub n: usize,
    pub n_measured: usize,
    pub mean_similarity: f64,
    pub wear_ratio_p50: Option<f64>,
    pub wear_ratio_p75: Option<f64>,
    pub sim_rate_ratio: Option<f64>,
    pub lines: Vec<String>,
    pub refs: Vec<RefCut>,
}

impl RefSummary {
    pub fn conservative_factor(&self) -> f64 {
        match self.wear_ratio_p75 {
            Some(r) if self.n_measured >= 3 => r.clamp(1.0, 2.5),
            _ => 1.0,
        }
    }
}
