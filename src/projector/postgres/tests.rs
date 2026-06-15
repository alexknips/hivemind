// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::memory::MemoryGraph;
use crate::projector::{
    project_from_ledger, GraphParams, GraphValue, GraphView, NodeKind, RelationKind,
};
use crate::queries::{get_decision, get_supersession_chain, search_decisions, QueryContext};
use crate::Result;

use super::PostgresGraphView;

const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

// ── Replay parity ─────────────────────────────────────────────────────────────

#[test]
fn postgres_projection_matches_memory_for_all_node_and_edge_types() -> Result<()> {
    with_postgres_graph("parity-all-types", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;

        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        for node_kind in NodeKind::ALL {
            let cypher = format!(
                "MATCH (node:`{}`) RETURN node.id AS id ORDER BY node.id;",
                node_kind.table_name()
            );
            let memory_ids = node_ids(&memory.query(&cypher, &GraphParams::new())?);
            let pg_ids = node_ids(&pg.query(&cypher, &GraphParams::new())?);
            if memory_ids != pg_ids {
                return Err(test_error(format!(
                    "node id mismatch for {}: memory={memory_ids:?} pg={pg_ids:?}",
                    node_kind.table_name()
                )));
            }
        }

        for relation in RelationKind::ALL {
            let (from_kind, to_kind) = relation.endpoints();
            let cypher = format!(
                "MATCH (from:`{}`)-[:`{}`]->(to:`{}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;",
                from_kind.table_name(),
                relation.table_name(),
                to_kind.table_name()
            );
            let memory_rows = memory.query(&cypher, &GraphParams::new())?;
            let pg_rows = pg.query(&cypher, &GraphParams::new())?;
            if memory_rows != pg_rows {
                return Err(test_error(format!(
                    "edge mismatch for {}: memory={memory_rows:?} pg={pg_rows:?}",
                    relation.table_name()
                )));
            }
        }

        Ok(())
    })
}

#[test]
fn get_decision_returns_same_result_as_memory() -> Result<()> {
    with_postgres_graph("get-decision-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let memory_result = get_decision(&memory, "decision:1")?.data;
        let pg_result = get_decision(pg, "decision:1")?.data;
        if memory_result != pg_result {
            return Err(test_error(format!(
                "get_decision mismatch: memory={memory_result:?} pg={pg_result:?}"
            )));
        }
        Ok(())
    })
}

#[test]
fn search_decisions_returns_same_ids_as_memory() -> Result<()> {
    with_postgres_graph("search-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let request = crate::queries::SearchDecisionRequest::default();
        let memory_ids: Vec<_> = search_decisions(&memory, &request)?
            .data
            .items
            .iter()
            .map(|r| r.decision.id.clone())
            .collect();
        let pg_ids: Vec<_> = search_decisions(pg, &request)?
            .data
            .items
            .iter()
            .map(|r| r.decision.id.clone())
            .collect();
        if memory_ids != pg_ids {
            return Err(test_error(format!(
                "search_decisions id mismatch: memory={memory_ids:?} pg={pg_ids:?}"
            )));
        }
        Ok(())
    })
}

#[test]
fn supersession_chain_matches_memory() -> Result<()> {
    with_postgres_graph("supersession-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let memory_chain = get_supersession_chain(&memory, "decision:1")?.data;
        let pg_chain = get_supersession_chain(pg, "decision:1")?.data;
        if memory_chain != pg_chain {
            return Err(test_error(format!(
                "supersession chain mismatch: memory={memory_chain:?} pg={pg_chain:?}"
            )));
        }
        Ok(())
    })
}

// ── Multi-tenant isolation ────────────────────────────────────────────────────

#[test]
fn two_tenants_decisions_are_isolated_in_projection() -> Result<()> {
    with_postgres_graph("tenant-isolation-a", |tenant_a| {
        let tenant_b = tenant_a.for_tenant(unique_tenant("tenant-isolation-b"))?;
        tenant_b.wipe()?;

        // Project fixture decisions into tenant_a only.
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, tenant_a, 0)?;

        // tenant_a can see its decisions.
        let in_a = get_decision(tenant_a, "decision:1")?.data;
        if in_a.is_none() {
            return Err(test_error("decision:1 missing from tenant_a view"));
        }

        // tenant_b shares the DB but its tenant scope is empty.
        let in_b = get_decision(&tenant_b, "decision:1")?.data;
        if in_b.is_some() {
            return Err(test_error(
                "decision:1 should not be visible in tenant_b view",
            ));
        }

        Ok(())
    })
}

// ── Rebuild (wipe + replay) ───────────────────────────────────────────────────

#[test]
fn rebuild_produces_identical_result() -> Result<()> {
    with_postgres_graph("rebuild-parity", |pg| {
        let ledger = fixture_ledger()?;
        crate::projector::rebuild_graph(&ledger, pg)?;
        let first = pg.query(
            "MATCH (node:`Decision`) RETURN node.id AS id ORDER BY node.id;",
            &GraphParams::new(),
        )?;

        crate::projector::rebuild_graph(&ledger, pg)?;
        let second = pg.query(
            "MATCH (node:`Decision`) RETURN node.id AS id ORDER BY node.id;",
            &GraphParams::new(),
        )?;

        if first != second {
            return Err(test_error("rebuild not idempotent"));
        }
        Ok(())
    })
}

// ── Node property storage ─────────────────────────────────────────────────────

#[test]
fn node_properties_round_trip_through_postgres() -> Result<()> {
    with_postgres_graph("node-props", |pg| {
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, pg, 0)?;

        let rows = pg.query(
            "MATCH (node:`Decision`) RETURN node.id AS id ORDER BY node.id;",
            &GraphParams::new(),
        )?;
        let decision = rows
            .iter()
            .find(|r| r.get("id") == Some(&GraphValue::String("decision:1".to_owned())));
        let Some(decision) = decision else {
            return Err(test_error("decision:1 missing from projection"));
        };
        if decision.get("title") != Some(&GraphValue::String("Use Kuzu for slice 1".to_owned())) {
            return Err(test_error(format!("title mismatch: {decision:?}")));
        }
        if decision.get("topic_keys")
            != Some(&GraphValue::StringList(vec![
                "architecture".to_owned(),
                "memory".to_owned(),
            ]))
        {
            return Err(test_error(format!("topic_keys mismatch: {decision:?}")));
        }
        Ok(())
    })
}

// ── Test helpers ──────────────────────────────────────────────────────────────

fn with_postgres_graph<T>(
    prefix: &str,
    f: impl FnOnce(&PostgresGraphView) -> Result<T>,
) -> Result<()> {
    let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        eprintln!("skipping Postgres graph test; set {TEST_DATABASE_URL_ENV}");
        return Ok(());
    };

    let tenant_id = unique_tenant(prefix);
    let graph = PostgresGraphView::connect_with_pool_size(&database_url, tenant_id, 2)?;
    graph.wipe()?;
    f(&graph)?;
    graph.wipe()?;
    Ok(())
}

fn node_ids(rows: &[crate::projector::GraphRow]) -> Vec<GraphValue> {
    rows.iter().filter_map(|r| r.get("id").cloned()).collect()
}

fn unique_tenant(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("tenant:test:{prefix}:{nanos}:{}", std::process::id())
}

fn make_event(event_type: EventType, actor_id: &str, payload: serde_json::Value) -> Event {
    Event {
        tenant_id: Default::default(),
        event_id: None,
        event_uuid: Uuid::new_v4(),
        correlation_id: None,
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Agent,
        source_ref: None,
        payload,
        ts: Some(chrono::Utc::now()),
    }
}

fn fixture_ledger() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in [
        make_event(
            EventType::EvidenceRecorded,
            "actor:alice",
            json!({
                "evidence_id": "evidence:1",
                "content": "Kuzu supports graph projection",
                "source": "unit-test"
            }),
        ),
        make_event(
            EventType::HypothesisRecorded,
            "actor:alice",
            json!({
                "hypothesis_id": "hypothesis:1",
                "statement": "Graph projection is viable"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": "decision:1",
                "title": "Use Kuzu for slice 1",
                "rationale": "It gives us graph queries without extra services",
                "topic_keys": ["architecture", "memory"],
                "option_ids": ["option:1"],
                "chosen_option_id": "option:2",
                "hypothesis_ids": ["hypothesis:1"],
                "evidence_ids": ["evidence:1"]
            }),
        ),
        make_event(
            EventType::DecisionAccepted,
            "actor:bob",
            json!({"decision_id": "decision:1"}),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:alice",
            json!({
                "decision_id": "decision:2",
                "title": "Use Kuzu with conservative Cypher",
                "rationale": "Keep future backend swap cheap",
                "topic_keys": ["architecture"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::DecisionSuperseded,
            "actor:alice",
            json!({
                "old_decision_id": "decision:1",
                "new_decision_id": "decision:2"
            }),
        ),
    ] {
        ledger.append(event)?;
    }
    Ok(ledger)
}

fn test_error(message: impl Into<String>) -> crate::HivemindError {
    crate::error::ProjectorError::Projection(message.into()).into()
}

// Tests that check QueryContext-scoped queries are in the non-postgres query
// test module; the Postgres projection tests verify parity with the memory
// graph which already exercises those code paths.
#[allow(dead_code)]
fn _uses_query_context() {
    // Silence unused import warning for QueryContext imported above.
    let _ = QueryContext::local();
}
