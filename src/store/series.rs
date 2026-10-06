use super::lance::{lerr, LanceDb};
use crate::timeseries::{self, Series};
use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array, RecordBatch, RecordBatchIterator, StringArray,
};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema, SchemaRef};
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::{DistanceType, Table};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

pub const WINDOW_DIM: usize = 20;
const SHAPE_POINTS: usize = 16;
const POINTS: &str = "ts_points";
const WINDOWS: &str = "ts_windows";

pub use super::lance::sql_str;

pub fn series_id(project_id: i64, tool_key: &str, metric: &str) -> String {
    format!("p{}|{}|{}", project_id, tool_key, metric)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointRow {
    pub series_id: String,
    pub project_id: i64,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub run_id: i64,
    pub tool_key: String,
    pub metric: String,
    pub unit: String,
    pub t_unit: String,
    pub t: f64,
    pub y: f64,
    pub source: String,
    pub ingested_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesMeta {
    pub series_id: String,
    pub project_id: i64,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub tool_key: String,
    pub metric: String,
    pub unit: String,
    pub t_unit: String,
    pub points: usize,
    pub t_min: f64,
    pub t_max: f64,
    pub last_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowRow {
    pub series_id: String,
    pub episode: i64,
    pub project_id: i64,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub metric: String,
    pub t_start: f64,
    pub t_end: f64,
    pub points: i64,
    pub duration_min: f64,
    pub final_ratio: f64,
    pub rate_per_hour: f64,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowHit {
    pub row: WindowRow,
    pub distance: f32,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeriesCounts {
    pub points: usize,
    pub windows: usize,
    pub path: String,
}

fn interp(t: &[f64], y: &[f64], x: f64) -> f64 {
    if x <= t[0] {
        return y[0];
    }
    for i in 1..t.len() {
        if x <= t[i] {
            let span = (t[i] - t[i - 1]).max(1e-12);
            return y[i - 1] + (y[i] - y[i - 1]) * (x - t[i - 1]) / span;
        }
    }
    *y.last().unwrap()
}

pub fn window_features(t: &[f64], y: &[f64], threshold: f64, minutes_per_t: Option<f64>) -> Option<(Vec<f32>, f64, f64, f64)> {
    if t.len() < 3 || t.len() != y.len() || threshold <= 0.0 {
        return None;
    }
    let t0 = t[0];
    let span = t[t.len() - 1] - t0;
    if span <= 0.0 {
        return None;
    }
    let y0 = y[0];
    let tn: Vec<f64> = t.iter().map(|v| (v - t0) / span).collect();
    let yn: Vec<f64> = y.iter().map(|v| (v - y0) / threshold).collect();
    let mut v: Vec<f32> = (0..SHAPE_POINTS)
        .map(|k| interp(&tn, &yn, k as f64 / (SHAPE_POINTS - 1) as f64) as f32)
        .collect();
    let dur = minutes_per_t.map(|k| span * k);
    let final_ratio = y[y.len() - 1] / threshold;
    let grown = (y[y.len() - 1] - y0) / threshold;
    let rate = dur.filter(|d| *d > 0.0).map(|d| grown / d * 60.0).unwrap_or(0.0);
    let half = interp(&tn, &yn, 0.5);
    let s1 = half.max(1e-6);
    let s2 = (yn[yn.len() - 1] - half).max(1e-6);
    let curvature = (s2 / s1).ln().clamp(-3.0, 3.0);
    v.push(dur.map(|d| ((1.0 + d).ln() / (241f64).ln()) as f32).unwrap_or(0.0));
    v.push(final_ratio.clamp(-2.0, 4.0) as f32);
    v.push((rate / 2.0).clamp(-4.0, 4.0) as f32);
    v.push((curvature / 2.0) as f32);
    Some((v, dur.unwrap_or(0.0), final_ratio, rate))
}

fn points_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("series_id", DataType::Utf8, false),
        Field::new("project_id", DataType::Int64, false),
        Field::new("endmill_id", DataType::Int64, false),
        Field::new("workpiece_id", DataType::Int64, false),
        Field::new("run_id", DataType::Int64, false),
        Field::new("tool_key", DataType::Utf8, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, false),
        Field::new("t_unit", DataType::Utf8, false),
        Field::new("t", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("ingested_at", DataType::Int64, false),
    ]))
}

fn windows_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("series_id", DataType::Utf8, false),
        Field::new("episode", DataType::Int64, false),
        Field::new("project_id", DataType::Int64, false),
        Field::new("endmill_id", DataType::Int64, false),
        Field::new("workpiece_id", DataType::Int64, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("t_start", DataType::Float64, false),
        Field::new("t_end", DataType::Float64, false),
        Field::new("points", DataType::Int64, false),
        Field::new("duration_min", DataType::Float64, false),
        Field::new("final_ratio", DataType::Float64, false),
        Field::new("rate_per_hour", DataType::Float64, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), WINDOW_DIM as i32),
            false,
        ),
    ]))
}

fn points_batch(rows: &[PointRow]) -> Result<RecordBatch, String> {
    let s = |f: fn(&PointRow) -> &str| Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<&str>>()));
    let i = |f: fn(&PointRow) -> i64| Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<i64>>()));
    let r = |f: fn(&PointRow) -> f64| Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<f64>>()));
    RecordBatch::try_new(
        points_schema(),
        vec![
            s(|p| &p.series_id),
            i(|p| p.project_id),
            i(|p| p.endmill_id),
            i(|p| p.workpiece_id),
            i(|p| p.run_id),
            s(|p| &p.tool_key),
            s(|p| &p.metric),
            s(|p| &p.unit),
            s(|p| &p.t_unit),
            r(|p| p.t),
            r(|p| p.y),
            s(|p| &p.source),
            i(|p| p.ingested_at),
        ],
    )
    .map_err(|e| format!("Arrow 배치 생성 실패: {}", e))
}

fn windows_batch(rows: &[WindowRow]) -> Result<RecordBatch, String> {
    let mut flat: Vec<f32> = Vec::with_capacity(rows.len() * WINDOW_DIM);
    for r in rows.iter() {
        if r.vector.len() != WINDOW_DIM {
            return Err(format!("시계열 창 벡터 차원 {} ≠ {}", r.vector.len(), WINDOW_DIM));
        }
        flat.extend_from_slice(&r.vector);
    }
    let vec = FixedSizeListArray::try_new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        WINDOW_DIM as i32,
        Arc::new(Float32Array::from(flat)),
        None,
    )
    .map_err(|e| e.to_string())?;
    let s = |f: fn(&WindowRow) -> &str| Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<&str>>()));
    let i = |f: fn(&WindowRow) -> i64| Arc::new(Int64Array::from(rows.iter().map(f).collect::<Vec<i64>>()));
    let r = |f: fn(&WindowRow) -> f64| Arc::new(Float64Array::from(rows.iter().map(f).collect::<Vec<f64>>()));
    RecordBatch::try_new(
        windows_schema(),
        vec![
            s(|w| &w.series_id),
            i(|w| w.episode),
            i(|w| w.project_id),
            i(|w| w.endmill_id),
            i(|w| w.workpiece_id),
            s(|w| &w.metric),
            r(|w| w.t_start),
            r(|w| w.t_end),
            i(|w| w.points),
            r(|w| w.duration_min),
            r(|w| w.final_ratio),
            r(|w| w.rate_per_hour),
            Arc::new(vec),
        ],
    )
    .map_err(|e| format!("Arrow 배치 생성 실패: {}", e))
}

fn col_str<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray, String> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| format!("열 '{}' 없음", name))
}

fn col_i64<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, String> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| format!("열 '{}' 없음", name))
}

fn col_f64<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Float64Array, String> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
        .ok_or_else(|| format!("열 '{}' 없음", name))
}

fn col_f32<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Float32Array, String> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
        .ok_or_else(|| format!("열 '{}' 없음", name))
}

pub struct SeriesStore {
    points: Option<Table>,
    windows: Option<Table>,
    db: Arc<LanceDb>,
}

impl SeriesStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::with(LanceDb::open(path)?)
    }

    pub fn with(db: Arc<LanceDb>) -> Result<Self, String> {
        let points = db.open_table(POINTS)?;
        let windows = db.open_table(WINDOWS)?;
        Ok(Self { points, windows, db })
    }

    fn rt(&self) -> &LanceDb {
        &self.db
    }

    fn ensure_points(&mut self) -> Result<Table, String> {
        if let Some(t) = &self.points {
            return Ok(t.clone());
        }
        let t = self.db.create_empty(POINTS, points_schema())?;
        self.points = Some(t.clone());
        Ok(t)
    }

    fn ensure_windows(&mut self) -> Result<Table, String> {
        if let Some(t) = &self.windows {
            return Ok(t.clone());
        }
        let t = self.db.create_empty(WINDOWS, windows_schema())?;
        self.windows = Some(t.clone());
        Ok(t)
    }

    pub fn upsert_points(&mut self, rows: &[PointRow]) -> Result<(u64, u64), String> {
        let mut uniq: BTreeMap<(String, u64), PointRow> = BTreeMap::new();
        for r in rows.iter().filter(|r| r.t.is_finite() && r.y.is_finite()) {
            uniq.insert((r.series_id.clone(), r.t.to_bits()), r.clone());
        }
        let rows: Vec<PointRow> = uniq.into_values().collect();
        if rows.is_empty() {
            return Ok((0, 0));
        }
        let t = self.ensure_points()?;
        let batch = points_batch(&rows)?;
        self.rt().block_on(async move {
            let mut m = t.merge_insert(&["series_id", "t"]);
            m.when_matched_update_all(None).when_not_matched_insert_all();
            let reader = RecordBatchIterator::new(vec![Ok(batch)], points_schema());
            let r = m.execute(Box::new(reader)).await.map_err(lerr)?;
            Ok((r.num_inserted_rows, r.num_updated_rows))
        })
    }

    async fn collect(t: &Table, filter: Option<String>, cols: Option<&[&str]>) -> Result<Vec<RecordBatch>, String> {
        let mut q = t.query();
        if let Some(f) = filter {
            q = q.only_if(f);
        }
        if let Some(c) = cols {
            q = q.select(Select::columns(c));
        }
        q.execute().await.map_err(lerr)?.try_collect::<Vec<_>>().await.map_err(lerr)
    }

    pub fn load(&self, series_id: &str) -> Result<Option<Series>, String> {
        let Some(t) = self.points.clone() else {
            return Ok(None);
        };
        let filter = format!("series_id = {}", sql_str(series_id));
        let batches = self
            .rt()
            .block_on(async move { Self::collect(&t, Some(filter), Some(&["metric", "unit", "t_unit", "t", "y"])).await })?;
        let mut pts: Vec<(f64, f64)> = Vec::new();
        let mut meta: Option<(String, String, String)> = None;
        for b in batches.iter() {
            let (m, u, tu) = (col_str(b, "metric")?, col_str(b, "unit")?, col_str(b, "t_unit")?);
            let (tt, yy) = (col_f64(b, "t")?, col_f64(b, "y")?);
            for i in 0..b.num_rows() {
                if meta.is_none() {
                    meta = Some((m.value(i).to_string(), u.value(i).to_string(), tu.value(i).to_string()));
                }
                pts.push((tt.value(i), yy.value(i)));
            }
        }
        let Some((metric, unit, t_unit)) = meta else {
            return Ok(None);
        };
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut s = Series::new(&metric, &unit).with_time_unit(&t_unit);
        for (t, y) in pts {
            s.push(t, y);
        }
        Ok(Some(s))
    }

    pub fn list(&self, project_id: Option<i64>, tool_key: Option<&str>) -> Result<Vec<SeriesMeta>, String> {
        let Some(t) = self.points.clone() else {
            return Ok(Vec::new());
        };
        let mut conds = Vec::new();
        if let Some(p) = project_id {
            conds.push(format!("project_id = {}", p));
        }
        if let Some(k) = tool_key {
            conds.push(format!("tool_key = {}", sql_str(k)));
        }
        let filter = if conds.is_empty() { None } else { Some(conds.join(" AND ")) };
        let batches = self.rt().block_on(async move {
            Self::collect(
                &t,
                filter,
                Some(&["series_id", "project_id", "endmill_id", "workpiece_id", "tool_key", "metric", "unit", "t_unit", "t", "y"]),
            )
            .await
        })?;
        let mut acc: BTreeMap<String, SeriesMeta> = BTreeMap::new();
        for b in batches.iter() {
            let sid = col_str(b, "series_id")?;
            let pid = col_i64(b, "project_id")?;
            let em = col_i64(b, "endmill_id")?;
            let wp = col_i64(b, "workpiece_id")?;
            let tk = col_str(b, "tool_key")?;
            let me = col_str(b, "metric")?;
            let un = col_str(b, "unit")?;
            let tu = col_str(b, "t_unit")?;
            let tt = col_f64(b, "t")?;
            let yy = col_f64(b, "y")?;
            for i in 0..b.num_rows() {
                let e = acc.entry(sid.value(i).to_string()).or_insert_with(|| SeriesMeta {
                    series_id: sid.value(i).to_string(),
                    project_id: pid.value(i),
                    endmill_id: em.value(i),
                    workpiece_id: wp.value(i),
                    tool_key: tk.value(i).to_string(),
                    metric: me.value(i).to_string(),
                    unit: un.value(i).to_string(),
                    t_unit: tu.value(i).to_string(),
                    points: 0,
                    t_min: f64::INFINITY,
                    t_max: f64::NEG_INFINITY,
                    last_y: 0.0,
                });
                e.points += 1;
                e.t_min = e.t_min.min(tt.value(i));
                if tt.value(i) >= e.t_max {
                    e.t_max = tt.value(i);
                    e.last_y = yy.value(i);
                }
            }
        }
        Ok(acc.into_values().collect())
    }

    pub fn delete_series(&mut self, series_id: &str) -> Result<(), String> {
        let pred = format!("series_id = {}", sql_str(series_id));
        if let Some(t) = self.points.clone() {
            let p = pred.clone();
            self.rt().block_on(async move { t.delete(p.as_str()).await.map(|_| ()).map_err(lerr) })?;
        }
        if let Some(t) = self.windows.clone() {
            self.rt().block_on(async move { t.delete(pred.as_str()).await.map(|_| ()).map_err(lerr) })?;
        }
        Ok(())
    }

    pub fn upsert_windows(&mut self, rows: &[WindowRow]) -> Result<u64, String> {
        if rows.is_empty() {
            return Ok(0);
        }
        let t = self.ensure_windows()?;
        let batch = windows_batch(rows)?;
        self.rt().block_on(async move {
            let mut m = t.merge_insert(&["series_id", "episode"]);
            m.when_matched_update_all(None).when_not_matched_insert_all();
            let reader = RecordBatchIterator::new(vec![Ok(batch)], windows_schema());
            let r = m.execute(Box::new(reader)).await.map_err(lerr)?;
            Ok(r.num_inserted_rows + r.num_updated_rows)
        })
    }

    pub fn windows_of(&self, series_id: &str) -> Result<Vec<WindowRow>, String> {
        let Some(t) = self.windows.clone() else {
            return Ok(Vec::new());
        };
        let filter = format!("series_id = {}", sql_str(series_id));
        let batches = self.rt().block_on(async move { Self::collect(&t, Some(filter), None).await })?;
        let mut out = Vec::new();
        for b in batches.iter() {
            out.extend(Self::window_rows(b, None)?.into_iter().map(|(w, _)| w));
        }
        out.sort_by_key(|w| w.episode);
        Ok(out)
    }

    fn window_rows(b: &RecordBatch, dist: Option<&Float32Array>) -> Result<Vec<(WindowRow, f32)>, String> {
        let sid = col_str(b, "series_id")?;
        let ep = col_i64(b, "episode")?;
        let pid = col_i64(b, "project_id")?;
        let em = col_i64(b, "endmill_id")?;
        let wp = col_i64(b, "workpiece_id")?;
        let me = col_str(b, "metric")?;
        let ts = col_f64(b, "t_start")?;
        let te = col_f64(b, "t_end")?;
        let np = col_i64(b, "points")?;
        let du = col_f64(b, "duration_min")?;
        let fr = col_f64(b, "final_ratio")?;
        let ra = col_f64(b, "rate_per_hour")?;
        let vec = b
            .column_by_name("vector")
            .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
            .ok_or_else(|| "열 'vector' 없음".to_string())?;
        let mut out = Vec::with_capacity(b.num_rows());
        for i in 0..b.num_rows() {
            let v = vec.value(i);
            let vf = v
                .as_any()
                .downcast_ref::<Float32Array>()
                .map(|a| a.values().to_vec())
                .unwrap_or_default();
            out.push((
                WindowRow {
                    series_id: sid.value(i).to_string(),
                    episode: ep.value(i),
                    project_id: pid.value(i),
                    endmill_id: em.value(i),
                    workpiece_id: wp.value(i),
                    metric: me.value(i).to_string(),
                    t_start: ts.value(i),
                    t_end: te.value(i),
                    points: np.value(i),
                    duration_min: du.value(i),
                    final_ratio: fr.value(i),
                    rate_per_hour: ra.value(i),
                    vector: vf,
                },
                dist.map(|d| d.value(i)).unwrap_or(0.0),
            ));
        }
        Ok(out)
    }

    pub fn similar_windows(&self, metric: &str, vector: &[f32], k: usize, exclude_series: Option<&str>) -> Result<Vec<WindowHit>, String> {
        let Some(t) = self.windows.clone() else {
            return Ok(Vec::new());
        };
        if vector.len() != WINDOW_DIM {
            return Err(format!("시계열 창 질의 차원 {} ≠ {}", vector.len(), WINDOW_DIM));
        }
        let mut filter = format!("metric = {}", sql_str(metric));
        if let Some(x) = exclude_series {
            filter.push_str(&format!(" AND series_id != {}", sql_str(x)));
        }
        let qv = vector.to_vec();
        let batches = self.rt().block_on(async move {
            t.query()
                .nearest_to(qv.as_slice())
                .map_err(lerr)?
                .distance_type(DistanceType::L2)
                .only_if(filter)
                .limit(k.max(1))
                .execute()
                .await
                .map_err(lerr)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(lerr)
        })?;
        let mut out = Vec::new();
        for b in batches.iter() {
            let d = col_f32(b, "_distance")?;
            for (row, dist) in Self::window_rows(b, Some(d))? {
                out.push(WindowHit {
                    similarity: super::attr::similarity_from_l2(dist),
                    distance: dist,
                    row,
                });
            }
        }
        out.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }

    pub fn counts(&self) -> SeriesCounts {
        let p = self.points.clone();
        let w = self.windows.clone();
        let (points, windows) = self.rt().block_on(async move {
            let a = match p {
                Some(t) => t.count_rows(None).await.unwrap_or(0),
                None => 0,
            };
            let b = match w {
                Some(t) => t.count_rows(None).await.unwrap_or(0),
                None => 0,
            };
            (a, b)
        });
        SeriesCounts {
            points,
            windows,
            path: self.db.path().display().to_string(),
        }
    }

    pub fn compact(&self) -> Vec<String> {
        let mut log = Vec::new();
        for (name, t) in [(POINTS, self.points.clone()), (WINDOWS, self.windows.clone())] {
            if let Some(t) = t {
                match self
                    .rt()
                    .block_on(async move { t.optimize(lancedb::table::OptimizeAction::All).await.map_err(lerr) })
                {
                    Ok(_) => log.push(format!("{} 압축·정리 완료", name)),
                    Err(e) => log.push(format!("{} 정리 실패: {}", name, e)),
                }
            }
        }
        log
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeWindow {
    pub episode: i64,
    pub t_start: f64,
    pub t_end: f64,
    pub points: usize,
    pub vector: Vec<f32>,
    pub duration_min: f64,
    pub final_ratio: f64,
    pub rate_per_hour: f64,
}

pub fn episode_windows(series: &Series, threshold: f64, minutes_per_t: Option<f64>) -> Vec<EpisodeWindow> {
    let mut out = Vec::new();
    for (i, (a, b)) in timeseries::episodes_of(series).into_iter().enumerate() {
        if b - a < 3 {
            continue;
        }
        let t = &series.t[a..b];
        let y = &series.y[a..b];
        if let Some((v, dur, fr, rate)) = window_features(t, y, threshold, minutes_per_t) {
            out.push(EpisodeWindow {
                episode: i as i64,
                t_start: t[0],
                t_end: t[t.len() - 1],
                points: b - a,
                vector: v,
                duration_min: dur,
                final_ratio: fr,
                rate_per_hour: rate,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("endmill_ts_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn pt(sid: &str, t: f64, y: f64) -> PointRow {
        PointRow {
            series_id: sid.into(),
            project_id: 1,
            endmill_id: 2,
            workpiece_id: 3,
            run_id: 0,
            tool_key: "T1".into(),
            metric: "vb_mm".into(),
            unit: "mm".into(),
            t_unit: "min".into(),
            t,
            y,
            source: "log".into(),
            ingested_at: 0,
        }
    }

    #[test]
    fn points_upsert_is_idempotent_and_sorted() {
        let dir = tmp("pts");
        let mut st = SeriesStore::open(&dir).unwrap();
        let sid = series_id(1, "T1", "vb_mm");
        st.upsert_points(&[pt(&sid, 2.0, 0.05), pt(&sid, 0.0, 0.0), pt(&sid, 1.0, 0.03)]).unwrap();
        let (ins, upd) = st.upsert_points(&[pt(&sid, 2.0, 0.06), pt(&sid, 3.0, 0.08)]).unwrap();
        assert_eq!((ins, upd), (1, 1));
        let s = st.load(&sid).unwrap().unwrap();
        assert_eq!(s.t, vec![0.0, 1.0, 2.0, 3.0]);
        assert!((s.y[2] - 0.06).abs() < 1e-12);
        assert_eq!(s.t_unit, "min");
        let list = st.list(Some(1), None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].points, 4);
        assert!((list[0].last_y - 0.08).abs() < 1e-12);
        drop(st);
        let st = SeriesStore::open(&dir).unwrap();
        assert_eq!(st.counts().points, 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn similar_wear_curves_rank_first() {
        let dir = tmp("win");
        let mut st = SeriesStore::open(&dir).unwrap();
        let mk = |sid: &str, ep: i64, ys: &[f64]| {
            let t: Vec<f64> = (0..ys.len()).map(|i| i as f64 * 5.0).collect();
            let (v, d, f, r) = window_features(&t, ys, 0.3, Some(1.0)).unwrap();
            WindowRow {
                series_id: sid.into(),
                episode: ep,
                project_id: 1,
                endmill_id: 1,
                workpiece_id: 1,
                metric: "vb_mm".into(),
                t_start: 0.0,
                t_end: t[t.len() - 1],
                points: ys.len() as i64,
                duration_min: d,
                final_ratio: f,
                rate_per_hour: r,
                vector: v,
            }
        };
        let linear = mk("a", 0, &[0.0, 0.05, 0.1, 0.15, 0.2, 0.25]);
        let accel = mk("b", 0, &[0.0, 0.02, 0.04, 0.07, 0.14, 0.3]);
        st.upsert_windows(&[linear.clone(), accel.clone()]).unwrap();
        let probe = mk("c", 0, &[0.0, 0.021, 0.042, 0.075, 0.15, 0.29]);
        let hits = st.similar_windows("vb_mm", &probe.vector, 2, Some("c")).unwrap();
        assert_eq!(hits[0].row.series_id, "b");
        assert_eq!(st.windows_of("a").unwrap().len(), 1);
        st.delete_series("a").unwrap();
        assert_eq!(st.windows_of("a").unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}