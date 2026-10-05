use crate::ml::ttm::TtmForecaster;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Series {
    pub key: String,
    pub unit: String,
    #[serde(default)]
    pub t_unit: String,
    pub t: Vec<f64>,
    pub y: Vec<f64>,
}

impl Series {
    pub fn new(key: &str, unit: &str) -> Self {
        Self {
            key: key.into(),
            unit: unit.into(),
            t_unit: "row".into(),
            t: Vec::new(),
            y: Vec::new(),
        }
    }

    pub fn with_time_unit(mut self, t_unit: &str) -> Self {
        self.t_unit = t_unit.into();
        self
    }

    pub fn minutes_per_t(&self, part_minutes: Option<f64>) -> Option<f64> {
        match self.t_unit.as_str() {
            "s" => Some(1.0 / 60.0),
            "min" => Some(1.0),
            "h" => Some(60.0),
            "part" => part_minutes.filter(|m| *m > 0.0),
            _ => None,
        }
    }

    pub fn slice(&self, start: usize, end: usize) -> Series {
        let e = end.min(self.len());
        let s = start.min(e);
        Series {
            key: self.key.clone(),
            unit: self.unit.clone(),
            t_unit: self.t_unit.clone(),
            t: self.t[s..e].to_vec(),
            y: self.y[s..e].to_vec(),
        }
    }

    pub fn push(&mut self, t: f64, y: f64) {
        if !t.is_finite() || !y.is_finite() {
            return;
        }
        let pos = self.t.iter().position(|v| *v > t).unwrap_or(self.t.len());
        self.t.insert(pos, t);
        self.y.insert(pos, y);
    }

    pub fn len(&self) -> usize {
        self.y.len()
    }

    pub fn is_empty(&self) -> bool {
        self.y.is_empty()
    }

    pub fn median_step(&self) -> Option<f64> {
        if self.t.len() < 2 {
            return None;
        }
        let mut d: Vec<f64> = self.t.windows(2).map(|w| w[1] - w[0]).filter(|v| *v > 0.0).collect();
        if d.is_empty() {
            return None;
        }
        d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Some(d[d.len() / 2])
    }

    pub fn regular(&self) -> (Vec<f64>, Vec<f64>, f64) {
        let step = match self.median_step() {
            Some(s) => s,
            None => return (self.t.clone(), self.y.clone(), 1.0),
        };
        let t0 = self.t[0];
        let t1 = *self.t.last().unwrap();
        let n = (((t1 - t0) / step).round() as usize + 1).min(20000);
        let mut ts = Vec::with_capacity(n);
        let mut ys = Vec::with_capacity(n);
        let mut k = 0usize;
        for i in 0..n {
            let t = t0 + step * i as f64;
            while k + 1 < self.t.len() && self.t[k + 1] < t {
                k += 1;
            }
            let y = if k + 1 < self.t.len() {
                let (ta, tb) = (self.t[k], self.t[k + 1]);
                let w = if tb > ta { ((t - ta) / (tb - ta)).clamp(0.0, 1.0) } else { 0.0 };
                self.y[k] * (1.0 - w) + self.y[k + 1] * w
            } else {
                self.y[k]
            };
            ts.push(t);
            ys.push(y);
        }
        (ts, ys, step)
    }
}

pub fn split_episodes(features: &[Vec<f64>], times: &[f64]) -> Vec<(usize, usize)> {
    let n = features.len();
    if n == 0 {
        return Vec::new();
    }
    if n < 4 || times.len() != n {
        return vec![(0, n)];
    }
    let dims = features[0].len();
    let mut mu = vec![0.0; dims];
    let mut sd = vec![0.0; dims];
    for f in features.iter() {
        for d in 0..dims {
            mu[d] += f[d] / n as f64;
        }
    }
    for f in features.iter() {
        for d in 0..dims {
            sd[d] += (f[d] - mu[d]).powi(2) / n as f64;
        }
    }
    for v in sd.iter_mut() {
        *v = v.sqrt().max(1e-9);
    }
    let sem: Vec<f64> = (0..n - 1)
        .map(|i| {
            (0..dims)
                .map(|d| ((features[i + 1][d] - features[i][d]) / sd[d]).powi(2))
                .sum::<f64>()
                .sqrt()
        })
        .collect();
    let dts: Vec<f64> = (0..n - 1).map(|i| (times[i + 1] - times[i]).max(0.0)).collect();
    let dt_mu = dts.iter().sum::<f64>() / dts.len() as f64;
    let dt_sd = (dts.iter().map(|d| (d - dt_mu).powi(2)).sum::<f64>() / dts.len() as f64).sqrt();
    let dist: Vec<f64> = (0..n - 1)
        .map(|i| {
            let z = if dt_sd <= 1e-12 { 0.0 } else { ((dts[i] - dt_mu) / dt_sd).max(0.0) };
            sem[i] * (1.0 + z)
        })
        .collect();
    let d_mu = dist.iter().sum::<f64>() / dist.len() as f64;
    let d_sd = (dist.iter().map(|x| (x - d_mu).powi(2)).sum::<f64>() / dist.len() as f64).sqrt();
    if d_sd <= 1e-9 {
        return vec![(0, n)];
    }
    let cut = d_mu + d_sd;
    let bounds: Vec<usize> = (0..n - 1).filter(|i| dist[*i] > cut).map(|i| i + 1).collect();
    if bounds.is_empty() {
        return vec![(0, n)];
    }
    let mut segs = Vec::new();
    let mut start = 0;
    for b in bounds {
        if b > start {
            segs.push((start, b));
        }
        start = b;
    }
    if start < n {
        segs.push((start, n));
    }
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for seg in segs {
        if seg.1 - seg.0 >= 2 {
            merged.push(seg);
            continue;
        }
        match merged.last_mut() {
            Some(prev) => prev.1 = seg.1,
            None => merged.push(seg),
        }
    }
    if merged.len() >= 2 && merged[0].1 - merged[0].0 < 2 {
        let head = merged.remove(0);
        merged[0].0 = head.0;
    }
    if merged.is_empty() {
        vec![(0, n)]
    } else {
        merged
    }
}

pub fn episodes_of(series: &Series) -> Vec<(usize, usize)> {
    let n = series.len();
    if n < 4 {
        return vec![(0, n)];
    }
    let feats: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let dy = if i == 0 { 0.0 } else { series.y[i] - series.y[i - 1] };
            vec![series.y[i], dy]
        })
        .collect();
    let mut cuts: Vec<usize> = split_episodes(&feats, &series.t)
        .iter()
        .map(|(a, _)| *a)
        .filter(|a| *a > 0)
        .collect();
    for i in 1..n {
        let prev = series.y[i - 1];
        if prev.abs() > 1e-9 && series.y[i] < prev - 0.5 * prev.abs() {
            cuts.push(i);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    let mut start = 0usize;
    for c in cuts {
        let prev = series.y[c - 1];
        let reset = series.y[c] < prev - 0.3 * prev.abs().max(1e-9);
        if reset && c > start {
            out.push((start, c));
            start = c;
        }
    }
    out.push((start, n));
    out
}

pub fn current_episode(series: &Series) -> (Series, usize) {
    let eps = episodes_of(series);
    let (a, b) = *eps.last().unwrap_or(&(0, series.len()));
    if b - a >= 4 || eps.len() == 1 {
        (series.slice(a, b), eps.len())
    } else {
        (series.clone(), eps.len())
    }
}

pub fn derive_recency_half_life(ages: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = ages.iter().filter(|a| **a >= 0.0 && a.is_finite()).cloned().collect();
    if v.len() < 4 {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = v[v.len() / 2];
    if median <= 1e-9 {
        return None;
    }
    Some(median)
}

pub fn recency_weight(age: f64, half_life: f64) -> f64 {
    if half_life <= 0.0 {
        return 1.0;
    }
    0.5f64.powf(age.max(0.0) / half_life).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoltFit {
    pub alpha: f64,
    pub beta: f64,
    pub phi: f64,
    pub sse: f64,
    pub sigma: f64,
}

pub fn holt_damped(y: &[f64], horizon: usize) -> (Vec<f64>, Vec<f64>, HoltFit) {
    let n = y.len();
    if n == 0 {
        return (vec![0.0; horizon], vec![0.0; horizon], HoltFit { alpha: 0.0, beta: 0.0, phi: 1.0, sse: 0.0, sigma: 0.0 });
    }
    if n < 3 {
        let last = *y.last().unwrap();
        return (vec![last; horizon], vec![0.0; horizon], HoltFit { alpha: 1.0, beta: 0.0, phi: 1.0, sse: 0.0, sigma: 0.0 });
    }
    let mut best = HoltFit {
        alpha: 0.5,
        beta: 0.1,
        phi: 0.98,
        sse: f64::INFINITY,
        sigma: 0.0,
    };
    for ai in 1..=9 {
        let alpha = ai as f64 / 10.0;
        for &beta in [0.05, 0.1, 0.2, 0.3].iter() {
            for &phi in [0.8, 0.9, 0.95, 0.98, 1.0].iter() {
                let mut l = y[0];
                let mut b = y[1] - y[0];
                let mut sse = 0.0;
                for t in 1..n {
                    let pred = l + phi * b;
                    let e = y[t] - pred;
                    sse += e * e;
                    let l_new = alpha * y[t] + (1.0 - alpha) * (l + phi * b);
                    b = beta * (l_new - l) + (1.0 - beta) * phi * b;
                    l = l_new;
                }
                if sse < best.sse {
                    best = HoltFit { alpha, beta, phi, sse, sigma: 0.0 };
                }
            }
        }
    }
    let mut l = y[0];
    let mut b = y[1] - y[0];
    for t in 1..n {
        let l_new = best.alpha * y[t] + (1.0 - best.alpha) * (l + best.phi * b);
        b = best.beta * (l_new - l) + (1.0 - best.beta) * best.phi * b;
        l = l_new;
    }
    best.sigma = (best.sse / (n - 1).max(1) as f64).sqrt();
    let mut point = Vec::with_capacity(horizon);
    let mut sd = Vec::with_capacity(horizon);
    let mut damp = 0.0;
    let mut var_acc = 1.0;
    for h in 1..=horizon {
        if h >= 2 {
            let c = best.alpha * (1.0 + best.beta * damp);
            var_acc += c * c;
        }
        damp += best.phi.powi(h as i32);
        point.push(l + damp * b);
        sd.push(best.sigma * var_acc.sqrt());
    }
    (point, sd, best)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdCross {
    pub limit: f64,
    pub p50_index: Option<usize>,
    pub p90_index: Option<usize>,
    pub p50_t: Option<f64>,
    pub p90_t: Option<f64>,
    pub already_exceeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastResult {
    pub key: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub t_unit: String,
    #[serde(default)]
    pub episodes: usize,
    #[serde(default)]
    pub prior_future: Vec<f64>,
    pub method: String,
    pub history_t: Vec<f64>,
    pub history_y: Vec<f64>,
    pub t_future: Vec<f64>,
    pub point: Vec<f64>,
    pub quantile_levels: Vec<f64>,
    pub quantiles: Vec<Vec<f64>>,
    pub step: f64,
    pub physics_residual: bool,
    pub threshold: Option<ThresholdCross>,
    pub notes: Vec<String>,
}

impl ForecastResult {
    pub fn band(&self, level: f64) -> Option<&Vec<f64>> {
        self.quantile_levels
            .iter()
            .position(|q| (q - level).abs() < 1e-6)
            .and_then(|i| self.quantiles.get(i))
    }
}

pub fn forecast(
    series: &Series,
    horizon: usize,
    ttm: Option<&TtmForecaster>,
    physics_prior: Option<&dyn Fn(f64) -> f64>,
    threshold: Option<f64>,
) -> ForecastResult {
    let mut notes = Vec::new();
    let (ts, ys, step) = series.regular();
    let prior_hist: Vec<f64> = match physics_prior {
        Some(f) => ts.iter().map(|t| f(*t)).collect(),
        None => vec![0.0; ys.len()],
    };
    let resid: Vec<f64> = ys.iter().zip(prior_hist.iter()).map(|(a, b)| a - b).collect();
    let mut horizon = horizon.max(1);
    let mut method = "holt-damped".to_string();
    let mut levels = vec![0.1, 0.5, 0.9];
    let mut qs: Vec<Vec<f64>>;
    let mut point: Vec<f64>;
    let ttm_out = match ttm {
        Some(m) if resid.len() >= m.min_history() => {
            if horizon > m.prediction_length() {
                notes.push(format!(
                    "TTM 예측 길이({})로 수평선을 제한했습니다.",
                    m.prediction_length()
                ));
                horizon = m.prediction_length();
            }
            match m.forecast(&resid) {
                Ok(o) => Some(o),
                Err(e) => {
                    notes.push(format!("TTM 실패, 통계 대체: {}", e));
                    None
                }
            }
        }
        Some(m) => {
            notes.push(format!(
                "이력 {}점 < TTM 최소 {}점이라 통계 예측을 사용했습니다.",
                resid.len(),
                m.min_history()
            ));
            None
        }
        None => None,
    };
    match ttm_out {
        Some(o) => {
            method = "ttm-r3".into();
            point = o.point.iter().take(horizon).cloned().collect();
            match o.quantiles {
                Some(q) => {
                    levels = o.quantile_levels.clone();
                    qs = q.iter().map(|r| r.iter().take(horizon).cloned().collect()).collect();
                }
                None => {
                    let (_, sd, _) = holt_damped(&resid, horizon);
                    qs = vec![
                        point.iter().zip(sd.iter()).map(|(p, s)| p - 1.2816 * s).collect(),
                        point.clone(),
                        point.iter().zip(sd.iter()).map(|(p, s)| p + 1.2816 * s).collect(),
                    ];
                }
            }
        }
        None => {
            let (p, sd, fit) = holt_damped(&resid, horizon);
            notes.push(format!("Holt 감쇠 추세 α={:.1} β={:.2} φ={:.2}", fit.alpha, fit.beta, fit.phi));
            point = p.clone();
            qs = vec![
                p.iter().zip(sd.iter()).map(|(a, s)| a - 1.2816 * s).collect(),
                p.clone(),
                p.iter().zip(sd.iter()).map(|(a, s)| a + 1.2816 * s).collect(),
            ];
        }
    }
    let t_last = ts.last().cloned().unwrap_or(0.0);
    let t_future: Vec<f64> = (1..=point.len()).map(|h| t_last + step * h as f64).collect();
    let mut prior_future = Vec::new();
    if let Some(f) = physics_prior {
        for (i, t) in t_future.iter().enumerate() {
            let add = f(*t);
            prior_future.push(add);
            point[i] += add;
            for q in qs.iter_mut() {
                q[i] += add;
            }
        }
    }
    for i in 0..point.len() {
        let mut col: Vec<f64> = qs.iter().map(|q| q[i]).collect();
        col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for (k, q) in qs.iter_mut().enumerate() {
            q[i] = col[k];
        }
    }
    let threshold = threshold.map(|limit| {
        let last = ys.last().cloned().unwrap_or(0.0);
        let p50_idx = qs
            .get(levels.iter().position(|l| (*l - 0.5).abs() < 1e-6).unwrap_or(qs.len() / 2))
            .and_then(|q| q.iter().position(|v| *v >= limit));
        let p90_idx = qs
            .get(levels.iter().position(|l| (*l - 0.9).abs() < 1e-6).unwrap_or(qs.len() - 1))
            .and_then(|q| q.iter().position(|v| *v >= limit));
        ThresholdCross {
            limit,
            p50_index: p50_idx,
            p90_index: p90_idx,
            p50_t: p50_idx.map(|i| t_future[i]),
            p90_t: p90_idx.map(|i| t_future[i]),
            already_exceeded: last >= limit,
        }
    });
    ForecastResult {
        key: series.key.clone(),
        unit: series.unit.clone(),
        t_unit: series.t_unit.clone(),
        episodes: 1,
        prior_future,
        method,
        history_t: ts,
        history_y: ys,
        t_future,
        point,
        quantile_levels: levels,
        quantiles: qs,
        step,
        physics_residual: physics_prior.is_some(),
        threshold,
        notes,
    }
}