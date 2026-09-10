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
use crate::queries::{
    get_decision, get_decision_outcome, get_decision_quality_candidates, get_supersession_chain,
    resolve_decision_by_description, search_decisions, DecisionQualityCandidatesRequest,
};
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
fn situational_decisions_return_same_ids_as_memory() -> Result<()> {
    with_postgres_graph("situational-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = situational_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let context = crate::queries::QueryContext::local();
        let request = crate::queries::SituationalRequest {
            paths: vec!["src/api/auth.rs".to_owned()],
            limit: 10,
            ..Default::default()
        };
        let memory_matches =
            crate::queries::get_situational_decisions(&context, &memory, &ledger, &request)?;
        let pg_matches =
            crate::queries::get_situational_decisions(&context, pg, &ledger, &request)?;

        let memory_ids: Vec<_> = memory_matches
            .data
            .matches
            .iter()
            .map(|m| m.decision.id.clone())
            .collect();
        let pg_ids: Vec<_> = pg_matches
            .data
            .matches
            .iter()
            .map(|m| m.decision.id.clone())
            .collect();
        if memory_ids != pg_ids {
            return Err(test_error(format!(
                "get_situational_decisions id mismatch: memory={memory_ids:?} pg={pg_ids:?}"
            )));
        }
        if memory_ids.is_empty() {
            return Err(test_error(
                "fixture should produce at least one situational match",
            ));
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

// ── Resolve-by-description parity (hivemind-tenv.1) ────────────────────────────
//
// `resolve_decision_by_description` reuses `collect_graph_search_results`'s tier system
// unmodified (docs/AGENT_FLUENT_QUERYING.md §1.1), so it needs no new Postgres query support.
// `get_decision_outcome`'s bespoke query shapes were a separate, pre-existing gap in
// `dispatch_query` on this backend (hivemind-kj0i; memory.rs was fixed earlier by
// hivemind-tenv.2) — see the outcome-parity tests below for that coverage.
// `get_decision_context`'s shapes remain unaudited on this backend; out of scope for kj0i.

#[test]
fn resolve_decision_by_description_matches_memory() -> Result<()> {
    with_postgres_graph("resolve-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        // "slice 1" uniquely matches decision:1's title ("Use Kuzu for slice 1") — decision:2's
        // title ("Use Kuzu with conservative Cypher") does not contain "slice".
        let memory_resolved = resolve_decision_by_description(&memory, "slice 1", None)?.data;
        let pg_resolved = resolve_decision_by_description(pg, "slice 1", None)?.data;
        if memory_resolved != pg_resolved {
            return Err(test_error(format!(
                "resolve_decision_by_description mismatch: memory={memory_resolved:?} pg={pg_resolved:?}"
            )));
        }

        // "Kuzu" alone matches both decisions at the same rank tier: both backends must agree
        // it is Ambiguous, in the same candidate order (rank asc, event_origin desc, id asc).
        let memory_ambiguous = resolve_decision_by_description(&memory, "Kuzu", None)?.data;
        let pg_ambiguous = resolve_decision_by_description(pg, "Kuzu", None)?.data;
        if memory_ambiguous != pg_ambiguous {
            return Err(test_error(format!(
                "resolve_decision_by_description ambiguity mismatch: memory={memory_ambiguous:?} pg={pg_ambiguous:?}"
            )));
        }
        Ok(())
    })
}

// ── get_decision_outcome / get_decision_quality_candidates parity (hivemind-kj0i) ──

#[test]
fn get_decision_outcome_matches_memory() -> Result<()> {
    with_postgres_graph("outcome-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        for decision_id in [
            "decision:clean",
            "decision:old",
            "decision:new",
            "decision:stale-direct",
            "decision:stale-option",
            "decision:contested",
            "decision:thin",
        ] {
            let memory_result = get_decision_outcome(&memory, decision_id)?.data;
            let pg_result = get_decision_outcome(pg, decision_id)?.data;
            if memory_result != pg_result {
                return Err(test_error(format!(
                    "get_decision_outcome mismatch for {decision_id}: memory={memory_result:?} pg={pg_result:?}"
                )));
            }
        }
        Ok(())
    })
}

#[test]
fn get_decision_quality_candidates_matches_memory() -> Result<()> {
    with_postgres_graph("quality-candidates-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let request = DecisionQualityCandidatesRequest {
            limit: 100,
            ..Default::default()
        };
        let memory_ids: Vec<_> = get_decision_quality_candidates(&memory, &request)?
            .data
            .iter()
            .map(|o| o.decision_id.clone())
            .collect();
        let pg_ids: Vec<_> = get_decision_quality_candidates(pg, &request)?
            .data
            .iter()
            .map(|o| o.decision_id.clone())
            .collect();
        if memory_ids != pg_ids {
            return Err(test_error(format!(
                "get_decision_quality_candidates id mismatch: memory={memory_ids:?} pg={pg_ids:?}"
            )));
        }

        // since_event_origin floors by the decision node's own event_origin.
        let since_request = DecisionQualityCandidatesRequest {
            since_event_origin: Some(15),
            limit: 100,
            ..Default::default()
        };
        let memory_since: Vec<_> = get_decision_quality_candidates(&memory, &since_request)?
            .data
            .iter()
            .map(|o| o.decision_id.clone())
            .collect();
        let pg_since: Vec<_> = get_decision_quality_candidates(pg, &since_request)?
            .data
            .iter()
            .map(|o| o.decision_id.clone())
            .collect();
        if memory_since != pg_since {
            return Err(test_error(format!(
                "get_decision_quality_candidates since-filter mismatch: memory={memory_since:?} pg={pg_since:?}"
            )));
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

fn situational_fixture_ledger() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in [
        make_event(
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:auth-note",
                "content": "Bearer auth on Postgres requires session tokens for the API layer",
                "source": "test"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:auth",
                "title": "Adopt bearer auth",
                "rationale": "Keeps session handling stateless",
                "topic_keys": ["auth"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": ["evidence:auth-note"]
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:cache",
                "title": "Use Redis for read-through cache",
                "rationale": "Cuts database load on hot reads",
                "topic_keys": ["caching"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
    ] {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// Exercises every get_decision_outcome signal (superseded, stale_premises via both
/// PREMISED_ON_DIRECT and CHOSE->PREMISED_ON, contested, thin_structure) plus a clean
/// decision, so the outcome-parity tests can diff backend behavior signal-by-signal.
fn outcome_fixture_ledger() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for event in [
        make_event(
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hypothesis:direct",
                "statement": "Direct premise holds"
            }),
        ),
        make_event(
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hypothesis:option",
                "statement": "Option premise holds"
            }),
        ),
        make_event(
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:refute-direct",
                "content": "Direct premise was wrong",
                "source": "test"
            }),
        ),
        make_event(
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:refute-option",
                "content": "Option premise was wrong",
                "source": "test"
            }),
        ),
        make_event(
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:clean",
                "content": "Supports the clean decision",
                "source": "test"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:clean",
                "title": "Clean decision",
                "rationale": "Well-supported",
                "topic_keys": ["infra"],
                "option_ids": ["option:clean"],
                "chosen_option_id": "option:clean",
                "hypothesis_ids": [],
                "evidence_ids": ["evidence:clean"]
            }),
        ),
        make_event(
            EventType::DecisionAccepted,
            "actor:alice",
            json!({"decision_id": "decision:clean"}),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:old",
                "title": "Superseded decision",
                "rationale": "Later replaced",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:new",
                "title": "Superseding decision",
                "rationale": "Replaces decision:old",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::DecisionSuperseded,
            "actor:planner",
            json!({
                "old_decision_id": "decision:old",
                "new_decision_id": "decision:new"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:stale-direct",
                "title": "Directly premised on a refuted hypothesis",
                "rationale": "No chosen option — PREMISED_ON_DIRECT",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hypothesis:direct"],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": "REFUTES",
                "from_id": "evidence:refute-direct",
                "to_id": "hypothesis:direct"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:stale-option",
                "title": "Premised via a chosen option on a refuted hypothesis",
                "rationale": "Chosen option — PREMISED_ON",
                "topic_keys": ["infra"],
                "option_ids": ["option:stale"],
                "chosen_option_id": "option:stale",
                "hypothesis_ids": ["hypothesis:option"],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": "REFUTES",
                "from_id": "evidence:refute-option",
                "to_id": "hypothesis:option"
            }),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:contested",
                "title": "Contested decision",
                "rationale": "Alice and bob disagree",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ),
        make_event(
            EventType::DecisionAccepted,
            "actor:alice",
            json!({"decision_id": "decision:contested"}),
        ),
        make_event(
            EventType::DecisionRejected,
            "actor:bob",
            json!({"decision_id": "decision:contested"}),
        ),
        make_event(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:thin",
                "title": "Thin decision",
                "rationale": "No options or evidence attached",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
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
