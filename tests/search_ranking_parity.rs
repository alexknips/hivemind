//! Cross-backend ranking parity (hivemind-4urn, follow-up to hivemind-n1ij).
//!
//! `search_decisions_fts_with_context` (SQLite FTS5 candidate selection) and
//! `search_decisions_with_ledger` (the backend-agnostic graph path the Postgres/shared
//! backend uses) must return decisions in identical order for the same fixture and query.
//! Both functions are generic over `impl EventLedger` / take any `GraphView`, so exercising
//! them against the same `SqliteEventLedger` here proves the same code-path parity the
//! Postgres backend gets in production, without standing up a live Postgres instance.

use clap::Parser;

use hivemind::cli::{run, Cli};
use hivemind::ledger::SqliteEventLedger;
use hivemind::projector::{memory::MemoryGraph, rebuild_graph};
use hivemind::queries::{
    search_decisions_fts_with_context, search_decisions_with_ledger, QueryContext,
    SearchDecisionRequest,
};

#[allow(dead_code)]
#[path = "support/seed_data.rs"]
mod seed_data;

use seed_data::{unique_temp_dir, TestResult};

fn propose(
    hivemind_dir: &std::path::Path,
    title: &str,
    rationale: &str,
    topic_key: &str,
    options: &str,
    chose: &str,
) -> TestResult<String> {
    let decision_id = run(&Cli::parse_from([
        "hivemind",
        "--actor",
        "agent-parity",
        "--hivemind-dir",
        hivemind_dir.to_str().ok_or("seed path must be utf-8")?,
        "emit",
        "decision.proposed",
        "--title",
        title,
        "--rationale",
        rationale,
        "--topic-keys",
        topic_key,
        "--options",
        options,
        "--chose",
        chose,
    ]))?;
    Ok(decision_id)
}

fn ranked_ids(
    response: &hivemind::queries::QueryResponse<hivemind::queries::DecisionSearchResults>,
) -> Vec<(String, u8)> {
    response
        .data
        .items
        .iter()
        .map(|item| (item.decision.id.clone(), item.rank))
        .collect()
}

#[test]
fn search_ranking_parity_across_backends() -> TestResult<()> {
    let dir = unique_temp_dir("search-ranking-parity");
    std::fs::create_dir_all(&dir)?;

    // Exact title match -> rank 0.
    propose(
        &dir,
        "Queue",
        "Unrelated rationale about caching layers",
        "infra",
        "sync,async",
        "async",
    )?;
    // Title term matches -> rank 1 (two of these, to also exercise the decision-id tiebreak).
    propose(
        &dir,
        "Adopt async queue for ingestion",
        "Durable delivery for the ingest pipeline",
        "infra",
        "sync,async",
        "async",
    )?;
    propose(
        &dir,
        "Batch queue writes for retries",
        "Reduce write amplification on retry storms",
        "infra",
        "sync,async",
        "async",
    )?;
    // Rationale term match -> rank 2. The term is repeated heavily on purpose: under the old
    // SQLite bm25()-score ordering this out-scored the exact title match below (bm25 rewards
    // term frequency over the field-rank hierarchy), which is exactly the parity bug this test
    // guards against — with rank-based ordering it correctly sorts behind rank 0/1 matches.
    propose(
        &dir,
        "Pick a durable event log",
        "queue queue queue queue queue queue queue queue backpressure queue queue",
        "infra",
        "sync,async",
        "async",
    )?;
    // One-hop graph context (option id) match -> rank 3.
    propose(
        &dir,
        "Route retry traffic through a broker",
        "Isolate retry storms from primary traffic",
        "infra",
        "queue-backed,direct-write",
        "queue-backed",
    )?;
    // No match at all -> excluded from the "queue" query entirely.
    propose(
        &dir,
        "Cache invalidation strategy",
        "Time-based expiry keeps reads fresh",
        "infra",
        "ttl,manual",
        "ttl",
    )?;

    let ledger = SqliteEventLedger::open(&dir)?;
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    let context = QueryContext::local();

    for query in [Some("queue".to_owned()), None] {
        let request = SearchDecisionRequest {
            query,
            limit: 50,
            ..SearchDecisionRequest::default()
        };

        let fts_response = search_decisions_fts_with_context(&context, &ledger, &graph, &request)?;
        let ledger_response = search_decisions_with_ledger(&context, &ledger, &graph, &request)?;

        assert_eq!(
            fts_response.data.total_matches, ledger_response.data.total_matches,
            "match count diverged for query {:?}",
            request.query,
        );
        let fts_order = ranked_ids(&fts_response);
        let ledger_order = ranked_ids(&ledger_response);
        assert_eq!(
            fts_order, ledger_order,
            "backend order diverged for query {:?}",
            request.query,
        );
        assert!(
            !fts_order.is_empty(),
            "fixture should produce matches for query {:?}",
            request.query,
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
