use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::LedgerError;
use crate::Result;

const MAP_DB_NAME: &str = "map.sqlite";
const MAX_VOCAB: usize = 512;
pub const BOW_MODEL_ID: &str = "bag-of-words-tfidf-v1";

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
    "from", "as", "is", "was", "are", "were", "be", "been", "being", "have", "has", "had", "do",
    "does", "did", "will", "would", "shall", "should", "may", "might", "must", "can", "could",
    "this", "that", "these", "those", "it", "its", "we", "our", "you", "your", "they", "their",
    "i", "my", "he", "she", "his", "her", "not", "no", "so", "if", "then", "than", "when", "which",
    "who", "what", "how", "all", "any", "each", "more", "also", "into", "about", "over", "use",
    "used", "using",
];

pub trait Embedder {
    fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>>;
    fn dims(&self) -> usize;
    fn model_id(&self) -> &str;
}

pub struct BagOfWordsEmbedder {
    vocab: Vec<String>,
    idf: Vec<f32>,
}

impl BagOfWordsEmbedder {
    pub fn new(all_texts: &[String]) -> Self {
        let n_docs = all_texts.len().max(1);
        let mut doc_freq: BTreeMap<String, usize> = BTreeMap::new();
        for text in all_texts {
            let tokens: std::collections::BTreeSet<String> = tokenize(text).into_iter().collect();
            for token in tokens {
                *doc_freq.entry(token).or_insert(0) += 1;
            }
        }

        let mut entries: Vec<(String, usize)> = doc_freq.into_iter().collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        entries.truncate(MAX_VOCAB);

        let vocab: Vec<String> = entries.iter().map(|(t, _)| t.clone()).collect();
        let idf: Vec<f32> = entries
            .iter()
            .map(|(_, df)| ((n_docs as f32 + 1.0) / (*df as f32 + 1.0)).ln() + 1.0)
            .collect();

        Self { vocab, idf }
    }
}

impl Embedder for BagOfWordsEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        if self.vocab.is_empty() {
            return texts.iter().map(|_| vec![]).collect();
        }
        let idx: BTreeMap<&str, usize> = self
            .vocab
            .iter()
            .enumerate()
            .map(|(i, t)| (t.as_str(), i))
            .collect();
        let dims = self.vocab.len();
        texts
            .iter()
            .map(|text| {
                let tokens = tokenize(text);
                let n = tokens.len().max(1) as f32;
                let mut tf = vec![0.0f32; dims];
                for token in &tokens {
                    if let Some(&i) = idx.get(token.as_str()) {
                        tf[i] += 1.0;
                    }
                }
                let mut v: Vec<f32> = tf
                    .iter()
                    .zip(&self.idf)
                    .map(|(t, idf)| (t / n) * idf)
                    .collect();
                let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 1e-9 {
                    v.iter_mut().for_each(|x| *x /= norm);
                }
                v
            })
            .collect()
    }

    fn dims(&self) -> usize {
        self.vocab.len()
    }

    fn model_id(&self) -> &str {
        BOW_MODEL_ID
    }
}

pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() > 1 && !STOPWORDS.contains(&s.as_str()))
        .collect()
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
        fs::create_dir_all(dir.as_ref()).map_err(|e| LedgerError::Storage(e.to_string()))?;
        let path = dir.as_ref().join(MAP_DB_NAME);
        let conn = Connection::open(&path).map_err(|e| LedgerError::Storage(e.to_string()))?;
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
            .map_err(|e| LedgerError::Storage(e.to_string()))?;
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
            .map_err(|e| LedgerError::Storage(e.to_string()))?;
        Ok(row.map(|b| bytes_to_floats(&b)))
    }

    pub fn get_all(&self, model_id: &str) -> Result<Vec<(String, Vec<f32>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT decision_id, embedding FROM decision_embeddings WHERE model_id = ?1 ORDER BY decision_id",
            )
            .map_err(|e| LedgerError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![model_id], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .map_err(|e| LedgerError::Storage(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            let (id, blob) = row.map_err(|e| LedgerError::Storage(e.to_string()))?;
            result.push((id, bytes_to_floats(&blob)));
        }
        Ok(result)
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
    .map_err(|e| LedgerError::Storage(e.to_string()))?;
    Ok(())
}

pub fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
