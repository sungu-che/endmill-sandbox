use lancedb::arrow::arrow_schema::SchemaRef;
use lancedb::{Connection, Table};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn lerr(e: lancedb::Error) -> String {
    format!("LanceDB: {}", e)
}

pub fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub struct LanceDb {
    rt: Option<tokio::runtime::Runtime>,
    db: Connection,
    path: PathBuf,
}

impl Drop for LanceDb {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl LanceDb {
    pub fn open(path: &Path) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lancedb")
            .enable_all()
            .build()
            .map_err(|e| format!("tokio 런타임 생성 실패: {}", e))?;
        let uri = path.display().to_string();
        let db = rt.block_on(async move { lancedb::connect(&uri).execute().await.map_err(lerr) })?;
        Ok(Arc::new(Self {
            rt: Some(rt),
            db,
            path: path.to_path_buf(),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn block_on<F: Future>(&self, f: F) -> F::Output {
        self.rt.as_ref().expect("LanceDB 런타임").block_on(f)
    }

    pub fn table_names(&self) -> Result<Vec<String>, String> {
        let db = self.db.clone();
        self.block_on(async move { db.table_names().execute().await.map_err(lerr) })
    }

    pub fn open_table(&self, name: &str) -> Result<Option<Table>, String> {
        if !self.table_names()?.iter().any(|n| n == name) {
            return Ok(None);
        }
        let db = self.db.clone();
        let name = name.to_string();
        self.block_on(async move { db.open_table(name).execute().await.map(Some).map_err(lerr) })
    }

    pub fn create_empty(&self, name: &str, schema: SchemaRef) -> Result<Table, String> {
        let db = self.db.clone();
        let name = name.to_string();
        self.block_on(async move { db.create_empty_table(name, schema).execute().await.map_err(lerr) })
    }

    pub fn drop_table(&self, name: &str) -> Result<(), String> {
        let db = self.db.clone();
        let name = name.to_string();
        self.block_on(async move { db.drop_table(name, &[]).await.map_err(lerr) })
    }

    pub fn table_dir(&self, name: &str) -> PathBuf {
        self.path.join(format!("{}.lance", name))
    }
}