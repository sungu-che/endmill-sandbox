use super::attr::{EndMillAlias, EndMillAttr, WorkpieceAttr};
use super::now_ms;
use crate::cutting::CuttingConditions;
use crate::profile::{EndMillMockupSetting, MachiningPreset, MachiningProfile};
use crate::workpiece_setup::WorkpieceSetup;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS projects(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS endmills(
  id INTEGER PRIMARY KEY,
  attr_key TEXT NOT NULL UNIQUE,
  diameter_mm REAL NOT NULL,
  flutes INTEGER NOT NULL,
  loc_mm REAL NOT NULL,
  oal_mm REAL NOT NULL,
  shank_mm REAL NOT NULL,
  helix_deg REAL NOT NULL,
  nose TEXT NOT NULL,
  corner_r_mm REAL NOT NULL,
  coating_family TEXT NOT NULL,
  variable_pitch INTEGER NOT NULL,
  substrate TEXT NOT NULL,
  attr_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_endmills_geom ON endmills(diameter_mm, flutes, nose);
CREATE TABLE IF NOT EXISTS endmill_aliases(
  endmill_id INTEGER NOT NULL REFERENCES endmills(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  model TEXT NOT NULL,
  high_end INTEGER NOT NULL,
  coating_name TEXT NOT NULL DEFAULT '',
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  uses INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY(endmill_id, name, model)
);
CREATE TABLE IF NOT EXISTS workpieces(
  id INTEGER PRIMARY KEY,
  attr_key TEXT NOT NULL UNIQUE,
  material_key TEXT NOT NULL,
  material_family TEXT NOT NULL,
  hardness_hrc REAL,
  shape TEXT NOT NULL,
  width_mm REAL NOT NULL,
  height_mm REAL NOT NULL,
  thickness_mm REAL NOT NULL,
  clamping TEXT NOT NULL,
  tolerance_mm REAL NOT NULL,
  ra_target_um REAL NOT NULL,
  attr_json TEXT NOT NULL,
  setup_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_workpieces_mat ON workpieces(material_family, hardness_hrc);
CREATE TABLE IF NOT EXISTS workpiece_aliases(
  workpiece_id INTEGER NOT NULL REFERENCES workpieces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  uses INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY(workpiece_id, name)
);
CREATE TABLE IF NOT EXISTS presets(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  builtin INTEGER NOT NULL DEFAULT 0,
  endmill_id INTEGER NOT NULL REFERENCES endmills(id),
  workpiece_id INTEGER NOT NULL REFERENCES workpieces(id),
  preset_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS profiles(
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  preset_id INTEGER REFERENCES presets(id) ON DELETE SET NULL,
  name TEXT NOT NULL,
  endmill_id INTEGER NOT NULL REFERENCES endmills(id),
  workpiece_id INTEGER NOT NULL REFERENCES workpieces(id),
  profile_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(project_id, name)
);
CREATE INDEX IF NOT EXISTS ix_profiles_preset ON profiles(preset_id);
CREATE TABLE IF NOT EXISTS runs(
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
  preset_id INTEGER REFERENCES presets(id) ON DELETE SET NULL,
  endmill_id INTEGER NOT NULL REFERENCES endmills(id),
  workpiece_id INTEGER NOT NULL REFERENCES workpieces(id),
  tool_key TEXT NOT NULL,
  kind TEXT NOT NULL,
  purpose TEXT NOT NULL,
  coolant TEXT NOT NULL,
  vc_m_min REAL NOT NULL,
  fz_mm REAL NOT NULL,
  ap_mm REAL NOT NULL,
  ae_mm REAL NOT NULL,
  rpm REAL NOT NULL,
  feed_mm_min REAL NOT NULL,
  conditions_json TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_runs_pair ON runs(endmill_id, workpiece_id);
CREATE INDEX IF NOT EXISTS ix_runs_project ON runs(project_id, created_at);
CREATE TABLE IF NOT EXISTS run_metrics(
  run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  metric TEXT NOT NULL,
  source TEXT NOT NULL,
  value REAL NOT NULL,
  unit TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(run_id, metric, source)
);
CREATE INDEX IF NOT EXISTS ix_metrics_metric ON run_metrics(metric, source);
CREATE TABLE IF NOT EXISTS ingest_seen(
  fingerprint TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  project_id INTEGER,
  at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS vector_refs(
  collection TEXT NOT NULL,
  pk TEXT NOT NULL,
  entity TEXT NOT NULL,
  entity_id INTEGER NOT NULL,
  project_id INTEGER,
  run_id INTEGER,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(collection, pk)
);
CREATE TABLE IF NOT EXISTS life_marks(
  series_id TEXT NOT NULL,
  episode INTEGER NOT NULL,
  run_id INTEGER NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  life_min REAL NOT NULL,
  PRIMARY KEY(series_id, episode)
);
"#;

fn err(e: rusqlite::Error) -> String {
    format!("SQLite: {}", e)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub profiles: i64,
    pub runs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndMillRow {
    pub id: i64,
    pub attr_key: String,
    pub attr: EndMillAttr,
    pub label: String,
    pub aliases: Vec<EndMillAlias>,
    pub runs: i64,
    pub workpieces: i64,
    pub created_at: i64,
}

impl EndMillRow {
    pub fn primary_alias(&self) -> Option<&EndMillAlias> {
        self.aliases.first()
    }

    pub fn setting(&self) -> EndMillMockupSetting {
        self.attr.to_setting(self.primary_alias())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkpieceRow {
    pub id: i64,
    pub attr_key: String,
    pub attr: WorkpieceAttr,
    pub setup: WorkpieceSetup,
    pub label: String,
    pub names: Vec<String>,
    pub runs: i64,
    pub endmills: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetRow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub preset: MachiningPreset,
    pub profiles: i64,
    pub runs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRow {
    pub id: i64,
    pub project_id: i64,
    pub preset_id: Option<i64>,
    pub name: String,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub profile: MachiningProfile,
    pub updated_at: i64,
    pub runs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PairStats {
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub runs: i64,
    pub life_meas: Option<f64>,
    pub life_meas_n: i64,
    pub life_pred: Option<f64>,
    pub wear_ratio: Option<f64>,
    pub wear_ratio_n: i64,
    pub wall_util: Option<f64>,
    pub chatter_margin: Option<f64>,
    pub power_util: Option<f64>,
    pub replace_count: i64,
    pub inspect_count: i64,
    pub last_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewRun {
    pub project_id: i64,
    pub profile_id: Option<i64>,
    pub preset_id: Option<i64>,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub tool_key: String,
    pub kind: String,
    pub purpose: String,
    pub coolant: String,
    pub conditions: CuttingConditions,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub metric: String,
    pub source: String,
    pub value: f64,
    pub unit: String,
}

impl Metric {
    pub fn new(metric: &str, source: &str, value: f64, unit: &str) -> Self {
        Self {
            metric: metric.into(),
            source: source.into(),
            value,
            unit: unit.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRow {
    pub id: i64,
    pub project_id: i64,
    pub profile_id: Option<i64>,
    pub preset_id: Option<i64>,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub tool_key: String,
    pub kind: String,
    pub purpose: String,
    pub coolant: String,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub note: String,
    pub created_at: i64,
    pub metrics: Vec<Metric>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RdbCounts {
    pub projects: i64,
    pub presets: i64,
    pub profiles: i64,
    pub endmills: i64,
    pub workpieces: i64,
    pub runs: i64,
    pub metrics: i64,
}

pub struct Rdb {
    conn: Connection,
}

impl Rdb {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        let conn = Connection::open(path).map_err(err)?;
        let db = Self { conn };
        db.init(true)?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(err)?;
        let db = Self { conn };
        db.init(false)?;
        Ok(db)
    }

    fn init(&self, wal: bool) -> Result<(), String> {
        if wal {
            self.conn
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
                .map_err(err)?;
            self.conn.execute_batch("PRAGMA synchronous=NORMAL;").map_err(err)?;
        }
        self.conn.execute_batch("PRAGMA foreign_keys=ON;").map_err(err)?;
        self.conn.execute_batch(SCHEMA).map_err(err)?;
        let v: Option<String> = self.meta("schema_version")?;
        if v.is_none() {
            self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        }
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(err)
    }

    pub fn ensure_project(&self, name: &str, description: &str) -> Result<i64, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("프로젝트 이름이 비어 있습니다".into());
        }
        if let Some(id) = self
            .conn
            .query_row("SELECT id FROM projects WHERE name = ?1", params![name], |r| r.get::<_, i64>(0))
            .optional()
            .map_err(err)?
        {
            return Ok(id);
        }
        let now = now_ms();
        self.conn
            .execute(
                "INSERT INTO projects(name, description, created_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
                params![name, description, now],
            )
            .map_err(err)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn touch_project(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute("UPDATE projects SET updated_at = ?2 WHERE id = ?1", params![id, now_ms()])
            .map(|_| ())
            .map_err(err)
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT p.id, p.name, p.description, p.created_at, p.updated_at,
                        (SELECT COUNT(*) FROM profiles f WHERE f.project_id = p.id),
                        (SELECT COUNT(*) FROM runs r WHERE r.project_id = p.id)
                 FROM projects p ORDER BY p.updated_at DESC, p.id",
            )
            .map_err(err)?;
        let rows = st
            .query_map([], |r| {
                Ok(ProjectRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    created_at: r.get(3)?,
                    updated_at: r.get(4)?,
                    profiles: r.get(5)?,
                    runs: r.get(6)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn project_name(&self, id: i64) -> Result<Option<String>, String> {
        self.conn
            .query_row("SELECT name FROM projects WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    pub fn active_project(&self) -> Result<i64, String> {
        if let Some(v) = self.meta("active_project")? {
            if let Ok(id) = v.parse::<i64>() {
                if self.project_name(id)?.is_some() {
                    return Ok(id);
                }
            }
        }
        let id = self.ensure_project("기본 프로젝트", "자동 생성된 기본 프로젝트")?;
        self.set_meta("active_project", &id.to_string())?;
        Ok(id)
    }

    pub fn set_active_project(&self, id: i64) -> Result<(), String> {
        if self.project_name(id)?.is_none() {
            return Err(format!("프로젝트 {} 이(가) 없습니다", id));
        }
        self.set_meta("active_project", &id.to_string())
    }

    pub fn upsert_endmill(&self, s: &EndMillMockupSetting) -> Result<(i64, bool), String> {
        let attr = EndMillAttr::from_setting(s);
        let key = attr.key();
        let now = now_ms();
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM endmills WHERE attr_key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let (id, created) = match existing {
            Some(id) => (id, false),
            None => {
                let json = serde_json::to_string(&attr).map_err(|e| e.to_string())?;
                self.conn
                    .execute(
                        "INSERT INTO endmills(attr_key, diameter_mm, flutes, loc_mm, oal_mm, shank_mm, helix_deg, nose, corner_r_mm,
                                              coating_family, variable_pitch, substrate, attr_json, created_at)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![
                            key,
                            attr.diameter_mm,
                            attr.flutes as i64,
                            attr.loc_mm,
                            attr.oal_mm,
                            attr.shank_mm,
                            attr.helix_deg,
                            attr.nose,
                            attr.corner_r_mm,
                            attr.coating_family,
                            attr.variable_pitch as i64,
                            attr.substrate,
                            json,
                            now
                        ],
                    )
                    .map_err(err)?;
                (self.conn.last_insert_rowid(), true)
            }
        };
        let alias = EndMillAttr::alias_of(s);
        self.conn
            .execute(
                "INSERT INTO endmill_aliases(endmill_id, name, model, high_end, coating_name, first_seen, last_seen, uses)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6, 1)
                 ON CONFLICT(endmill_id, name, model) DO UPDATE SET
                   last_seen = excluded.last_seen, uses = uses + 1, high_end = excluded.high_end, coating_name = excluded.coating_name",
                params![id, alias.name, alias.model, alias.high_end as i64, alias.coating_name, now],
            )
            .map_err(err)?;
        Ok((id, created))
    }

    fn endmill_aliases(&self, id: i64) -> Result<Vec<EndMillAlias>, String> {
        let mut st = self
            .conn
            .prepare("SELECT name, model, high_end, coating_name FROM endmill_aliases WHERE endmill_id = ?1 ORDER BY uses DESC, last_seen DESC")
            .map_err(err)?;
        let rows = st
            .query_map(params![id], |r| {
                Ok(EndMillAlias {
                    name: r.get(0)?,
                    model: r.get(1)?,
                    high_end: r.get::<_, i64>(2)? != 0,
                    coating_name: r.get(3)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    fn endmill_from_row(&self, id: i64, key: String, json: String, created_at: i64, runs: i64, wps: i64) -> Result<EndMillRow, String> {
        let attr: EndMillAttr = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok(EndMillRow {
            id,
            label: attr.label(),
            attr_key: key,
            attr,
            aliases: self.endmill_aliases(id)?,
            runs,
            workpieces: wps,
            created_at,
        })
    }

    const ENDMILL_SELECT: &'static str = "SELECT e.id, e.attr_key, e.attr_json, e.created_at,
            (SELECT COUNT(*) FROM runs r WHERE r.endmill_id = e.id),
            (SELECT COUNT(DISTINCT r.workpiece_id) FROM runs r WHERE r.endmill_id = e.id)
          FROM endmills e";

    pub fn endmill(&self, id: i64) -> Result<Option<EndMillRow>, String> {
        let row = self
            .conn
            .query_row(&format!("{} WHERE e.id = ?1", Self::ENDMILL_SELECT), params![id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?))
            })
            .optional()
            .map_err(err)?;
        match row {
            Some((id, k, j, c, n, w)) => Ok(Some(self.endmill_from_row(id, k, j, c, n, w)?)),
            None => Ok(None),
        }
    }

    pub fn endmill_id_by_key(&self, key: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row("SELECT id FROM endmills WHERE attr_key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    pub fn list_endmills(&self, limit: usize) -> Result<Vec<EndMillRow>, String> {
        let mut st = self
            .conn
            .prepare(&format!("{} ORDER BY e.diameter_mm, e.flutes, e.id LIMIT ?1", Self::ENDMILL_SELECT))
            .map_err(err)?;
        let raw = st
            .query_map(params![limit as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        raw.into_iter().map(|(id, k, j, c, n, w)| self.endmill_from_row(id, k, j, c, n, w)).collect()
    }

    pub fn upsert_workpiece(&self, w: &WorkpieceSetup) -> Result<(i64, bool), String> {
        let attr = WorkpieceAttr::from_setup(w);
        let key = attr.key();
        let now = now_ms();
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM workpieces WHERE attr_key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let (id, created) = match existing {
            Some(id) => (id, false),
            None => {
                let aj = serde_json::to_string(&attr).map_err(|e| e.to_string())?;
                let sj = serde_json::to_string(w).map_err(|e| e.to_string())?;
                self.conn
                    .execute(
                        "INSERT INTO workpieces(attr_key, material_key, material_family, hardness_hrc, shape, width_mm, height_mm,
                                                thickness_mm, clamping, tolerance_mm, ra_target_um, attr_json, setup_json, created_at)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![
                            key,
                            attr.material_key,
                            attr.material_family,
                            attr.hardness_hrc,
                            attr.shape,
                            attr.width_mm,
                            attr.height_mm,
                            attr.thickness_mm,
                            attr.clamping,
                            attr.tolerance_mm,
                            attr.ra_target_um,
                            aj,
                            sj,
                            now
                        ],
                    )
                    .map_err(err)?;
                (self.conn.last_insert_rowid(), true)
            }
        };
        let name = if w.name.trim().is_empty() { w.material_label() } else { w.name.trim().to_string() };
        self.conn
            .execute(
                "INSERT INTO workpiece_aliases(workpiece_id, name, first_seen, last_seen, uses) VALUES(?1, ?2, ?3, ?3, 1)
                 ON CONFLICT(workpiece_id, name) DO UPDATE SET last_seen = excluded.last_seen, uses = uses + 1",
                params![id, name, now],
            )
            .map_err(err)?;
        Ok((id, created))
    }

    const WORKPIECE_SELECT: &'static str = "SELECT w.id, w.attr_key, w.attr_json, w.setup_json, w.created_at,
            (SELECT COUNT(*) FROM runs r WHERE r.workpiece_id = w.id),
            (SELECT COUNT(DISTINCT r.endmill_id) FROM runs r WHERE r.workpiece_id = w.id)
          FROM workpieces w";

    fn workpiece_names(&self, id: i64) -> Result<Vec<String>, String> {
        let mut st = self
            .conn
            .prepare("SELECT name FROM workpiece_aliases WHERE workpiece_id = ?1 ORDER BY uses DESC, last_seen DESC")
            .map_err(err)?;
        let rows = st.query_map(params![id], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    #[allow(clippy::too_many_arguments)]
    fn workpiece_from_row(&self, id: i64, key: String, aj: String, sj: String, created_at: i64, runs: i64, ems: i64) -> Result<WorkpieceRow, String> {
        let attr: WorkpieceAttr = serde_json::from_str(&aj).map_err(|e| e.to_string())?;
        let setup: WorkpieceSetup = serde_json::from_str(&sj).map_err(|e| e.to_string())?;
        Ok(WorkpieceRow {
            id,
            label: attr.label(&setup),
            attr_key: key,
            attr,
            setup,
            names: self.workpiece_names(id)?,
            runs,
            endmills: ems,
            created_at,
        })
    }

    pub fn workpiece(&self, id: i64) -> Result<Option<WorkpieceRow>, String> {
        let row = self
            .conn
            .query_row(&format!("{} WHERE w.id = ?1", Self::WORKPIECE_SELECT), params![id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .optional()
            .map_err(err)?;
        match row {
            Some((id, k, a, s, c, n, e)) => Ok(Some(self.workpiece_from_row(id, k, a, s, c, n, e)?)),
            None => Ok(None),
        }
    }

    pub fn workpiece_id_by_key(&self, key: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row("SELECT id FROM workpieces WHERE attr_key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    pub fn list_workpieces(&self, limit: usize) -> Result<Vec<WorkpieceRow>, String> {
        let mut st = self
            .conn
            .prepare(&format!("{} ORDER BY w.material_family, w.hardness_hrc, w.id LIMIT ?1", Self::WORKPIECE_SELECT))
            .map_err(err)?;
        let raw = st
            .query_map(params![limit as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        raw.into_iter().map(|(id, k, a, s, c, n, e)| self.workpiece_from_row(id, k, a, s, c, n, e)).collect()
    }

    pub fn upsert_preset(&self, p: &MachiningPreset, builtin: bool) -> Result<i64, String> {
        let (em, _) = self.upsert_endmill(&p.endmill_setting)?;
        let (wp, _) = self.upsert_workpiece(&p.workpiece_setup)?;
        let json = serde_json::to_string(p).map_err(|e| e.to_string())?;
        let now = now_ms();
        self.conn
            .execute(
                "INSERT INTO presets(name, description, builtin, endmill_id, workpiece_id, preset_json, created_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(name) DO UPDATE SET description = excluded.description, endmill_id = excluded.endmill_id,
                   workpiece_id = excluded.workpiece_id, preset_json = excluded.preset_json, updated_at = excluded.updated_at,
                   builtin = MAX(presets.builtin, excluded.builtin)",
                params![p.name, p.description, builtin as i64, em, wp, json, now],
            )
            .map_err(err)?;
        self.conn
            .query_row("SELECT id FROM presets WHERE name = ?1", params![p.name], |r| r.get(0))
            .map_err(err)
    }

    pub fn preset_id(&self, name: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row("SELECT id FROM presets WHERE name = ?1", params![name], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    pub fn delete_preset(&self, name: &str) -> Result<bool, String> {
        let n = self
            .conn
            .execute("DELETE FROM presets WHERE name = ?1 AND builtin = 0", params![name])
            .map_err(err)?;
        Ok(n > 0)
    }

    pub fn list_presets(&self) -> Result<Vec<PresetRow>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT p.id, p.name, p.description, p.builtin, p.endmill_id, p.workpiece_id, p.preset_json,
                        (SELECT COUNT(*) FROM profiles f WHERE f.preset_id = p.id),
                        (SELECT COUNT(*) FROM runs r WHERE r.preset_id = p.id)
                 FROM presets p ORDER BY p.builtin DESC, p.name",
            )
            .map_err(err)?;
        let raw = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        raw.into_iter()
            .map(|(id, name, description, b, em, wp, j, nprof, nruns)| {
                Ok(PresetRow {
                    id,
                    name,
                    description,
                    builtin: b != 0,
                    endmill_id: em,
                    workpiece_id: wp,
                    preset: serde_json::from_str(&j).map_err(|e| e.to_string())?,
                    profiles: nprof,
                    runs: nruns,
                })
            })
            .collect()
    }

    pub fn upsert_profile(&self, project_id: i64, p: &MachiningProfile, preset_id: Option<i64>) -> Result<i64, String> {
        let (em, _) = self.upsert_endmill(&p.endmill_setting)?;
        let (wp, _) = self.upsert_workpiece(&p.workpiece_setup)?;
        let json = serde_json::to_string(p).map_err(|e| e.to_string())?;
        let now = now_ms();
        self.conn
            .execute(
                "INSERT INTO profiles(project_id, preset_id, name, endmill_id, workpiece_id, profile_json, created_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(project_id, name) DO UPDATE SET
                   preset_id = COALESCE(excluded.preset_id, profiles.preset_id), endmill_id = excluded.endmill_id,
                   workpiece_id = excluded.workpiece_id, profile_json = excluded.profile_json, updated_at = excluded.updated_at",
                params![project_id, preset_id, p.name, em, wp, json, now],
            )
            .map_err(err)?;
        self.touch_project(project_id)?;
        self.conn
            .query_row(
                "SELECT id FROM profiles WHERE project_id = ?1 AND name = ?2",
                params![project_id, p.name],
                |r| r.get(0),
            )
            .map_err(err)
    }

    pub fn delete_profile(&self, project_id: i64, name: &str) -> Result<bool, String> {
        let n = self
            .conn
            .execute("DELETE FROM profiles WHERE project_id = ?1 AND name = ?2", params![project_id, name])
            .map_err(err)?;
        Ok(n > 0)
    }

    fn profiles_where(&self, clause: &str, arg: i64) -> Result<Vec<ProfileRow>, String> {
        let sql = format!(
            "SELECT f.id, f.project_id, f.preset_id, f.name, f.endmill_id, f.workpiece_id, f.profile_json, f.updated_at,
                    (SELECT COUNT(*) FROM runs r WHERE r.profile_id = f.id)
             FROM profiles f WHERE {} ORDER BY f.updated_at DESC, f.id",
            clause
        );
        let mut st = self.conn.prepare(&sql).map_err(err)?;
        let raw = st
            .query_map(params![arg], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        raw.into_iter()
            .map(|(id, project_id, preset_id, name, em, wp, j, upd, runs)| {
                Ok(ProfileRow {
                    id,
                    project_id,
                    preset_id,
                    name,
                    endmill_id: em,
                    workpiece_id: wp,
                    profile: serde_json::from_str(&j).map_err(|e| e.to_string())?,
                    updated_at: upd,
                    runs,
                })
            })
            .collect()
    }

    pub fn list_profiles(&self, project_id: i64) -> Result<Vec<ProfileRow>, String> {
        self.profiles_where("f.project_id = ?1", project_id)
    }

    pub fn profiles_by_preset(&self, preset_id: i64) -> Result<Vec<ProfileRow>, String> {
        self.profiles_where("f.preset_id = ?1", preset_id)
    }

    pub fn profile_ids(&self, project_id: i64, name: &str) -> Result<Option<(i64, Option<i64>)>, String> {
        self.conn
            .query_row(
                "SELECT id, preset_id FROM profiles WHERE project_id = ?1 AND name = ?2",
                params![project_id, name],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .map_err(err)
    }

    pub fn insert_run(&mut self, run: &NewRun, metrics: &[Metric]) -> Result<i64, String> {
        let tx = self.conn.transaction().map_err(err)?;
        let c = &run.conditions;
        let cj = serde_json::to_string(c).map_err(|e| e.to_string())?;
        let rpm = c.spindle_rpm as f64;
        tx.execute(
            "INSERT INTO runs(project_id, profile_id, preset_id, endmill_id, workpiece_id, tool_key, kind, purpose, coolant,
                              vc_m_min, fz_mm, ap_mm, ae_mm, rpm, feed_mm_min, conditions_json, note, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                run.project_id,
                run.profile_id,
                run.preset_id,
                run.endmill_id,
                run.workpiece_id,
                run.tool_key,
                run.kind,
                run.purpose,
                run.coolant,
                c.cutting_speed_m_min,
                c.feed_per_tooth_mm,
                c.axial_doc_mm,
                c.radial_doc_mm,
                rpm,
                c.feed_rate_mm_min,
                cj,
                run.note,
                now_ms()
            ],
        )
        .map_err(err)?;
        let id = tx.last_insert_rowid();
        for m in metrics.iter().filter(|m| m.value.is_finite()) {
            tx.execute(
                "INSERT INTO run_metrics(run_id, metric, source, value, unit) VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(run_id, metric, source) DO UPDATE SET value = excluded.value, unit = excluded.unit",
                params![id, m.metric, m.source, m.value, m.unit],
            )
            .map_err(err)?;
        }
        tx.execute("UPDATE projects SET updated_at = ?2 WHERE id = ?1", params![run.project_id, now_ms()])
            .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(id)
    }

    pub fn life_marked(&self, series_id: &str, episode: i64) -> Result<bool, String> {
        let n: Option<i64> = self
            .conn
            .query_row(
                "SELECT run_id FROM life_marks WHERE series_id = ?1 AND episode = ?2",
                params![series_id, episode],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        Ok(n.is_some())
    }

    pub fn mark_life(&self, series_id: &str, episode: i64, run_id: i64, life_min: f64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO life_marks(series_id, episode, run_id, life_min) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(series_id, episode) DO UPDATE SET run_id = excluded.run_id, life_min = excluded.life_min",
                params![series_id, episode, run_id, life_min],
            )
            .map(|_| ())
            .map_err(err)
    }

    pub fn runs(&self, project_id: Option<i64>, endmill_id: Option<i64>, workpiece_id: Option<i64>, limit: usize) -> Result<Vec<RunRow>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT id, project_id, profile_id, preset_id, endmill_id, workpiece_id, tool_key, kind, purpose, coolant,
                        vc_m_min, fz_mm, ap_mm, ae_mm, note, created_at
                 FROM runs
                 WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR endmill_id = ?2) AND (?3 IS NULL OR workpiece_id = ?3)
                 ORDER BY created_at DESC, id DESC LIMIT ?4",
            )
            .map_err(err)?;
        let mut rows = st
            .query_map(params![project_id, endmill_id, workpiece_id, limit as i64], |r| {
                Ok(RunRow {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    profile_id: r.get(2)?,
                    preset_id: r.get(3)?,
                    endmill_id: r.get(4)?,
                    workpiece_id: r.get(5)?,
                    tool_key: r.get(6)?,
                    kind: r.get(7)?,
                    purpose: r.get(8)?,
                    coolant: r.get(9)?,
                    vc_m_min: r.get(10)?,
                    fz_mm: r.get(11)?,
                    ap_mm: r.get(12)?,
                    ae_mm: r.get(13)?,
                    note: r.get(14)?,
                    created_at: r.get(15)?,
                    metrics: Vec::new(),
                })
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut mst = self
            .conn
            .prepare("SELECT metric, source, value, unit FROM run_metrics WHERE run_id = ?1 ORDER BY metric, source")
            .map_err(err)?;
        for row in rows.iter_mut() {
            row.metrics = mst
                .query_map(params![row.id], |r| {
                    Ok(Metric {
                        metric: r.get(0)?,
                        source: r.get(1)?,
                        value: r.get(2)?,
                        unit: r.get(3)?,
                    })
                })
                .map_err(err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
        }
        Ok(rows)
    }

    pub fn pair_stats(&self, endmill_id: Option<i64>, workpiece_id: Option<i64>, project_id: Option<i64>) -> Result<Vec<PairStats>, String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT r.endmill_id, r.workpiece_id, COUNT(DISTINCT r.id),
                        AVG(CASE WHEN m.metric = 'tool_life_min' AND m.source = 'meas' THEN m.value END),
                        COUNT(CASE WHEN m.metric = 'tool_life_min' AND m.source = 'meas' THEN 1 END),
                        AVG(CASE WHEN m.metric = 'tool_life_min' AND m.source = 'pred' THEN m.value END),
                        AVG(CASE WHEN m.metric = 'wear_ratio' AND m.source = 'calib' THEN m.value END),
                        COUNT(CASE WHEN m.metric = 'wear_ratio' AND m.source = 'calib' THEN 1 END),
                        AVG(CASE WHEN m.metric = 'wall_util' AND m.source = 'pred' THEN m.value END),
                        AVG(CASE WHEN m.metric = 'chatter_margin' AND m.source = 'pred' THEN m.value END),
                        AVG(CASE WHEN m.metric = 'power_util' AND m.source = 'pred' THEN m.value END),
                        COUNT(CASE WHEN m.metric = 'decision_code' AND m.value >= 3 THEN 1 END),
                        COUNT(CASE WHEN m.metric = 'decision_code' AND m.value >= 2 AND m.value < 3 THEN 1 END),
                        MAX(r.created_at)
                 FROM runs r LEFT JOIN run_metrics m ON m.run_id = r.id
                 WHERE (?1 IS NULL OR r.endmill_id = ?1) AND (?2 IS NULL OR r.workpiece_id = ?2) AND (?3 IS NULL OR r.project_id = ?3)
                 GROUP BY r.endmill_id, r.workpiece_id
                 ORDER BY COUNT(DISTINCT r.id) DESC",
            )
            .map_err(err)?;
        let rows = st
            .query_map(params![endmill_id, workpiece_id, project_id], |r| {
                Ok(PairStats {
                    endmill_id: r.get(0)?,
                    workpiece_id: r.get(1)?,
                    runs: r.get(2)?,
                    life_meas: r.get(3)?,
                    life_meas_n: r.get(4)?,
                    life_pred: r.get(5)?,
                    wear_ratio: r.get(6)?,
                    wear_ratio_n: r.get(7)?,
                    wall_util: r.get(8)?,
                    chatter_margin: r.get(9)?,
                    power_util: r.get(10)?,
                    replace_count: r.get(11)?,
                    inspect_count: r.get(12)?,
                    last_at: r.get(13)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn is_seen(&self, fp: &str) -> Result<bool, String> {
        let n: Option<String> = self
            .conn
            .query_row("SELECT fingerprint FROM ingest_seen WHERE fingerprint = ?1", params![fp], |r| r.get(0))
            .optional()
            .map_err(err)?;
        Ok(n.is_some())
    }

    pub fn mark_seen(&self, fp: &str, kind: &str, name: &str, project_id: Option<i64>) -> Result<bool, String> {
        let n = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO ingest_seen(fingerprint, kind, name, project_id, at) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![fp, kind, name, project_id, now_ms()],
            )
            .map_err(err)?;
        Ok(n > 0)
    }

    pub fn seen_all(&self) -> Result<Vec<String>, String> {
        let mut st = self.conn.prepare("SELECT fingerprint FROM ingest_seen").map_err(err)?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn add_vector_ref(&self, collection: &str, pk: &str, entity: &str, entity_id: i64, project_id: Option<i64>, run_id: Option<i64>) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO vector_refs(collection, pk, entity, entity_id, project_id, run_id, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(collection, pk) DO UPDATE SET entity = excluded.entity, entity_id = excluded.entity_id,
                   project_id = excluded.project_id, run_id = excluded.run_id",
                params![collection, pk, entity, entity_id, project_id, run_id, now_ms()],
            )
            .map(|_| ())
            .map_err(err)
    }

    pub fn counts(&self) -> Result<RdbCounts, String> {
        let one = |sql: &str| -> Result<i64, String> { self.conn.query_row(sql, [], |r| r.get(0)).map_err(err) };
        Ok(RdbCounts {
            projects: one("SELECT COUNT(*) FROM projects")?,
            presets: one("SELECT COUNT(*) FROM presets")?,
            profiles: one("SELECT COUNT(*) FROM profiles")?,
            endmills: one("SELECT COUNT(*) FROM endmills")?,
            workpieces: one("SELECT COUNT(*) FROM workpieces")?,
            runs: one("SELECT COUNT(*) FROM runs")?,
            metrics: one("SELECT COUNT(*) FROM run_metrics")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileStore;

    #[test]
    fn attribute_identity_merges_brands() {
        let db = Rdb::open_in_memory().unwrap();
        let a = EndMillMockupSetting::default_10mm_4flute();
        let mut b = a.clone();
        b.name = "타사 Ø10".into();
        b.model = "XX-4F".into();
        let (ia, ca) = db.upsert_endmill(&a).unwrap();
        let (ib, cb) = db.upsert_endmill(&b).unwrap();
        assert!(ca && !cb);
        assert_eq!(ia, ib);
        let row = db.endmill(ia).unwrap().unwrap();
        assert_eq!(row.aliases.len(), 2);
    }

    #[test]
    fn profiles_by_project_and_preset() {
        let mut db = Rdb::open_in_memory().unwrap();
        let store = ProfileStore::new();
        let preset = &store.presets[2];
        let pid = db.upsert_preset(preset, true).unwrap();
        let p1 = db.ensure_project("A", "").unwrap();
        let p2 = db.ensure_project("B", "").unwrap();
        let prof = MachiningProfile::from_preset(preset, "강 범용");
        let f1 = db.upsert_profile(p1, &prof, Some(pid)).unwrap();
        let f2 = db.upsert_profile(p2, &prof, Some(pid)).unwrap();
        assert_ne!(f1, f2);
        assert_eq!(db.list_profiles(p1).unwrap().len(), 1);
        assert_eq!(db.profiles_by_preset(pid).unwrap().len(), 2);
        let (em, _) = db.upsert_endmill(&prof.endmill_setting).unwrap();
        let (wp, _) = db.upsert_workpiece(&prof.workpiece_setup).unwrap();
        let run = NewRun {
            project_id: p1,
            profile_id: Some(f1),
            preset_id: Some(pid),
            endmill_id: em,
            workpiece_id: wp,
            tool_key: "T1".into(),
            kind: "process".into(),
            purpose: "roughing".into(),
            coolant: "mist".into(),
            conditions: prof.conditions.clone(),
            note: String::new(),
        };
        db.insert_run(&run, &[Metric::new("tool_life_min", "pred", 40.0, "min"), Metric::new("decision_code", "meta", 3.0, "")])
            .unwrap();
        db.insert_run(&run, &[Metric::new("tool_life_min", "meas", 30.0, "min"), Metric::new("wear_ratio", "calib", 1.3, "")])
            .unwrap();
        let stats = db.pair_stats(Some(em), None, None).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].runs, 2);
        assert_eq!(stats[0].life_meas, Some(30.0));
        assert_eq!(stats[0].replace_count, 1);
        assert_eq!(db.runs(Some(p1), None, None, 10).unwrap().len(), 2);
        let presets = db.list_presets().unwrap();
        assert_eq!(presets[0].profiles, 2);
        assert_eq!(presets[0].runs, 2);
    }

    #[test]
    fn active_project_defaults() {
        let db = Rdb::open_in_memory().unwrap();
        let a = db.active_project().unwrap();
        assert_eq!(db.active_project().unwrap(), a);
        let b = db.ensure_project("신규", "").unwrap();
        db.set_active_project(b).unwrap();
        assert_eq!(db.active_project().unwrap(), b);
        assert!(db.mark_seen("fp1", "log", "a.csv", Some(b)).unwrap());
        assert!(!db.mark_seen("fp1", "log", "a.csv", Some(b)).unwrap());
        assert!(db.is_seen("fp1").unwrap());
    }
}