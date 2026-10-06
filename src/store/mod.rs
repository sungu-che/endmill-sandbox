pub mod attr;
pub mod lance;
pub mod rdb;
pub mod record;
pub mod series;
pub mod vector;

use crate::profile::{EndMillMockupSetting, MachiningPreset, MachiningProfile, ProfileStore};
use crate::workpiece_setup::WorkpieceSetup;
use attr::{EndMillAttr, WorkpieceAttr};
use lance::LanceDb;
use rdb::{ProjectRow, Rdb, RdbCounts};
use serde::{Deserialize, Serialize};
use series::{SeriesCounts, SeriesStore};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use vector::{CollectionStatus, Scalar, VecDoc, VectorStore};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreStatus {
    pub root: String,
    pub project: Option<ProjectRow>,
    pub sqlite: RdbCounts,
    pub vectors: Vec<CollectionStatus>,
    pub vector_error: Option<String>,
    pub lancedb: Option<SeriesCounts>,
    pub lancedb_error: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub id: i64,
    pub similarity: f64,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeNeighbor {
    pub run_id: i64,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub coating: String,
    pub chem: String,
    pub coolant: String,
    pub dominant: String,
    pub tool_life_min: f64,
    pub similarity: f64,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeDoc {
    pub run_id: i64,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub project_id: i64,
    pub coating: String,
    pub chem: String,
    pub coolant: String,
    pub dominant: String,
    pub vc_m_min: f64,
    pub interface_c: f64,
    pub tool_life_min: f64,
    pub summary: String,
    pub vector: Vec<f32>,
}

pub struct Store {
    pub root: PathBuf,
    pub rdb: Rdb,
    pub vec: Option<VectorStore>,
    pub ts: Option<SeriesStore>,
    pub project_id: i64,
    pub notes: Vec<String>,
    vector_error: Option<String>,
    lancedb_error: Option<String>,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {}", root.display(), e))?;
        let rdb = Rdb::open(&root.join("library.sqlite"))?;
        Self::assemble(root, rdb)
    }

    pub fn open_with_memory_rdb(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {}", root.display(), e))?;
        Self::assemble(root, Rdb::open_in_memory()?)
    }

    fn assemble(root: &Path, rdb: Rdb) -> Result<Self, String> {
        let mut notes = Vec::new();
        let (vec, ts, vector_error, lancedb_error) = match LanceDb::open(&root.join("lancedb")) {
            Ok(db) => {
                let (vec, verr) = match VectorStore::with(db.clone()) {
                    Ok(mut v) => {
                        let mut err = None;
                        for spec in [vector::endmill_spec(), vector::workpiece_spec(), vector::regime_spec()] {
                            if let Err(e) = v.ensure(spec) {
                                err = Some(e);
                            }
                        }
                        notes.extend(v.notes.iter().cloned());
                        (Some(v), err)
                    }
                    Err(e) => (None, Some(e)),
                };
                let (ts, terr) = match SeriesStore::with(db) {
                    Ok(s) => (Some(s), None),
                    Err(e) => (None, Some(e)),
                };
                (vec, ts, verr, terr)
            }
            Err(e) => (None, None, Some(e.clone()), Some(e)),
        };
        if root.join("zvec").is_dir() {
            notes.push(
                "이전 zvec 폴더가 남아 있습니다 — 속성 벡터는 SQLite 기준으로 LanceDB 에 재색인되며, 이미지·형상 임베딩은 다음 입력부터 LanceDB 에 기록됩니다 (확인 후 zvec 폴더는 삭제해도 됩니다)"
                    .into(),
            );
        }
        let project_id = rdb.active_project()?;
        let mut st = Self {
            root: root.to_path_buf(),
            rdb,
            vec,
            ts,
            project_id,
            notes,
            vector_error,
            lancedb_error,
        };
        match st.reindex_attributes_if_stale() {
            Ok(n) if n > 0 => st.notes.push(format!("SQLite 기준으로 LanceDB 속성 벡터 {}건 재색인", n)),
            Err(e) => st.notes.push(format!("속성 벡터 재색인 실패: {}", e)),
            _ => {}
        }
        Ok(st)
    }

    pub fn status(&self) -> StoreStatus {
        let project = self
            .rdb
            .list_projects()
            .ok()
            .and_then(|v| v.into_iter().find(|p| p.id == self.project_id));
        StoreStatus {
            root: self.root.display().to_string(),
            project,
            sqlite: self.rdb.counts().unwrap_or_default(),
            vectors: self.vec.as_ref().map(|v| v.status()).unwrap_or_default(),
            vector_error: self.vector_error.clone(),
            lancedb: self.ts.as_ref().map(|t| t.counts()),
            lancedb_error: self.lancedb_error.clone(),
            notes: self.notes.iter().rev().take(20).cloned().collect(),
        }
    }

    pub fn flush(&mut self) -> Vec<String> {
        let mut log = Vec::new();
        if let Some(v) = self.vec.as_mut() {
            log.extend(v.flush());
        }
        if let Some(t) = self.ts.as_ref() {
            log.extend(t.compact());
        }
        log
    }

    pub fn use_project(&mut self, id: i64) -> Result<(), String> {
        self.rdb.set_active_project(id)?;
        self.project_id = id;
        Ok(())
    }

    pub fn create_project(&mut self, name: &str, description: &str) -> Result<i64, String> {
        let id = self.rdb.ensure_project(name, description)?;
        self.use_project(id)?;
        Ok(id)
    }

    pub fn index_endmill(&mut self, s: &EndMillMockupSetting) -> Result<i64, String> {
        let (id, created) = self.rdb.upsert_endmill(s)?;
        if created {
            self.put_endmill_vectors(&[(id, EndMillAttr::from_setting(s))])?;
        }
        Ok(id)
    }

    fn endmill_doc(id: i64, a: &EndMillAttr) -> VecDoc {
        VecDoc {
            pk: format!("em{}", id),
            vector: a.vector(),
            fields: vec![
                ("endmill_id".into(), Scalar::Int(id)),
                ("flutes".into(), Scalar::Int(a.flutes as i64)),
                ("diameter_mm".into(), Scalar::Real(a.diameter_mm)),
                ("nose".into(), Scalar::Text(a.nose.clone())),
                ("coating".into(), Scalar::Text(a.coating_family.clone())),
            ],
        }
    }

    fn put_endmill_vectors(&mut self, items: &[(i64, EndMillAttr)]) -> Result<usize, String> {
        let Some(v) = self.vec.as_mut() else {
            return Ok(0);
        };
        let docs: Vec<VecDoc> = items.iter().map(|(id, a)| Self::endmill_doc(*id, a)).collect();
        let n = v.upsert("endmill_attr", &docs)?;
        for (id, _) in items.iter() {
            self.rdb.add_vector_ref("endmill_attr", &format!("em{}", id), "endmill", *id, None, None)?;
        }
        Ok(n)
    }

    pub fn index_workpiece(&mut self, w: &WorkpieceSetup) -> Result<i64, String> {
        let (id, created) = self.rdb.upsert_workpiece(w)?;
        if created {
            self.put_workpiece_vectors(&[(id, WorkpieceAttr::from_setup(w), w.clone())])?;
        }
        Ok(id)
    }

    fn put_workpiece_vectors(&mut self, items: &[(i64, WorkpieceAttr, WorkpieceSetup)]) -> Result<usize, String> {
        let Some(v) = self.vec.as_mut() else {
            return Ok(0);
        };
        let docs: Vec<VecDoc> = items
            .iter()
            .map(|(id, a, w)| VecDoc {
                pk: format!("wp{}", id),
                vector: a.vector(w),
                fields: vec![
                    ("workpiece_id".into(), Scalar::Int(*id)),
                    ("material_family".into(), Scalar::Text(a.material_family.clone())),
                    ("hardness_hrc".into(), Scalar::Real(a.hardness_hrc.unwrap_or(-1.0))),
                ],
            })
            .collect();
        let n = v.upsert("workpiece_attr", &docs)?;
        for (id, _, _) in items.iter() {
            self.rdb.add_vector_ref("workpiece_attr", &format!("wp{}", id), "workpiece", *id, None, None)?;
        }
        Ok(n)
    }

    fn reindex_attributes_if_stale(&mut self) -> Result<usize, String> {
        if self.vec.is_none() {
            return Ok(0);
        }
        let docs = self
            .vec
            .as_ref()
            .map(|v| v.status().iter().filter(|c| c.name == "endmill_attr" || c.name == "workpiece_attr").map(|c| c.docs).sum::<u64>())
            .unwrap_or(0);
        let counts = self.rdb.counts()?;
        let version_ok = self.rdb.meta("attr_vector_version")?.as_deref() == Some(attr::ATTR_VECTOR_VERSION);
        if version_ok && docs as i64 >= counts.endmills + counts.workpieces {
            return Ok(0);
        }
        self.reindex_attributes()
    }

    pub fn reindex_attributes(&mut self) -> Result<usize, String> {
        if self.vec.is_none() {
            return Err("LanceDB 벡터 저장소가 열려 있지 않습니다".into());
        }
        let em: Vec<(i64, EndMillAttr)> = self.rdb.list_endmills(100_000)?.into_iter().map(|e| (e.id, e.attr)).collect();
        let wp: Vec<(i64, WorkpieceAttr, WorkpieceSetup)> =
            self.rdb.list_workpieces(100_000)?.into_iter().map(|w| (w.id, w.attr, w.setup)).collect();
        let n = em.len() + wp.len();
        self.put_endmill_vectors(&em)?;
        self.put_workpiece_vectors(&wp)?;
        if let Some(v) = self.vec.as_mut() {
            v.optimize_all();
        }
        self.rdb.set_meta("attr_vector_version", attr::ATTR_VECTOR_VERSION)?;
        Ok(n)
    }

    pub fn put_regime(&mut self, d: &RegimeDoc) -> Result<Option<vector::IndexKind>, String> {
        if d.vector.len() != crate::tribology::REGIME_DIM {
            return Ok(None);
        }
        let Some(v) = self.vec.as_mut() else {
            return Ok(None);
        };
        let index = v.ensure(vector::regime_spec())?;
        let pk = format!("r{}", d.run_id);
        v.upsert(
            "cut_regime",
            &[VecDoc {
                pk: pk.clone(),
                vector: d.vector.clone(),
                fields: vec![
                    ("endmill_id".into(), Scalar::Int(d.endmill_id)),
                    ("workpiece_id".into(), Scalar::Int(d.workpiece_id)),
                    ("project_id".into(), Scalar::Int(d.project_id)),
                    ("run_id".into(), Scalar::Int(d.run_id)),
                    ("coating".into(), Scalar::Text(d.coating.clone())),
                    ("chem".into(), Scalar::Text(d.chem.clone())),
                    ("coolant".into(), Scalar::Text(d.coolant.clone())),
                    ("dominant".into(), Scalar::Text(d.dominant.clone())),
                    ("vc_m_min".into(), Scalar::Real(d.vc_m_min)),
                    ("interface_c".into(), Scalar::Real(d.interface_c)),
                    ("tool_life_min".into(), Scalar::Real(d.tool_life_min)),
                    ("summary".into(), Scalar::Text(d.summary.clone())),
                ],
            }],
        )?;
        self.rdb
            .add_vector_ref("cut_regime", &pk, "pair", d.endmill_id, Some(d.project_id), Some(d.run_id))?;
        Ok(Some(index))
    }

    pub fn similar_regimes(&mut self, regime: &[f32], k: usize, filter: Option<&str>) -> Result<Vec<RegimeNeighbor>, String> {
        let Some(v) = self.vec.as_mut() else {
            return Ok(Vec::new());
        };
        if regime.len() != crate::tribology::REGIME_DIM {
            return Ok(Vec::new());
        }
        let hits = v.search("cut_regime", regime, k, filter)?;
        Ok(hits
            .into_iter()
            .map(|h| RegimeNeighbor {
                run_id: h.int("run_id").unwrap_or(-1),
                endmill_id: h.int("endmill_id").unwrap_or(-1),
                workpiece_id: h.int("workpiece_id").unwrap_or(-1),
                coating: h.text("coating"),
                chem: h.text("chem"),
                coolant: h.text("coolant"),
                dominant: h.text("dominant"),
                tool_life_min: h.real("tool_life_min").unwrap_or(0.0),
                similarity: h.similarity,
                distance: h.score,
            })
            .collect())
    }

    pub fn similar_endmills(&mut self, a: &EndMillAttr, k: usize, exclude: Option<i64>, filter: Option<&str>) -> Result<Vec<Neighbor>, String> {
        let Some(v) = self.vec.as_mut() else {
            return Ok(Vec::new());
        };
        let mut f: Vec<String> = Vec::new();
        if let Some(x) = exclude {
            f.push(format!("endmill_id != {}", x));
        }
        if let Some(extra) = filter.filter(|s| !s.trim().is_empty()) {
            f.push(extra.to_string());
        }
        let filter = if f.is_empty() { None } else { Some(f.join(" and ")) };
        let hits = v.search("endmill_attr", &a.vector(), k, filter.as_deref())?;
        Ok(hits
            .into_iter()
            .filter_map(|h| {
                h.int("endmill_id").map(|id| Neighbor {
                    id,
                    similarity: h.similarity,
                    distance: h.score,
                })
            })
            .collect())
    }

    pub fn similar_workpieces(&mut self, w: &WorkpieceSetup, k: usize, exclude: Option<i64>) -> Result<Vec<Neighbor>, String> {
        let Some(v) = self.vec.as_mut() else {
            return Ok(Vec::new());
        };
        let a = WorkpieceAttr::from_setup(w);
        let filter = exclude.map(|x| format!("workpiece_id != {}", x));
        let hits = v.search("workpiece_attr", &a.vector(w), k, filter.as_deref())?;
        Ok(hits
            .into_iter()
            .filter_map(|h| {
                h.int("workpiece_id").map(|id| Neighbor {
                    id,
                    similarity: h.similarity,
                    distance: h.score,
                })
            })
            .collect())
    }

    pub fn sync_builtin_presets(&mut self, presets: &[MachiningPreset]) -> Result<(), String> {
        for p in presets.iter() {
            self.index_endmill(&p.endmill_setting)?;
            self.index_workpiece(&p.workpiece_setup)?;
            self.rdb.upsert_preset(p, true)?;
        }
        Ok(())
    }

    pub fn save_preset(&mut self, p: &MachiningPreset) -> Result<i64, String> {
        self.index_endmill(&p.endmill_setting)?;
        self.index_workpiece(&p.workpiece_setup)?;
        self.rdb.upsert_preset(p, false)
    }

    pub fn save_profile(&mut self, p: &MachiningProfile, preset_name: Option<&str>) -> Result<i64, String> {
        self.index_endmill(&p.endmill_setting)?;
        self.index_workpiece(&p.workpiece_setup)?;
        let preset_id = match preset_name {
            Some(n) => self.rdb.preset_id(n)?,
            None => self.rdb.preset_id(&p.name)?,
        };
        self.rdb.upsert_profile(self.project_id, p, preset_id)
    }

    pub fn profile_store(&self) -> Result<ProfileStore, String> {
        let presets: Vec<MachiningPreset> = self.rdb.list_presets()?.into_iter().map(|r| r.preset).collect();
        let profiles: Vec<MachiningProfile> = self
            .rdb
            .list_profiles(self.project_id)?
            .into_iter()
            .map(|r| r.profile)
            .collect();
        Ok(ProfileStore { profiles, presets })
    }

    pub fn import_legacy(&mut self, data_dir: &Path) -> Result<Vec<String>, String> {
        let mut log = Vec::new();
        if self.rdb.meta("legacy_imported")?.is_some() {
            return Ok(log);
        }
        let builtin = ProfileStore::new();
        self.sync_builtin_presets(&builtin.presets)?;
        let path = data_dir.join("profiles.json");
        if let Ok(legacy) = ProfileStore::load_from_file(&path.display().to_string()) {
            for p in legacy.presets.iter() {
                if self.rdb.preset_id(&p.name)?.is_none() {
                    self.save_preset(p)?;
                }
            }
            for p in legacy.profiles.iter() {
                let preset = legacy.presets.iter().find(|x| x.name == p.name).map(|x| x.name.clone());
                self.save_profile(p, preset.as_deref())?;
            }
            log.push(format!("기존 profiles.json 프로필 {}개를 '{}' 프로젝트로 이관", legacy.profiles.len(), self.project_label()));
        }
        if let Ok(t) = std::fs::read_to_string(data_dir.join("seen.json")) {
            if let Ok(seen) = serde_json::from_str::<BTreeSet<String>>(&t) {
                for fp in seen.iter() {
                    self.rdb.mark_seen(fp, "legacy", "", None)?;
                }
                log.push(format!("seen.json 지문 {}건을 SQLite 로 이관", seen.len()));
            }
        }
        self.rdb.set_meta("legacy_imported", &now_ms().to_string())?;
        Ok(log)
    }

    pub fn project_label(&self) -> String {
        self.rdb
            .project_name(self.project_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| format!("#{}", self.project_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("endmill_store_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn attribute_neighbors_come_from_lancedb() {
        let dir = tmp("nb");
        let mut st = Store::open(&dir).unwrap();
        let a = EndMillMockupSetting::default_10mm_4flute();
        let mut b = a.clone();
        b.diameter_mm = 10.5;
        b.model = "B".into();
        let c = EndMillMockupSetting::default_6mm_2flute_aluminum();
        let ia = st.index_endmill(&a).unwrap();
        let ib = st.index_endmill(&b).unwrap();
        let ic = st.index_endmill(&c).unwrap();
        assert_ne!(ia, ib);
        let nb = st.similar_endmills(&EndMillAttr::from_setting(&a), 3, Some(ia), None).unwrap();
        assert_eq!(nb[0].id, ib);
        assert!(nb.iter().any(|n| n.id == ic));
        assert!(nb[0].similarity > nb[nb.len() - 1].similarity);
        let w1 = st.index_workpiece(&WorkpieceSetup::default_steel_block()).unwrap();
        let w2 = st.index_workpiece(&WorkpieceSetup::default_aluminum_plate()).unwrap();
        let mut steel2 = WorkpieceSetup::default_steel_block();
        steel2.width_mm = 140.0;
        let w3 = st.index_workpiece(&steel2).unwrap();
        let nw = st.similar_workpieces(&WorkpieceSetup::default_steel_block(), 2, Some(w1)).unwrap();
        assert_eq!(nw[0].id, w3);
        assert_ne!(nw[0].id, w2);
        st.flush();
        drop(st);
        let mut st = Store::open(&dir).unwrap();
        let nb = st.similar_endmills(&EndMillAttr::from_setting(&a), 1, Some(ia), None).unwrap();
        assert_eq!(nb[0].id, ib);
        let filtered = st.similar_endmills(&EndMillAttr::from_setting(&a), 3, Some(ia), Some("flutes = 2")).unwrap();
        assert!(filtered.iter().all(|n| n.id == ic));
        let status = st.status();
        assert!(status.vectors.iter().any(|c| c.name == "endmill_attr" && c.docs == 3));
        assert!(status.vectors.iter().any(|c| c.name == "cut_regime"));
        assert!(status.lancedb.is_some());
        drop(st);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_attribute_vectors_are_rebuilt_from_sqlite() {
        let dir = tmp("stale");
        let mut st = Store::open(&dir).unwrap();
        let a = EndMillMockupSetting::default_10mm_4flute();
        let ia = st.index_endmill(&a).unwrap();
        st.rdb.set_meta("attr_vector_version", "old").unwrap();
        drop(st);
        let mut st = Store::open(&dir).unwrap();
        assert!(st.notes.iter().any(|n| n.contains("재색인")), "{:?}", st.notes);
        let nb = st.similar_endmills(&EndMillAttr::from_setting(&a), 1, None, None).unwrap();
        assert_eq!(nb[0].id, ia);
        drop(st);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_import_and_project_scoped_profiles() {
        let dir = tmp("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let mut legacy = ProfileStore::new();
        let p = MachiningProfile::from_preset(&legacy.presets[0].clone(), "레거시 프로필");
        legacy.add_profile(p);
        legacy.save_to_file(&dir.join("profiles.json").display().to_string()).unwrap();
        std::fs::write(dir.join("seen.json"), "[\"abc\"]").unwrap();
        let mut st = Store::open(&dir.join("library")).unwrap();
        let log = st.import_legacy(&dir).unwrap();
        assert_eq!(log.len(), 2);
        assert!(st.import_legacy(&dir).unwrap().is_empty());
        let ps = st.profile_store().unwrap();
        assert_eq!(ps.profiles.len(), 1);
        assert!(ps.presets.len() >= 3);
        assert!(st.rdb.is_seen("abc").unwrap());
        let other = st.create_project("두번째", "").unwrap();
        assert_eq!(st.project_id, other);
        assert!(st.profile_store().unwrap().profiles.is_empty());
        drop(st);
        let _ = std::fs::remove_dir_all(&dir);
    }
}