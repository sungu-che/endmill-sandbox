use super::lance::{lerr, sql_str, LanceDb};
use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, Float64Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema, SchemaRef};
use lancedb::index::vector::IvfHnswSqIndexBuilder;
use lancedb::index::{Index, IndexType};
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::OptimizeAction;
use lancedb::{DistanceType, Table};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

pub const ANN_MIN_ROWS: usize = 4096;
pub const ANN_NPROBES: usize = 32;
pub const ANN_REFINE: u32 = 4;
const OPTIMIZE_EVERY: usize = 256;
const PK: &str = "pk";
const VECTOR: &str = "vector";
const VECTOR_INDEX: &str = "vector_idx";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexKind {
    Flat,
    IvfHnswSq,
}

impl IndexKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Flat => "Flat 정확 검색",
            Self::IvfHnswSq => "IVF_HNSW_SQ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    L2,
    Cosine,
}

impl Metric {
    fn distance(&self) -> DistanceType {
        match self {
            Self::L2 => DistanceType::L2,
            Self::Cosine => DistanceType::Cosine,
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Self::L2 => "l2",
            Self::Cosine => "cosine",
        }
    }

    pub fn similarity(&self, score: f32) -> f64 {
        match self {
            Self::L2 => super::attr::similarity_from_l2(score),
            Self::Cosine => (1.0 - score as f64).clamp(-1.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    Int,
    Real,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub kind: FieldKind,
    pub indexed: bool,
}

impl FieldDef {
    pub fn int(name: &str) -> Self {
        Self { name: name.into(), kind: FieldKind::Int, indexed: true }
    }
    pub fn real(name: &str) -> Self {
        Self { name: name.into(), kind: FieldKind::Real, indexed: true }
    }
    pub fn text(name: &str) -> Self {
        Self { name: name.into(), kind: FieldKind::Text, indexed: true }
    }
    pub fn note(name: &str) -> Self {
        Self { name: name.into(), kind: FieldKind::Text, indexed: false }
    }

    fn arrow(&self) -> DataType {
        match self.kind {
            FieldKind::Int => DataType::Int64,
            FieldKind::Real => DataType::Float64,
            FieldKind::Text => DataType::Utf8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionSpec {
    pub name: String,
    pub dim: u32,
    pub metric: Metric,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scalar {
    Int(i64),
    Real(f64),
    Text(String),
}

impl Scalar {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Real(v) => Some(*v as i64),
            Self::Text(t) => t.parse().ok(),
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(v) => Some(*v as f64),
            Self::Real(v) => Some(*v),
            Self::Text(t) => t.parse().ok(),
        }
    }
    pub fn as_text(&self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::Real(v) => format!("{}", v),
            Self::Text(t) => t.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VecDoc {
    pub pk: String,
    pub vector: Vec<f32>,
    pub fields: Vec<(String, Scalar)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VecHit {
    pub pk: String,
    pub score: f32,
    pub similarity: f64,
    pub fields: BTreeMap<String, Scalar>,
}

impl VecHit {
    pub fn int(&self, k: &str) -> Option<i64> {
        self.fields.get(k).and_then(|v| v.as_i64())
    }
    pub fn real(&self, k: &str) -> Option<f64> {
        self.fields.get(k).and_then(|v| v.as_f64())
    }
    pub fn text(&self, k: &str) -> String {
        self.fields.get(k).map(|v| v.as_text()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStatus {
    pub name: String,
    pub dim: u32,
    pub metric: Metric,
    pub index: IndexKind,
    pub docs: u64,
    pub index_completeness: f32,
    pub pending: usize,
}

struct Handle {
    table: Table,
    spec: CollectionSpec,
    index: IndexKind,
    pending: usize,
}

pub struct VectorStore {
    cols: BTreeMap<String, Handle>,
    pub notes: Vec<String>,
    db: Arc<LanceDb>,
}

fn schema_of(spec: &CollectionSpec) -> SchemaRef {
    let mut fields = vec![Field::new(PK, DataType::Utf8, false)];
    for f in spec.fields.iter() {
        fields.push(Field::new(&f.name, f.arrow(), true));
    }
    fields.push(Field::new(
        VECTOR,
        DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), spec.dim as i32),
        false,
    ));
    let meta: HashMap<String, String> = [("metric".to_string(), spec.metric.key().to_string())].into_iter().collect();
    Arc::new(Schema::new_with_metadata(fields, meta))
}

fn vector_dim(schema: &Schema) -> Option<u32> {
    match schema.field_with_name(VECTOR).ok()?.data_type() {
        DataType::FixedSizeList(_, n) => Some(*n as u32),
        _ => None,
    }
}

fn compatible(schema: &Schema, spec: &CollectionSpec) -> bool {
    let metric_ok = schema.metadata().get("metric").map(|m| m == spec.metric.key()).unwrap_or(true);
    let fields_ok = spec
        .fields
        .iter()
        .all(|f| schema.field_with_name(&f.name).map(|x| x.data_type() == &f.arrow()).unwrap_or(false));
    metric_ok && fields_ok && schema.fields().len() == spec.fields.len() + 2 && vector_dim(schema) == Some(spec.dim)
}

fn batch_of(spec: &CollectionSpec, docs: &[&VecDoc]) -> Result<RecordBatch, String> {
    let mut cols: Vec<ArrayRef> = Vec::with_capacity(spec.fields.len() + 2);
    cols.push(Arc::new(StringArray::from(docs.iter().map(|d| d.pk.as_str()).collect::<Vec<&str>>())));
    let value = |d: &VecDoc, name: &str| d.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    for f in spec.fields.iter() {
        let arr: ArrayRef = match f.kind {
            FieldKind::Int => Arc::new(Int64Array::from(
                docs.iter()
                    .map(|d| value(*d, &f.name).and_then(|v| v.as_i64()).or(if f.indexed { Some(-1) } else { None }))
                    .collect::<Vec<Option<i64>>>(),
            )),
            FieldKind::Real => Arc::new(Float64Array::from(
                docs.iter()
                    .map(|d| value(*d, &f.name).and_then(|v| v.as_f64()).or(if f.indexed { Some(0.0) } else { None }))
                    .collect::<Vec<Option<f64>>>(),
            )),
            FieldKind::Text => Arc::new(StringArray::from(
                docs.iter()
                    .map(|d| value(*d, &f.name).map(|v| v.as_text()).or(if f.indexed { Some(String::new()) } else { None }))
                    .collect::<Vec<Option<String>>>(),
            )),
        };
        cols.push(arr);
    }
    let mut flat: Vec<f32> = Vec::with_capacity(docs.len() * spec.dim as usize);
    for d in docs.iter() {
        flat.extend_from_slice(&d.vector);
    }
    let vec = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        spec.dim as i32,
        Arc::new(Float32Array::from(flat)),
        None,
    )
    .map_err(|e| e.to_string())?;
    cols.push(Arc::new(vec));
    RecordBatch::try_new(schema_of(spec), cols).map_err(|e| format!("Arrow 배치 생성 실패: {}", e))
}

fn read_fields(b: &RecordBatch, i: usize, fields: &[FieldDef]) -> BTreeMap<String, Scalar> {
    let mut out = BTreeMap::new();
    for f in fields.iter() {
        let Some(c) = b.column_by_name(&f.name) else {
            continue;
        };
        if c.is_null(i) {
            continue;
        }
        let v = match f.kind {
            FieldKind::Int => c.as_any().downcast_ref::<Int64Array>().map(|a| Scalar::Int(a.value(i))),
            FieldKind::Real => c.as_any().downcast_ref::<Float64Array>().map(|a| Scalar::Real(a.value(i))),
            FieldKind::Text => c.as_any().downcast_ref::<StringArray>().map(|a| Scalar::Text(a.value(i).to_string())),
        };
        if let Some(v) = v {
            out.insert(f.name.clone(), v);
        }
    }
    out
}

fn pk_column(b: &RecordBatch) -> Result<&StringArray, String> {
    b.column_by_name(PK)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| "열 'pk' 없음".to_string())
}

async fn index_kind(t: &Table) -> IndexKind {
    match t.list_indices().await {
        Ok(list) if list.iter().any(|i| i.columns.iter().any(|c| c == VECTOR) && i.index_type == IndexType::IvfHnswSq) => IndexKind::IvfHnswSq,
        _ => IndexKind::Flat,
    }
}

impl VectorStore {
    pub fn open(root: &Path) -> Result<Self, String> {
        Self::with(LanceDb::open(root)?)
    }

    pub fn with(db: Arc<LanceDb>) -> Result<Self, String> {
        Ok(Self {
            cols: BTreeMap::new(),
            notes: vec![format!("LanceDB 벡터 저장소 {}", db.path().display())],
            db,
        })
    }

    pub fn root(&self) -> &Path {
        self.db.path()
    }

    pub fn has(&self, name: &str) -> bool {
        self.cols.contains_key(name)
    }

    pub fn ensure(&mut self, spec: CollectionSpec) -> Result<IndexKind, String> {
        if let Some(h) = self.cols.get(&spec.name) {
            if h.spec.dim == spec.dim {
                return Ok(h.index);
            }
            return Err(format!("컬렉션 '{}' 차원 불일치 ({} ≠ {})", spec.name, h.spec.dim, spec.dim));
        }
        if let Some(t) = self.db.open_table(&spec.name)? {
            let schema = self.db.block_on(async { t.schema().await.map_err(lerr) })?;
            if compatible(&schema, &spec) {
                let index = self.db.block_on(index_kind(&t));
                let name = spec.name.clone();
                self.cols.insert(name, Handle { table: t, spec, index, pending: 0 });
                return Ok(index);
            }
            drop(t);
            let dir = self.db.table_dir(&spec.name);
            let bak = self.db.path().join(format!("{}.lance.bak-{}", spec.name, super::now_ms()));
            match std::fs::rename(&dir, &bak) {
                Ok(()) => self.notes.push(format!(
                    "'{}' 스키마(차원·필드·거리) 변경 → 기존 테이블을 {} 로 보관 후 재생성",
                    spec.name,
                    bak.display()
                )),
                Err(_) => {
                    self.db.drop_table(&spec.name)?;
                    self.notes.push(format!("'{}' 스키마 변경 → 기존 테이블 삭제 후 재생성", spec.name));
                }
            }
        }
        let table = self.db.create_empty(&spec.name, schema_of(&spec))?;
        let name = spec.name.clone();
        self.cols.insert(name, Handle { table, spec, index: IndexKind::Flat, pending: 0 });
        Ok(IndexKind::Flat)
    }

    fn handle(&mut self, name: &str) -> Result<&mut Handle, String> {
        self.cols
            .get_mut(name)
            .ok_or_else(|| format!("벡터 컬렉션 '{}' 이(가) 열려 있지 않습니다", name))
    }

    pub fn upsert(&mut self, name: &str, docs: &[VecDoc]) -> Result<usize, String> {
        if docs.is_empty() {
            return Ok(0);
        }
        let db = self.db.clone();
        let h = self.handle(name)?;
        let mut uniq: BTreeMap<&str, &VecDoc> = BTreeMap::new();
        for d in docs.iter() {
            if d.vector.len() as u32 != h.spec.dim {
                return Err(format!("'{}' 벡터 차원 {} ≠ {}", name, d.vector.len(), h.spec.dim));
            }
            if d.vector.iter().any(|x| !x.is_finite()) {
                return Err(format!("'{}' 벡터에 유한하지 않은 값이 있습니다 ({})", name, d.pk));
            }
            uniq.insert(d.pk.as_str(), d);
        }
        let rows: Vec<&VecDoc> = uniq.into_values().collect();
        let batch = batch_of(&h.spec, &rows)?;
        let schema = batch.schema();
        let t = h.table.clone();
        let n = db.block_on(async move {
            let mut m = t.merge_insert(&[PK]);
            m.when_matched_update_all(None).when_not_matched_insert_all();
            let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
            let r = m.execute(Box::new(reader)).await.map_err(lerr)?;
            Ok::<_, String>((r.num_inserted_rows + r.num_updated_rows) as usize)
        })?;
        h.pending += n;
        if h.pending >= OPTIMIZE_EVERY {
            let name = name.to_string();
            self.optimize_one(&name)?;
        }
        Ok(n)
    }

    pub fn search(&mut self, name: &str, vector: &[f32], topk: usize, filter: Option<&str>) -> Result<Vec<VecHit>, String> {
        let db = self.db.clone();
        let h = self.handle(name)?;
        if vector.len() as u32 != h.spec.dim {
            return Err(format!("'{}' 질의 벡터 차원 {} ≠ {}", name, vector.len(), h.spec.dim));
        }
        let metric = h.spec.metric;
        let fields = h.spec.fields.clone();
        let mut cols: Vec<String> = vec![PK.to_string()];
        cols.extend(fields.iter().map(|f| f.name.clone()));
        let t = h.table.clone();
        let ann = h.index == IndexKind::IvfHnswSq;
        let qv = vector.to_vec();
        let filter = filter.filter(|f| !f.trim().is_empty()).map(|f| f.to_string());
        let batches = db.block_on(async move {
            let mut q = t
                .query()
                .nearest_to(qv.as_slice())
                .map_err(lerr)?
                .distance_type(metric.distance())
                .limit(topk.max(1))
                .select(Select::columns(&cols));
            if let Some(f) = filter {
                q = q.only_if(f);
            }
            if ann {
                q = q.nprobes(ANN_NPROBES).refine_factor(ANN_REFINE);
            }
            q.execute().await.map_err(lerr)?.try_collect::<Vec<_>>().await.map_err(lerr)
        })?;
        let mut out = Vec::new();
        for b in batches.iter() {
            let pk = pk_column(b)?;
            let dist = b
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .ok_or_else(|| "열 '_distance' 없음".to_string())?;
            for i in 0..b.num_rows() {
                let score = dist.value(i);
                out.push(VecHit {
                    pk: pk.value(i).to_string(),
                    score,
                    similarity: metric.similarity(score),
                    fields: read_fields(b, i, &fields),
                });
            }
        }
        out.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }

    pub fn fetch(&mut self, name: &str, pk: &str) -> Result<Option<(Vec<f32>, BTreeMap<String, Scalar>)>, String> {
        let db = self.db.clone();
        let h = self.handle(name)?;
        let fields = h.spec.fields.clone();
        let t = h.table.clone();
        let filter = format!("{} = {}", PK, sql_str(pk));
        let batches = db.block_on(async move {
            t.query()
                .only_if(filter)
                .limit(1)
                .execute()
                .await
                .map_err(lerr)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(lerr)
        })?;
        for b in batches.iter() {
            if b.num_rows() == 0 {
                continue;
            }
            let vec = b
                .column_by_name(VECTOR)
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
                .ok_or_else(|| "열 'vector' 없음".to_string())?;
            let v = vec.value(0);
            let vf = v
                .as_any()
                .downcast_ref::<Float32Array>()
                .map(|a| a.values().to_vec())
                .unwrap_or_default();
            return Ok(Some((vf, read_fields(b, 0, &fields))));
        }
        Ok(None)
    }

    pub fn delete(&mut self, name: &str, pks: &[&str]) -> Result<u64, String> {
        if pks.is_empty() {
            return Ok(0);
        }
        let db = self.db.clone();
        let h = self.handle(name)?;
        let t = h.table.clone();
        let list = pks.iter().map(|p| sql_str(p)).collect::<Vec<_>>().join(", ");
        let pred = format!("{} IN ({})", PK, list);
        db.block_on(async move { t.delete(pred.as_str()).await.map(|r| r.num_deleted_rows).map_err(lerr) })
    }

    fn optimize_one(&mut self, name: &str) -> Result<String, String> {
        let db = self.db.clone();
        let h = self.handle(name)?;
        let t = h.table.clone();
        let metric = h.spec.metric;
        let had_ann = h.index == IndexKind::IvfHnswSq;
        let (rows, built) = db.block_on(async move {
            t.optimize(OptimizeAction::All).await.map_err(lerr)?;
            let rows = t.count_rows(None).await.map_err(lerr)?;
            let mut built = false;
            if !had_ann && rows >= ANN_MIN_ROWS {
                t.create_index(&[VECTOR], Index::IvfHnswSq(IvfHnswSqIndexBuilder::default().distance_type(metric.distance())))
                    .name(VECTOR_INDEX.to_string())
                    .execute()
                    .await
                    .map_err(lerr)?;
                built = true;
            }
            Ok::<_, String>((rows, built))
        })?;
        let pending = h.pending;
        h.pending = 0;
        if built {
            h.index = IndexKind::IvfHnswSq;
        }
        Ok(format!(
            "{}: {}건 반영 · 전체 {}건 ({}{})",
            name,
            pending,
            rows,
            h.index.label(),
            if built { " 인덱스 신규 생성" } else { "" }
        ))
    }

    pub fn optimize_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self.cols.iter().filter(|(_, h)| h.pending > 0).map(|(n, _)| n.clone()).collect();
        let mut log = Vec::new();
        for n in names {
            match self.optimize_one(&n) {
                Ok(l) => log.push(l),
                Err(e) => log.push(format!("{}: 정리 실패 {}", n, e)),
            }
        }
        log
    }

    pub fn flush(&mut self) -> Vec<String> {
        self.optimize_all()
    }

    pub fn status(&self) -> Vec<CollectionStatus> {
        self.cols
            .iter()
            .map(|(name, h)| {
                let t = h.table.clone();
                let ann = h.index == IndexKind::IvfHnswSq;
                let (docs, completeness) = self.db.block_on(async move {
                    let docs = t.count_rows(None).await.unwrap_or(0) as u64;
                    let completeness = if ann {
                        match t.index_stats(VECTOR_INDEX).await {
                            Ok(Some(s)) if s.num_indexed_rows + s.num_unindexed_rows > 0 => {
                                s.num_indexed_rows as f32 / (s.num_indexed_rows + s.num_unindexed_rows) as f32
                            }
                            _ => 0.0,
                        }
                    } else {
                        1.0
                    };
                    (docs, completeness)
                });
                CollectionStatus {
                    name: name.clone(),
                    dim: h.spec.dim,
                    metric: h.spec.metric,
                    index: h.index,
                    docs,
                    index_completeness: completeness,
                    pending: h.pending,
                }
            })
            .collect()
    }
}

pub fn endmill_spec() -> CollectionSpec {
    CollectionSpec {
        name: "endmill_attr".into(),
        dim: super::attr::ENDMILL_DIM as u32,
        metric: Metric::L2,
        fields: vec![
            FieldDef::int("endmill_id"),
            FieldDef::int("flutes"),
            FieldDef::real("diameter_mm"),
            FieldDef::text("nose"),
            FieldDef::text("coating"),
        ],
    }
}

pub fn workpiece_spec() -> CollectionSpec {
    CollectionSpec {
        name: "workpiece_attr".into(),
        dim: super::attr::WORKPIECE_DIM as u32,
        metric: Metric::L2,
        fields: vec![
            FieldDef::int("workpiece_id"),
            FieldDef::text("material_family"),
            FieldDef::real("hardness_hrc"),
        ],
    }
}

pub fn regime_spec() -> CollectionSpec {
    CollectionSpec {
        name: "cut_regime".into(),
        dim: crate::tribology::REGIME_DIM as u32,
        metric: Metric::L2,
        fields: vec![
            FieldDef::int("endmill_id"),
            FieldDef::int("workpiece_id"),
            FieldDef::int("project_id"),
            FieldDef::int("run_id"),
            FieldDef::text("coating"),
            FieldDef::text("chem"),
            FieldDef::text("coolant"),
            FieldDef::text("dominant"),
            FieldDef::real("vc_m_min"),
            FieldDef::real("interface_c"),
            FieldDef::real("tool_life_min"),
            FieldDef::note("summary"),
        ],
    }
}

pub fn tool_image_spec(dim: u32) -> CollectionSpec {
    CollectionSpec {
        name: format!("tool_image_d{}", dim),
        dim,
        metric: Metric::Cosine,
        fields: vec![
            FieldDef::int("endmill_id"),
            FieldDef::int("workpiece_id"),
            FieldDef::int("project_id"),
            FieldDef::int("run_id"),
            FieldDef::text("tool_key"),
            FieldDef::text("view"),
            FieldDef::text("role"),
            FieldDef::text("state"),
            FieldDef::real("minutes"),
            FieldDef::real("radial_loss_um"),
        ],
    }
}

pub fn mold_geom_spec(dim: u32) -> CollectionSpec {
    CollectionSpec {
        name: format!("mold_geom_d{}", dim),
        dim,
        metric: Metric::Cosine,
        fields: vec![
            FieldDef::int("workpiece_id"),
            FieldDef::int("endmill_id"),
            FieldDef::int("project_id"),
            FieldDef::text("mold_id"),
            FieldDef::text("role"),
            FieldDef::real("max_depth_mm"),
            FieldDef::real("steep_ratio"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("endmill_vec_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn lancedb_roundtrip_with_filter_and_reopen() {
        let dir = tmp("rt");
        let spec = CollectionSpec {
            name: "t_attr".into(),
            dim: 4,
            metric: Metric::L2,
            fields: vec![FieldDef::int("eid"), FieldDef::text("fam"), FieldDef::note("memo")],
        };
        {
            let mut vs = VectorStore::open(&dir).unwrap();
            assert_eq!(vs.ensure(spec.clone()).unwrap(), IndexKind::Flat);
            let docs: Vec<VecDoc> = (0..5)
                .map(|i| VecDoc {
                    pk: format!("e{}", i),
                    vector: vec![i as f32, 0.0, 0.0, 0.0],
                    fields: vec![
                        ("eid".into(), Scalar::Int(i)),
                        ("fam".into(), Scalar::Text(if i % 2 == 0 { "a".into() } else { "b".into() })),
                    ],
                })
                .collect();
            assert_eq!(vs.upsert("t_attr", &docs).unwrap(), 5);
            let hits = vs.search("t_attr", &[1.2, 0.0, 0.0, 0.0], 3, Some("fam = 'a'")).unwrap();
            assert_eq!(hits[0].pk, "e2");
            assert!(hits.iter().all(|h| h.text("fam") == "a"));
            assert!((hits[0].score - 0.64).abs() < 1e-4, "{}", hits[0].score);
            assert!(hits[0].fields.get("memo").is_none());
            let hits = vs.search("t_attr", &[0.0, 0.0, 0.0, 0.0], 2, Some("eid != 0")).unwrap();
            assert_eq!(hits[0].int("eid"), Some(1));
            let moved = VecDoc {
                pk: "e4".into(),
                vector: vec![1.1, 0.0, 0.0, 0.0],
                fields: vec![("eid".into(), Scalar::Int(4)), ("fam".into(), Scalar::Text("a".into()))],
            };
            assert_eq!(vs.upsert("t_attr", &[moved]).unwrap(), 1);
            let hits = vs.search("t_attr", &[1.2, 0.0, 0.0, 0.0], 1, Some("fam = 'a'")).unwrap();
            assert_eq!(hits[0].pk, "e4");
            assert_eq!(vs.delete("t_attr", &["e0"]).unwrap(), 1);
            vs.flush();
        }
        let mut vs = VectorStore::open(&dir).unwrap();
        vs.ensure(spec).unwrap();
        let st = vs.status();
        assert_eq!(st[0].docs, 4);
        let got = vs.fetch("t_attr", "e3").unwrap().unwrap();
        assert_eq!(got.0, vec![3.0, 0.0, 0.0, 0.0]);
        assert_eq!(got.1.get("eid"), Some(&Scalar::Int(3)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_change_rotates_collection() {
        let dir = tmp("dim");
        let mk = |dim| CollectionSpec {
            name: "t_dim".into(),
            dim,
            metric: Metric::Cosine,
            fields: vec![FieldDef::int("eid")],
        };
        {
            let mut vs = VectorStore::open(&dir).unwrap();
            vs.ensure(mk(3)).unwrap();
            vs.upsert(
                "t_dim",
                &[VecDoc { pk: "a".into(), vector: vec![1.0, 0.0, 0.0], fields: vec![("eid".into(), Scalar::Int(1))] }],
            )
            .unwrap();
            let hits = vs.search("t_dim", &[2.0, 0.0, 0.0], 1, None).unwrap();
            assert!(hits[0].score.abs() < 1e-5 && (hits[0].similarity - 1.0).abs() < 1e-5);
            vs.flush();
        }
        let mut vs = VectorStore::open(&dir).unwrap();
        vs.ensure(mk(5)).unwrap();
        assert_eq!(vs.status()[0].docs, 0);
        assert!(vs.notes.iter().any(|n| n.contains("변경")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_collection_builds_ann_index() {
        let dir = tmp("ann");
        let spec = CollectionSpec {
            name: "t_ann".into(),
            dim: 8,
            metric: Metric::L2,
            fields: vec![FieldDef::int("eid")],
        };
        let mut vs = VectorStore::open(&dir).unwrap();
        vs.ensure(spec).unwrap();
        let docs: Vec<VecDoc> = (0..ANN_MIN_ROWS as i64 + 64)
            .map(|i| {
                let a = i as f32 * 0.37;
                VecDoc {
                    pk: format!("d{}", i),
                    vector: (0..8).map(|k| (a * (k as f32 + 1.0)).sin()).collect(),
                    fields: vec![("eid".into(), Scalar::Int(i))],
                }
            })
            .collect();
        vs.upsert("t_ann", &docs).unwrap();
        vs.flush();
        let st = vs.status();
        assert_eq!(st[0].index, IndexKind::IvfHnswSq);
        let probe = docs[100].vector.clone();
        let hits = vs.search("t_ann", &probe, 3, None).unwrap();
        assert_eq!(hits[0].pk, "d100");
        let _ = std::fs::remove_dir_all(&dir);
    }
}