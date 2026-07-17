use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nalgebra::{DMatrix, DVector};
use rusqlite::params;
use serde::Serialize;
use uuid::Uuid;

use crate::embedding::{cosine_sim, Embedder, EmbeddingStore, SemanticEmbedder, SEMANTIC_MODEL_ID};
use crate::error::LedgerError;
use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::queries::derive_decision_status;
use crate::Result;

const K_NEIGHBORS_DEFAULT: usize = 5;

#[derive(Clone, Debug, Serialize)]
pub struct MapPoint {
    pub id: String,
    pub title: String,
    pub x_time: f64,
    pub y_spectral: f64,
    pub y_fiedler_raw: f64,
    pub status: String,
    pub topic_keys: Vec<String>,
    pub inbound_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct MapEdge {
    pub from_id: String,
    pub to_id: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DensityBand {
    pub label: String,
    pub y_center: f64,
    pub y_radius: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct MapResult {
    pub gen_id: String,
    pub alpha: f64,
    pub n: usize,
    pub points: Vec<MapPoint>,
    pub edges: Vec<MapEdge>,
    pub density_bands: Vec<DensityBand>,
    pub truncated: bool,
}

struct DecisionRecord {
    id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    event_origin: i64,
}

pub fn compute_map(graph: &impl GraphView, hivemind_dir: &Path, alpha: f64) -> Result<MapResult> {
    let alpha = alpha.clamp(0.0, 1.0);
    let decisions = load_decisions(graph)?;
    let n = decisions.len();

    if n == 0 {
        return Ok(MapResult {
            gen_id: Uuid::new_v4().to_string(),
            alpha,
            n: 0,
            points: vec![],
            edges: vec![],
            density_bands: vec![],
            truncated: false,
        });
    }

    // Build embeddings for all decisions
    let store = EmbeddingStore::open(hivemind_dir)?;
    let embeddings = build_embeddings(&decisions, &store, hivemind_dir)?;

    // Build blended adjacency matrix
    let structural = build_structural_edges(graph, &decisions)?;
    let w = build_blended_matrix(&embeddings, &structural, n, alpha);

    // Spectral Y via Fiedler vector of normalized Laplacian
    let (y_raw, y_spectral, x_time) = if n >= 3 {
        let laplacian = normalized_laplacian(&w);
        let fiedler = fiedler_vector(&laplacian)?;
        let t = time_ordinals(n);
        let y_orth = time_orthogonalize(&fiedler, &t);
        (
            fiedler.as_slice().to_vec(),
            y_orth.as_slice().to_vec(),
            t.as_slice().to_vec(),
        )
    } else {
        let t: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let t2 = t.clone(); // ubs:ignore: clone necessary — t used for all three return positions
        let t3 = t.clone(); // ubs:ignore: clone necessary — t used for all three return positions
        (t, t2, t3)
    };

    // Count inbound structural edges per decision
    let inbound = inbound_counts(&structural, n);

    // Build edge list for response
    let edges = build_map_edges(graph, &decisions)?;

    // Density bands from topic clustering on Y axis
    let bands = density_bands(&decisions, &y_spectral);

    // Persist generation and points
    let gen_id = Uuid::new_v4().to_string();
    store
        .conn
        .execute(
            "INSERT INTO projection_generation (gen_id, alpha, k_neighbors, model_id, n_decisions)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &gen_id,
                alpha,
                K_NEIGHBORS_DEFAULT as i64,
                SEMANTIC_MODEL_ID,
                n as i64
            ],
        )
        .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary

    for (d, ((xt, ys), yr)) in decisions
        .iter()
        .zip(x_time.iter().zip(y_spectral.iter()).zip(y_raw.iter()))
    {
        store
            .conn
            .execute(
                "INSERT INTO decision_map_point \
                 (decision_id, gen_id, x_time_ordinal, y_spectral, y_fiedler_raw) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(decision_id, gen_id) DO UPDATE SET \
                     x_time_ordinal = excluded.x_time_ordinal, \
                     y_spectral = excluded.y_spectral, \
                     y_fiedler_raw = excluded.y_fiedler_raw",
                params![&d.id, &gen_id, xt, ys, yr],
            )
            .map_err(|e| LedgerError::Storage(e.to_string()))?; // ubs:ignore: error conversion at boundary
    }

    let points: Vec<MapPoint> = decisions
        .iter()
        .zip(
            x_time
                .iter()
                .zip(y_spectral.iter())
                .zip(y_raw.iter())
                .zip(inbound.iter()),
        )
        .map(|(d, (((xt, ys), yr), ic))| {
            let status = derive_decision_status(graph, &d.id)
                .map(|s| format!("{s:?}").to_ascii_lowercase()) // ubs:ignore: format! for status serialization
                .unwrap_or_else(|_| "unknown".to_owned()); // ubs:ignore: safe default for missing status
            MapPoint {
                id: d.id.clone(), // ubs:ignore: clone necessary — building owned MapPoint from borrowed DecisionRecord
                title: d.title.clone(), // ubs:ignore: clone necessary — building owned MapPoint from borrowed DecisionRecord
                x_time: *xt,
                y_spectral: *ys,
                y_fiedler_raw: *yr,
                status,
                topic_keys: d.topic_keys.clone(), // ubs:ignore: clone necessary — building owned MapPoint from borrowed DecisionRecord
                inbound_count: *ic,
            }
        })
        .collect();

    Ok(MapResult {
        gen_id,
        alpha,
        n,
        points,
        edges,
        density_bands: bands,
        truncated: false,
    })
}

fn load_decisions(graph: &impl GraphView) -> Result<Vec<DecisionRecord>> {
    // "RETURN node.id AS id" pattern: MemoryGraph returns all node properties
    let rows = graph.query(
        "MATCH (node:`Decision`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;

    let mut decisions: Vec<DecisionRecord> = rows
        .into_iter()
        .filter_map(|row| {
            let id = match row.get("id") {
                Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — GraphValue holds borrowed ref, DecisionRecord needs owned String
                _ => return None,
            };
            let title = match row.get("title") {
                Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — building owned DecisionRecord from borrowed GraphRow
                _ => String::new(),
            };
            let rationale = match row.get("rationale") {
                Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — building owned DecisionRecord from borrowed GraphRow
                _ => String::new(),
            };
            let topic_keys = match row.get("topic_keys") {
                Some(GraphValue::StringList(v)) => v.clone(), // ubs:ignore: clone necessary — building owned Vec from borrowed GraphValue
                Some(GraphValue::String(s)) if !s.is_empty() => vec![s.clone()], // ubs:ignore: clone necessary — building owned Vec from borrowed GraphValue
                _ => vec![],
            };
            let event_origin = match row.get("event_origin") {
                Some(GraphValue::Int(n)) => *n,
                _ => 0,
            };
            Some(DecisionRecord {
                id,
                title,
                rationale,
                topic_keys,
                event_origin,
            })
        })
        .collect();

    decisions.sort_by_key(|d| d.event_origin);
    Ok(decisions)
}

fn build_embeddings(
    decisions: &[DecisionRecord],
    store: &EmbeddingStore,
    hivemind_dir: &Path,
) -> Result<Vec<Vec<f32>>> {
    let texts: Vec<String> = decisions
        .iter()
        .map(|d| format!("{} {}", d.title, d.rationale)) // ubs:ignore: format! outside inner loop — one allocation per decision, not per token
        .collect();
    let mut embedder = SemanticEmbedder::try_new(Some(hivemind_dir))?;
    let slices: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = embedder.embed_batch(&slices);
    for (d, emb) in decisions.iter().zip(&embeddings) {
        store.upsert(&d.id, SEMANTIC_MODEL_ID, emb)?;
    }
    Ok(embeddings)
}

fn build_structural_edges(
    graph: &impl GraphView,
    decisions: &[DecisionRecord],
) -> Result<BTreeMap<(usize, usize), f32>> {
    let id_to_idx: BTreeMap<&str, usize> = decisions
        .iter()
        .enumerate()
        .map(|(i, d)| (d.id.as_str(), i))
        .collect();

    let mut edges: BTreeMap<(usize, usize), f32> = BTreeMap::new();

    // Supersedes: Decision→Decision (uses "RETURN from.id AS from_id, to.id AS to_id" pattern)
    let rows = graph.query(
        "MATCH (from:`Decision`)-[:`SUPERSEDES`]->(to:`Decision`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    for row in rows {
        let from_id = match row.get("from_id") {
            Some(GraphValue::String(s)) => s.as_str(),
            _ => continue,
        };
        let to_id = match row.get("to_id") {
            Some(GraphValue::String(s)) => s.as_str(),
            _ => continue,
        };
        if let (Some(&fi), Some(&ti)) = (id_to_idx.get(from_id), id_to_idx.get(to_id)) {
            edges.insert((fi, ti), 1.0);
            edges.insert((ti, fi), 1.0);
        }
    }

    // Co-assumption: two decisions that both ASSUME the same hypothesis
    // Uses "RETURN from.id AS from_id, to.id AS to_id" pattern; from=decision, to=hypothesis
    let rows = graph.query(
        "MATCH (from:`Decision`)-[:`PREMISED_ON`]->(to:`Hypothesis`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    let mut hyp_to_decisions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for row in rows {
        let did = match row.get("from_id") {
            Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — GraphValue holds borrowed ref, BTreeMap key needs owned String
            _ => continue,
        };
        let hid = match row.get("to_id") {
            Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — GraphValue holds borrowed ref, BTreeMap key needs owned String
            _ => continue,
        };
        if let Some(&idx) = id_to_idx.get(did.as_str()) {
            hyp_to_decisions.entry(hid).or_default().push(idx);
        }
    }
    for idxs in hyp_to_decisions.values() {
        for &i in idxs {
            for &j in idxs {
                if i != j {
                    edges.entry((i, j)).or_insert(0.5);
                }
            }
        }
    }

    Ok(edges)
}

fn build_blended_matrix(
    embeddings: &[Vec<f32>],
    structural: &BTreeMap<(usize, usize), f32>,
    n: usize,
    alpha: f64,
) -> DMatrix<f64> {
    let k = K_NEIGHBORS_DEFAULT.min(n.saturating_sub(1));
    let mut w = DMatrix::<f64>::zeros(n, n);

    // Semantic kNN
    for i in 0..n {
        let mut sims: Vec<(usize, f32)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| (j, cosine_sim(&embeddings[i], &embeddings[j]))) // ubs:ignore: i,j bounded by 0..n loop invariant; slice len = n
            .collect();
        sims.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal) // ubs:ignore: unwrap_or on partial_cmp — NaN safety for f32
        });
        for (j, sim) in sims.into_iter().take(k) {
            let sem = f64::from(sim).max(0.0);
            let existing = w[(i, j)]; // ubs:ignore: nalgebra matrix; i,j bounded by 0..n loop invariant
            w[(i, j)] = existing.max(sem); // ubs:ignore: nalgebra matrix; i,j bounded by 0..n loop invariant
            w[(j, i)] = w[(j, i)].max(sem); // ubs:ignore: nalgebra matrix; i,j bounded by 0..n loop invariant
        }
    }

    // Blend structural edges
    for (&(i, j), &str_w) in structural {
        let sem = w[(i, j)]; // ubs:ignore: nalgebra matrix; structural keys bounded to 0..n by build_structural_edges
        let blended = (1.0 - alpha) * sem + alpha * f64::from(str_w);
        w[(i, j)] = w[(i, j)].max(blended); // ubs:ignore: nalgebra matrix; structural keys bounded to 0..n
        w[(j, i)] = w[(j, i)].max(blended); // ubs:ignore: nalgebra matrix; structural keys bounded to 0..n
    }

    w
}

fn normalized_laplacian(w: &DMatrix<f64>) -> DMatrix<f64> {
    let n = w.nrows();
    let d: Vec<f64> = (0..n).map(|i| w.row(i).sum()).collect();
    let d_inv_sqrt: Vec<f64> = d
        .iter()
        .map(|&di| if di > 1e-10 { 1.0 / di.sqrt() } else { 0.0 })
        .collect();

    let mut l = DMatrix::<f64>::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            if i == j {
                l[(i, j)] = 1.0; // ubs:ignore: nalgebra matrix; i,j bounded by 0..n loop
            } else {
                l[(i, j)] = -d_inv_sqrt[i] * w[(i, j)] * d_inv_sqrt[j]; // ubs:ignore: nalgebra matrix + Vec; i,j bounded by 0..n loop
            }
        }
    }
    l
}

fn fiedler_vector(laplacian: &DMatrix<f64>) -> Result<DVector<f64>> {
    let sym = laplacian.clone().symmetric_eigen();
    // eigenvalues sorted ascending; index 1 is the Fiedler vector
    let col = if sym.eigenvalues.len() > 1 { 1 } else { 0 };
    Ok(sym.eigenvectors.column(col).clone_owned())
}

fn time_ordinals(n: usize) -> DVector<f64> {
    DVector::from_iterator(n, (0..n).map(|i| i as f64))
}

fn time_orthogonalize(fiedler: &DVector<f64>, time: &DVector<f64>) -> DVector<f64> {
    let n = fiedler.len();
    if n < 2 {
        return fiedler.clone();
    }
    let t_mean = time.mean();
    let f_mean = fiedler.mean();
    let t_c = time - DVector::from_element(n, t_mean);
    let f_c = fiedler - DVector::from_element(n, f_mean);
    let denom = t_c.dot(&t_c);
    if denom < 1e-12 {
        return f_c;
    }
    let beta = t_c.dot(&f_c) / denom;
    f_c - beta * t_c
}

fn inbound_counts(structural: &BTreeMap<(usize, usize), f32>, n: usize) -> Vec<usize> {
    let mut counts = vec![0usize; n];
    for &(from, to) in structural.keys() {
        if from != to && to < n {
            counts[to] += 1; // ubs:ignore: to < n bound check on the line above guarantees safety
        }
    }
    counts
}

fn build_map_edges(graph: &impl GraphView, decisions: &[DecisionRecord]) -> Result<Vec<MapEdge>> {
    let decision_ids: BTreeSet<&str> = decisions.iter().map(|d| d.id.as_str()).collect();

    // Uses "RETURN from.id AS from_id, to.id AS to_id" pattern
    let rows = graph.query(
        "MATCH (from:`Decision`)-[:`SUPERSEDES`]->(to:`Decision`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
        &GraphParams::new(),
    )?;
    let mut edges = Vec::new();
    for row in rows {
        let from_id = match row.get("from_id") {
            Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — GraphValue holds borrowed ref, need owned String for MapEdge
            _ => continue,
        };
        let to_id = match row.get("to_id") {
            Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — GraphValue holds borrowed ref, need owned String for MapEdge
            _ => continue,
        };
        if decision_ids.contains(from_id.as_str()) && decision_ids.contains(to_id.as_str()) {
            edges.push(MapEdge {
                from_id,
                to_id,
                kind: "supersedes".to_owned(), // ubs:ignore: static string literal; to_owned is the idiomatic conversion
            });
        }
    }
    Ok(edges)
}

fn density_bands(decisions: &[DecisionRecord], y_spectral: &[f64]) -> Vec<DensityBand> {
    if decisions.is_empty() {
        return vec![];
    }
    let mut topic_y: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (d, &y) in decisions.iter().zip(y_spectral) {
        let topic = d
            .topic_keys
            .first()
            .cloned() // ubs:ignore: clone necessary — first() returns &String, topic_y needs owned key
            .unwrap_or_else(|| "misc".to_owned()); // ubs:ignore: safe default for decisions with no topic
        topic_y.entry(topic).or_default().push(y);
    }
    let mut bands: Vec<DensityBand> = topic_y
        .into_iter()
        .filter(|(_, ys)| !ys.is_empty())
        .map(|(label, ys)| {
            let med = median(&ys);
            let spread = ys.iter().map(|&y| (y - med).abs()).fold(0.0_f64, f64::max);
            DensityBand {
                label,
                y_center: med,
                y_radius: (spread + 0.05).max(0.05),
            }
        })
        .collect();
    bands.sort_by(|a, b| {
        a.y_center
            .partial_cmp(&b.y_center)
            .unwrap_or(std::cmp::Ordering::Equal) // ubs:ignore: unwrap_or on partial_cmp — NaN safety for f64
    });
    bands
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal) // ubs:ignore: unwrap_or on partial_cmp — NaN safety for f64
    });
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n.is_multiple_of(2) {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 // ubs:ignore: n is sorted.len(); n/2-1 and n/2 are in-bounds; n==0 returns early above
    } else {
        sorted[n / 2] // ubs:ignore: n is sorted.len(); n/2 < n for n >= 1; n==0 returns early above
    }
}
