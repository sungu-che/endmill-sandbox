use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const SDS_RECIPE: &str = "endmill-sds-v1:welford+ring/hier-write/tool-mold-split/calib-shrink";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ledger {
    Tool,
    Mold,
    Process,
    Decision,
    Wear,
}

impl Ledger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Ledger::Tool => "tool",
            Ledger::Mold => "mold",
            Ledger::Process => "process",
            Ledger::Decision => "decision",
            Ledger::Wear => "wear",
        }
    }

    pub fn file_name(&self) -> String {
        format!("{}.json", self.as_str())
    }

    pub fn all() -> [Ledger; 5] {
        [Ledger::Tool, Ledger::Mold, Ledger::Process, Ledger::Decision, Ledger::Wear]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Track {
    ToolImage,
    ToolGeom,
    MoldGeom,
    MoldImage,
    Process,
    Decision,
    Wear,
}

impl Track {
    pub fn as_str(&self) -> &'static str {
        match self {
            Track::ToolImage => "tool.image",
            Track::ToolGeom => "tool.geom",
            Track::MoldGeom => "mold.geom",
            Track::MoldImage => "mold.image",
            Track::Process => "process",
            Track::Decision => "decision",
            Track::Wear => "wear",
        }
    }

    pub fn min_obs(&self) -> (u64, u64, u64) {
        match self {
            Track::ToolImage => (5, 4, 8),
            Track::ToolGeom => (4, 3, 6),
            Track::MoldGeom => (4, 3, 6),
            Track::MoldImage => (5, 4, 8),
            Track::Process => (3, 2, 4),
            Track::Decision => (10, 6, 15),
            Track::Wear => (3, 2, 4),
        }
    }

    pub fn ring_len(&self) -> usize {
        self.min_obs().1.max(4) as usize
    }

    pub fn ledger(&self) -> Ledger {
        match self {
            Track::ToolImage | Track::ToolGeom => Ledger::Tool,
            Track::MoldGeom | Track::MoldImage => Ledger::Mold,
            Track::Process => Ledger::Process,
            Track::Decision => Ledger::Decision,
            Track::Wear => Ledger::Wear,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub track: Track,
    pub primary: String,
    pub secondary: String,
}

impl Scope {
    pub fn new(track: Track, primary: &str, secondary: &str) -> Self {
        Self {
            track,
            primary: primary.trim().to_lowercase(),
            secondary: secondary.trim().to_lowercase(),
        }
    }

    pub fn key_secondary(&self) -> String {
        format!("{}|{}|{}", self.track.as_str(), self.primary, self.secondary)
    }

    pub fn key_primary(&self) -> String {
        format!("{}|{}|", self.track.as_str(), self.primary)
    }

    pub fn key_global(&self) -> String {
        format!("{}||", self.track.as_str())
    }

    fn write_keys(&self) -> Vec<String> {
        let mut keys = vec![self.key_global()];
        if !self.primary.is_empty() {
            keys.push(self.key_primary());
            if !self.secondary.is_empty() {
                keys.push(self.key_secondary());
            }
        }
        keys
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Welford {
    pub n: u64,
    pub mean: f64,
    pub m2: f64,
    #[serde(default)]
    pub ring: Vec<f64>,
}

impl Welford {
    pub fn push(&mut self, x: f64, ring_len: usize) {
        if !x.is_finite() {
            return;
        }
        self.n += 1;
        let d = x - self.mean;
        self.mean += d / self.n as f64;
        self.m2 += d * (x - self.mean);
        self.ring.push(x);
        if ring_len > 0 && self.ring.len() > ring_len {
            let excess = self.ring.len() - ring_len;
            self.ring.drain(0..excess);
        }
    }

    pub fn variance(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n - 1) as f64
        }
    }

    pub fn sd(&self) -> f64 {
        self.variance().max(0.0).sqrt()
    }

    pub fn recent_mean(&self) -> f64 {
        if self.ring.is_empty() {
            return self.mean;
        }
        self.ring.iter().sum::<f64>() / self.ring.len() as f64
    }

    pub fn drift_z(&self) -> f64 {
        let sd = self.sd();
        if sd <= 0.0 || self.ring.is_empty() {
            return 0.0;
        }
        (self.recent_mean() - self.mean) / sd
    }

    pub fn merge(&mut self, other: &Welford, ring_len: usize) {
        if other.n == 0 {
            return;
        }
        if self.n == 0 {
            *self = other.clone();
        } else {
            let na = self.n as f64;
            let nb = other.n as f64;
            let n = na + nb;
            let delta = other.mean - self.mean;
            self.mean += delta * (nb / n);
            self.m2 += other.m2 + delta * delta * (na * nb / n);
            self.n += other.n;
            self.ring.extend(other.ring.iter().cloned());
        }
        if ring_len > 0 && self.ring.len() > ring_len {
            let excess = self.ring.len() - ring_len;
            self.ring.drain(0..excess);
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DecayShape {
    pub n: usize,
    pub top1: f64,
    pub top2: f64,
    pub margin: f64,
    pub top_gap_ratio: f64,
    pub tail_flatness: f64,
    pub entropy_norm: f64,
}

pub fn decay_shape(scores: &[f32]) -> Option<DecayShape> {
    let mut v: Vec<f64> = scores.iter().filter(|s| s.is_finite()).map(|s| *s as f64).collect();
    if v.len() < 2 {
        return None;
    }
    v.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    let (top1, top2) = (v[0], v[1]);
    let margin = top1 - top2;
    let tail = &v[1..];
    let tail_median = tail[tail.len() / 2];
    let span = (top1 - tail_median).abs();
    let top_gap_ratio = if span > 1e-9 { (margin / span).clamp(0.0, 1.0) } else { 0.0 };
    let tail_mean = tail.iter().sum::<f64>() / tail.len() as f64;
    let tail_var = tail.iter().map(|x| (x - tail_mean).powi(2)).sum::<f64>() / tail.len() as f64;
    let full = (v[0] - v[n - 1]).abs();
    let tail_flatness = if full > 1e-9 { (tail_var.sqrt() / full).clamp(0.0, 1.0) } else { 1.0 };
    let sd = tail_var.sqrt();
    let entropy_norm = if sd <= 1e-9 {
        1.0
    } else {
        let exps: Vec<f64> = v.iter().map(|x| ((x - v[0]) / sd).exp()).collect();
        let s: f64 = exps.iter().sum::<f64>().max(1e-12);
        let h: f64 = exps.iter().map(|e| e / s).filter(|p| *p > 1e-12).map(|p| -p * p.ln()).sum();
        let hmax = (n as f64).ln();
        if hmax > 1e-9 {
            (h / hmax).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    Some(DecayShape {
        n,
        top1,
        top2,
        margin,
        top_gap_ratio,
        tail_flatness,
        entropy_norm,
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecayStat {
    pub margin: Welford,
    pub top_gap_ratio: Welford,
    pub tail_flatness: Welford,
    pub entropy_norm: Welford,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfusionStat {
    pub ties: u64,
    pub a_wins: u64,
    pub b_wins: u64,
    pub margin: Welford,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GateStat {
    pub evaluated: u64,
    pub blocked: u64,
    pub sole: u64,
    #[serde(default)]
    pub severity: Welford,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeStat {
    #[serde(default)]
    pub baseline: HashMap<String, Welford>,
    #[serde(default)]
    pub calibration: HashMap<String, Welford>,
    #[serde(default)]
    pub decay: HashMap<String, DecayStat>,
    #[serde(default)]
    pub confusion: HashMap<String, ConfusionStat>,
    #[serde(default)]
    pub agreement: HashMap<String, Welford>,
    #[serde(default)]
    pub transition: HashMap<String, u64>,
    #[serde(default)]
    pub gates: HashMap<String, GateStat>,
    pub updated_at: i64,
}

impl ScopeStat {
    pub fn absorb(&mut self, other: ScopeStat, ring: usize) {
        for (k, v) in other.baseline {
            self.baseline.entry(k).or_default().merge(&v, ring);
        }
        for (k, v) in other.calibration {
            self.calibration.entry(k).or_default().merge(&v, ring);
        }
        for (k, v) in other.decay {
            let d = self.decay.entry(k).or_default();
            d.margin.merge(&v.margin, ring);
            d.top_gap_ratio.merge(&v.top_gap_ratio, ring);
            d.tail_flatness.merge(&v.tail_flatness, ring);
            d.entropy_norm.merge(&v.entropy_norm, ring);
        }
        for (k, v) in other.confusion {
            let c = self.confusion.entry(k).or_default();
            c.ties += v.ties;
            c.a_wins += v.a_wins;
            c.b_wins += v.b_wins;
            c.margin.merge(&v.margin, ring);
        }
        for (k, v) in other.agreement {
            self.agreement.entry(k).or_default().merge(&v, ring);
        }
        for (k, v) in other.transition {
            *self.transition.entry(k).or_insert(0) += v;
        }
        for (k, v) in other.gates {
            let g = self.gates.entry(k).or_default();
            g.evaluated += v.evaluated;
            g.blocked += v.blocked;
            g.sole += v.sole;
            g.severity.merge(&v.severity, ring);
        }
        self.updated_at = now_ms();
    }

    fn observations(&self) -> u64 {
        self.baseline.values().map(|w| w.n).sum::<u64>()
            + self.calibration.values().map(|w| w.n).sum::<u64>()
            + self.confusion.values().map(|c| c.ties).sum::<u64>()
            + self.gates.values().map(|g| g.evaluated).sum::<u64>()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SdsFile {
    pub recipe: String,
    pub updated_at: i64,
    #[serde(default)]
    pub scopes: HashMap<String, ScopeStat>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RunFile {
    recipe: String,
    ledger: String,
    label: String,
    started_at: i64,
    updated_at: i64,
    observations: u64,
    scopes: HashMap<String, ScopeStat>,
}

#[derive(Debug, Clone, Default)]
struct RunBuf {
    label: String,
    file: String,
    started_ms: i64,
    observations: u64,
    scopes: HashMap<String, ScopeStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerStatus {
    pub ledger: String,
    pub scopes: usize,
    pub observations: u64,
    pub run_observations: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisStatus {
    pub scope: String,
    pub axis: String,
    pub kind: String,
    pub n: u64,
    pub mean: f64,
    pub sd: f64,
    pub recent: f64,
    pub drift_z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRate {
    pub scope: String,
    pub gate: String,
    pub evaluated: u64,
    pub blocked_rate: f64,
    pub sole_rate: f64,
    pub mean_severity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfusionStatus {
    pub scope: String,
    pub pair: String,
    pub ties: u64,
    pub a_wins: u64,
    pub b_wins: u64,
    pub mean_margin: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdsStatus {
    pub dir: String,
    pub recipe: String,
    pub ledgers: Vec<LedgerStatus>,
    pub axes: Vec<AxisStatus>,
    pub confusions: Vec<ConfusionStatus>,
    pub log: Vec<String>,
}

pub struct SdsStore {
    dir: PathBuf,
    bases: HashMap<Ledger, SdsFile>,
    runs: HashMap<Ledger, RunBuf>,
    dirty: HashMap<Ledger, bool>,
    write_tick: u32,
    last_flush: Option<std::time::Instant>,
    log: Vec<String>,
}

const AUTO_FLUSH_EVERY: u32 = 8;
const AUTO_FLUSH_MIN_GAP_MS: u128 = 5_000;
const CAP_BYTES: usize = 2 * 1024 * 1024;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn sanitize_label(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() {
        "run".into()
    } else {
        s.chars().take(40).collect()
    }
}

fn ring_for_key(key: &str) -> usize {
    let track = key.split('|').next().unwrap_or("");
    match track {
        "tool.image" => Track::ToolImage.ring_len(),
        "tool.geom" => Track::ToolGeom.ring_len(),
        "mold.geom" => Track::MoldGeom.ring_len(),
        "mold.image" => Track::MoldImage.ring_len(),
        "process" => Track::Process.ring_len(),
        "decision" => Track::Decision.ring_len(),
        "wear" => Track::Wear.ring_len(),
        _ => 6,
    }
}

impl SdsStore {
    pub fn open(dir: &Path) -> Self {
        let mut store = Self {
            dir: dir.to_path_buf(),
            bases: HashMap::new(),
            runs: HashMap::new(),
            dirty: HashMap::new(),
            write_tick: 0,
            last_flush: None,
            log: Vec::new(),
        };
        let _ = std::fs::create_dir_all(dir);
        let mut loaded = 0usize;
        for l in Ledger::all() {
            let path = dir.join(l.file_name());
            let mut fresh = SdsFile {
                recipe: SDS_RECIPE.into(),
                updated_at: now_ms(),
                scopes: HashMap::new(),
            };
            if path.exists() {
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<SdsFile>(&t).map_err(|e| e.to_string())) {
                    Ok(f) if f.recipe == SDS_RECIPE => {
                        loaded += 1;
                        fresh = f;
                    }
                    Ok(f) => store.log.push(format!(
                        "[SDS] [{}] 레시피 세대 불일치 ('{}' vs '{}') — 원장을 폐기하고 새로 시작합니다",
                        l.as_str(),
                        f.recipe,
                        SDS_RECIPE
                    )),
                    Err(e) => store.log.push(format!("[SDS] [{}] 원장 읽기/파싱 실패({}) — 새로 시작합니다", l.as_str(), e)),
                }
            }
            store.bases.insert(l, fresh);
        }
        store.log.push(if loaded == 0 {
            "[SDS] 저장된 원장이 없어 냉간 시작합니다 (보정 미개입, 물리 기본값 사용)".into()
        } else {
            format!("[SDS] 원장 {}종을 불러왔습니다", loaded)
        });
        store
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn open_run(&mut self, l: Ledger, label: &str) -> &mut RunBuf {
        let dir = self.dir.clone();
        self.runs.entry(l).or_insert_with(|| {
            let stamp = now_ms();
            let lab = sanitize_label(label);
            let mut file = format!("{}_{}.json", stamp, lab);
            let mut i = 2;
            while dir.join("runs").join(l.as_str()).join(&file).exists() {
                file = format!("{}_{}_{}.json", stamp, lab, i);
                i += 1;
            }
            RunBuf {
                label: lab,
                file,
                started_ms: stamp,
                observations: 0,
                scopes: HashMap::new(),
            }
        })
    }

    fn with_scopes<F: Fn(&mut ScopeStat, usize)>(&mut self, scope: &Scope, label: &str, f: F) {
        let ledger = scope.track.ledger();
        let ring = scope.track.ring_len();
        let keys = scope.write_keys();
        let run = self.open_run(ledger, label);
        run.observations = run.observations.saturating_add(1);
        for k in keys {
            let e = run.scopes.entry(k).or_default();
            f(e, ring);
            e.updated_at = now_ms();
        }
        self.dirty.insert(ledger, true);
        self.write_tick = self.write_tick.saturating_add(1);
        if self.write_tick >= AUTO_FLUSH_EVERY
            && self
                .last_flush
                .map(|t| t.elapsed().as_millis() >= AUTO_FLUSH_MIN_GAP_MS)
                .unwrap_or(true)
        {
            self.flush();
        }
    }

    pub fn record_baseline(&mut self, scope: &Scope, axis: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        let a = axis.to_string();
        self.with_scopes(scope, scope.track.as_str(), move |s, ring| {
            s.baseline.entry(a.clone()).or_default().push(value, ring);
        });
    }

    pub fn record_calibration(&mut self, scope: &Scope, axis: &str, measured: f64, predicted: f64) {
        if !measured.is_finite() || !predicted.is_finite() || predicted.abs() < 1e-9 {
            return;
        }
        let ratio = (measured / predicted).clamp(0.05, 20.0);
        let a = axis.to_string();
        self.with_scopes(scope, "calibration", move |s, ring| {
            s.calibration.entry(a.clone()).or_default().push(ratio, ring);
        });
    }

    pub fn record_decay(&mut self, scope: &Scope, axis: &str, scores: &[f32]) {
        let shape = match decay_shape(scores) {
            Some(s) => s,
            None => return,
        };
        let a = axis.to_string();
        self.with_scopes(scope, scope.track.as_str(), move |s, ring| {
            let d = s.decay.entry(a.clone()).or_default();
            d.margin.push(shape.margin, ring);
            d.top_gap_ratio.push(shape.top_gap_ratio, ring);
            d.tail_flatness.push(shape.tail_flatness, ring);
            d.entropy_norm.push(shape.entropy_norm, ring);
        });
    }

    pub fn record_confusion(&mut self, scope: &Scope, winner: &str, loser: &str, margin: f64) {
        if winner.is_empty() || loser.is_empty() || winner == loser {
            return;
        }
        let (a, b, a_won) = if winner <= loser {
            (winner.to_string(), loser.to_string(), true)
        } else {
            (loser.to_string(), winner.to_string(), false)
        };
        let key = format!("{}|{}", a, b);
        self.with_scopes(scope, "decision", move |s, ring| {
            let e = s.confusion.entry(key.clone()).or_default();
            e.ties += 1;
            if a_won {
                e.a_wins += 1;
            } else {
                e.b_wins += 1;
            }
            if margin.is_finite() {
                e.margin.push(margin, ring);
            }
        });
    }

    pub fn record_agreement(&mut self, scope: &Scope, axis: &str, agreed: bool) {
        let a = axis.to_string();
        self.with_scopes(scope, "decision", move |s, ring| {
            s.agreement.entry(a.clone()).or_default().push(if agreed { 1.0 } else { 0.0 }, ring);
        });
    }

    pub fn record_transition(&mut self, scope: &Scope, from: &str, to: &str) {
        if from.is_empty() || to.is_empty() {
            return;
        }
        let key = format!("{}>{}", from, to);
        self.with_scopes(scope, scope.track.as_str(), move |s, _| {
            *s.transition.entry(key.clone()).or_insert(0) += 1;
        });
    }

    pub fn record_gates(&mut self, scope: &Scope, items: &[(String, u8, bool, bool)]) {
        if items.is_empty() {
            return;
        }
        let list: Vec<(String, u8, bool, bool)> = items.to_vec();
        self.with_scopes(scope, "gates", move |s, ring| {
            for (id, sev, blocked, sole) in list.iter() {
                let g = s.gates.entry(id.clone()).or_default();
                g.evaluated += 1;
                if *blocked {
                    g.blocked += 1;
                }
                if *sole {
                    g.sole += 1;
                }
                g.severity.push(*sev as f64, ring);
            }
        });
    }

    pub fn gate_rates(&self, scope: &Scope) -> Vec<GateRate> {
        let l = scope.track.ledger();
        let mut out = Vec::new();
        for key in [scope.key_secondary(), scope.key_primary(), scope.key_global()] {
            if let Some(st) = self.merged_scope(l, &key) {
                if st.gates.is_empty() {
                    continue;
                }
                for (id, g) in st.gates.iter() {
                    let n = g.evaluated.max(1) as f64;
                    out.push(GateRate {
                        scope: key.clone(),
                        gate: id.clone(),
                        evaluated: g.evaluated,
                        blocked_rate: g.blocked as f64 / n,
                        sole_rate: g.sole as f64 / n,
                        mean_severity: g.severity.mean,
                    });
                }
                break;
            }
        }
        out.sort_by(|a, b| b.blocked_rate.partial_cmp(&a.blocked_rate).unwrap_or(std::cmp::Ordering::Equal).then(a.gate.cmp(&b.gate)));
        out
    }

    pub fn fusion_mse(&self, scope: &Scope, method: &str) -> Option<(f64, u64)> {
        self.adaptive_baseline(scope, &format!("fusion_mse:{}", method)).map(|(m, _, n)| (m, n))
    }

    pub fn axes_of(&self, scope: &Scope, prefix: &str) -> Vec<AxisStatus> {
        let l = scope.track.ledger();
        for key in [scope.key_secondary(), scope.key_primary(), scope.key_global()] {
            if let Some(st) = self.merged_scope(l, &key) {
                let mut v: Vec<AxisStatus> = st
                    .baseline
                    .iter()
                    .filter(|(a, _)| a.starts_with(prefix))
                    .map(|(a, w)| AxisStatus {
                        scope: key.clone(),
                        axis: a.clone(),
                        kind: "baseline".into(),
                        n: w.n,
                        mean: w.mean,
                        sd: w.sd(),
                        recent: w.recent_mean(),
                        drift_z: w.drift_z(),
                    })
                    .collect();
                if v.is_empty() {
                    continue;
                }
                v.sort_by(|a, b| a.axis.cmp(&b.axis));
                return v;
            }
        }
        Vec::new()
    }

    fn merged_scope(&self, l: Ledger, key: &str) -> Option<ScopeStat> {
        let base = self.bases.get(&l).and_then(|f| f.scopes.get(key).cloned());
        let run = self.runs.get(&l).and_then(|r| r.scopes.get(key).cloned());
        match (base, run) {
            (Some(mut b), Some(r)) => {
                b.absorb(r, ring_for_key(key));
                Some(b)
            }
            (Some(b), None) => Some(b),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        }
    }

    fn resolve<T, F: Fn(&ScopeStat) -> Option<(T, u64)>>(&self, scope: &Scope, pick: F) -> Option<(T, String)> {
        let (m2, m1, mg) = scope.track.min_obs();
        let l = scope.track.ledger();
        for (key, need) in [
            (scope.key_secondary(), m2),
            (scope.key_primary(), m1),
            (scope.key_global(), mg),
        ] {
            if need == 0 {
                continue;
            }
            if let Some(st) = self.merged_scope(l, &key) {
                if let Some((v, n)) = pick(&st) {
                    if n >= need {
                        return Some((v, key));
                    }
                }
            }
        }
        None
    }

    pub fn adaptive_baseline(&self, scope: &Scope, axis: &str) -> Option<(f64, f64, u64)> {
        self.resolve(scope, |st| st.baseline.get(axis).map(|w| ((w.mean, w.sd(), w.n), w.n))).map(|v| v.0)
    }

    pub fn adaptive_recent(&self, scope: &Scope, axis: &str) -> Option<(f64, f64, f64)> {
        self.resolve(scope, |st| st.baseline.get(axis).map(|w| ((w.recent_mean(), w.mean, w.drift_z()), w.n))).map(|v| v.0)
    }

    pub fn calibration_factor(&self, scope: &Scope, axis: &str) -> Option<(f64, u64, f64, String)> {
        self.resolve(scope, |st| {
            st.calibration.get(axis).map(|w| {
                let drift = w.drift_z();
                let center = if drift.abs() > 2.0 { w.recent_mean() } else { w.mean };
                let k = 5.0;
                let shrunk = 1.0 + (center - 1.0) * (w.n as f64 / (w.n as f64 + k));
                ((shrunk.clamp(0.25, 4.0), w.n, drift), w.n)
            })
        })
        .map(|((f, n, d), key)| (f, n, d, key))
    }

    pub fn transition_prior(&self, scope: &Scope, from: &str, to: &str) -> Option<f64> {
        let (m2, m1, _) = scope.track.min_obs();
        let l = scope.track.ledger();
        for (key, need) in [(scope.key_secondary(), m2), (scope.key_primary(), m1)] {
            let st = match self.merged_scope(l, &key) {
                Some(v) => v,
                None => continue,
            };
            let prefix = format!("{}>", from);
            let total: u64 = st.transition.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, v)| *v).sum();
            if total < need {
                continue;
            }
            let outs = st.transition.keys().filter(|k| k.starts_with(&prefix)).count().max(1) as f64;
            let hit = *st.transition.get(&format!("{}>{}", from, to)).unwrap_or(&0) as f64;
            return Some((hit + 1.0) / (total as f64 + outs + 1.0));
        }
        None
    }

    pub fn agreement_rate(&self, scope: &Scope, axis: &str) -> Option<(f64, u64)> {
        self.resolve(scope, |st| st.agreement.get(axis).map(|w| ((w.mean, w.n), w.n))).map(|v| v.0)
    }

    pub fn adaptive_decay(&self, scope: &Scope, axis: &str) -> Option<(f64, f64, u64)> {
        self.resolve(scope, |st| st.decay.get(axis).map(|d| ((d.margin.mean, d.margin.sd(), d.margin.n), d.margin.n)))
            .map(|v| v.0)
    }

    pub fn confusion_share(&self, scope: &Scope, a: &str, b: &str) -> Option<(f64, u64)> {
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        let key = format!("{}|{}", x, y);
        let total_of = |st: &ScopeStat| st.confusion.values().map(|c| c.ties).sum::<u64>();
        self.resolve(scope, |st| {
            let total = total_of(st);
            st.confusion.get(&key).map(|c| ((c.ties as f64 / total.max(1) as f64, c.ties), total))
        })
        .map(|v| v.0)
    }

    pub fn flush(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let open: Vec<Ledger> = self.runs.keys().cloned().collect();
        for l in Ledger::all() {
            let dirty = *self.dirty.get(&l).unwrap_or(&false);
            if !dirty && !open.contains(&l) {
                continue;
            }
            if let Some(run) = self.runs.get(&l) {
                let rf = RunFile {
                    recipe: SDS_RECIPE.into(),
                    ledger: l.as_str().into(),
                    label: run.label.clone(),
                    started_at: run.started_ms,
                    updated_at: now_ms(),
                    observations: run.observations,
                    scopes: run.scopes.clone(),
                };
                let dir = self.dir.join("runs").join(l.as_str());
                let _ = std::fs::create_dir_all(&dir);
                if let Ok(txt) = serde_json::to_string_pretty(&rf) {
                    match std::fs::write(dir.join(&run.file), txt) {
                        Ok(_) => lines.push(format!("{}={}건→{}", l.as_str(), run.observations, run.file)),
                        Err(e) => lines.push(format!("{} 실행 파일 저장 실패: {}", l.as_str(), e)),
                    }
                }
            }
            let mut snapshot = self.bases.get(&l).cloned().unwrap_or_default();
            snapshot.recipe = SDS_RECIPE.into();
            if let Some(run) = self.runs.get(&l) {
                for (k, v) in run.scopes.iter() {
                    snapshot.scopes.entry(k.clone()).or_default().absorb(v.clone(), ring_for_key(k));
                }
            }
            snapshot.updated_at = now_ms();
            let mut txt = serde_json::to_string_pretty(&snapshot).unwrap_or_default();
            if txt.len() > CAP_BYTES {
                let mut keys: Vec<(String, u64, i64)> = snapshot.scopes.iter().map(|(k, v)| (k.clone(), v.observations(), v.updated_at)).collect();
                keys.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
                for (k, _, _) in keys {
                    if txt.len() <= CAP_BYTES {
                        break;
                    }
                    snapshot.scopes.remove(&k);
                    txt = serde_json::to_string_pretty(&snapshot).unwrap_or(txt);
                }
                lines.push(format!("{} 상한 초과로 저관측 스코프 절삭 (잔존 {})", l.as_str(), snapshot.scopes.len()));
            }
            match std::fs::write(self.dir.join(l.file_name()), txt.as_bytes()) {
                Ok(_) => {
                    self.dirty.insert(l, false);
                    lines.push(format!("{}.json={}B", l.as_str(), txt.len()));
                }
                Err(e) => lines.push(format!("{} 원장 저장 실패: {}", l.as_str(), e)),
            }
        }
        self.write_tick = 0;
        self.last_flush = Some(std::time::Instant::now());
        if self.log.len() >= 200 {
            self.log.drain(0..100);
        }
        if !lines.is_empty() {
            self.log.push(format!("[SDS] 저장: {}", lines.join(" | ")));
        }
        lines
    }

    pub fn close_runs(&mut self) {
        self.flush();
        let runs: Vec<(Ledger, RunBuf)> = self.runs.drain().collect();
        for (l, run) in runs {
            let base = self.bases.entry(l).or_default();
            for (k, v) in run.scopes.into_iter() {
                let ring = ring_for_key(&k);
                base.scopes.entry(k).or_default().absorb(v, ring);
            }
            base.updated_at = now_ms();
            self.dirty.insert(l, true);
        }
        self.flush();
    }

    pub fn purge(&mut self) {
        self.bases.clear();
        self.runs.clear();
        self.dirty.clear();
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::create_dir_all(&self.dir);
        for l in Ledger::all() {
            self.bases.insert(
                l,
                SdsFile {
                    recipe: SDS_RECIPE.into(),
                    updated_at: now_ms(),
                    scopes: HashMap::new(),
                },
            );
        }
        self.log.push("[SDS] 원장·실행 파일을 전량 삭제했습니다. 판정은 물리 기본값으로 복귀합니다".into());
    }

    pub fn status(&self, max_axes: usize) -> SdsStatus {
        let mut ledgers = Vec::new();
        let mut axes = Vec::new();
        let mut confusions = Vec::new();
        for l in Ledger::all() {
            let base = self.bases.get(&l);
            let run = self.runs.get(&l);
            let mut keys: Vec<String> = base.map(|b| b.scopes.keys().cloned().collect()).unwrap_or_default();
            if let Some(r) = run {
                for k in r.scopes.keys() {
                    if !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
            let mut obs = 0u64;
            for k in keys.iter() {
                if let Some(st) = self.merged_scope(l, k) {
                    obs += st.observations();
                    for (a, w) in st.baseline.iter() {
                        axes.push(AxisStatus {
                            scope: k.clone(),
                            axis: a.clone(),
                            kind: "baseline".into(),
                            n: w.n,
                            mean: w.mean,
                            sd: w.sd(),
                            recent: w.recent_mean(),
                            drift_z: w.drift_z(),
                        });
                    }
                    for (a, w) in st.calibration.iter() {
                        axes.push(AxisStatus {
                            scope: k.clone(),
                            axis: a.clone(),
                            kind: "calibration".into(),
                            n: w.n,
                            mean: w.mean,
                            sd: w.sd(),
                            recent: w.recent_mean(),
                            drift_z: w.drift_z(),
                        });
                    }
                    for (a, d) in st.decay.iter() {
                        axes.push(AxisStatus {
                            scope: k.clone(),
                            axis: a.clone(),
                            kind: "decay_margin".into(),
                            n: d.margin.n,
                            mean: d.margin.mean,
                            sd: d.margin.sd(),
                            recent: d.margin.recent_mean(),
                            drift_z: d.margin.drift_z(),
                        });
                    }
                    for (a, w) in st.agreement.iter() {
                        axes.push(AxisStatus {
                            scope: k.clone(),
                            axis: a.clone(),
                            kind: "agreement".into(),
                            n: w.n,
                            mean: w.mean,
                            sd: w.sd(),
                            recent: w.recent_mean(),
                            drift_z: w.drift_z(),
                        });
                    }
                    for (g, gs) in st.gates.iter() {
                        let n = gs.evaluated.max(1) as f64;
                        axes.push(AxisStatus {
                            scope: k.clone(),
                            axis: g.clone(),
                            kind: "gate_block".into(),
                            n: gs.evaluated,
                            mean: gs.blocked as f64 / n,
                            sd: gs.severity.sd(),
                            recent: gs.sole as f64 / n,
                            drift_z: gs.severity.drift_z(),
                        });
                    }
                    for (pair, c) in st.confusion.iter() {
                        confusions.push(ConfusionStatus {
                            scope: k.clone(),
                            pair: pair.clone(),
                            ties: c.ties,
                            a_wins: c.a_wins,
                            b_wins: c.b_wins,
                            mean_margin: c.margin.mean,
                        });
                    }
                }
            }
            ledgers.push(LedgerStatus {
                ledger: l.as_str().into(),
                scopes: keys.len(),
                observations: obs,
                run_observations: run.map(|r| r.observations).unwrap_or(0),
            });
        }
        axes.sort_by(|a, b| b.n.cmp(&a.n));
        axes.truncate(max_axes);
        confusions.sort_by(|a, b| b.ties.cmp(&a.ties));
        confusions.truncate(max_axes);
        SdsStatus {
            dir: self.dir.display().to_string(),
            recipe: SDS_RECIPE.into(),
            ledgers,
            axes,
            confusions,
            log: self.log.iter().rev().take(30).cloned().collect(),
        }
    }
}

impl Drop for SdsStore {
    fn drop(&mut self) {
        if self.dirty.values().any(|d| *d) || !self.runs.is_empty() {
            self.flush();
        }
    }
}