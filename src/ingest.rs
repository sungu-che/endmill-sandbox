use crate::cutting::{CuttingConditions, WorkpieceMaterial};
use crate::gcode::ToolPathSegment;
use crate::loadsim::HeightmapView;
use crate::mold::{self, WallPoint};
use crate::profile::{EndMillMockupSetting, MachiningProfile, ToolNose};
use crate::spec::EndMillSpec;
use crate::timeseries::Series;
use crate::workpiece_setup::WorkpieceSetup;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocKind {
    ToolSpec,
    Workpiece,
    Profile,
    GCode,
    MeasurementLog,
    ToolImage,
    MoldImage,
    MoldDepth,
    MoldCsv,
    WallCsv,
    Unknown,
}

impl DocKind {
    pub fn from_key(k: &str) -> Option<Self> {
        Some(match k.trim().to_lowercase().as_str() {
            "tool_spec" | "tool" => DocKind::ToolSpec,
            "workpiece" => DocKind::Workpiece,
            "profile" => DocKind::Profile,
            "gcode" | "nc" => DocKind::GCode,
            "measurement" | "log" => DocKind::MeasurementLog,
            "tool_image" => DocKind::ToolImage,
            "mold_image" => DocKind::MoldImage,
            "mold_depth" | "depth" => DocKind::MoldDepth,
            "mold_csv" | "heightmap" => DocKind::MoldCsv,
            "wall_csv" | "cmm" => DocKind::WallCsv,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            DocKind::ToolSpec => "공구 사양서",
            DocKind::Workpiece => "피삭재/원소재",
            DocKind::Profile => "가공 프로필",
            DocKind::GCode => "NC 프로그램(G-code)",
            DocKind::MeasurementLog => "측정·가공 로그",
            DocKind::ToolImage => "공구 이미지",
            DocKind::MoldImage => "금형 표면 이미지",
            DocKind::MoldDepth => "금형 깊이맵 이미지",
            DocKind::MoldCsv => "금형 높이맵 CSV",
            DocKind::WallCsv => "벽면 CMM CSV",
            DocKind::Unknown => "알 수 없음",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldValue {
    pub value: serde_json::Value,
    pub unit: String,
    pub source: String,
    pub assumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub level: String,
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCodeSummary {
    pub lines: usize,
    pub moves: usize,
    pub rapids: usize,
    pub feeds: usize,
    pub arcs: usize,
    pub plunges: usize,
    pub units: String,
    pub spindle_rpm: Option<f64>,
    pub feed_min: f64,
    pub feed_max: f64,
    pub bounds: [f64; 6],
    pub tools: Vec<String>,
    pub ignored_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedDocument {
    pub name: String,
    pub kind: DocKind,
    pub kind_label: String,
    pub fields: BTreeMap<String, FieldValue>,
    pub tool: Option<EndMillMockupSetting>,
    pub material: Option<WorkpieceMaterial>,
    pub workpiece: Option<WorkpieceSetup>,
    pub profile: Option<MachiningProfile>,
    pub segments: Option<Vec<ToolPathSegment>>,
    pub gcode: Option<GCodeSummary>,
    pub series: Vec<Series>,
    pub heightmap: Option<HeightmapView>,
    pub wall_points: Option<Vec<WallPoint>>,
    pub image_bytes_len: usize,
    pub issues: Vec<Issue>,
    pub required: Vec<String>,
    pub missing: Vec<String>,
    pub completeness: f64,
}

impl NormalizedDocument {
    fn new(name: &str, kind: DocKind) -> Self {
        Self {
            name: name.into(),
            kind,
            kind_label: kind.label().into(),
            fields: BTreeMap::new(),
            tool: None,
            material: None,
            workpiece: None,
            profile: None,
            segments: None,
            gcode: None,
            series: Vec::new(),
            heightmap: None,
            wall_points: None,
            image_bytes_len: 0,
            issues: Vec::new(),
            required: Vec::new(),
            missing: Vec::new(),
            completeness: 0.0,
        }
    }

    fn issue(&mut self, level: &str, field: &str, message: impl Into<String>) {
        self.issues.push(Issue {
            level: level.into(),
            field: field.into(),
            message: message.into(),
        });
    }

    fn set(&mut self, key: &str, value: serde_json::Value, unit: &str, source: &str, assumed: bool) {
        self.fields.insert(
            key.into(),
            FieldValue {
                value,
                unit: unit.into(),
                source: source.into(),
                assumed,
            },
        );
    }

    fn finish(mut self, required: &[&str]) -> Self {
        self.required = required.iter().map(|s| s.to_string()).collect();
        self.missing = required
            .iter()
            .filter(|k| self.fields.get(**k).map(|f| f.assumed).unwrap_or(true))
            .map(|s| s.to_string())
            .collect();
        let present = required.len() - self.missing.len();
        let errors = self.issues.iter().filter(|i| i.level == "error").count();
        let base = if required.is_empty() { 1.0 } else { present as f64 / required.len() as f64 };
        self.completeness = (base - 0.15 * errors as f64).clamp(0.0, 1.0);
        self
    }
}

pub fn length_to_mm(v: f64, unit: &str) -> f64 {
    match unit.trim().to_lowercase().as_str() {
        "in" | "inch" | "inches" | "\"" | "″" => v * 25.4,
        "cm" => v * 10.0,
        "um" | "µm" | "μm" => v / 1000.0,
        "m" => v * 1000.0,
        _ => v,
    }
}

pub fn speed_to_m_min(v: f64, unit: &str) -> f64 {
    match unit.trim().to_lowercase().as_str() {
        "sfm" | "ft/min" | "fpm" => v * 0.3048,
        "m/s" => v * 60.0,
        _ => v,
    }
}

pub fn feed_to_mm_min(v: f64, unit: &str) -> f64 {
    match unit.trim().to_lowercase().as_str() {
        "ipm" | "in/min" => v * 25.4,
        "m/min" => v * 1000.0,
        _ => v,
    }
}

pub fn temp_to_c(v: f64, unit: &str) -> f64 {
    match unit.trim().to_lowercase().as_str() {
        "f" | "°f" | "degf" => (v - 32.0) * 5.0 / 9.0,
        "k" => v - 273.15,
        _ => v,
    }
}

pub fn pressure_to_bar(v: f64, unit: &str) -> f64 {
    match unit.trim().to_lowercase().as_str() {
        "psi" => v * 0.0689476,
        "mpa" => v * 10.0,
        "kpa" => v / 100.0,
        _ => v,
    }
}

fn interp(points: &[(f64, f64)], x: f64) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        if x <= w[1].0 {
            return w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / (w[1].0 - w[0].0);
        }
    }
    points[points.len() - 1].1
}

pub fn hardness_to_hrc(v: f64, scale: &str) -> f64 {
    match scale.trim().to_lowercase().as_str() {
        "hb" | "hbw" | "brinell" => interp(
            &[(150.0, 0.0), (200.0, 13.0), (250.0, 24.0), (300.0, 32.0), (350.0, 38.0), (400.0, 43.0), (450.0, 47.0), (500.0, 52.0), (600.0, 58.0)],
            v,
        ),
        "hv" | "vickers" => interp(
            &[(200.0, 10.0), (300.0, 30.0), (400.0, 41.0), (500.0, 49.0), (600.0, 55.0), (700.0, 60.0), (800.0, 64.0), (900.0, 67.0)],
            v,
        ),
        _ => v,
    }
}

pub struct MaterialParse {
    pub material: WorkpieceMaterial,
    pub hrc_assumed: bool,
    pub designation: String,
}

pub fn parse_material(raw: &str, hrc: Option<u8>) -> Result<MaterialParse, String> {
    let t = raw.trim().to_lowercase().replace([' ', '_', '-'], "");
    if t.is_empty() {
        return Err("소재가 비어 있습니다".into());
    }
    let alloy = |default_hrc: u8, name: &str| -> MaterialParse {
        MaterialParse {
            material: WorkpieceMaterial::AlloySteel {
                hardness_hrc: hrc.unwrap_or(default_hrc),
            },
            hrc_assumed: hrc.is_none(),
            designation: name.into(),
        }
    };
    let plain = |m: WorkpieceMaterial, name: &str| MaterialParse {
        material: m,
        hrc_assumed: false,
        designation: name.into(),
    };
    let keys_alloy: &[(&str, u8, &str)] = &[
        ("skd11", 60, "SKD11 (D2)"),
        ("d2", 60, "D2"),
        ("skd61", 50, "SKD61 (H13)"),
        ("h13", 50, "H13"),
        ("nak80", 40, "NAK80"),
        ("nak55", 40, "NAK55"),
        ("stavax", 52, "STAVAX"),
        ("hpm38", 31, "HPM38"),
        ("p20", 30, "P20"),
        ("scm440", 30, "SCM440 (4140)"),
        ("4140", 30, "4140"),
        ("4340", 32, "4340"),
        ("dc53", 60, "DC53"),
        ("alloysteel", 45, "합금강"),
        ("alloy", 45, "합금강"),
        ("합금강", 45, "합금강"),
        ("금형강", 52, "금형강"),
        ("hardened", 55, "경화강"),
        ("toolsteel", 55, "공구강"),
    ];
    for (k, h, name) in keys_alloy.iter() {
        if t.contains(k) {
            return Ok(alloy(*h, name));
        }
    }
    let table: &[(&[&str], WorkpieceMaterial, &str)] = &[
        (&["aluminum", "aluminium", "알루미늄", "a6061", "a7075", "a5052", "a2024", "6061", "7075", "5052", "2024", "adc12", "ac4c"], WorkpieceMaterial::Aluminum, "알루미늄 합금"),
        (&["stainless", "sus", "sts", "304", "316", "스테인리스", "스텐"], WorkpieceMaterial::StainlessSteel, "스테인리스"),
        (&["inconel", "in718", "in625", "718", "625", "인코넬"], WorkpieceMaterial::Inconel, "인코넬"),
        (&["waspaloy", "hastelloy", "rene", "superalloy", "수퍼알로이", "슈퍼알로이", "stellite"], WorkpieceMaterial::SuperAlloy, "수퍼알로이"),
        (&["titanium", "ti6al4v", "ti64", "gr5", "티타늄"], WorkpieceMaterial::Titanium, "티타늄"),
        (&["cfrp", "carbonfiber", "탄소섬유"], WorkpieceMaterial::CFRP, "CFRP"),
        (&["carbonsteel", "s45c", "s50c", "s55c", "sm45c", "1045", "1050", "c45", "ss400", "탄소강", "mildsteel"], WorkpieceMaterial::CarbonSteel, "탄소강"),
    ];
    for (keys, m, name) in table.iter() {
        if keys.iter().any(|k| t == *k || t.contains(k)) {
            return Ok(plain(m.clone(), name));
        }
    }
    if t == "al" {
        return Ok(plain(WorkpieceMaterial::Aluminum, "알루미늄 합금"));
    }
    if t == "ti" {
        return Ok(plain(WorkpieceMaterial::Titanium, "티타늄"));
    }
    Err(format!("지원하지 않는 소재 표기입니다: '{}'", raw))
}

pub fn material_from_key(key: &str, hrc: Option<u8>) -> Result<WorkpieceMaterial, String> {
    match key.trim() {
        "aluminum" => Ok(WorkpieceMaterial::Aluminum),
        "carbon_steel" => Ok(WorkpieceMaterial::CarbonSteel),
        "alloy_steel" => Ok(WorkpieceMaterial::AlloySteel {
            hardness_hrc: hrc.ok_or_else(|| "합금강/금형강은 경도(HRC)를 입력해야 합니다".to_string())?,
        }),
        "stainless" => Ok(WorkpieceMaterial::StainlessSteel),
        "titanium" => Ok(WorkpieceMaterial::Titanium),
        "inconel" => Ok(WorkpieceMaterial::Inconel),
        "superalloy" => Ok(WorkpieceMaterial::SuperAlloy),
        "cfrp" => Ok(WorkpieceMaterial::CFRP),
        other => parse_material(other, hrc).map(|p| p.material),
    }
}

fn num(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok()
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

pub fn parse_tool_text(name: &str, text: &str) -> NormalizedDocument {
    let mut doc = NormalizedDocument::new(name, DocKind::ToolSpec);
    let low = text.to_lowercase();
    let unit_of = |u: Option<regex::Match>| u.map(|m| m.as_str().to_string()).unwrap_or_default();
    let mut diameter = None;
    for p in [
        r"(?:ø|φ|⌀)\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
        r"(?:\bdia(?:meter)?|직경|외경|날경|공구경)\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
        r"\bd\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?\b",
    ] {
        if let Some(c) = re(p).captures(&low) {
            if let Some(v) = c.get(1).and_then(|m| num(m.as_str())) {
                let u = unit_of(c.get(2));
                diameter = Some(length_to_mm(v, &u));
                doc.set("diameter_mm", serde_json::json!(length_to_mm(v, &u)), "mm", c.get(0).unwrap().as_str(), false);
                break;
            }
        }
    }
    let mut flutes = None;
    for p in [
        r"(\d+)\s*(?:f\b|fl\b|flutes?\b|날\b|날수)",
        r"(?:z|날\s*수|flutes?)\s*[:=]\s*(\d+)",
        r"(\d+)\s*날",
    ] {
        if let Some(c) = re(p).captures(&low) {
            if let Some(v) = c.get(1).and_then(|m| m.as_str().parse::<u8>().ok()) {
                flutes = Some(v);
                doc.set("flutes", serde_json::json!(v), "", c.get(0).unwrap().as_str(), false);
                break;
            }
        }
    }
    let grab = |doc: &mut NormalizedDocument, key: &str, pats: &[&str]| -> Option<f64> {
        for p in pats {
            if let Some(c) = re(p).captures(&low) {
                if let Some(v) = c.get(1).and_then(|m| num(m.as_str())) {
                    let u = unit_of(c.get(2));
                    let mm = length_to_mm(v, &u);
                    doc.set(key, serde_json::json!(mm), "mm", c.get(0).unwrap().as_str(), false);
                    return Some(mm);
                }
            }
        }
        None
    };
    let loc = grab(&mut doc, "loc_mm", &[
        r"(?:\bloc\b|\bl1\b|length\s+of\s+cut|cutting\s+length|flute\s+length|유효\s*장|날\s*장|절삭\s*장|유효\s*절삭\s*길이)\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
    ]);
    let oal = grab(&mut doc, "oal_mm", &[
        r"(?:\boal\b|overall\s+length|total\s+length|전\s*장|전체\s*길이|전장)\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
        r"\bl\s*[:=]\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
    ]);
    let shank = grab(&mut doc, "shank_mm", &[
        r"(?:shank|\bd1\b|\bds\b|샹크|생크)\s*(?:dia(?:meter)?|직경|경)?\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
    ]);
    let stickout = grab(&mut doc, "stickout_mm", &[
        r"(?:stick\s*out|돌출\s*(?:길이|량)?|overhang)\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(mm|in|inch|\x22)?",
    ]);
    let mut helix = None;
    for p in [r"(?:helix|헬릭스|나선각|비틀림각)\s*(?:angle)?\s*[:=]?\s*(\d+(?:[.,]\d+)?)", r"(\d+(?:[.,]\d+)?)\s*°\s*(?:helix|헬릭스)"] {
        if let Some(c) = re(p).captures(&low) {
            if let Some(v) = c.get(1).and_then(|m| num(m.as_str())) {
                helix = Some(v);
                doc.set("helix_deg", serde_json::json!(v), "deg", c.get(0).unwrap().as_str(), false);
                break;
            }
        }
    }
    let ball = re(r"\bball\b|볼\s*노즈|볼엔드|\bbn\b|ball\s*nose").is_match(&low);
    let mut nose = ToolNose::Square;
    if ball {
        nose = ToolNose::Ball;
        doc.set("nose", serde_json::json!("ball"), "", "ball", false);
    } else if let Some(c) = re(r"(?:corner\s*(?:radius|r)|코너\s*r|\bcr|\br)\s*[:=]?\s*(\d+(?:[.,]\d+)?)").captures(&low) {
        if let Some(v) = c.get(1).and_then(|m| num(m.as_str())) {
            if v > 0.0 {
                nose = ToolNose::CornerRadius { radius_mm: v };
                doc.set("nose", serde_json::json!(format!("corner_r {}", v)), "mm", c.get(0).unwrap().as_str(), false);
            }
        }
    }
    if !doc.fields.contains_key("nose") {
        doc.set("nose", serde_json::json!("square"), "", "", true);
    }
    let coatings = ["naco", "altin", "tialn", "alcrn", "tisin", "ticn", "tin", "crn", "zrn", "dlc", "diamond", "hipims"];
    let coating = coatings
        .iter()
        .find(|c| re(&format!(r"\b{}\b", c)).is_match(&low))
        .map(|c| c.to_string());
    if let Some(c) = &coating {
        doc.set("coating", serde_json::json!(c), "", c, false);
    }
    let high = re(r"high[\s-]*end|premium|고급|하이엔드").is_match(&low);
    let low_end = re(r"low[\s-]*end|economy|저가|보급").is_match(&low);
    let model = re(r"(?:model|품번|모델)\s*[:=]\s*([^\n,;]+)")
        .captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .unwrap_or_else(|| text.lines().next().unwrap_or(name).trim().chars().take(40).collect());
    let d = match diameter {
        Some(d) => d,
        None => {
            doc.issue("error", "diameter_mm", "공구 직경을 찾지 못했습니다 (예: Ø10, D10, 직경 10)");
            return doc.finish(&["diameter_mm", "flutes", "loc_mm", "oal_mm", "shank_mm", "helix_deg"]);
        }
    };
    let z = flutes.unwrap_or_else(|| {
        doc.issue("warn", "flutes", "날 수가 없어 4날로 가정했습니다");
        doc.set("flutes", serde_json::json!(4), "", "", true);
        4
    });
    let loc_v = loc.unwrap_or_else(|| {
        let v = 2.5 * d;
        doc.issue("warn", "loc_mm", format!("유효 절삭 길이가 없어 2.5D = {:.1} mm 로 가정했습니다", v));
        doc.set("loc_mm", serde_json::json!(v), "mm", "", true);
        v
    });
    let shank_v = shank.unwrap_or_else(|| {
        doc.issue("info", "shank_mm", "샹크 직경이 없어 공구 직경과 같다고 가정했습니다");
        doc.set("shank_mm", serde_json::json!(d), "mm", "", true);
        d
    });
    let oal_v = oal.unwrap_or_else(|| {
        let v = loc_v + 4.0 * shank_v;
        doc.issue("warn", "oal_mm", format!("전장이 없어 LOC + 4Ds = {:.1} mm 로 가정했습니다", v));
        doc.set("oal_mm", serde_json::json!(v), "mm", "", true);
        v
    });
    let helix_v = helix.unwrap_or_else(|| {
        doc.issue("info", "helix_deg", "헬릭스 각이 없어 35° 로 가정했습니다");
        doc.set("helix_deg", serde_json::json!(35.0), "deg", "", true);
        35.0
    });
    let mut tool = EndMillMockupSetting::custom(
        model.clone(),
        model,
        d,
        z,
        loc_v,
        oal_v,
        shank_v,
        helix_v,
        coating.clone(),
        high || !low_end,
    )
    .with_nose(nose)
    .with_stickout(stickout);
    if let Some(r) = re(r"(?:runout|런아웃|흔들림)\s*[:=]?\s*(\d+(?:[.,]\d+)?)\s*(um|µm|μm|mm)?").captures(&low) {
        if let Some(v) = r.get(1).and_then(|m| num(m.as_str())) {
            let um = if r.get(2).map(|m| m.as_str() == "mm").unwrap_or(false) { v * 1000.0 } else { v };
            tool.runout_um = Some(um);
            doc.set("runout_um", serde_json::json!(um), "µm", r.get(0).unwrap().as_str(), false);
        }
    }
    if let Err(e) = tool.validate() {
        doc.issue("error", "tool", e);
    }
    if oal_v < loc_v + shank_v {
        doc.issue("warn", "oal_mm", "전장이 LOC + 샹크 직경보다 짧아 척킹 길이가 부족할 수 있습니다");
    }
    if coating.is_none() {
        doc.issue("info", "coating", "코팅 표기가 없어 마찰·내열 특성을 무코팅으로 계산합니다");
    }
    doc.tool = Some(tool);
    doc.finish(&["diameter_mm", "flutes", "loc_mm", "oal_mm", "shank_mm", "helix_deg", "nose"])
}

pub fn parse_gcode(name: &str, text: &str) -> NormalizedDocument {
    let mut doc = NormalizedDocument::new(name, DocKind::GCode);
    let mut segs: Vec<ToolPathSegment> = Vec::new();
    let (mut x, mut y, mut z) = (0.0f64, 0.0f64, 0.0f64);
    let mut have_pos = false;
    let mut modal = 0u8;
    let mut abs = true;
    let mut inch = false;
    let mut feed = 0.0f64;
    let mut rpm: Option<f64> = None;
    let mut ignored: Vec<String> = Vec::new();
    let mut tools: Vec<String> = Vec::new();
    let mut plane_warned = false;
    let mut units_seen = false;
    let word = re(r"([A-Za-z])\s*([-+]?\d*\.?\d+)");
    let mut lines = 0usize;
    let (mut bmin, mut bmax) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let (mut fmin, mut fmax) = (f64::INFINITY, 0.0f64);
    for raw in text.lines() {
        let mut line = raw.to_string();
        while let (Some(a), Some(b)) = (line.find('('), line.find(')')) {
            if b > a {
                line.replace_range(a..=b, " ");
            } else {
                break;
            }
        }
        if let Some(p) = line.find(';') {
            line.truncate(p);
        }
        let line = line.trim();
        if line.is_empty() || line == "%" {
            continue;
        }
        lines += 1;
        let mut words: Vec<(char, f64)> = Vec::new();
        for c in word.captures_iter(line) {
            let l = c[1].chars().next().unwrap().to_ascii_uppercase();
            if let Ok(v) = c[2].parse::<f64>() {
                words.push((l, v));
            }
        }
        let mut tx: Option<f64> = None;
        let mut ty: Option<f64> = None;
        let mut tz: Option<f64> = None;
        let mut ii: Option<f64> = None;
        let mut jj: Option<f64> = None;
        let mut rr: Option<f64> = None;
        let mut motion_word = false;
        for (l, v) in words.iter() {
            match l {
                'G' => {
                    let g = (*v * 10.0).round() as i64;
                    match g {
                        0 => {
                            modal = 0;
                            motion_word = true;
                        }
                        10 => {
                            modal = 1;
                            motion_word = true;
                        }
                        20 => {
                            modal = 2;
                            motion_word = true;
                        }
                        30 => {
                            modal = 3;
                            motion_word = true;
                        }
                        900 => abs = true,
                        910 => abs = false,
                        200 => {
                            inch = true;
                            units_seen = true;
                        }
                        210 => {
                            inch = false;
                            units_seen = true;
                        }
                        170 | 940 | 400 | 490 | 800 | 430 | 440 | 540 | 550 | 560 | 570 | 580 | 590 | 280 | 530 => {}
                        180 | 190 => {
                            if !plane_warned {
                                doc.issue("warn", "plane", "G18/G19 평면 원호는 XY 근사로 처리됩니다");
                                plane_warned = true;
                            }
                        }
                        other => {
                            let s = format!("G{}", other as f64 / 10.0);
                            if !ignored.contains(&s) {
                                ignored.push(s);
                            }
                        }
                    }
                }
                'X' => tx = Some(*v),
                'Y' => ty = Some(*v),
                'Z' => tz = Some(*v),
                'I' => ii = Some(*v),
                'J' => jj = Some(*v),
                'R' => rr = Some(*v),
                'F' => feed = *v,
                'S' => rpm = Some(*v),
                'T' => {
                    let s = format!("T{}", *v as i64);
                    if !tools.contains(&s) {
                        tools.push(s);
                    }
                }
                _ => {}
            }
        }
        let k = if inch { 25.4 } else { 1.0 };
        if tx.is_none() && ty.is_none() && tz.is_none() {
            let _ = motion_word;
            continue;
        }
        let nx = tx.map(|v| if abs { v * k } else { x + v * k }).unwrap_or(x);
        let ny = ty.map(|v| if abs { v * k } else { y + v * k }).unwrap_or(y);
        let nz = tz.map(|v| if abs { v * k } else { z + v * k }).unwrap_or(z);
        let f = feed * k;
        if modal != 0 {
            fmin = fmin.min(f);
            fmax = fmax.max(f);
            if f <= 0.0 {
                doc.issue("error", "feed", format!("이송(F) 없이 절삭 이동이 있습니다: '{}'", line));
            }
        }
        let seg = match modal {
            0 => ToolPathSegment::Rapid { x: nx, y: ny, z: nz },
            1 => {
                if (nx - x).abs() < 1e-9 && (ny - y).abs() < 1e-9 && nz < z {
                    ToolPathSegment::Plunge { x: nx, y: ny, z_start: z, z_end: nz, feed: f }
                } else {
                    ToolPathSegment::Linear { x: nx, y: ny, z: nz, feed: f }
                }
            }
            m => {
                let (ci, cj) = match (ii, jj, rr) {
                    (Some(_), _, _) | (_, Some(_), _) => (ii.unwrap_or(0.0) * k, jj.unwrap_or(0.0) * k),
                    (None, None, Some(r)) => {
                        let r = r * k;
                        let (dx, dy) = (nx - x, ny - y);
                        let d = (dx * dx + dy * dy).sqrt();
                        if d < 1e-9 || d > 2.0 * r.abs() + 1e-6 {
                            doc.issue("error", "arc", format!("R 원호 반경이 현 길이보다 작습니다: '{}'", line));
                            (0.0, 0.0)
                        } else {
                            let h = (r * r - d * d / 4.0).max(0.0).sqrt();
                            let (mx, my) = (dx / 2.0, dy / 2.0);
                            let (px, py) = (-dy / d, dx / d);
                            let cw = m == 2;
                            let sign = if (r > 0.0) ^ cw { 1.0 } else { -1.0 };
                            (mx + sign * h * px, my + sign * h * py)
                        }
                    }
                    _ => {
                        doc.issue("error", "arc", format!("원호에 I/J/R 이 없습니다: '{}'", line));
                        (0.0, 0.0)
                    }
                };
                if m == 2 {
                    ToolPathSegment::ArcCW { x: nx, y: ny, z: nz, i: ci, j: cj, feed: f }
                } else {
                    ToolPathSegment::ArcCCW { x: nx, y: ny, z: nz, i: ci, j: cj, feed: f }
                }
            }
        };
        if !have_pos && modal != 0 {
            doc.issue("warn", "start", "첫 절삭 이동 전에 위치가 지정되지 않아 원점(0,0,0)에서 시작한다고 가정했습니다");
        }
        have_pos = true;
        for (i, v) in [nx, ny, nz].iter().enumerate() {
            bmin[i] = bmin[i].min(*v);
            bmax[i] = bmax[i].max(*v);
        }
        segs.push(seg);
        x = nx;
        y = ny;
        z = nz;
    }
    if !units_seen {
        doc.issue("warn", "units", "G20/G21 단위 지정이 없어 mm(G21)로 가정했습니다");
    }
    if segs.is_empty() {
        doc.issue("error", "moves", "이동 명령이 없습니다");
    }
    let summary = GCodeSummary {
        lines,
        moves: segs.len(),
        rapids: segs.iter().filter(|s| matches!(s, ToolPathSegment::Rapid { .. })).count(),
        feeds: segs.iter().filter(|s| matches!(s, ToolPathSegment::Linear { .. })).count(),
        arcs: segs.iter().filter(|s| matches!(s, ToolPathSegment::ArcCW { .. } | ToolPathSegment::ArcCCW { .. })).count(),
        plunges: segs.iter().filter(|s| matches!(s, ToolPathSegment::Plunge { .. })).count(),
        units: if inch { "inch→mm".into() } else { "mm".into() },
        spindle_rpm: rpm,
        feed_min: if fmin.is_finite() { fmin } else { 0.0 },
        feed_max: fmax,
        bounds: [bmin[0], bmin[1], bmin[2], bmax[0], bmax[1], bmax[2]],
        tools,
        ignored_codes: ignored.clone(),
    };
    if !ignored.is_empty() {
        doc.issue("info", "codes", format!("해석하지 않은 코드: {}", ignored.join(", ")));
    }
    if let Some(r) = rpm {
        doc.set("spindle_rpm", serde_json::json!(r), "rpm", "S", false);
    } else {
        doc.issue("warn", "spindle_rpm", "S(회전수) 지정이 없어 프로필의 회전수를 사용합니다");
    }
    doc.set("moves", serde_json::json!(summary.moves), "", "", false);
    doc.set("feed_max", serde_json::json!(summary.feed_max), "mm/min", "F", summary.feed_max <= 0.0);
    doc.set("units", serde_json::json!(summary.units.clone()), "", "", !units_seen);
    doc.gcode = Some(summary);
    doc.segments = Some(segs);
    doc.finish(&["moves", "feed_max", "units", "spindle_rpm"])
}

fn header_kind(h: &str) -> Option<(&'static str, &'static str)> {
    let t = h.to_lowercase();
    let tokens: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).collect();
    let checks: &[(&[&str], &[&str], &str, &str)] = &[
        (&["vb", "vbmax"], &["flank", "플랭크", "마모폭"], "vb_mm", "mm"),
        (&[], &["radial", "반경손실", "dia_loss", "diameter loss", "직경감소"], "radial_loss_um", "µm"),
        (&[], &["wear", "마모"], "wear", ""),
        (&["dev", "err"], &["deviation", "편차", "error", "오차"], "deviation_um", "µm"),
        (&["load"], &["부하", "current", "전류"], "spindle_load", "%"),
        (&["kw"], &["power", "동력"], "spindle_power_kw", "kW"),
        (&["temp"], &["temperature", "온도"], "temperature_c", "°C"),
        (&["vib"], &["vibration", "진동", "accel"], "vibration", ""),
        (&["ra", "rz"], &["rough", "조도"], "roughness_um", "µm"),
        (&["dia"], &["diameter", "직경"], "diameter_mm", "mm"),
    ];
    for (toks, subs, name, unit) in checks.iter() {
        if toks.iter().any(|k| tokens.iter().any(|x| x == k)) || subs.iter().any(|k| t.contains(k)) {
            return Some((name, unit));
        }
    }
    None
}

fn time_kind(h: &str) -> Option<(f64, &'static str)> {
    let t = h.to_lowercase();
    if t.contains("part") || t.contains("부품") || t.contains("index") || t == "no" || t.contains("순번") || t.contains("count") || t.contains("개수") {
        return Some((1.0, "part"));
    }
    if t.contains("min") || t.contains("분") {
        return Some((60.0, "s"));
    }
    if t.contains("hour") || t.contains("시간(h") || t == "h" {
        return Some((3600.0, "s"));
    }
    if t == "t" || t.contains("time") || t.contains("sec") || t.contains("시간") || t.contains("초") {
        return Some((1.0, "s"));
    }
    None
}

pub fn parse_measurement_csv(name: &str, text: &str) -> NormalizedDocument {
    let mut doc = NormalizedDocument::new(name, DocKind::MeasurementLog);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).collect();
    if lines.len() < 2 {
        doc.issue("error", "rows", "헤더와 데이터 행이 필요합니다");
        return doc.finish(&["time", "metric"]);
    }
    let sep = if lines[0].contains('\t') {
        '\t'
    } else if lines[0].contains(';') {
        ';'
    } else {
        ','
    };
    let header: Vec<String> = lines[0].split(sep).map(|h| h.trim().trim_matches('"').to_string()).collect();
    let time_col = header.iter().position(|h| time_kind(h).is_some());
    let mut metric_cols: Vec<(usize, String, String)> = Vec::new();
    for (i, h) in header.iter().enumerate() {
        if Some(i) == time_col {
            continue;
        }
        if let Some((k, u)) = header_kind(h) {
            let unit = re(r"[\(\[]([^\)\]]+)[\)\]]")
                .captures(h)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| u.to_string());
            metric_cols.push((i, k.to_string(), unit));
        }
    }
    if metric_cols.is_empty() {
        doc.issue("error", "metric", "측정 항목 열을 인식하지 못했습니다 (VB, 마모, 편차, 부하, 동력, 온도 등)");
        return doc.finish(&["time", "metric"]);
    }
    let (scale, t_unit) = time_col.and_then(|c| time_kind(&header[c])).unwrap_or((1.0, "row"));
    if time_col.is_none() {
        doc.issue("warn", "time", "시간/순번 열이 없어 행 순서를 시간축으로 사용합니다");
        doc.set("time", serde_json::json!("row_index"), "", "", true);
    } else {
        doc.set("time", serde_json::json!(header[time_col.unwrap()].clone()), "", "", false);
    }
    let canonical_unit = |k: &str, u: &str| -> String {
        match k {
            "vb_mm" | "diameter_mm" => "mm".into(),
            "radial_loss_um" | "deviation_um" | "roughness_um" => "µm".into(),
            "temperature_c" => "°C".into(),
            "spindle_power_kw" => "kW".into(),
            _ => u.to_string(),
        }
    };
    let mut series: Vec<Series> = metric_cols
        .iter()
        .map(|(_, k, u)| Series::new(k, &canonical_unit(k, u)).with_time_unit(t_unit))
        .collect();
    let mut bad = 0usize;
    for (ri, line) in lines[1..].iter().enumerate() {
        let cells: Vec<&str> = line.split(sep).collect();
        let t = match time_col {
            Some(c) => match cells.get(c).and_then(|v| num(v.trim().trim_matches('"'))) {
                Some(v) => v * scale,
                None => {
                    bad += 1;
                    continue;
                }
            },
            None => ri as f64,
        };
        for (si, (c, k, u)) in metric_cols.iter().enumerate() {
            if let Some(mut v) = cells.get(*c).and_then(|v| num(v.trim().trim_matches('"'))) {
                if k == "temperature_c" {
                    v = temp_to_c(v, u);
                }
                if k == "vb_mm" && (u.contains("um") || u.contains("µm")) {
                    v /= 1000.0;
                }
                if (k == "deviation_um" || k == "radial_loss_um" || k == "roughness_um") && u.trim() == "mm" {
                    v *= 1000.0;
                }
                if k == "spindle_power_kw" && u.trim().eq_ignore_ascii_case("w") {
                    v /= 1000.0;
                }
                series[si].push(t, v);
            }
        }
    }
    if bad > 0 {
        doc.issue("warn", "time", format!("시간 값을 해석할 수 없는 {}행을 제외했습니다", bad));
    }
    for s in series.iter() {
        doc.set(&format!("series.{}", s.key), serde_json::json!(s.len()), &s.unit, "", false);
        if s.len() < 8 {
            doc.issue("info", &s.key, format!("{} 점 — TTM 최소 이력(8점) 미만이라 통계 예측을 사용합니다", s.len()));
        }
    }
    doc.set("metric", serde_json::json!(series.iter().map(|s| s.key.clone()).collect::<Vec<_>>()), "", "", false);
    doc.series = series;
    doc.finish(&["time", "metric"])
}

fn check_conditions(doc: &mut NormalizedDocument, c: &CuttingConditions, d: f64, z: u8) {
    let vc = std::f64::consts::PI * d * c.spindle_rpm as f64 / 1000.0;
    if c.cutting_speed_m_min > 0.0 && (vc - c.cutting_speed_m_min).abs() / c.cutting_speed_m_min > 0.05 {
        doc.issue(
            "warn",
            "conditions.cutting_speed_m_min",
            format!("절삭속도 {:.1} 과 회전수로 계산한 {:.1} m/min 이 5% 넘게 다릅니다", c.cutting_speed_m_min, vc),
        );
    }
    let vf = c.spindle_rpm as f64 * z as f64 * c.feed_per_tooth_mm;
    if c.feed_rate_mm_min > 0.0 && (vf - c.feed_rate_mm_min).abs() / c.feed_rate_mm_min > 0.05 {
        doc.issue(
            "warn",
            "conditions.feed_rate_mm_min",
            format!("이송 {:.0} 과 n·z·fz = {:.0} mm/min 이 5% 넘게 다릅니다", c.feed_rate_mm_min, vf),
        );
    }
    if c.radial_doc_mm > d {
        doc.issue("error", "conditions.radial_doc_mm", "반경 절입이 공구 직경보다 큽니다");
    }
}

pub fn parse_json(name: &str, text: &str) -> NormalizedDocument {
    if let Ok(p) = serde_json::from_str::<MachiningProfile>(text) {
        let mut doc = NormalizedDocument::new(name, DocKind::Profile);
        if let Err(e) = p.endmill_setting.validate() {
            doc.issue("error", "endmill_setting", e);
        }
        check_conditions(&mut doc, &p.conditions, p.endmill_setting.diameter_mm, p.endmill_setting.flute_count);
        if p.conditions.axial_doc_mm > p.endmill_setting.loc_mm {
            doc.issue("error", "conditions.axial_doc_mm", "축 절입이 유효 절삭 길이를 초과합니다");
        }
        if matches!(p.workpiece_setup.material, WorkpieceMaterial::AlloySteel { .. }) && p.workpiece_setup.hardness_hrc.is_none() {
            doc.issue("info", "workpiece_setup.hardness_hrc", "합금강 경도는 material 의 HRC 값을 사용합니다");
        }
        for k in ["endmill_setting", "conditions", "workpiece_setup", "coolant_config"] {
            doc.set(k, serde_json::json!(true), "", "", false);
        }
        doc.tool = Some(p.endmill_setting.clone());
        doc.material = Some(p.workpiece_setup.effective_material());
        doc.workpiece = Some(p.workpiece_setup.clone());
        doc.profile = Some(p);
        return doc.finish(&["endmill_setting", "conditions", "workpiece_setup", "coolant_config"]);
    }
    if let Ok(t) = serde_json::from_str::<EndMillMockupSetting>(text) {
        let mut doc = NormalizedDocument::new(name, DocKind::ToolSpec);
        if let Err(e) = t.validate() {
            doc.issue("error", "tool", e);
        }
        for (k, v) in [
            ("diameter_mm", t.diameter_mm),
            ("loc_mm", t.loc_mm),
            ("oal_mm", t.oal_mm),
            ("shank_mm", t.shank_diameter_mm),
            ("helix_deg", t.helix_angle_deg),
        ] {
            doc.set(k, serde_json::json!(v), "", "", false);
        }
        doc.set("flutes", serde_json::json!(t.flute_count), "", "", false);
        doc.set("nose", serde_json::json!(t.nose.key()), "", "", false);
        doc.tool = Some(t);
        return doc.finish(&["diameter_mm", "flutes", "loc_mm", "oal_mm", "shank_mm", "helix_deg", "nose"]);
    }
    if let Ok(spec) = serde_json::from_str::<EndMillSpec>(text) {
        let mut doc = NormalizedDocument::new(name, DocKind::ToolSpec);
        let z = match &spec.flute_geometry {
            crate::spec::FluteGeometry::TwoFlute => 2,
            crate::spec::FluteGeometry::ThreeFlute => 3,
            crate::spec::FluteGeometry::FourFlute => 4,
            crate::spec::FluteGeometry::FiveFlute => 5,
            crate::spec::FluteGeometry::MultiFlute { count } => *count,
            crate::spec::FluteGeometry::BallNose | crate::spec::FluteGeometry::CornerRadius { .. } => {
                doc.issue("warn", "flutes", "형상 표기에 날 수가 없어 2날(볼)/4날(코너R)로 가정했습니다");
                if matches!(spec.flute_geometry, crate::spec::FluteGeometry::BallNose) {
                    2
                } else {
                    4
                }
            }
        };
        let nose = match &spec.flute_geometry {
            crate::spec::FluteGeometry::BallNose => ToolNose::Ball,
            crate::spec::FluteGeometry::CornerRadius { radius_mm } => ToolNose::CornerRadius { radius_mm: *radius_mm },
            _ => ToolNose::Square,
        };
        let mut t = EndMillMockupSetting::custom(
            spec.model.clone(),
            spec.model.clone(),
            spec.diameter_mm,
            z,
            spec.length_of_cut_mm,
            spec.overall_length_mm,
            spec.shank_diameter_mm,
            spec.helix_angle_deg,
            spec.coating.as_ref().map(|c| c.name.clone()),
            matches!(spec.tier, crate::metadata::PriceTier::HighEnd),
        )
        .with_nose(nose);
        t.variable_pitch = spec.variable_pitch;
        for (k, v) in [
            ("diameter_mm", spec.diameter_mm),
            ("loc_mm", spec.length_of_cut_mm),
            ("oal_mm", spec.overall_length_mm),
            ("shank_mm", spec.shank_diameter_mm),
            ("helix_deg", spec.helix_angle_deg),
        ] {
            doc.set(k, serde_json::json!(v), "", "", false);
        }
        doc.set("flutes", serde_json::json!(z), "", "", false);
        doc.set("nose", serde_json::json!(nose.key()), "", "", false);
        if let Some(c) = &spec.reference_conditions {
            check_conditions(&mut doc, c, spec.diameter_mm, z);
        }
        if let Err(e) = t.validate() {
            doc.issue("error", "tool", e);
        }
        doc.tool = Some(t);
        return doc.finish(&["diameter_mm", "flutes", "loc_mm", "oal_mm", "shank_mm", "helix_deg", "nose"]);
    }
    if let Ok(w) = serde_json::from_str::<WorkpieceSetup>(text) {
        let mut doc = NormalizedDocument::new(name, DocKind::Workpiece);
        for (k, v) in [("width_mm", w.width_mm), ("height_mm", w.height_mm), ("thickness_mm", w.thickness_mm)] {
            doc.set(k, serde_json::json!(v), "mm", "", false);
            if v <= 0.0 {
                doc.issue("error", k, "치수는 0 보다 커야 합니다");
            }
        }
        doc.set("material", serde_json::json!(w.material_label()), "", "", false);
        doc.material = Some(w.effective_material());
        doc.workpiece = Some(w);
        return doc.finish(&["material", "width_mm", "height_mm", "thickness_mm"]);
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(arr) = v.as_array() {
            let mut csv = String::new();
            let keys: Vec<String> = arr
                .first()
                .and_then(|o| o.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            if !keys.is_empty() {
                csv.push_str(&keys.join(","));
                csv.push('\n');
                for o in arr.iter() {
                    let row: Vec<String> = keys
                        .iter()
                        .map(|k| o.get(k).map(|x| x.to_string().trim_matches('"').to_string()).unwrap_or_default())
                        .collect();
                    csv.push_str(&row.join(","));
                    csv.push('\n');
                }
                return parse_measurement_csv(name, &csv);
            }
        }
    }
    let mut doc = NormalizedDocument::new(name, DocKind::Unknown);
    doc.issue("error", "json", "알려진 스키마(프로필/공구/원소재/측정 배열)와 일치하지 않습니다");
    doc.finish(&[])
}

fn looks_like_gcode(text: &str) -> bool {
    let r = re(r"(?im)^\s*(?:N\d+\s*)?(?:G0?[0-3]\b|G9[01]\b|G2[01]\b|M0?[36]\b)");
    r.find_iter(text).take(3).count() >= 2
}

fn is_numeric_grid(text: &str) -> bool {
    let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(20).collect();
    rows.len() >= 3
        && rows.iter().all(|l| {
            l.split(|c| c == ',' || c == ';' || c == '\t' || c == ' ')
                .filter(|t| !t.trim().is_empty())
                .all(|t| num(t).is_some())
        })
}

pub fn ingest(name: &str, bytes: &[u8], hint: Option<DocKind>) -> NormalizedDocument {
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("").to_string();
    let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp");
    if is_image || matches!(hint, Some(DocKind::ToolImage) | Some(DocKind::MoldImage) | Some(DocKind::MoldDepth)) {
        let kind = hint.unwrap_or(DocKind::ToolImage);
        let mut doc = NormalizedDocument::new(name, kind);
        doc.image_bytes_len = bytes.len();
        match crate::toolimage::decode(bytes) {
            Ok(img) => {
                doc.set("width_px", serde_json::json!(img.width()), "px", "", false);
                doc.set("height_px", serde_json::json!(img.height()), "px", "", false);
                let sixteen = matches!(img.color(), image::ColorType::L16 | image::ColorType::La16 | image::ColorType::Rgb16 | image::ColorType::Rgba16);
                doc.set("bit_depth", serde_json::json!(if sixteen { 16 } else { 8 }), "bit", "", false);
                if kind == DocKind::MoldDepth && !sixteen {
                    doc.issue("warn", "bit_depth", "8비트 깊이맵은 깊이 분해능이 256단계로 제한됩니다 (16비트 PNG 권장)");
                }
                if img.width().min(img.height()) < 512 && kind != DocKind::MoldDepth {
                    doc.issue("warn", "resolution", "짧은 변이 512px 미만이라 패치 벡터 분해능이 떨어집니다");
                }
            }
            Err(e) => doc.issue("error", "image", e),
        }
        return doc.finish(&["width_px", "height_px"]);
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t.to_string(),
        Err(_) => {
            let decoded: String = bytes.iter().map(|b| *b as char).collect();
            let mut doc = NormalizedDocument::new(name, DocKind::Unknown);
            doc.issue("warn", "encoding", "UTF-8 이 아니어서 Latin-1 로 해석했습니다");
            if looks_like_gcode(&decoded) {
                return parse_gcode(name, &decoded);
            }
            return doc.finish(&[]);
        }
    };
    let text = text.trim_start_matches('\u{feff}').to_string();
    match hint {
        Some(DocKind::GCode) => return parse_gcode(name, &text),
        Some(DocKind::MeasurementLog) => return parse_measurement_csv(name, &text),
        Some(DocKind::ToolSpec) if ext != "json" => return parse_tool_text(name, &text),
        Some(DocKind::MoldCsv) => {
            let mut doc = NormalizedDocument::new(name, DocKind::MoldCsv);
            match mold::heightmap_from_csv(&text, None) {
                Ok(h) => {
                    doc.set("cells", serde_json::json!(h.nx * h.ny), "", "", false);
                    doc.heightmap = Some(h);
                }
                Err(e) => doc.issue("error", "heightmap", e),
            }
            return doc.finish(&["cells"]);
        }
        Some(DocKind::WallCsv) => {
            let mut doc = NormalizedDocument::new(name, DocKind::WallCsv);
            let pts = mold::parse_wall_points(&text);
            if pts.is_empty() {
                doc.issue("error", "points", "x,y,z,편차(µm) 형식의 행이 없습니다");
            } else {
                doc.set("points", serde_json::json!(pts.len()), "", "", false);
            }
            doc.wall_points = Some(pts);
            return doc.finish(&["points"]);
        }
        _ => {}
    }
    if ext == "json" || text.trim_start().starts_with('{') || text.trim_start().starts_with('[') {
        return parse_json(name, &text);
    }
    if matches!(ext.as_str(), "nc" | "tap" | "ngc" | "gcode" | "cnc" | "mpf" | "eia" | "ptp") || looks_like_gcode(&text) {
        return parse_gcode(name, &text);
    }
    let first = text.lines().next().unwrap_or("").to_lowercase();
    if (first.contains("dev") || first.contains("편차")) && first.contains('x') && first.contains('y') {
        return ingest(name, bytes, Some(DocKind::WallCsv));
    }
    if is_numeric_grid(&text) {
        return ingest(name, bytes, Some(DocKind::MoldCsv));
    }
    if (first.contains(',') || first.contains('\t') || first.contains(';'))
        && first.split(|c| c == ',' || c == '\t' || c == ';').any(|h| header_kind(h).is_some())
    {
        return parse_measurement_csv(name, &text);
    }
    let tool_doc = parse_tool_text(name, &text);
    if tool_doc.tool.is_some() {
        return tool_doc;
    }
    let mut doc = NormalizedDocument::new(name, DocKind::Unknown);
    if let Ok(m) = parse_material(&text, None) {
        doc.kind = DocKind::Workpiece;
        doc.kind_label = DocKind::Workpiece.label().into();
        doc.set("material", serde_json::json!(m.designation), "", "", false);
        if m.hrc_assumed {
            doc.issue("warn", "hardness_hrc", "경도 표기가 없어 대표 공급 경도로 가정했습니다");
        }
        doc.material = Some(m.material);
        return doc.finish(&["material"]);
    }
    doc.issue("error", "kind", "문서 유형을 판별하지 못했습니다. 유형 힌트를 지정해 주세요");
    doc.finish(&[])
}