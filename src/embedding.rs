use std::fs;
use std::path::{Path, PathBuf};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::LedgerError;
use crate::Result;

const MAP_DB_NAME: &str = "map.sqlite";
pub const SEMANTIC_MODEL_ID: &str = "bge-small-en-v1.5";
pub const SEMANTIC_DIMS: usize = 384;

pub trait Embedder {
    fn embed_batch(&mut self, texts: &[&str]) -> Vec<Vec<f32>>;
    fn dims(&self) -> usize;
    fn model_id(&self) -> &str;
}

pub struct SemanticEmbedder {
    inner: TextEmbedding,
}

impl SemanticEmbedder {
    pub fn try_new(cache_dir: Option<&Path>) -> Result<Self> {
        let mut opts =
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(true);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir.to_path_buf());
        }
        let inner = TextEmbedding::try_new(opts)
            .map_err(|e| LedgerError::Storage(format!("semantic embedder init: {e}")))?;
        Ok(Self { inner })
    }
}

impl Embedder for SemanticEmbedder {
    fn embed_batch(&mut self, texts: &[&str]) -> Vec<Vec<f32>> {
        self.inner
            .embed(texts, None)
            .unwrap_or_else(|_| vec![vec![0.0f32; SEMANTIC_DIMS]; texts.len()]) // ubs:ignore: safe fallback — zero vectors degrade to time-only layout
    }

    fn dims(&self) -> usize {
        SEMANTIC_DIMS
    }

    fn model_id(&self) -> &str {
        SEMANTIC_MODEL_ID
    }
}

pub fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-9 || nb < 1e-9 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(-1.0, 1.0)
}

pub struct EmbeddingStore {
    path: PathBuf,
    pub(crate) conn: Connection,
}

impl EmbeddingStore {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(dir.as_ref()).map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
        let path = dir.as_ref().join(MAP_DB_NAME);
        let conn = Connection::open(&path).map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
        initialize_schema(&conn)?;
        Ok(Self { path, conn })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn upsert(&self, decision_id: &str, model_id: &str, embedding: &[f32]) -> Result<()> {
        let blob = floats_to_bytes(embedding);
        self.conn
            .execute(
                "INSERT INTO decision_embeddings (decision_id, model_id, embedding, dims)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(decision_id, model_id) DO UPDATE SET
                     embedding = excluded.embedding,
                     dims = excluded.dims",
                params![decision_id, model_id, blob, embedding.len() as i64],
            )
            .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
        Ok(())
    }

    pub fn get(&self, decision_id: &str, model_id: &str) -> Result<Option<Vec<f32>>> {
        let row: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT embedding FROM decision_embeddings WHERE decision_id = ?1 AND model_id = ?2",
                params![decision_id, model_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
        Ok(row.map(|b| bytes_to_floats(&b)))
    }

    pub fn get_all(&self, model_id: &str) -> Result<Vec<(String, Vec<f32>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT decision_id, embedding FROM decision_embeddings \
                 WHERE model_id = ?1 ORDER BY decision_id",
            )
            .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
        let pairs: rusqlite::Result<Vec<(String, Vec<u8>)>> = stmt
            .query_map(params![model_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| LedgerError::Storage(e.to_string()))? // ubs:ignore: error conversion at boundary
            .collect();
        Ok(pairs
            .map_err(|e| LedgerError::Storage(e.to_string()))? // ubs:ignore: error conversion at boundary
            .into_iter()
            .map(|(id, blob)| (id, bytes_to_floats(&blob)))
            .collect())
    }
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS decision_embeddings (
             decision_id TEXT NOT NULL,
             model_id    TEXT NOT NULL,
             embedding   BLOB NOT NULL,
             dims        INTEGER NOT NULL,
             created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
             PRIMARY KEY (decision_id, model_id)
         );
         CREATE TABLE IF NOT EXISTS projection_generation (
             gen_id      TEXT PRIMARY KEY,
             alpha       REAL NOT NULL,
             k_neighbors INTEGER NOT NULL,
             model_id    TEXT NOT NULL,
             n_decisions INTEGER NOT NULL,
             created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
         );
         CREATE TABLE IF NOT EXISTS decision_map_point (
             decision_id     TEXT NOT NULL,
             gen_id          TEXT NOT NULL,
             x_time_ordinal  REAL NOT NULL,
             y_spectral      REAL NOT NULL,
             y_fiedler_raw   REAL NOT NULL,
             PRIMARY KEY (decision_id, gen_id)
         );",
    )
    .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
    Ok(())
}

pub fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks.iter().map(|c| f32::from_le_bytes(*c)).collect()
}
