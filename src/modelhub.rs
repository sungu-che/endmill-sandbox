use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
pub const MANIFEST: &str = ".endmill-download.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub key: String,
    pub label: String,
    pub repo: String,
    pub revision: String,
    pub dir: String,
    pub files: Vec<ModelFile>,
    pub used_for: String,
    pub fallback: Option<String>,
}

impl ModelSpec {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn url(&self, endpoint: &str, file: &str) -> String {
        format!("{}/{}/resolve/{}/{}", endpoint.trim_end_matches('/'), self.repo, self.revision, file)
    }
}

fn spec(key: &str, label: &str, repo: &str, revision: &str, dir: &str, files: &[(&str, u64)], used_for: &str, fallback: Option<&str>) -> ModelSpec {
    ModelSpec {
        key: key.into(),
        label: label.into(),
        repo: repo.into(),
        revision: revision.into(),
        dir: dir.into(),
        files: files.iter().map(|(p, s)| ModelFile { path: (*p).into(), size: *s }).collect(),
        used_for: used_for.into(),
        fallback: fallback.map(|s| s.into()),
    }
}

pub fn catalog() -> Vec<ModelSpec> {
    vec![
        spec(
            "siglip",
            "SigLIP2 large patch16-512 (영어)",
            "google/siglip2-large-patch16-512",
            "49488218e80259885f3be61d7a9455faf833b7a8",
            "siglip2-large-patch16-512",
            &[
                ("config.json", 537),
                ("preprocessor_config.json", 394),
                ("special_tokens_map.json", 636),
                ("tokenizer_config.json", 47_164),
                ("tokenizer.json", 34_363_039),
                ("model.safetensors", 3_529_350_088),
            ],
            "공구·금형 이미지 입력 (형상 벡터 · 제로샷 마모 유형)",
            None,
        ),
        spec(
            "ttm",
            "Granite TTM-R3 (context 52 → 16)",
            "ibm-granite/granite-timeseries-ttm-r3",
            "52-16-dec-52-r3",
            "granite-timeseries-ttm-r3/52-16-dec-52-r3",
            &[("config.json", 2_669), ("model.safetensors", 5_885_560)],
            "마모·부하 시계열 예측",
            Some("Holt 감쇠 추세 + 물리 사전식"),
        ),
        spec(
            "laya",
            "laya-typed-decisions (ModernBERT-large)",
            "convaiinnovations/laya-typed-decisions",
            "main",
            "laya-typed-decisions",
            &[
                ("rl_agent_config.json", 847),
                ("encoder/config.json", 2_084),
                ("tokenizer/tokenizer_config.json", 337),
                ("tokenizer/tokenizer.json", 3_583_228),
                ("model.safetensors", 842_609_220),
            ],
            "판단 조언 (상향만 허용)",
            Some("결정적 안전 게이트만 사용"),
        ),
    ]
}

pub fn spec_of(key: &str) -> Option<ModelSpec> {
    catalog().into_iter().find(|s| s.key == key)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadJob {
    pub key: String,
    pub state: String,
    pub done: u64,
    pub total: u64,
    pub speed_bps: f64,
    pub eta_s: Option<f64>,
    pub file: String,
    pub error: Option<String>,
    pub attempt: u32,
    pub started_ms: i64,
    pub updated_ms: i64,
}

impl DownloadJob {
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "queued" | "downloading" | "verifying")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallStatus {
    pub dir: String,
    pub installed: bool,
    pub partial_bytes: u64,
    pub on_disk_bytes: u64,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    repo: String,
    revision: String,
    files: Vec<ModelFile>,
    completed_ms: i64,
}

struct JobCtl {
    info: Arc<Mutex<DownloadJob>>,
    cancel: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

pub struct ModelHub {
    pub root: PathBuf,
    pub endpoint: String,
    pub token: Option<String>,
    jobs: Mutex<HashMap<String, JobCtl>>,
}

fn now_ms() -> i64 {
    crate::store::now_ms()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

pub fn part_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn install_status(root: &Path, spec: &ModelSpec) -> InstallStatus {
    let dir = root.join(&spec.dir);
    let manifest: Option<Manifest> = std::fs::read_to_string(dir.join(MANIFEST)).ok().and_then(|t| serde_json::from_str(&t).ok());
    let mut missing = Vec::new();
    let mut partial = 0u64;
    let mut on_disk = 0u64;
    for f in spec.files.iter() {
        let p = dir.join(&f.path);
        let expected = manifest
            .as_ref()
            .and_then(|m| m.files.iter().find(|x| x.path == f.path).map(|x| x.size))
            .unwrap_or(f.size);
        let len = file_len(&p);
        on_disk += len;
        let part = file_len(&part_path(&p));
        partial += part;
        on_disk += part;
        if !p.exists() || (expected > 0 && len != expected) {
            missing.push(f.path.clone());
        }
    }
    InstallStatus {
        dir: dir.display().to_string(),
        installed: missing.is_empty(),
        partial_bytes: partial,
        on_disk_bytes: on_disk,
        missing,
    }
}

impl ModelHub {
    pub fn new(root: &Path, endpoint: Option<&str>, token: Option<&str>) -> Self {
        Self {
            root: root.to_path_buf(),
            endpoint: endpoint.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_ENDPOINT).to_string(),
            token: token.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    pub fn configure(&mut self, endpoint: Option<&str>, token: Option<&str>) {
        self.endpoint = endpoint.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_ENDPOINT).to_string();
        self.token = token.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    }

    pub fn any_active(&self) -> bool {
        lock(&self.jobs).values().any(|j| lock(&j.info).active())
    }

    pub fn set_root(&mut self, root: &Path) -> Result<(), String> {
        if self.any_active() {
            return Err("다운로드가 진행 중이라 모델 폴더를 바꿀 수 없습니다. 먼저 다운로드를 중지하세요".into());
        }
        self.root = root.to_path_buf();
        lock(&self.jobs).clear();
        Ok(())
    }

    pub fn job(&self, key: &str) -> Option<DownloadJob> {
        lock(&self.jobs).get(key).map(|j| lock(&j.info).clone())
    }

    pub fn status(&self, key: &str) -> Option<InstallStatus> {
        spec_of(key).map(|s| install_status(&self.root, &s))
    }

    pub fn start(&self, key: &str) -> Result<DownloadJob, String> {
        let spec = spec_of(key).ok_or_else(|| format!("알 수 없는 모델: {}", key))?;
        let mut jobs = lock(&self.jobs);
        if let Some(j) = jobs.get(key) {
            let info = lock(&j.info).clone();
            if info.active() {
                return Ok(info);
            }
        }
        let st = install_status(&self.root, &spec);
        let info = Arc::new(Mutex::new(DownloadJob {
            key: key.into(),
            state: if st.installed { "done".into() } else { "queued".into() },
            done: 0,
            total: spec.total_bytes(),
            speed_bps: 0.0,
            eta_s: None,
            file: String::new(),
            error: None,
            attempt: 0,
            started_ms: now_ms(),
            updated_ms: now_ms(),
        }));
        if st.installed {
            let snapshot = lock(&info).clone();
            jobs.insert(key.into(), JobCtl { info, cancel: Arc::new(AtomicBool::new(false)), handle: None });
            return Ok(snapshot);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (root, endpoint, token) = (self.root.clone(), self.endpoint.clone(), self.token.clone());
        let (i2, c2) = (info.clone(), cancel.clone());
        let handle = std::thread::Builder::new()
            .name(format!("model-download-{}", key))
            .spawn(move || run_job(root, endpoint, token, spec, i2, c2))
            .map_err(|e| format!("다운로드 스레드를 만들 수 없습니다: {}", e))?;
        let snapshot = lock(&info).clone();
        jobs.insert(key.into(), JobCtl { info, cancel, handle: Some(handle) });
        Ok(snapshot)
    }

    pub fn cancel(&self, key: &str) -> Option<DownloadJob> {
        let jobs = lock(&self.jobs);
        let j = jobs.get(key)?;
        j.cancel.store(true, Ordering::SeqCst);
        let info = lock(&j.info).clone();
        Some(info)
    }

    pub fn wait(&self, key: &str, timeout: Duration) {
        let handle = {
            let mut jobs = lock(&self.jobs);
            jobs.get_mut(key).and_then(|j| j.handle.take())
        };
        if let Some(h) = handle {
            let start = Instant::now();
            while !h.is_finished() && start.elapsed() < timeout {
                std::thread::sleep(Duration::from_millis(50));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    pub fn delete(&self, key: &str) -> Result<u64, String> {
        let spec = spec_of(key).ok_or_else(|| format!("알 수 없는 모델: {}", key))?;
        if self.job(key).map(|j| j.active()).unwrap_or(false) {
            self.cancel(key);
            self.wait(key, Duration::from_secs(10));
        }
        let root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let dir = self.root.join(&spec.dir);
        if !dir.exists() {
            lock(&self.jobs).remove(key);
            return Ok(0);
        }
        let real = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        if !real.starts_with(&root) || real == root {
            return Err(format!("모델 폴더 밖의 경로는 삭제하지 않습니다: {}", real.display()));
        }
        let freed = dir_size(&real);
        std::fs::remove_dir_all(&real).map_err(|e| {
            format!(
                "{} 삭제 실패: {} (모델이 메모리에 로드되어 파일이 열려 있으면 먼저 언로드해야 합니다)",
                real.display(),
                e
            )
        })?;
        if let Some(parent) = real.parent() {
            if parent != root && parent.starts_with(&root) && std::fs::read_dir(parent).map(|mut d| d.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(parent);
            }
        }
        lock(&self.jobs).remove(key);
        Ok(freed)
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else {
                total += file_len(&path);
            }
        }
    }
    total
}

fn set_state(info: &Arc<Mutex<DownloadJob>>, f: impl FnOnce(&mut DownloadJob)) {
    let mut g = lock(info);
    f(&mut g);
    g.updated_ms = now_ms();
}

fn run_job(root: PathBuf, endpoint: String, token: Option<String>, spec: ModelSpec, info: Arc<Mutex<DownloadJob>>, cancel: Arc<AtomicBool>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            set_state(&info, |j| {
                j.state = "failed".into();
                j.error = Some(format!("비동기 런타임 생성 실패: {}", e));
            });
            return;
        }
    };
    set_state(&info, |j| j.state = "downloading".into());
    let result = rt.block_on(download_all(&root, &endpoint, token.as_deref(), &spec, &info, &cancel));
    match result {
        Ok(()) => set_state(&info, |j| {
            j.state = "done".into();
            j.done = j.total;
            j.eta_s = Some(0.0);
            j.speed_bps = 0.0;
            j.error = None;
        }),
        Err(e) if cancel.load(Ordering::SeqCst) => set_state(&info, |j| {
            j.state = "cancelled".into();
            j.speed_bps = 0.0;
            j.eta_s = None;
            j.error = Some(e);
        }),
        Err(e) => set_state(&info, |j| {
            j.state = "failed".into();
            j.speed_bps = 0.0;
            j.eta_s = None;
            j.error = Some(e);
        }),
    }
}

enum Fetch {
    Done,
    Retry(String),
    Fatal(String),
}

async fn download_all(root: &Path, endpoint: &str, token: Option<&str>, spec: &ModelSpec, info: &Arc<Mutex<DownloadJob>>, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let dir = root.join(&spec.dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("endmill-sandbox/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("HTTP 클라이언트 생성 실패: {}", e))?;
    let mut sizes: Vec<ModelFile> = spec.files.clone();
    let total: u64 = sizes.iter().map(|f| f.size).sum();
    set_state(info, |j| j.total = total);
    let mut completed: u64 = 0;
    for idx in 0..sizes.len() {
        let rel = sizes[idx].path.clone();
        let target = dir.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
        }
        if target.exists() && (sizes[idx].size == 0 || file_len(&target) == sizes[idx].size) {
            completed += file_len(&target);
            set_state(info, |j| j.done = completed);
            continue;
        }
        let mut attempt = 0u32;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("사용자가 다운로드를 중지했습니다 (받은 부분은 이어받기용으로 보관)".into());
            }
            attempt += 1;
            set_state(info, |j| {
                j.file = rel.clone();
                j.attempt = attempt;
            });
            match fetch_file(&client, &spec.url(endpoint, &rel), token, &target, &mut sizes[idx].size, completed, info, cancel).await {
                Fetch::Done => break,
                Fetch::Fatal(e) => return Err(e),
                Fetch::Retry(e) => {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("사용자가 다운로드를 중지했습니다 (받은 부분은 이어받기용으로 보관)".into());
                    }
                    if attempt >= 6 {
                        return Err(format!("{} 다운로드 실패 ({}회 재시도): {}", rel, attempt, e));
                    }
                    set_state(info, |j| j.error = Some(format!("재시도 {}/6: {}", attempt, e)));
                    let wait = Duration::from_secs(1u64 << attempt.min(4));
                    let start = Instant::now();
                    while start.elapsed() < wait {
                        if cancel.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
        completed += file_len(&target);
        let new_total: u64 = sizes.iter().map(|f| f.size).sum();
        set_state(info, |j| {
            j.done = completed;
            j.total = new_total.max(completed);
            j.error = None;
        });
    }
    set_state(info, |j| j.state = "verifying".into());
    for f in sizes.iter() {
        let p = dir.join(&f.path);
        let len = file_len(&p);
        if f.size > 0 && len != f.size {
            return Err(format!("{} 크기 불일치: {} / {} 바이트", f.path, len, f.size));
        }
    }
    let manifest = Manifest {
        repo: spec.repo.clone(),
        revision: spec.revision.clone(),
        files: sizes,
        completed_ms: now_ms(),
    };
    let _ = std::fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&manifest).unwrap_or_default());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn fetch_file(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    target: &Path,
    expected: &mut u64,
    completed: u64,
    info: &Arc<Mutex<DownloadJob>>,
    cancel: &Arc<AtomicBool>,
) -> Fetch {
    use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE};
    let part = part_path(target);
    let mut offset = file_len(&part);
    if *expected > 0 && offset > *expected {
        let _ = std::fs::remove_file(&part);
        offset = 0;
    }
    let mut req = client.get(url).header(ACCEPT_ENCODING, "identity");
    if offset > 0 {
        req = req.header(RANGE, format!("bytes={}-", offset));
    }
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Fetch::Retry(format!("연결 실패: {}", e)),
    };
    let status = resp.status().as_u16();
    let append = match status {
        206 => true,
        200 => {
            offset = 0;
            false
        }
        416 => {
            if *expected > 0 && offset == *expected {
                return match std::fs::rename(&part, target) {
                    Ok(()) => Fetch::Done,
                    Err(e) => Fetch::Fatal(format!("{}: {}", target.display(), e)),
                };
            }
            let _ = std::fs::remove_file(&part);
            return Fetch::Retry("서버가 이어받기 범위를 거부해 처음부터 다시 받습니다".into());
        }
        401 | 403 => return Fetch::Fatal(format!("접근 거부 (HTTP {}) — 게이트 모델이면 Hugging Face 토큰을 설정하세요: {}", status, url)),
        404 => return Fetch::Fatal(format!("파일을 찾을 수 없습니다 (HTTP 404): {}", url)),
        429 | 500..=599 => return Fetch::Retry(format!("서버 응답 HTTP {}", status)),
        other => return Fetch::Fatal(format!("예상하지 못한 응답 HTTP {}: {}", other, url)),
    };
    let remote_total = if append {
        resp.headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.trim().parse::<u64>().ok())
    } else {
        resp.content_length()
    };
    if let Some(t) = remote_total {
        if t > 0 {
            *expected = t;
        }
    }
    let file = if append {
        std::fs::OpenOptions::new().append(true).open(&part)
    } else {
        std::fs::File::create(&part)
    };
    let mut file = match file {
        Ok(f) => std::io::BufWriter::with_capacity(1 << 20, f),
        Err(e) => return Fetch::Fatal(format!("{}: {}", part.display(), e)),
    };
    let mut written = offset;
    let mut last_report = Instant::now();
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    let mut speed = 0.0f64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = file.flush();
            return Fetch::Retry("중지 요청".into());
        }
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = file.write_all(&chunk) {
                    return Fetch::Fatal(format!("{} 쓰기 실패 (디스크 공간 확인): {}", part.display(), e));
                }
                written += chunk.len() as u64;
                window_bytes += chunk.len() as u64;
                let el = window_start.elapsed().as_secs_f64();
                if el >= 1.0 {
                    let inst = window_bytes as f64 / el;
                    speed = if speed <= 0.0 { inst } else { 0.7 * speed + 0.3 * inst };
                    window_start = Instant::now();
                    window_bytes = 0;
                }
                if last_report.elapsed() >= Duration::from_millis(200) {
                    last_report = Instant::now();
                    let exp = *expected;
                    set_state(info, |j| {
                        j.done = completed + written;
                        j.speed_bps = speed;
                        let remaining = j.total.saturating_sub(j.done) as f64;
                        j.eta_s = if speed > 1.0 { Some(remaining / speed) } else { None };
                        if exp > 0 && j.total < completed + exp {
                            j.total = completed + exp;
                        }
                    });
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush();
                return Fetch::Retry(format!("전송 중단: {}", e));
            }
        }
    }
    if let Err(e) = file.flush() {
        return Fetch::Fatal(format!("{} 쓰기 실패: {}", part.display(), e));
    }
    drop(file);
    let len = file_len(&part);
    if *expected > 0 && len != *expected {
        return Fetch::Retry(format!("받은 크기 {} / 예상 {} 바이트", len, *expected));
    }
    match std::fs::rename(&part, target) {
        Ok(()) => Fetch::Done,
        Err(e) => Fetch::Fatal(format!("{}: {}", target.display(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_loader_layout() {
        let c = catalog();
        assert_eq!(c.len(), 3);
        let s = spec_of("siglip").unwrap();
        assert!(s.dir.contains("siglip") && s.files.iter().any(|f| f.path == "model.safetensors"));
        assert!(s.url(DEFAULT_ENDPOINT, "config.json").starts_with("https://huggingface.co/google/siglip2-large-patch16-512/resolve/"));
        let t = spec_of("ttm").unwrap();
        assert!(t.dir.starts_with("granite-timeseries-ttm-r3/") && t.files.iter().any(|f| f.path == "config.json"));
        let l = spec_of("laya").unwrap();
        assert!(l.files.iter().any(|f| f.path == "encoder/config.json") && l.files.iter().any(|f| f.path == "tokenizer/tokenizer.json"));
        assert!(s.total_bytes() > 3_500_000_000);
    }

    #[test]
    fn install_status_tracks_parts_and_manifest() {
        let root = std::env::temp_dir().join(format!("endmill-hub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut spec = spec_of("ttm").unwrap();
        spec.files = vec![ModelFile { path: "config.json".into(), size: 4 }, ModelFile { path: "model.safetensors".into(), size: 8 }];
        let dir = root.join(&spec.dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), b"{}\n\n").unwrap();
        std::fs::write(part_path(&dir.join("model.safetensors")), b"1234").unwrap();
        let st = install_status(&root, &spec);
        assert!(!st.installed);
        assert_eq!(st.partial_bytes, 4);
        assert_eq!(st.missing, vec!["model.safetensors".to_string()]);
        std::fs::rename(part_path(&dir.join("model.safetensors")), dir.join("model.safetensors")).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"12345678").unwrap();
        assert!(install_status(&root, &spec).installed);
        let _ = std::fs::remove_dir_all(&root);
    }
}
