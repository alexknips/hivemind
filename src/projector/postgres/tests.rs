// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
use std::fs;
use std::path::PathBuf;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType};
use crate::ledger::{
    AnyLedger, EventLedger, InMemoryEventLedger, PostgresEventLedger, SqliteEventLedger,
};
use crate::projector::memory::MemoryGraph;
use crate::projector::{
    project_from_ledger, GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind,
};
use crate::queries::{
    export_decision_log, get_decision, get_decision_brief, get_decision_brief_at,
    get_decision_context, get_decision_context_candidates, get_decision_neighborhood,
    get_decision_outcome, get_decision_outcome_at, get_decision_quality_candidates,
    get_decision_quality_score, get_failure_attribution, get_supersession_chain, grounding_of_at,
    resolve_decision_by_description, scan_decision_quality, search_decisions,
    DecisionContextRequest, DecisionLogOutcome, DecisionLogRequest,
    DecisionQualityCandidatesRequest, FailureAttributionRequest, GroundingAdded, GroundingKind,
    NeighborhoodRequest, QueryContext, ResolveOutcome, ScanQualityRequest, ScorerConfig,
    SearchDecisionRequest,
};
use crate::summarize::{recall_decisions, RecallRequest, RECALL_MAX_LIMIT};
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
fn postgres_projects_projection_matches_memory_with_links_and_anchors() -> Result<()> {
    with_postgres_graph("projects-links-anchors", |pg| {
        let memory = MemoryGraph::default();
        let ledger = InMemoryEventLedger::new();
        for event in [
            make_event(
                EventType::ProjectRegistered,
                "actor:alice",
                json!({"handle": "platform", "display_name": "Platform"}),
            ),
            make_event(
                EventType::ProjectRegistered,
                "actor:alice",
                json!({"handle": "billing", "display_name": "Billing"}),
            ),
            make_event(
                EventType::ProjectRegistered,
                "actor:alice",
                json!({"handle": "auth", "display_name": "Auth"}),
            ),
            make_event(
                EventType::ProjectLinked,
                "actor:alice",
                json!({"from": "billing", "to": "platform", "kind": "part_of"}),
            ),
            make_event(
                EventType::ProjectLinked,
                "actor:alice",
                json!({"from": "billing", "to": "auth", "kind": "depends_on"}),
            ),
            make_event(
                EventType::ProjectAnchored,
                "actor:alice",
                json!({"handle": "billing", "anchor_kind": "folder", "value": "services/billing"}),
            ),
            make_event(
                EventType::ProjectAnchored,
                "actor:alice",
                json!({"handle": "billing", "anchor_kind": "rig", "value": "hivemind"}),
            ),
        ] {
            ledger.append(event)?;
        }

        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let node_cypher = "MATCH (node:`Project`) RETURN node.id AS id, node.handle AS handle, node.display_name AS display_name, node.anchors AS anchors ORDER BY node.id;";
        let memory_rows = memory.query(node_cypher, &GraphParams::new())?;
        let pg_rows = pg.query(node_cypher, &GraphParams::new())?;
        if memory_rows != pg_rows {
            return Err(test_error(format!(
                "project node mismatch: memory={memory_rows:?} pg={pg_rows:?}"
            )));
        }
        if memory_rows.len() != 3 {
            return Err(test_error(format!(
                "expected 3 project nodes, got {}",
                memory_rows.len()
            )));
        }

        let billing = pg_rows
            .iter()
            .find(|row| row.get("id") == Some(&GraphValue::String("billing".to_owned())))
            .ok_or_else(|| test_error("billing project node missing on postgres"))?;
        match billing.get("anchors") {
            Some(GraphValue::StringList(values)) => {
                let mut sorted = values.clone();
                sorted.sort();
                let expected = vec![
                    "folder:services/billing".to_owned(),
                    "rig:hivemind".to_owned(),
                ];
                if sorted != expected {
                    return Err(test_error(format!(
                        "unexpected anchors on postgres: {sorted:?}"
                    )));
                }
            }
            other => {
                return Err(test_error(format!(
                    "expected anchors StringList on postgres, got {other:?}"
                )))
            }
        }

        for (relation, expected_to) in [("PART_OF", "platform"), ("DEPENDS_ON", "auth")] {
            let cypher = format!(
                "MATCH (from:`Project`)-[:`{relation}`]->(to:`Project`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;"
            );
            let memory_rows = memory.query(&cypher, &GraphParams::new())?;
            let pg_rows = pg.query(&cypher, &GraphParams::new())?;
            if memory_rows != pg_rows {
                return Err(test_error(format!(
                    "{relation} mismatch: memory={memory_rows:?} pg={pg_rows:?}"
                )));
            }
            let expected_row = GraphRow::from([
                (
                    "from_id".to_owned(),
                    GraphValue::String("billing".to_owned()),
                ),
                (
                    "to_id".to_owned(),
                    GraphValue::String(expected_to.to_owned()),
                ),
            ]);
            if pg_rows != vec![expected_row] {
                return Err(test_error(format!(
                    "unexpected {relation} rows on postgres: {pg_rows:?}"
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

// ── Grounding (hivemind-gwhr.1): FOLLOWS_FROM + hypothesis kind/check_by/would_change_if ──

#[test]
fn follows_from_and_hypothesis_kind_match_memory() -> Result<()> {
    with_postgres_graph("grounding-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = InMemoryEventLedger::new();
        for event in [
            make_event(
                EventType::HypothesisRecorded,
                "actor:alice",
                json!({
                    "hypothesis_id": "hypothesis:bet",
                    "statement": "Latency stays under 50ms at 10x load",
                    "kind": "bet",
                    "check_by": "2026-10-01T00:00:00Z",
                    "would_change_if": "A 10x load test shows p99 above 50ms"
                }),
            ),
            make_event(
                EventType::DecisionProposed,
                "actor:alice",
                json!({
                    "decision_id": "decision:premise",
                    "title": "Earlier decision",
                    "rationale": "Established earlier as a standing premise decision",
                    "topic_keys": ["architecture"],
                    "option_ids": [],
                    "chosen_option_id": null,
                    "hypothesis_ids": [],
                    "evidence_ids": []
                }),
            ),
            make_event(
                EventType::DecisionProposed,
                "actor:alice",
                json!({
                    "decision_id": "decision:follower",
                    "title": "Later decision",
                    "rationale": "Follows from the earlier one",
                    "topic_keys": ["architecture"],
                    "option_ids": [],
                    "chosen_option_id": null,
                    "hypothesis_ids": [],
                    "evidence_ids": []
                }),
            ),
            make_event(
                EventType::RelationAdded,
                "actor:alice",
                json!({
                    "relation": "FOLLOWS_FROM",
                    "from_id": "decision:follower",
                    "to_id": "decision:premise"
                }),
            ),
        ] {
            ledger.append(event)?;
        }

        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let edge_cypher = "MATCH (from:`Decision`)-[:`FOLLOWS_FROM`]->(to:`Decision`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id;";
        let memory_edges = memory.query(edge_cypher, &GraphParams::new())?;
        let pg_edges = pg.query(edge_cypher, &GraphParams::new())?;
        if memory_edges != pg_edges || memory_edges.is_empty() {
            return Err(test_error(format!(
                "FOLLOWS_FROM edge mismatch: memory={memory_edges:?} pg={pg_edges:?}"
            )));
        }

        let node_cypher = "MATCH (node:`Hypothesis` {id: $id}) RETURN node.kind AS kind, node.check_by AS check_by, node.would_change_if AS would_change_if;";
        let params = GraphParams::from([(
            "id".to_owned(),
            GraphValue::String("hypothesis:bet".to_owned()),
        )]);
        let memory_rows = memory.query(node_cypher, &params)?;
        let pg_rows = pg.query(node_cypher, &params)?;
        if memory_rows != pg_rows {
            return Err(test_error(format!(
                "hypothesis kind/check_by/would_change_if mismatch: memory={memory_rows:?} pg={pg_rows:?}"
            )));
        }
        if pg_rows.first().and_then(|row| row.get("kind"))
            != Some(&GraphValue::String("bet".to_owned()))
        {
            return Err(test_error(format!(
                "expected kind=bet on postgres: {pg_rows:?}"
            )));
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
// `get_decision_context`'s and `get_decision_brief`'s shapes were the same class of gap,
// closed by hivemind-ookw — see the context/brief-parity tests below.

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

        // A natural question resolves through the same stop-word and stemming path on both
        // backends: "why did we ..." drops the question words, "slice 1" pins decision:1.
        let question = "why did we use Kuzu for slice 1";
        let memory_question = resolve_decision_by_description(&memory, question, None)?.data;
        let pg_question = resolve_decision_by_description(pg, question, None)?.data;
        if memory_question != pg_question
            || !matches!(&memory_question, ResolveOutcome::Resolved { candidate } if candidate.decision_id == "decision:1")
        {
            return Err(test_error(format!(
                "natural-question resolution mismatch: memory={memory_question:?} pg={pg_question:?}"
            )));
        }

        // A word no decision contains ("finally") leaves only close candidates, best first,
        // each listing what it lacks -- the same on both backends, never Resolved. decision:2
        // trails decision:1: it lacks "slice" too and matches "1" only via its supersedes id.
        let close = "why did we finally use Kuzu for slice 1";
        let memory_close = resolve_decision_by_description(&memory, close, None)?.data;
        let pg_close = resolve_decision_by_description(pg, close, None)?.data;
        if memory_close != pg_close
            || !matches!(&memory_close, ResolveOutcome::Ambiguous { candidates }
                if candidates.first().is_some_and(|best| best.decision_id == "decision:1"
                    && best.missing_terms == ["finally"])
                    && candidates.iter().all(|candidate| !candidate.missing_terms.is_empty()))
        {
            return Err(test_error(format!(
                "close-candidate resolution mismatch: memory={memory_close:?} pg={pg_close:?}"
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

// ── get_decision_context / get_decision_context_candidates parity (hivemind-ookw) ──

#[test]
fn get_decision_context_matches_memory() -> Result<()> {
    with_postgres_graph("context-parity", |pg| {
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
            let memory_result = get_decision_context(&memory, decision_id)?.data;
            let pg_result = get_decision_context(pg, decision_id)?.data;
            if memory_result != pg_result {
                return Err(test_error(format!(
                    "get_decision_context mismatch for {decision_id}: memory={memory_result:?} pg={pg_result:?}"
                )));
            }
        }

        // Nonexistent decision: both backends must agree on None, not error.
        let memory_missing = get_decision_context(&memory, "decision:missing")?.data;
        let pg_missing = get_decision_context(pg, "decision:missing")?.data;
        if memory_missing != pg_missing {
            return Err(test_error(format!(
                "get_decision_context missing-decision mismatch: memory={memory_missing:?} pg={pg_missing:?}"
            )));
        }
        Ok(())
    })
}

#[test]
fn get_decision_context_candidates_matches_memory() -> Result<()> {
    with_postgres_graph("context-candidates-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let request = DecisionContextRequest {
            limit: 100,
            ..Default::default()
        };
        let memory_ids: Vec<_> = get_decision_context_candidates(&memory, &request)?
            .data
            .iter()
            .map(|c| c.decision_id.clone())
            .collect();
        let pg_ids: Vec<_> = get_decision_context_candidates(pg, &request)?
            .data
            .iter()
            .map(|c| c.decision_id.clone())
            .collect();
        if memory_ids != pg_ids {
            return Err(test_error(format!(
                "get_decision_context_candidates id mismatch: memory={memory_ids:?} pg={pg_ids:?}"
            )));
        }

        // since_event_origin floors by the decision node's own event_origin, same as
        // get_decision_quality_candidates's since-filter test above.
        let since_request = DecisionContextRequest {
            since_event_origin: Some(15),
            limit: 100,
            ..Default::default()
        };
        let memory_since: Vec<_> = get_decision_context_candidates(&memory, &since_request)?
            .data
            .iter()
            .map(|c| c.decision_id.clone())
            .collect();
        let pg_since: Vec<_> = get_decision_context_candidates(pg, &since_request)?
            .data
            .iter()
            .map(|c| c.decision_id.clone())
            .collect();
        if memory_since != pg_since {
            return Err(test_error(format!(
                "get_decision_context_candidates since-filter mismatch: memory={memory_since:?} pg={pg_since:?}"
            )));
        }
        Ok(())
    })
}

// ── scan_decision_quality / get_decision_quality_score / get_failure_attribution parity
//    (hivemind-bbnw.1) — these MCP-facing entry points chain through
//    get_decision_quality_candidates and get_decision_context_candidates above, but had no
//    direct parity coverage of their own; a dispatch-table gap specific to how they call
//    through (e.g. a cursor/limit combination neither of the callers above exercises) would
//    have gone undetected.

#[test]
fn scan_decision_quality_matches_memory() -> Result<()> {
    with_postgres_graph("scan-quality-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let request = ScanQualityRequest {
            limit: 100,
            ..Default::default()
        };
        let memory_scored =
            scan_decision_quality(&memory, &request, &ScorerConfig::default())?.data;
        let pg_scored = scan_decision_quality(pg, &request, &ScorerConfig::default())?.data;
        if memory_scored != pg_scored {
            return Err(test_error(format!(
                "scan_decision_quality mismatch: memory={memory_scored:?} pg={pg_scored:?}"
            )));
        }
        Ok(())
    })
}

#[test]
fn get_decision_quality_score_matches_memory() -> Result<()> {
    with_postgres_graph("quality-score-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        for decision_id in ["decision:clean", "decision:contested", "decision:missing"] {
            let memory_result =
                get_decision_quality_score(&memory, decision_id, &ScorerConfig::default())?.data;
            let pg_result =
                get_decision_quality_score(pg, decision_id, &ScorerConfig::default())?.data;
            if memory_result != pg_result {
                return Err(test_error(format!(
                    "get_decision_quality_score mismatch for {decision_id}: memory={memory_result:?} pg={pg_result:?}"
                )));
            }
        }
        Ok(())
    })
}

#[test]
fn get_failure_attribution_matches_memory() -> Result<()> {
    with_postgres_graph("failure-attribution-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = outcome_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let request = FailureAttributionRequest::default();
        let memory_report = get_failure_attribution(&memory, &request)?.data;
        let pg_report = get_failure_attribution(pg, &request)?.data;
        if memory_report != pg_report {
            return Err(test_error(format!(
                "get_failure_attribution mismatch: memory={memory_report:?} pg={pg_report:?}"
            )));
        }
        Ok(())
    })
}

// ── get_decision_brief parity (hivemind-ookw) ───────────────────────────────────
//
// Composes get_decision + get_decision_context + get_decision_outcome + resolve_option_label
// (the "MATCH (o:`Option` {id: $id}) RETURN o.label AS label" shape) — exercises all four
// dispatch_query gaps this bead closes in one call, including chosen/rejected option labels.

#[test]
fn get_decision_brief_matches_memory() -> Result<()> {
    with_postgres_graph("brief-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        for decision_id in ["decision:1", "decision:2", "decision:missing"] {
            let memory_result = get_decision_brief(&memory, decision_id)?.data;
            let pg_result = get_decision_brief(pg, decision_id)?.data;
            if memory_result != pg_result {
                return Err(test_error(format!(
                    "get_decision_brief mismatch for {decision_id}: memory={memory_result:?} pg={pg_result:?}"
                )));
            }
        }
        Ok(())
    })
}

// ── Delegation marker parity (hivemind-zdsh.6) ──────────────────────────────────
//
// `delegated_by` rides on the Decision node as a plain property (Alex's option 1), so it
// must land and read back through the Postgres JSONB merge exactly as it does in memory —
// across context, the brief behind `verify`, the search context behind the digest, and the
// attribution report — and the merge must not disturb the proposal's own properties.

#[test]
fn delegation_marker_reads_match_memory() -> Result<()> {
    with_postgres_graph("delegation-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = delegation_fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        let decision_ids = ["decision:human", "decision:delegated", "decision:alone"];
        for decision_id in decision_ids {
            let memory_context = get_decision_context(&memory, decision_id)?.data;
            let pg_context = get_decision_context(pg, decision_id)?.data;
            if memory_context != pg_context {
                return Err(test_error(format!(
                    "delegation context mismatch for {decision_id}: memory={memory_context:?} pg={pg_context:?}"
                )));
            }
            let memory_brief = get_decision_brief(&memory, decision_id)?.data;
            let pg_brief = get_decision_brief(pg, decision_id)?.data;
            if memory_brief != pg_brief {
                return Err(test_error(format!(
                    "delegation brief mismatch for {decision_id}: memory={memory_brief:?} pg={pg_brief:?}"
                )));
            }
        }

        // The marker itself, not just agreement: present on the delegated decision, absent
        // (not null) on the other two.
        let delegated = get_decision_context(pg, "decision:delegated")?
            .data
            .ok_or_else(|| test_error("delegated decision context missing"))?;
        if delegated.delegated_by.as_deref() != Some("human:alex") {
            return Err(test_error(format!(
                "Postgres lost the delegation marker: {delegated:?}"
            )));
        }
        for decision_id in ["decision:human", "decision:alone"] {
            let context = get_decision_context(pg, decision_id)?
                .data
                .ok_or_else(|| test_error(format!("{decision_id} context missing")))?;
            if context.delegated_by.is_some() {
                return Err(test_error(format!(
                    "{decision_id} must carry no delegation marker: {context:?}"
                )));
            }
        }

        // The merge must not disturb what the proposal projected onto the same node.
        let memory_decision = get_decision(&memory, "decision:delegated")?.data;
        let pg_decision = get_decision(pg, "decision:delegated")?.data;
        if memory_decision != pg_decision || pg_decision.is_none() {
            return Err(test_error(format!(
                "delegation marker disturbed the decision node: memory={memory_decision:?} pg={pg_decision:?}"
            )));
        }

        let memory_candidates = get_decision_context_candidates(
            &memory,
            &DecisionContextRequest {
                limit: 10,
                ..Default::default()
            },
        )?
        .data;
        let pg_candidates = get_decision_context_candidates(
            pg,
            &DecisionContextRequest {
                limit: 10,
                ..Default::default()
            },
        )?
        .data;
        if memory_candidates != pg_candidates {
            return Err(test_error(format!(
                "delegation context candidates mismatch: memory={memory_candidates:?} pg={pg_candidates:?}"
            )));
        }

        let memory_markers = search_markers(&memory)?;
        let pg_markers = search_markers(pg)?;
        if memory_markers != pg_markers
            || !pg_markers.contains(&(
                "decision:delegated".to_owned(),
                Some("human:alex".to_owned()),
            ))
        {
            return Err(test_error(format!(
                "search graph_context delegated_by mismatch: memory={memory_markers:?} pg={pg_markers:?}"
            )));
        }

        // The decision-log export states the delegation next to the decider (hivemind-o7p2),
        // and reads it the same on both backends.
        let request = DecisionLogRequest::default();
        let (DecisionLogOutcome::Exported(memory_export), DecisionLogOutcome::Exported(pg_export)) = (
            export_decision_log(&memory, &ledger, &request)?,
            export_decision_log(pg, &ledger, &request)?,
        ) else {
            return Err(test_error(
                "an unfiltered export cannot be project_not_found",
            ));
        };
        if memory_export != pg_export {
            return Err(test_error(format!(
                "decision-log export mismatch: memory={memory_export:?} pg={pg_export:?}"
            )));
        }
        let delegated_lines = pg_export
            .files
            .values()
            .flat_map(|content| content.lines())
            .filter(|line| line.contains("Delegated by"))
            .count();
        // One INDEX cell in the root and one in the personal project's index, plus the
        // decision's own Provenance line; the two other decisions add none.
        if delegated_lines != 3 {
            return Err(test_error(format!(
                "export must state the delegation for the delegated decision only, found {delegated_lines} line(s): {:?}",
                pg_export.files
            )));
        }

        let memory_report =
            get_failure_attribution(&memory, &FailureAttributionRequest::default())?.data;
        let pg_report = get_failure_attribution(pg, &FailureAttributionRequest::default())?.data;
        if memory_report.by_delegation != pg_report.by_delegation
            || pg_report.by_delegation.len() != 2
        {
            return Err(test_error(format!(
                "by_delegation mismatch: memory={:?} pg={:?}",
                memory_report.by_delegation, pg_report.by_delegation
            )));
        }
        Ok(())
    })
}

// hivemind-s15q.10: `decision.moved` upserts only `project` / `project_source` through the JSONB
// merge, so a move (and a move back) leaves the proposal's own source, source_ref, tenant and
// event_origin in place and the decision keeps its place in an event_origin-ordered listing —
// the same checks `projector/tests.rs` runs on `MemoryGraph`.
#[test]
fn decision_moved_and_back_keeps_capture_origin_on_postgres() -> Result<()> {
    with_postgres_graph("decision-moved", |pg| {
        for (events, expected_project) in [(3, "pricing"), (4, "billing")] {
            pg.wipe()?;
            let ledger = crate::projector::tests::decision_moved_fixture_ledger(events)?;
            project_from_ledger(&ledger, pg, 0)?;
            crate::projector::tests::assert_move_keeps_capture_origin(pg, expected_project)?;
        }
        Ok(())
    })
}

fn search_markers(graph: &impl GraphView) -> Result<Vec<(String, Option<String>)>> {
    let results = search_decisions(graph, &SearchDecisionRequest::default())?.data;
    let mut markers: Vec<_> = results
        .items
        .into_iter()
        .map(|item| (item.decision.id, item.graph_context.delegated_by))
        .collect();
    markers.sort();
    Ok(markers)
}

/// Case 1 (a human accepts an agent's proposal), case 2 (the agent self-accepts under
/// `human:alex`'s delegation) and case 3 (the agent self-accepts alone), as raw events.
fn delegation_fixture_ledger() -> Result<InMemoryEventLedger> {
    let ledger = InMemoryEventLedger::new();
    for (decision_id, title, accepter, delegated_by) in [
        ("decision:human", "Human decided", "human:alex", None),
        (
            "decision:delegated",
            "Agent decided under delegation",
            "agent:claude:builder",
            Some("human:alex"),
        ),
        (
            "decision:alone",
            "Agent decided alone",
            "agent:claude:builder",
            None,
        ),
    ] {
        ledger.append(make_event(
            EventType::DecisionProposed,
            "agent:claude:builder",
            json!({
                "decision_id": decision_id,
                "title": title,
                "rationale": "A stated reason the projection tests do not read",
                "topic_keys": ["governance"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        ))?;
        let mut accepted = json!({ "decision_id": decision_id });
        if let Some(delegated_by) = delegated_by {
            accepted["delegated_by"] = json!(delegated_by);
        }
        ledger.append(make_event(EventType::DecisionAccepted, accepter, accepted))?;
    }
    Ok(ledger)
}

// ── get_decision_neighborhood parity (hivemind-5gwg) ────────────────────────────
//
// `why` composes the structural neighborhood with get_decision_brief and per-node label lookups
// (decision title, option label, hypothesis statement, evidence content) -- the last two shapes
// are the dispatch_query entries this bead adds.

#[test]
fn get_decision_neighborhood_matches_memory() -> Result<()> {
    with_postgres_graph("neighborhood-parity", |pg| {
        let memory = MemoryGraph::default();
        let ledger = fixture_ledger()?;
        project_from_ledger(&ledger, &memory, 0)?;
        project_from_ledger(&ledger, pg, 0)?;

        for decision_id in ["decision:1", "decision:2", "decision:missing"] {
            let request = NeighborhoodRequest::all();
            let mut memory_result = get_decision_neighborhood(&memory, decision_id, &request)?.data;
            let mut pg_result = get_decision_neighborhood(pg, decision_id, &request)?.data;
            // Neighbor-pair queries return the edge's event_origin on Postgres but not on the
            // in-memory graph (a pre-existing backend difference, not what this test pins).
            for edge in memory_result
                .edges
                .iter_mut()
                .chain(pg_result.edges.iter_mut())
            {
                edge.event_origin = None;
            }
            if memory_result != pg_result {
                return Err(test_error(format!(
                    "get_decision_neighborhood mismatch for {decision_id}: memory={memory_result:?} pg={pg_result:?}"
                )));
            }
        }

        // The labels are present, not just equal: decision:1's premise, evidence and the
        // decision that superseded it all read back as text on Postgres.
        let view = get_decision_neighborhood(pg, "decision:1", &NeighborhoodRequest::all())?.data;
        let label_of = |id: &str| {
            view.nodes
                .iter()
                .find(|node| node.id == id)
                .and_then(|node| node.label.clone())
        };
        for (id, expected) in [
            ("hypothesis:1", "Graph projection is viable"),
            ("evidence:1", "Kuzu supports graph projection"),
            ("decision:2", "Use Kuzu with conservative Cypher"),
        ] {
            if label_of(id).as_deref() != Some(expected) {
                return Err(test_error(format!(
                    "expected label {expected:?} on {id}, got {:?}",
                    label_of(id)
                )));
            }
        }
        if view.root.brief.is_none() {
            return Err(test_error("root brief missing on Postgres".to_owned()));
        }
        Ok(())
    })
}

// ── recall_decisions parity (hivemind-ot72.3) ───────────────────────────────────
//
// `recall_decisions` only accepts `&AnyLedger` (search_decisions_any dispatches on
// it), so this test — unlike the house pattern above — drives two real ledgers
// instead of one shared `InMemoryEventLedger`: a temp `SqliteEventLedger` behind
// `AnyLedger::Sqlite` (search_decisions_any routes this to FTS,
// search_decisions_fts_with_context) and a `PostgresEventLedger` behind
// `AnyLedger::Postgres` (routed to the backend-agnostic in-memory matcher,
// search_decisions_with_ledger). Both are seeded with the same fixture events, so
// only the returned decision-id *set* is compared: FTS and the portable matcher
// use different scoring internals and are not required to agree on order (that's
// tests/search_ranking_parity.rs's job).
#[test]
fn recall_decisions_returns_same_decision_set() -> Result<()> {
    assert_recall_parity("recall-parity", None)
}

// The documented question form: "what did we decide about X" drops the question words before
// searching, on SQLite (FTS) and Postgres (portable matcher) alike (hivemind-5gwg).
#[test]
fn recall_question_form_returns_same_decision_set() -> Result<()> {
    assert_recall_parity(
        "recall-question-parity",
        Some("what did we decide about Kuzu"),
    )
}

fn assert_recall_parity(prefix: &str, question: Option<&str>) -> Result<()> {
    let request = RecallRequest {
        q: question.map(str::to_owned),
        topic_keys: Vec::new(),
        statuses: Vec::new(),
        actor_ids: Vec::new(),
        sources: Vec::new(),
        since: None,
        until: None,
        limit: RECALL_MAX_LIMIT,
        cursor: None,
        project: None,
    };
    let Some((sqlite_response, postgres_response)) =
        recall_on_both_backends(prefix, fixture_events(), &request)?
    else {
        return Ok(());
    };

    let mut sqlite_ids: Vec<_> = sqlite_response
        .data
        .ranked
        .items
        .iter()
        .map(|item| item.decision.id.clone())
        .collect();
    let mut postgres_ids: Vec<_> = postgres_response
        .data
        .ranked
        .items
        .iter()
        .map(|item| item.decision.id.clone())
        .collect();
    sqlite_ids.sort();
    postgres_ids.sort();

    if sqlite_ids != postgres_ids {
        return Err(test_error(format!(
            "recall_decisions decision-id set mismatch: sqlite(FTS)={sqlite_ids:?} postgres(portable)={postgres_ids:?}"
        )));
    }
    if sqlite_ids.is_empty() {
        return Err(test_error(
            "fixture should produce at least one recall match",
        ));
    }
    Ok(())
}

// Recall asked from a project (hivemind-s15q.7): the SQLite FTS path and the Postgres portable
// matcher both filter on the decision's project and group own, then the parent's, then a
// dependency's -- the same decisions in the same order, with the same labels and scope note.
#[test]
fn project_first_recall_matches_between_backends() -> Result<()> {
    let fixture = crate::queries::test_fixtures::project_first_fixture()?;
    let mut events = Vec::new();
    fixture.ledger.replay_from(0, &mut |event| {
        events.push(event.clone());
        Ok(())
    })?;

    let request = RecallRequest {
        q: Some("pricing".to_owned()),
        topic_keys: Vec::new(),
        statuses: Vec::new(),
        actor_ids: Vec::new(),
        sources: Vec::new(),
        since: None,
        until: None,
        limit: RECALL_MAX_LIMIT,
        cursor: None,
        project: Some("billing".to_owned()),
    };
    let Some((sqlite_response, postgres_response)) =
        recall_on_both_backends("recall-project-first-parity", events, &request)?
    else {
        return Ok(());
    };

    let shape = |response: &crate::queries::QueryResponse<crate::summarize::RecallResponse>| {
        response
            .data
            .ranked
            .items
            .iter()
            .map(|item| {
                (
                    item.decision.id.clone(),
                    item.scope.as_ref().map(|scope| scope.label.clone()),
                )
            })
            .collect::<Vec<_>>()
    };
    if shape(&sqlite_response) != shape(&postgres_response) {
        return Err(test_error(format!(
            "project-first recall mismatch: sqlite(FTS)={:?} postgres(portable)={:?}",
            shape(&sqlite_response),
            shape(&postgres_response)
        )));
    }
    if sqlite_response.data.scope != postgres_response.data.scope {
        return Err(test_error(format!(
            "scope note mismatch: sqlite(FTS)={:?} postgres(portable)={:?}",
            sqlite_response.data.scope, postgres_response.data.scope
        )));
    }
    // Billing's own, its parent's and Auth's two decisions, and nothing else.
    if sqlite_response.data.ranked.items.len() != 4 {
        return Err(test_error(format!(
            "billing's recall should hold its own, its parent's and auth's two decisions, got {:?}",
            shape(&sqlite_response)
        )));
    }
    Ok(())
}

/// Runs one recall request on a SQLite ledger (FTS) and on a Postgres ledger (the portable
/// matcher), both seeded with the same events. `None` when no Postgres database is configured.
fn recall_on_both_backends(
    prefix: &str,
    events: Vec<Event>,
    request: &RecallRequest,
) -> Result<
    Option<(
        crate::queries::QueryResponse<crate::summarize::RecallResponse>,
        crate::queries::QueryResponse<crate::summarize::RecallResponse>,
    )>,
> {
    let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping Postgres graph test; set {TEST_DATABASE_URL_ENV}");
        return Ok(None);
    };

    let memory_graph = MemoryGraph::default();
    let sqlite_dir = temp_hivemind_dir(prefix);
    let sqlite_ledger = SqliteEventLedger::open(&sqlite_dir)?;
    for event in events.iter().cloned() {
        sqlite_ledger.append(event)?;
    }
    project_from_ledger(&sqlite_ledger, &memory_graph, 0)?;

    let tenant_id = unique_tenant(prefix);
    let pg_graph = PostgresGraphView::connect_with_pool_size(&database_url, tenant_id.clone(), 2)?;
    pg_graph.wipe()?;
    let postgres_ledger =
        PostgresEventLedger::connect_with_pool_size(&database_url, tenant_id.clone(), 2)?;
    for event in events {
        postgres_ledger.append(event)?;
    }
    project_from_ledger(&postgres_ledger, &pg_graph, 0)?;

    let sqlite_context = QueryContext::local();
    // A project's links are read from the ledger of the context's tenant, so the Postgres side
    // asks as the tenant its ledger was seeded under.
    let postgres_context = QueryContext::new(
        crate::events::TenantId::new(tenant_id).map_err(|error| test_error(error.to_string()))?,
    );

    let sqlite_any = AnyLedger::Sqlite(sqlite_ledger);
    let postgres_any = AnyLedger::Postgres(postgres_ledger);

    let sqlite_response = recall_decisions(&sqlite_context, &sqlite_any, &memory_graph, request);
    let postgres_response = recall_decisions(&postgres_context, &postgres_any, &pg_graph, request);

    let _ = fs::remove_dir_all(&sqlite_dir);
    pg_graph.wipe()?;

    Ok(Some((sqlite_response?, postgres_response?)))
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

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
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
    for event in fixture_events() {
        ledger.append(event)?;
    }
    Ok(ledger)
}

/// Same event set as `fixture_ledger`, exposed directly for tests that need to
/// replay it into a real `SqliteEventLedger` or `PostgresEventLedger` rather
/// than an `InMemoryEventLedger`.
fn fixture_events() -> Vec<Event> {
    vec![
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
    ]
}

/// A project-scoped situational answer (hivemind-s15q.6): the same decisions in the same order,
/// with the same relations and scope note, on the Postgres graph as on the in-memory one.
#[test]
fn project_first_situational_matches_memory() -> Result<()> {
    with_postgres_graph("situational-project-first-parity", |pg| {
        let fixture = crate::queries::test_fixtures::project_first_fixture()?;
        let memory = MemoryGraph::default();
        project_from_ledger(&fixture.ledger, &memory, 0)?;
        project_from_ledger(&fixture.ledger, pg, 0)?;

        let context = crate::queries::QueryContext::local();
        let request = crate::queries::SituationalRequest {
            paths: vec!["pricing".to_owned()],
            limit: 50,
            project: Some("billing".to_owned()),
            ..Default::default()
        };
        let memory_answer = crate::queries::get_situational_decisions(
            &context,
            &memory,
            &fixture.ledger,
            &request,
        )?
        .data;
        let pg_answer =
            crate::queries::get_situational_decisions(&context, pg, &fixture.ledger, &request)?
                .data;

        let shape = |answer: &crate::queries::SituationalResults| -> Vec<(String, String)> {
            answer
                .matches
                .iter()
                .map(|m| {
                    (
                        m.decision.id.clone(),
                        m.scope
                            .as_ref()
                            .map(|scope| scope.label.clone())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        };
        if shape(&memory_answer) != shape(&pg_answer) {
            return Err(test_error(format!(
                "project-first situational mismatch: memory={:?} pg={:?}",
                shape(&memory_answer),
                shape(&pg_answer)
            )));
        }
        if memory_answer.scope != pg_answer.scope {
            return Err(test_error(format!(
                "scope note mismatch: memory={:?} pg={:?}",
                memory_answer.scope, pg_answer.scope
            )));
        }
        if memory_answer.matches.len() != 4 {
            return Err(test_error(format!(
                "billing's answer should hold its own, its parent's and auth's two decisions, got {}",
                memory_answer.matches.len()
            )));
        }
        Ok(())
    })
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

// ── grounding parity (hivemind-gwhr.3) ──────────────────────────────────────────
//
// What a decision rests on is read through edge provenance (`added_by`, `added_at`,
// `causation_event_id`) and single-node lookups, both of which need their own query support on
// this backend. Every read below must agree with the in-memory graph, and the fixture makes sure
// the interesting cases are actually present rather than agreeing on emptiness: a prior decision
// named at capture, one attributed later, evidence with a source, an open, overdue, held and
// failed bet, an assumption, and premises that are superseded, rejected and contested.

fn grounding_scenario() -> Result<crate::queries::test_fixtures::Scenario> {
    use crate::queries::test_fixtures::Scenario;

    let s = Scenario::new();
    s.decision(
        "d:goal",
        "Ship the hosted MVP",
        "human:alex",
        "2026-01-01T00:00:00Z",
    )?;
    s.accept("d:goal", "human:alex", "2026-01-01T00:00:01Z")?;
    s.evidence(
        "e:audit",
        "27 of 27 captures carry no premise",
        Some("mayor audit 2026-09-22"),
        "2026-01-01T00:00:02Z",
    )?;
    s.hypothesis(
        "h:open",
        "Bet open",
        "bet",
        Some("2026-12-01T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    s.hypothesis(
        "h:late",
        "Bet overdue",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    s.hypothesis(
        "h:held",
        "Bet held",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    s.hypothesis(
        "h:failed",
        "Bet failed",
        "bet",
        Some("2026-02-01T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    s.hypothesis(
        "h:assumed",
        "Load stays flat",
        "assumption",
        None,
        "2026-01-01T00:00:03Z",
    )?;

    // Named at capture: premise link, evidence, an open bet, an assumption through the chosen option.
    let proposal = s.decision_with(
        "d:derived",
        "Premises are asked at capture",
        "agent:claude:crew",
        "2026-01-02T00:00:00Z",
        true,
        &["e:audit"],
        &["h:open", "h:assumed"],
        Some("high"),
    )?;
    s.relation(
        "FOLLOWS_FROM",
        "d:derived",
        "d:goal",
        "agent:claude:crew",
        Some(proposal),
        "2026-01-02T00:00:01Z",
    )?;

    // Attributed later: a premise link and an assumption, by someone else.
    s.decision(
        "d:later",
        "Use Postgres",
        "agent:claude:crew",
        "2026-01-03T00:00:00Z",
    )?;
    s.relation(
        "FOLLOWS_FROM",
        "d:later",
        "d:goal",
        "human:alex",
        None,
        "2026-03-05T10:00:00Z",
    )?;
    s.relation(
        "ASSUMES",
        "d:later",
        "h:assumed",
        "human:bob",
        None,
        "2026-03-06T10:00:00Z",
    )?;

    // Bets that were checked, or should have been.
    for (decision, bet) in [
        ("d:late", "h:late"),
        ("d:held", "h:held"),
        ("d:failed", "h:failed"),
    ] {
        s.decision_with(
            decision,
            &format!("Decision on {bet}"),
            "human:alice",
            "2026-01-04T00:00:00Z",
            false,
            &[],
            &[bet],
            None,
        )?;
    }
    s.evidence("e:checked", "Checked it", None, "2026-03-01T00:00:00Z")?;
    s.relation(
        "SUPPORTS",
        "e:checked",
        "h:held",
        "agent:tester",
        None,
        "2026-03-01T00:00:01Z",
    )?;
    s.relation(
        "REFUTES",
        "e:checked",
        "h:failed",
        "agent:tester",
        None,
        "2026-03-01T00:00:02Z",
    )?;

    // A superseded premise, a rejected one and a contested one, each with a decision resting on it.
    s.decision(
        "d:premise-old",
        "Old goal",
        "human:alex",
        "2026-01-05T00:00:00Z",
    )?;
    s.decision(
        "d:premise-new",
        "New goal",
        "human:alex",
        "2026-02-01T00:00:00Z",
    )?;
    s.supersede(
        "d:premise-old",
        "d:premise-new",
        "human:alex",
        "2026-02-01T00:00:01Z",
    )?;
    s.decision(
        "d:premise-rejected",
        "Rejected goal",
        "human:alex",
        "2026-01-05T00:00:02Z",
    )?;
    s.reject("d:premise-rejected", "human:bob", "2026-01-06T00:00:00Z")?;
    s.decision(
        "d:premise-contested",
        "Contested goal",
        "human:alex",
        "2026-01-05T00:00:03Z",
    )?;
    s.accept("d:premise-contested", "human:alex", "2026-01-06T00:00:01Z")?;
    s.reject("d:premise-contested", "human:bob", "2026-01-06T00:00:02Z")?;
    for (decision, premise) in [
        ("d:stale-superseded", "d:premise-old"),
        ("d:stale-rejected", "d:premise-rejected"),
        ("d:not-stale-contested", "d:premise-contested"),
    ] {
        let proposal = s.decision(
            decision,
            &format!("Follows from {premise}"),
            "agent:claude:crew",
            "2026-02-02T00:00:00Z",
        )?;
        s.relation(
            "FOLLOWS_FROM",
            decision,
            premise,
            "agent:claude:crew",
            Some(proposal),
            "2026-02-02T00:00:01Z",
        )?;
    }
    Ok(s)
}

const GROUNDING_DECISION_IDS: [&str; 12] = [
    "d:goal",
    "d:derived",
    "d:later",
    "d:late",
    "d:held",
    "d:failed",
    "d:premise-old",
    "d:premise-rejected",
    "d:premise-contested",
    "d:stale-superseded",
    "d:stale-rejected",
    "d:not-stale-contested",
];

#[test]
fn grounding_reads_match_memory() -> Result<()> {
    with_postgres_graph("grounding-parity", |pg| {
        let scenario = grounding_scenario()?;
        let memory = scenario.graph()?;
        project_from_ledger(scenario.ledger(), pg, 0)?;
        let now = crate::queries::test_fixtures::ts("2026-06-01T00:00:00Z");

        for decision_id in GROUNDING_DECISION_IDS {
            let memory_grounding = grounding_of_at(&memory, decision_id, now)?;
            let pg_grounding = grounding_of_at(pg, decision_id, now)?;
            if memory_grounding != pg_grounding {
                return Err(test_error(format!(
                    "grounding_of_at mismatch for {decision_id}: memory={memory_grounding:?} pg={pg_grounding:?}"
                )));
            }
            let memory_brief = get_decision_brief_at(&memory, decision_id, now)?.data;
            let pg_brief = get_decision_brief_at(pg, decision_id, now)?.data;
            if memory_brief != pg_brief {
                return Err(test_error(format!(
                    "get_decision_brief_at mismatch for {decision_id}: memory={memory_brief:?} pg={pg_brief:?}"
                )));
            }
            let memory_outcome = get_decision_outcome_at(&memory, decision_id, now)?.data;
            let pg_outcome = get_decision_outcome_at(pg, decision_id, now)?.data;
            if memory_outcome != pg_outcome {
                return Err(test_error(format!(
                    "get_decision_outcome_at mismatch for {decision_id}: memory={memory_outcome:?} pg={pg_outcome:?}"
                )));
            }
            let memory_view = get_decision(&memory, decision_id)?.data;
            let pg_view = get_decision(pg, decision_id)?.data;
            if memory_view != pg_view {
                return Err(test_error(format!(
                    "get_decision mismatch for {decision_id}: memory={memory_view:?} pg={pg_view:?}"
                )));
            }
        }
        Ok(())
    })
}

#[test]
fn grounding_states_and_stale_premises_read_right_on_postgres() -> Result<()> {
    with_postgres_graph("grounding-states", |pg| {
        let scenario = grounding_scenario()?;
        project_from_ledger(scenario.ledger(), pg, 0)?;
        let now = crate::queries::test_fixtures::ts("2026-06-01T00:00:00Z");

        // Named at capture: four kinds of item, all at capture, and the premise counts a dependent.
        let derived = get_decision_brief_at(pg, "d:derived", now)?
            .data
            .ok_or_else(|| test_error("d:derived missing"))?;
        let kinds: Vec<GroundingKind> = derived.rests_on.iter().map(|item| item.kind).collect();
        if kinds
            != [
                GroundingKind::Decision,
                GroundingKind::Evidence,
                GroundingKind::Assumption,
                GroundingKind::Bet,
            ]
        {
            return Err(test_error(format!("unexpected kinds: {kinds:?}")));
        }
        if !derived
            .rests_on
            .iter()
            .all(|item| item.added == GroundingAdded::AtCapture)
        {
            return Err(test_error(format!(
                "expected all at capture: {:?}",
                derived.rests_on
            )));
        }
        if derived.expressed_confidence.as_deref() != Some("high") {
            return Err(test_error("expressed_confidence lost on postgres"));
        }
        let goal = get_decision_brief_at(pg, "d:goal", now)?
            .data
            .ok_or_else(|| test_error("d:goal missing"))?;
        if goal.dependents_count != 2 {
            return Err(test_error(format!(
                "d:goal dependents: {}",
                goal.dependents_count
            )));
        }

        // Attributed later: who and when survive the round trip through hm_edges.
        let later = grounding_of_at(pg, "d:later", now)?;
        let later_actors: Vec<Option<String>> = later
            .items
            .iter()
            .map(|item| match &item.added {
                GroundingAdded::Later { actor_id, .. } => actor_id.clone(),
                GroundingAdded::AtCapture => None,
            })
            .collect();
        if later_actors != [Some("human:alex".to_owned()), Some("human:bob".to_owned())] {
            return Err(test_error(format!(
                "unexpected later attribution: {later_actors:?}"
            )));
        }

        // Bets: overdue is reported, not stale; held is not overdue; failed is stale.
        let late = get_decision_outcome_at(pg, "d:late", now)?
            .data
            .ok_or_else(|| test_error("d:late missing"))?;
        if !late.held_up || late.unchecked.len() != 1 {
            return Err(test_error(format!("overdue bet outcome wrong: {late:?}")));
        }
        let held = get_decision_outcome_at(pg, "d:held", now)?
            .data
            .ok_or_else(|| test_error("d:held missing"))?;
        if !held.held_up || !held.unchecked.is_empty() {
            return Err(test_error(format!("held bet outcome wrong: {held:?}")));
        }
        let failed = get_decision_outcome_at(pg, "d:failed", now)?
            .data
            .ok_or_else(|| test_error("d:failed missing"))?;
        if failed.held_up || !failed.stale_premises {
            return Err(test_error(format!("failed bet outcome wrong: {failed:?}")));
        }

        // A premise that was superseded or rejected makes its dependent stale; a contested one
        // does not.
        let reasons_of = |id: &str| -> Result<Vec<crate::queries::OutcomeReason>> {
            Ok(get_decision_outcome_at(pg, id, now)?
                .data
                .ok_or_else(|| test_error(format!("{id} missing")))?
                .reasons)
        };
        if !reasons_of("d:stale-superseded")?.contains(
            &crate::queries::OutcomeReason::PremiseSuperseded {
                decision_id: "d:premise-old".to_owned(),
                by_id: "d:premise-new".to_owned(),
            },
        ) {
            return Err(test_error(
                "superseded premise did not make its dependent stale",
            ));
        }
        if !reasons_of("d:stale-rejected")?.contains(
            &crate::queries::OutcomeReason::PremiseRejected {
                decision_id: "d:premise-rejected".to_owned(),
            },
        ) {
            return Err(test_error(
                "rejected premise did not make its dependent stale",
            ));
        }
        let contested = get_decision_outcome_at(pg, "d:not-stale-contested", now)?
            .data
            .ok_or_else(|| test_error("d:not-stale-contested missing"))?;
        if !contested.held_up {
            return Err(test_error("a contested premise must not flip held_up"));
        }
        Ok(())
    })
}
