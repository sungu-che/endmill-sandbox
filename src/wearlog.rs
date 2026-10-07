use crate::loadsim::WearBin;
use crate::ml::ttm::TtmForecaster;
use crate::physics::WearTrajectory;
use crate::timeseries::{holt_damped, ForecastResult, ThresholdCross};
use serde::{Deserialize, Serialize};

pub const LOG_POINTS: usize = 64;
pub const HOLDOUT: usize = 16;
pub const MIN_POINTS: usize = 24;
pub const HISTORY_CAP: f64 = 20.0;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Histogram {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub weight: String,
    pub edges: Vec<f64>,
    pub counts: Vec<f64>,
    pub n: usize,
    pub mean: f64,
    pub sd: f64,
    pub min: f64,
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WearPhase {
    pub key: String,
    pub label: String,
    pub t0_s: f64,
    pub t1_s: f64,
    pub rate_um_min: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Backtest {
    pub method: String,
    pub label: String,
    pub rmse_um: f64,
    pub mae_um: f64,
    pub bias_um: f64,
    pub weight: f64,
    pub history_n: u64,
    pub history_rmse_um: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WearLog {
    pub step_s: f64,
    pub t_cut_s: Vec<f64>,
    pub vb_mm: Vec<f64>,
    pub radial_um: Vec<f64>,
    pub removed_mm3: Vec<f64>,
    pub mrr_mm3_s: Vec<f64>,
    pub force_n: Vec<f64>,
    pub temp_c: Vec<f64>,
    pub chatter: Vec<f64>,
    pub h_um: Vec<f64>,
    pub wp_c: Vec<f64>,
    pub raw_bins: usize,
    pub cut_time_s: f64,
    pub removed_cm3: f64,
    pub vb_end_mm: f64,
    pub specific_wear_um_cm3: f64,
    pub vb_rate_end_um_min: f64,
    pub histograms: Vec<Histogram>,
    pub phases: Vec<WearPhase>,
    pub forecast: Option<ForecastResult>,
    pub backtests: Vec<Backtest>,
    pub ttm_used: bool,
    pub vb_limit_mm: f64,
    pub per_part_vb_p50_mm: Option<f64>,
    pub per_part_vb_p90_mm: Option<f64>,
    pub notes: Vec<String>,
}

struct Uniform {
    t: Vec<f64>,
    vb: Vec<f64>,
    radial: Vec<f64>,
    removed_cum: Vec<f64>,
    mrr: Vec<f64>,
    force: Vec<f64>,
    temp: Vec<f64>,
    chatter: Vec<f64>,
    h: Vec<f64>,
    wp: Vec<f64>,
}

fn interp_at(ts: &[f64], ys: &[f64], t: f64) -> f64 {
    if ts.is_empty() {
        return 0.0;
    }
    if t <= ts[0] {
        return ys[0] * (t / ts[0].max(1e-12)).clamp(0.0, 1.0);
    }
    let i = ts.partition_point(|x| *x < t);
    if i >= ts.len() {
        return ys[ys.len() - 1];
    }
    let (t0, t1) = (ts[i - 1], ts[i]);
    ys[i - 1] + (ys[i] - ys[i - 1]) * (t - t0) / (t1 - t0).max(1e-12)
}

fn resample(bins: &[WearBin], n: usize) -> Uniform {
    let total = bins.last().map(|b| b.t_cut_s).unwrap_or(0.0).max(1e-9);
    let step = total / n as f64;
    let ends: Vec<f64> = bins.iter().map(|b| b.t_cut_s).collect();
    let vbs: Vec<f64> = bins.iter().map(|b| b.vb_mm).collect();
    let rads: Vec<f64> = bins.iter().map(|b| b.radial_loss_um).collect();
    let mut u = Uniform {
        t: Vec::with_capacity(n),
        vb: Vec::with_capacity(n),
        radial: Vec::with_capacity(n),
        removed_cum: Vec::with_capacity(n),
        mrr: Vec::with_capacity(n),
        force: Vec::with_capacity(n),
        temp: Vec::with_capacity(n),
        chatter: Vec::with_capacity(n),
        h: Vec::with_capacity(n),
        wp: Vec::with_capacity(n),
    };
    let mut start = 0usize;
    let mut cum = 0.0;
    let mut last = (0.0, 20.0, 0.0, 0.0, 20.0);
    for k in 0..n {
        let a = k as f64 * step;
        let b = (k + 1) as f64 * step;
        while start < bins.len() && bins[start].t_cut_s <= a {
            start += 1;
        }
        let (mut w, mut f, mut te, mut hh, mut rem, mut wp) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut chat = f64::INFINITY;
        let mut i = start;
        while i < bins.len() {
            let bin = &bins[i];
            let bs = bin.t_cut_s - bin.dt_s;
            if bs >= b {
                break;
            }
            let ov = (bin.t_cut_s.min(b) - bs.max(a)).max(0.0);
            if ov > 0.0 {
                w += ov;
                f += bin.force_n * ov;
                te += bin.temp_c * ov;
                hh += bin.h_um * ov;
                wp += bin.wp_c * ov;
                rem += bin.removed_mm3 * ov / bin.dt_s.max(1e-12);
                if bin.chatter_min > 0.0 {
                    chat = chat.min(bin.chatter_min);
                }
            }
            i += 1;
        }
        if w > 0.0 {
            last = (f / w, te / w, hh / w, if chat.is_finite() { chat } else { 0.0 }, wp / w);
        }
        cum += rem;
        u.t.push(b);
        u.vb.push(interp_at(&ends, &vbs, b));
        u.radial.push(interp_at(&ends, &rads, b));
        u.removed_cum.push(cum);
        u.mrr.push(rem / step.max(1e-12));
        u.force.push(last.0);
        u.temp.push(last.1);
        u.h.push(last.2);
        u.chatter.push(last.3);
        u.wp.push(last.4);
    }
    u
}

fn weighted_stats(vals: &[(f64, f64)]) -> Option<(f64, f64, f64, f64, f64, f64, f64)> {
    let mut v: Vec<(f64, f64)> = vals.iter().cloned().filter(|(x, w)| x.is_finite() && *w > 0.0).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let tw: f64 = v.iter().map(|p| p.1).sum();
    let mean = v.iter().map(|p| p.0 * p.1).sum::<f64>() / tw;
    let var = v.iter().map(|p| (p.0 - mean).powi(2) * p.1).sum::<f64>() / tw;
    let q = |p: f64| {
        let target = p * tw;
        let mut acc = 0.0;
        for (x, w) in v.iter() {
            acc += w;
            if acc >= target {
                return *x;
            }
        }
        v[v.len() - 1].0
    };
    Some((mean, var.sqrt(), v[0].0, q(0.1), q(0.5), q(0.9), v[v.len() - 1].0))
}

pub fn histogram(key: &str, label: &str, unit: &str, weight: &str, vals: &[(f64, f64)], nbins: usize) -> Option<Histogram> {
    let (mean, sd, min, p10, p50, p90, max) = weighted_stats(vals)?;
    let lo = {
        let q01 = weighted_quantile(vals, 0.01).unwrap_or(min);
        if (max - min).abs() < 1e-12 { min - 0.5 } else { q01 }
    };
    let hi = {
        let q99 = weighted_quantile(vals, 0.99).unwrap_or(max);
        if (max - min).abs() < 1e-12 { max + 0.5 } else { q99.max(lo + 1e-9) }
    };
    let nb = nbins.max(4);
    let width = (hi - lo) / nb as f64;
    let mut counts = vec![0.0; nb];
    let mut tw = 0.0;
    for (x, w) in vals.iter().filter(|(x, w)| x.is_finite() && *w > 0.0) {
        let idx = (((x - lo) / width).floor().max(0.0) as usize).min(nb - 1);
        counts[idx] += w;
        tw += w;
    }
    if tw > 0.0 {
        for c in counts.iter_mut() {
            *c /= tw;
        }
    }
    Some(Histogram {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        weight: weight.into(),
        edges: (0..=nb).map(|i| lo + width * i as f64).collect(),
        counts,
        n: vals.len(),
        mean,
        sd,
        min,
        p10,
        p50,
        p90,
        max,
    })
}

fn weighted_quantile(vals: &[(f64, f64)], p: f64) -> Option<f64> {
    let mut v: Vec<(f64, f64)> = vals.iter().cloned().filter(|(x, w)| x.is_finite() && *w > 0.0).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let tw: f64 = v.iter().map(|p| p.1).sum();
    let mut acc = 0.0;
    for (x, w) in v.iter() {
        acc += w;
        if acc >= p * tw {
            return Some(*x);
        }
    }
    Some(v[v.len() - 1].0)
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 2]
}

fn phases_of(t: &[f64], vb: &[f64]) -> Vec<WearPhase> {
    let n = vb.len();
    if n < 8 {
        return Vec::new();
    }
    let mut rate: Vec<f64> = Vec::with_capacity(n);
    for k in 0..n {
        let (a, b) = if k == 0 { (0usize, 1usize) } else { (k - 1, k) };
        let dt = (t[b] - if k == 0 { 0.0 } else { t[a] }).max(1e-9);
        let dv = vb[b] - if k == 0 { 0.0 } else { vb[a] };
        rate.push(dv / dt);
    }
    let smooth: Vec<f64> = (0..n)
        .map(|k| {
            let lo = k.saturating_sub(2);
            let hi = (k + 3).min(n);
            median(&rate[lo..hi])
        })
        .collect();
    let steady = median(&smooth[n / 4..(3 * n / 4).max(n / 4 + 1)]);
    let to_um_min = 60.0 * 1000.0;
    if steady <= 1e-12 {
        return vec![WearPhase {
            key: "steady".into(),
            label: "정상 마모".into(),
            t0_s: 0.0,
            t1_s: t[n - 1],
            rate_um_min: smooth.iter().sum::<f64>() / n as f64 * to_um_min,
        }];
    }
    let mut k1 = 0usize;
    while k1 < n / 2 && smooth[k1] > 1.3 * steady {
        k1 += 1;
    }
    let mut k2 = n;
    while k2 > n / 2 && smooth[k2 - 1] > 1.3 * steady {
        k2 -= 1;
    }
    let mean_rate = |a: usize, b: usize| if b > a { smooth[a..b].iter().sum::<f64>() / (b - a) as f64 * to_um_min } else { 0.0 };
    let mut out = Vec::new();
    let t_at = |k: usize| if k == 0 { 0.0 } else { t[k - 1] };
    if k1 > 0 {
        out.push(WearPhase {
            key: "break_in".into(),
            label: "초기 마모 (길들이기)".into(),
            t0_s: 0.0,
            t1_s: t_at(k1),
            rate_um_min: mean_rate(0, k1),
        });
    }
    out.push(WearPhase {
        key: "steady".into(),
        label: "정상 마모".into(),
        t0_s: t_at(k1),
        t1_s: t_at(k2),
        rate_um_min: mean_rate(k1, k2),
    });
    if k2 < n {
        out.push(WearPhase {
            key: "accelerated".into(),
            label: "가속 마모".into(),
            t0_s: t_at(k2),
            t1_s: t[n - 1],
            rate_um_min: mean_rate(k2, n),
        });
    }
    out
}

pub fn bins_csv(bins: &[WearBin]) -> String {
    let mut s = String::from("t_cut_s,t_s,dt_s,removed_mm3,mrr_mm3_s,force_n,force_peak_n,temp_c,temp_max_c,power_kw,h_um,engage_deg,chatter_min,wall_um,preheat_c,vb_mm,radial_loss_um,wp_c,seg0,seg1\n");
    for b in bins.iter() {
        s.push_str(&format!(
            "{:.4},{:.4},{:.5},{:.5},{:.4},{:.3},{:.3},{:.2},{:.2},{:.5},{:.4},{:.2},{:.4},{:.3},{:.3},{:.6},{:.4},{:.3},{},{}\n",
            b.t_cut_s,
            b.t_s,
            b.dt_s,
            b.removed_mm3,
            b.removed_mm3 / b.dt_s.max(1e-12),
            b.force_n,
            b.force_peak_n,
            b.temp_c,
            b.temp_max_c,
            b.power_kw,
            b.h_um,
            b.engage_deg,
            b.chatter_min,
            b.wall_um,
            b.preheat_c,
            b.vb_mm,
            b.radial_loss_um,
            b.wp_c,
            b.seg0,
            b.seg1
        ));
    }
    s
}

type Band = Option<(Vec<f64>, Vec<f64>)>;

fn method_label(key: &str) -> &'static str {
    match key {
        "ttm" => "TTM-R3",
        "holt" => "Holt 감쇠 추세",
        _ => "물리 마모 궤적",
    }
}

fn predict(key: &str, ctx: &[f64], h: usize, step: f64, traj: Option<&WearTrajectory>, ttm: Option<&TtmForecaster>) -> Option<(Vec<f64>, Band)> {
    let last = *ctx.last()?;
    match key {
        "physics" => {
            let tr = traj?;
            if tr.t_min.is_empty() {
                return None;
            }
            Some(((1..=h).map(|k| last + tr.growth_from(last, k as f64 * step / 60.0)).collect(), None))
        }
        "holt" => {
            if ctx.len() < 3 {
                return None;
            }
            let (p, sd, _) = holt_damped(ctx, h);
            let lo: Vec<f64> = p.iter().zip(sd.iter()).map(|(a, s)| a - 1.2816 * s).collect();
            let hi: Vec<f64> = p.iter().zip(sd.iter()).map(|(a, s)| a + 1.2816 * s).collect();
            Some((p, Some((lo, hi))))
        }
        "ttm" => {
            let m = ttm?;
            if ctx.len() < m.min_history() || h > m.prediction_length() {
                return None;
            }
            let o = m.forecast(ctx).ok()?;
            let point: Vec<f64> = o.point.iter().take(h).cloned().collect();
            let band = o.quantiles.as_ref().and_then(|q| {
                let lo_i = o.quantile_levels.iter().position(|l| (*l - 0.1).abs() < 1e-6)?;
                let hi_i = o.quantile_levels.iter().position(|l| (*l - 0.9).abs() < 1e-6)?;
                Some((q[lo_i].iter().take(h).cloned().collect::<Vec<f64>>(), q[hi_i].iter().take(h).cloned().collect::<Vec<f64>>()))
            });
            Some((point, band))
        }
        _ => None,
    }
}

pub fn analyze(
    bins: &[WearBin],
    wall_residuals: &[f64],
    traj: Option<&WearTrajectory>,
    ttm: Option<&TtmForecaster>,
    history: &[(String, f64, u64)],
    vb_limit_mm: f64,
) -> WearLog {
    let mut log = WearLog {
        vb_limit_mm,
        raw_bins: bins.len(),
        ..Default::default()
    };
    if bins.is_empty() {
        log.notes.push("절삭 구간이 없어 마모 로그를 만들지 않았습니다".into());
        return log;
    }
    let total = bins.last().map(|b| b.t_cut_s).unwrap_or(0.0);
    let removed: f64 = bins.iter().map(|b| b.removed_mm3).sum();
    log.cut_time_s = total;
    log.removed_cm3 = removed / 1000.0;
    log.vb_end_mm = bins.last().map(|b| b.vb_mm).unwrap_or(0.0);
    log.specific_wear_um_cm3 = if log.removed_cm3 > 1e-9 { log.vb_end_mm * 1000.0 / log.removed_cm3 } else { 0.0 };
    let n = LOG_POINTS.min(bins.len().max(2));
    let u = resample(bins, n);
    log.step_s = total / n as f64;
    log.phases = phases_of(&u.t, &u.vb);
    if n >= 4 {
        let k = n.min(6);
        let dv = u.vb[n - 1] - u.vb[n - k];
        let dt = (u.t[n - 1] - u.t[n - k]).max(1e-9);
        log.vb_rate_end_um_min = dv / dt * 60.0 * 1000.0;
    }
    let tw: Vec<(f64, f64)> = bins.iter().map(|b| (b.removed_mm3 / b.dt_s.max(1e-12), b.dt_s)).collect();
    let mut hist = Vec::new();
    if let Some(h) = histogram("mrr", "소재 제거율", "mm³/s", "절삭 시간", &tw, 24) {
        hist.push(h);
    }
    let fw: Vec<(f64, f64)> = bins.iter().map(|b| (b.force_n, b.dt_s)).collect();
    if let Some(h) = histogram("force", "절삭력 (평균 합력)", "N", "절삭 시간", &fw, 24) {
        hist.push(h);
    }
    let th: Vec<(f64, f64)> = bins.iter().map(|b| (b.temp_c, b.dt_s)).collect();
    if let Some(h) = histogram("temp", "날끝 온도", "°C", "절삭 시간", &th, 24) {
        hist.push(h);
    }
    let hw: Vec<(f64, f64)> = bins.iter().map(|b| (b.h_um, b.dt_s)).collect();
    if let Some(h) = histogram("chip", "평균 칩 두께", "µm", "절삭 시간", &hw, 24) {
        hist.push(h);
    }
    let mut vr: Vec<(f64, f64)> = Vec::new();
    for w in bins.windows(2) {
        let dt = w[1].dt_s.max(1e-12);
        vr.push(((w[1].vb_mm - w[0].vb_mm) / dt * 60.0 * 1000.0, dt));
    }
    if let Some(h) = histogram("vb_rate", "플랭크 마모 속도", "µm/min", "절삭 시간", &vr, 24) {
        hist.push(h);
    }
    let cw: Vec<(f64, f64)> = bins.iter().filter(|b| b.chatter_min > 0.0).map(|b| (b.chatter_min, b.dt_s)).collect();
    if let Some(h) = histogram("chatter", "채터 여유 (한계/실제 ap)", "배", "절삭 시간", &cw, 24) {
        hist.push(h);
    }
    let ww: Vec<(f64, f64)> = wall_residuals.iter().map(|v| (*v, 1.0)).collect();
    if let Some(h) = histogram("wall", "최종 벽면 잔여 오차", "µm", "벽면 샘플", &ww, 24) {
        hist.push(h);
    }
    log.histograms = hist;
    log.t_cut_s = u.t.clone();
    log.vb_mm = u.vb.clone();
    log.radial_um = u.radial.clone();
    log.removed_mm3 = u.removed_cum.clone();
    log.mrr_mm3_s = u.mrr.clone();
    log.force_n = u.force.clone();
    log.temp_c = u.temp.clone();
    log.chatter = u.chatter.clone();
    log.h_um = u.h.clone();
    log.wp_c = u.wp.clone();
    if n < MIN_POINTS {
        log.notes.push(format!("절삭 시간 구간이 {}개라 백테스트·예측을 하지 않았습니다 (최소 {})", n, MIN_POINTS));
        return log;
    }
    let hz = HOLDOUT.min(n / 3).max(4);
    let step = log.step_s;
    let split = n - hz;
    let mut backtests: Vec<Backtest> = Vec::new();
    for key in ["ttm", "holt", "physics"] {
        let label = method_label(key);
        let pred = predict(key, &u.vb[..split], hz, step, traj, ttm);
        let pred = match pred {
            Some(p) => p.0,
            None => continue,
        };
        let errs: Vec<f64> = pred.iter().zip(u.vb[split..].iter()).map(|(p, a)| (p - a) * 1000.0).collect();
        let k = errs.len().max(1) as f64;
        let mse = errs.iter().map(|e| e * e).sum::<f64>() / k;
        let hist = history.iter().find(|(m, _, _)| m.as_str() == key);
        backtests.push(Backtest {
            method: key.to_string(),
            label: label.to_string(),
            rmse_um: mse.sqrt(),
            mae_um: errs.iter().map(|e| e.abs()).sum::<f64>() / k,
            bias_um: errs.iter().sum::<f64>() / k,
            weight: 0.0,
            history_n: hist.map(|h| h.2).unwrap_or(0),
            history_rmse_um: hist.map(|h| h.1.max(0.0).sqrt()),
        });
    }
    if backtests.is_empty() {
        log.notes.push("사용 가능한 예측기가 없습니다".into());
        return log;
    }
    let eps = 0.01;
    let eff: Vec<f64> = backtests
        .iter()
        .map(|b| {
            let now = b.rmse_um * b.rmse_um;
            match b.history_rmse_um {
                Some(r) if b.history_n > 0 => {
                    let nh = (b.history_n as f64).min(HISTORY_CAP);
                    (nh * r * r + now) / (nh + 1.0)
                }
                _ => now,
            }
        })
        .collect();
    let inv: Vec<f64> = eff.iter().map(|m| 1.0 / (m + eps)).collect();
    let s: f64 = inv.iter().sum::<f64>().max(1e-12);
    for (b, w) in backtests.iter_mut().zip(inv.iter()) {
        b.weight = w / s;
    }
    let rmse_fused_mm = backtests.iter().zip(eff.iter()).map(|(b, m)| b.weight * m).sum::<f64>().sqrt() / 1000.0;
    let mut fcs: Vec<(String, Vec<f64>, f64, Band)> = Vec::new();
    for b in backtests.iter() {
        if let Some((p, band)) = predict(&b.method, &u.vb, hz, step, traj, ttm) {
            fcs.push((b.method.clone(), p, b.weight, band));
        }
    }
    let wsum: f64 = fcs.iter().map(|f| f.2).sum::<f64>().max(1e-12);
    let vb_last = *u.vb.last().unwrap_or(&0.0);
    let mut p50 = vec![0.0; hz];
    for (_, point, w, _) in fcs.iter() {
        for (i, v) in point.iter().enumerate().take(hz) {
            p50[i] += v * w / wsum;
        }
    }
    let band_src = fcs.iter().find(|f| f.0 == "ttm" && f.3.is_some()).or_else(|| fcs.iter().find(|f| f.3.is_some()));
    let mut p10 = vec![0.0; hz];
    let mut p90 = vec![0.0; hz];
    for i in 0..hz {
        let (lo_off, hi_off) = match band_src {
            Some((_, point, _, Some((lo, hi)))) => ((point[i] - lo[i]).max(0.0), (hi[i] - point[i]).max(0.0)),
            _ => (0.0, 0.0),
        };
        let sig = 1.2816 * rmse_fused_mm * (((i + 1) as f64) / hz as f64).sqrt() * std::f64::consts::SQRT_2;
        p50[i] = p50[i].max(vb_last);
        p90[i] = (p50[i] + (hi_off * hi_off + sig * sig).sqrt()).max(p50[i]);
        p10[i] = (p50[i] - (lo_off * lo_off + sig * sig).sqrt()).max(vb_last).min(p50[i]);
    }
    for i in 1..hz {
        p50[i] = p50[i].max(p50[i - 1]);
        p90[i] = p90[i].max(p90[i - 1]);
        p10[i] = p10[i].max(p10[i - 1]).min(p50[i]);
    }
    let t_last = *u.t.last().unwrap_or(&0.0);
    let t_future: Vec<f64> = (1..=hz).map(|k| t_last + k as f64 * step).collect();
    let horizon_s = hz as f64 * step;
    let s50 = (p50[hz - 1] - vb_last) / horizon_s.max(1e-9);
    let s90 = (p90[hz - 1] - vb_last) / horizon_s.max(1e-9);
    log.per_part_vb_p50_mm = Some((s50 * total).max(0.0));
    log.per_part_vb_p90_mm = Some((s90 * total).max(0.0));
    log.ttm_used = fcs.iter().any(|f| f.0 == "ttm");
    let prior_future: Vec<f64> = fcs.iter().find(|f| f.0 == "physics").map(|f| f.1.clone()).unwrap_or_default();
    let p50_idx = p50.iter().position(|v| *v >= vb_limit_mm);
    let p90_idx = p90.iter().position(|v| *v >= vb_limit_mm);
    let methods_used: Vec<String> = fcs.iter().map(|f| format!("{} {:.0}%", method_label(&f.0), f.2 / wsum * 100.0)).collect();
    log.notes.push(format!(
        "백테스트(마지막 {}점 홀드아웃) 역분산 가중 융합: {}",
        hz,
        methods_used.join(" · ")
    ));
    if !log.ttm_used {
        log.notes.push(match ttm {
            Some(_) => "TTM-R3 가 로드됐지만 예측에 실패해 Holt·물리 궤적만 융합했습니다".into(),
            None => "TTM-R3 미로드: Holt 감쇠 추세와 물리 마모 궤적만 융합했습니다 (설정 > 모델에서 받으면 다음 실행부터 자동 사용)".into(),
        });
    }
    log.forecast = Some(ForecastResult {
        key: "vb_mm".into(),
        unit: "mm".into(),
        t_unit: "s".into(),
        episodes: 1,
        prior_future,
        method: if log.ttm_used { "fused:ttm-r3+holt+physics".into() } else { "fused:holt+physics".into() },
        history_t: u.t.clone(),
        history_y: u.vb.clone(),
        t_future: t_future.clone(),
        point: p50.clone(),
        quantile_levels: vec![0.1, 0.5, 0.9],
        quantiles: vec![p10, p50, p90],
        step,
        physics_residual: false,
        threshold: Some(ThresholdCross {
            limit: vb_limit_mm,
            p50_index: p50_idx,
            p90_index: p90_idx,
            p50_t: p50_idx.map(|i| t_future[i]),
            p90_t: p90_idx.map(|i| t_future[i]),
            already_exceeded: vb_last >= vb_limit_mm,
        }),
        notes: Vec::new(),
    });
    log.backtests = backtests;
    log
}

impl WearLog {
    pub fn p90_growth_factor(&self) -> f64 {
        match (self.per_part_vb_p50_mm, self.per_part_vb_p90_mm) {
            (Some(a), Some(b)) if a > 1e-9 => (b / a).clamp(1.0, 4.0),
            _ => 1.5,
        }
    }
}