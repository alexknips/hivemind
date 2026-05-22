// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::cell::Cell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::events::{Event, EventSource, EventType, RelationKind as EventRelationKind};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{
    memory::MemoryGraph, rebuild_graph, GraphParams, GraphProperties, GraphRow, GraphValue,
    GraphView, NodeKind, RelationKind,
};
use crate::Result;

use super::*;

#[derive(Debug, Default)]
struct StatusGraph {
    edges: BTreeSet<(RelationKind, String, String)>,
    mutation_calls: Cell<usize>,
}

impl StatusGraph {
    fn with_edges(edges: &[(RelationKind, &str, &str)]) -> Self {
        Self {
            edges: edges
                .iter()
                .map(|(kind, from, to)| (*kind, (*from).to_owned(), (*to).to_owned()))
                .collect(),
            mutation_calls: Cell::new(0),
        }
    }

    fn mutation_calls(&self) -> usize {
        self.mutation_calls.get()
    }
}

impl GraphView for StatusGraph {
    fn upsert_node(&self, _kind: NodeKind, _id: &str, _properties: &GraphProperties) -> Result<()> {
        self.mutation_calls.set(self.mutation_calls.get() + 1);
        Ok(())
    }

    fn upsert_edge(
        &self,
        _kind: RelationKind,
        _from_id: &str,
        _to_id: &str,
        _properties: &GraphProperties,
    ) -> Result<()> {
        self.mutation_calls.set(self.mutation_calls.get() + 1);
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        let id = match params.get("id") {
            Some(GraphValue::String(id)) => id,
            _ => return Err(query_error("id param missing").into()),
        };
        let relation = relation_from_query(cypher)?;
        let incoming = cypher.contains("<-[rel:");
        let count = self
            .edges
            .iter()
            .filter(|(kind, from, to)| {
                *kind == relation && if incoming { to == id } else { from == id }
            })
            .count();
        let count = i64::try_from(count)
            .map_err(|error| query_error(format!("count overflow: {error}")))?;

        Ok(vec![GraphRow::from([(
            "count".to_owned(),
            GraphValue::Int(count),
        )])])
    }

    fn wipe(&self) -> Result<()> {
        self.mutation_calls.set(self.mutation_calls.get() + 1);
        Ok(())
    }
}

#[test]
fn derives_all_decision_status_cases() -> Result<()> {
    let cases = [
        (
            "proposed",
            StatusGraph::with_edges(&[(RelationKind::ProposedBy, "proposed", "actor:1")]),
            DecisionStatus::Proposed,
        ),
        (
            "accepted",
            StatusGraph::with_edges(&[(RelationKind::AcceptedBy, "accepted", "actor:1")]),
            DecisionStatus::Accepted,
        ),
        (
            "rejected",
            StatusGraph::with_edges(&[(RelationKind::RejectedBy, "rejected", "actor:1")]),
            DecisionStatus::Rejected,
        ),
        (
            "contested",
            StatusGraph::with_edges(&[
                (RelationKind::AcceptedBy, "contested", "actor:1"),
                (RelationKind::RejectedBy, "contested", "actor:2"),
            ]),
            DecisionStatus::Contested,
        ),
        (
            "superseded",
            StatusGraph::with_edges(&[
                (RelationKind::AcceptedBy, "superseded", "actor:1"),
                (RelationKind::RejectedBy, "superseded", "actor:2"),
                (RelationKind::Supersedes, "newer", "superseded"),
            ]),
            DecisionStatus::Superseded,
        ),
    ];

    for (decision_id, graph, expected) in cases {
        assert_eq!(derive_decision_status(&graph, decision_id)?, expected);
        assert_eq!(graph.mutation_calls(), 0);
    }

    Ok(())
}

#[test]
fn derives_all_hypothesis_status_cases() -> Result<()> {
    let cases = [
        ("open", StatusGraph::default(), HypothesisStatus::Open),
        (
            "supported",
            StatusGraph::with_edges(&[(RelationKind::Supports, "evidence:1", "supported")]),
            HypothesisStatus::Supported,
        ),
        (
            "refuted",
            StatusGraph::with_edges(&[
                (RelationKind::Supports, "evidence:1", "refuted"),
                (RelationKind::Refutes, "evidence:2", "refuted"),
            ]),
            HypothesisStatus::Refuted,
        ),
    ];

    for (hypothesis_id, graph, expected) in cases {
        assert_eq!(derive_hypothesis_status(&graph, hypothesis_id)?, expected);
        assert_eq!(graph.mutation_calls(), 0);
    }

    Ok(())
}

#[test]
fn active_decision_blockers_include_status_indicators_and_notification_state() -> Result<()> {
    let graph = graph_from_events([
        test_event(
            1,
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hypothesis:queue-safe",
                "statement": "Queue handoff is safe"
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
            EventType::EvidenceRecorded,
            "actor:auditor",
            json!({
                "evidence_id": "evidence:incident",
                "content": "Recent replay found data loss",
                "source": "test"
            }),
            "2026-01-01T00:01:00Z",
        ),
        test_event(
            3,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:queue",
                "title": "Choose queue behavior",
                "rationale": "Workers need a durable handoff",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hypothesis:queue-safe"],
                "evidence_ids": []
            }),
            "2026-01-01T00:02:00Z",
        ),
        test_event(
            4,
            EventType::RelationAdded,
            "actor:auditor",
            json!({
                "relation": EventRelationKind::Refutes,
                "from_id": "evidence:incident",
                "to_id": "hypothesis:queue-safe"
            }),
            "2026-01-01T00:03:00Z",
        ),
        test_event(
            5,
            EventType::BlockerReported,
            "agent:worker",
            json!({
                "blocker_id": "blocker:queue-owner",
                "blocked_actor_id": "agent:worker",
                "decision_id": "decision:queue",
                "topic_keys": ["infra"],
                "blocked_ref": "task:rollout",
                "blocked_ref_type": "task",
                "reason": "Need owner to choose rollback or queue hardening",
                "priority": "p1",
                "last_progress_at": "2026-01-01T00:00:00Z",
                "required_owner_id": "actor:owner"
            }),
            "2026-01-01T00:10:00Z",
        ),
        test_event(
            6,
            EventType::NotificationSent,
            "notification-worker",
            json!({
                "blocker_id": "blocker:queue-owner",
                "recipient_actor_id": "actor:owner",
                "channel": "direct",
                "threshold_rule": "p1_direct_15m",
                "source_event_ids": [5],
                "dedupe_key": "tenant:default|decision:decision:queue|state:active|blocked_actor:agent:worker|required_owner:actor:owner|priority:p1",
                "sent_at": "2026-01-01T00:30:00Z"
            }),
            "2026-01-01T00:30:00Z",
        ),
        test_event(
            7,
            EventType::BlockerReported,
            "agent:other",
            json!({
                "blocker_id": "blocker:resolved",
                "blocked_actor_id": "agent:other",
                "decision_id": null,
                "topic_keys": ["infra"],
                "blocked_ref": "task:old",
                "blocked_ref_type": "task",
                "reason": "Old blocker",
                "priority": "p2",
                "last_progress_at": "2026-01-01T00:00:00Z",
                "required_owner_id": "actor:owner"
            }),
            "2026-01-01T00:05:00Z",
        ),
        test_event(
            8,
            EventType::BlockerResolved,
            "actor:owner",
            json!({
                "blocker_id": "blocker:resolved",
                "resolution_event_id": 7,
                "resolution_reason": null
            }),
            "2026-01-01T00:06:00Z",
        ),
    ])?;

    let response = get_active_decision_blockers(
        &graph,
        &ActiveDecisionBlockersRequest {
            filters: DecisionBlockerFilters {
                now: Some(ts("2026-01-01T02:00:00Z")),
                stale_after_seconds: Some(60 * 60),
                ..DecisionBlockerFilters::default()
            },
            limit: 10,
            cursor: None,
        },
    )?;

    assert_eq!(response.result_count, 1);
    assert!(!response.truncated);
    let blocker = &response.data.items[0];
    assert_eq!(blocker.id, "blocker:queue-owner");
    assert_eq!(blocker.decision_status, Some(DecisionStatus::Proposed));
    assert!(blocker.stale);
    assert_eq!(
        blocker.refuted_assumption_ids,
        vec!["hypothesis:queue-safe".to_owned()]
    );
    assert_eq!(
        blocker.notification_state.state,
        BlockerNotificationStateKind::Sent
    );
    assert_eq!(
        blocker.notification_state.notification_id.as_deref(),
        Some("00000000-0000-0000-0000-000000000006")
    );
    assert_eq!(blocker.source_event_ids, vec![5]);

    Ok(())
}

#[test]
fn notification_candidates_use_thresholds_and_dedupe_keys() -> Result<()> {
    let graph = graph_from_events([test_event(
        1,
        EventType::BlockerReported,
        "agent:worker",
        json!({
            "blocker_id": "blocker:topic-only",
            "blocked_actor_id": "agent:worker",
            "decision_id": null,
            "topic_keys": ["billing", "rollout"],
            "blocked_ref": "issue:123",
            "blocked_ref_type": "issue",
            "reason": "Need human approval before continuing rollout",
            "priority": "p1",
            "last_progress_at": "2026-01-01T00:00:00Z",
            "required_owner_id": "actor:owner"
        }),
        "2026-01-01T00:00:00Z",
    )])?;

    let too_early = get_blocker_notification_candidates(
        &graph,
        &BlockerNotificationCandidatesRequest {
            now: ts("2026-01-01T00:14:59Z"),
            policy_version: "default-v1".to_owned(),
            limit: 10,
            cursor: None,
        },
    )?;
    assert_eq!(too_early.result_count, 0);

    let response = get_blocker_notification_candidates(
        &graph,
        &BlockerNotificationCandidatesRequest {
            now: ts("2026-01-01T00:15:00Z"),
            policy_version: "default-v1".to_owned(),
            limit: 10,
            cursor: None,
        },
    )?;

    assert_eq!(response.result_count, 1);
    let candidate = &response.data.items[0];
    assert_eq!(candidate.blocker_id, "blocker:topic-only");
    assert_eq!(candidate.channel, "direct");
    assert_eq!(candidate.threshold_rule, "p1_direct_15m");
    assert_eq!(
        candidate.dedupe_key,
        "tenant:default|topic:billing+rollout|state:active|blocked_actor:agent:worker|required_owner:actor:owner|priority:p1"
    );
    assert_eq!(candidate.source_event_ids, vec![1]);
    assert_eq!(
        candidate.notification_state.state,
        BlockerNotificationStateKind::None
    );

    Ok(())
}

fn graph_from_events(events: impl IntoIterator<Item = Event>) -> Result<MemoryGraph> {
    let ledger = InMemoryEventLedger::new();
    for event in events {
        ledger.append(event)?;
    }
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok(graph)
}

fn test_event(
    sequence: u128,
    event_type: EventType,
    actor_id: &str,
    payload: serde_json::Value,
    timestamp: &str,
) -> Event {
    Event {
        event_id: None,
        event_uuid: Uuid::from_u128(sequence),
        correlation_id: Some("blocker-query-test".to_owned()),
        causation_event_id: None,
        event_type,
        actor_id: actor_id.to_owned(),
        source: EventSource::Api,
        source_ref: Some("blocker-query-test".to_owned()),
        payload,
        ts: Some(ts(timestamp)),
    }
}

fn ts(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

fn relation_from_query(cypher: &str) -> Result<RelationKind> {
    for relation in RelationKind::ALL {
        if cypher.contains(&format!("`{}`", relation.table_name())) {
            return Ok(relation);
        }
    }
    Err(query_error(format!("unknown relation in query: {cypher}")).into())
}

#[derive(Debug, Default)]
struct FixtureGraph {
    decisions: BTreeMap<String, (String, String, Vec<String>)>,
    actors: BTreeSet<String>,
    evidence: BTreeMap<String, String>,
    options: BTreeMap<String, (String, String)>,
    hypotheses: BTreeMap<String, String>,
    edges: BTreeSet<(RelationKind, String, String)>,
}

impl FixtureGraph {
    fn sample() -> Self {
        let mut graph = Self::default();
        graph.decisions.insert(
            "d1".to_owned(),
            (
                "Pick queue".to_owned(),
                "Need reliability".to_owned(),
                vec!["infra".to_owned(), "latency".to_owned()],
            ),
        );
        graph.decisions.insert(
            "d2".to_owned(),
            (
                "Keep sync path".to_owned(),
                "Prefer simplicity".to_owned(),
                vec!["infra".to_owned()],
            ),
        );
        graph
            .hypotheses
            .insert("h1".to_owned(), "Queue improves p95".to_owned());
        graph
            .evidence
            .insert("e1".to_owned(), "Kuzu supports graph projection".to_owned());
        graph.options.insert(
            "o1".to_owned(),
            (
                "Synchronous path".to_owned(),
                "Keep request processing inline".to_owned(),
            ),
        );
        graph.options.insert(
            "o2".to_owned(),
            (
                "Async queue".to_owned(),
                "Use durable queued handoff".to_owned(),
            ),
        );
        graph.actors.extend(
            ["actor:1", "actor:2", "actor:3", "actor:4"]
                .into_iter()
                .map(str::to_owned),
        );

        graph.edges.insert((
            RelationKind::ProposedBy,
            "d1".to_owned(),
            "actor:1".to_owned(),
        ));
        graph.edges.insert((
            RelationKind::AcceptedBy,
            "d1".to_owned(),
            "actor:2".to_owned(),
        ));
        graph
            .edges
            .insert((RelationKind::HasOption, "d1".to_owned(), "o1".to_owned()));
        graph
            .edges
            .insert((RelationKind::HasOption, "d1".to_owned(), "o2".to_owned()));
        graph
            .edges
            .insert((RelationKind::Chose, "d1".to_owned(), "o2".to_owned()));
        graph
            .edges
            .insert((RelationKind::BasedOn, "d1".to_owned(), "e1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Assumes, "d1".to_owned(), "h1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Supports, "e1".to_owned(), "h1".to_owned()));

        graph.edges.insert((
            RelationKind::ProposedBy,
            "d2".to_owned(),
            "actor:3".to_owned(),
        ));
        graph.edges.insert((
            RelationKind::RejectedBy,
            "d2".to_owned(),
            "actor:4".to_owned(),
        ));
        graph
    }
}

impl GraphView for FixtureGraph {
    fn upsert_node(&self, _kind: NodeKind, _id: &str, _properties: &GraphProperties) -> Result<()> {
        Ok(())
    }

    fn upsert_edge(
        &self,
        _kind: RelationKind,
        _from_id: &str,
        _to_id: &str,
        _properties: &GraphProperties,
    ) -> Result<()> {
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        if cypher.contains("RETURN node.id AS id") {
            if cypher.contains("`Decision`") {
                return Ok(self
                    .decisions
                    .iter()
                    .map(|(id, (title, rationale, topics))| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("title".to_owned(), GraphValue::String(title.clone())),
                            (
                                "rationale".to_owned(),
                                GraphValue::String(rationale.clone()),
                            ),
                            (
                                "topic_keys".to_owned(),
                                GraphValue::StringList(topics.clone()),
                            ),
                            ("source".to_owned(), GraphValue::String("agent".to_owned())),
                        ])
                    })
                    .collect());
            }
            if cypher.contains("`Actor`") {
                return Ok(self
                    .actors
                    .iter()
                    .map(|id| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("source".to_owned(), GraphValue::String("agent".to_owned())),
                            ("source_ref".to_owned(), GraphValue::String(id.clone())),
                        ])
                    })
                    .collect());
            }
            if cypher.contains("`Evidence`") {
                return Ok(self
                    .evidence
                    .iter()
                    .map(|(id, content)| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("content".to_owned(), GraphValue::String(content.clone())),
                        ])
                    })
                    .collect());
            }
            if cypher.contains("`Option`") {
                return Ok(self
                    .options
                    .iter()
                    .map(|(id, (label, description))| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("label".to_owned(), GraphValue::String(label.clone())),
                            (
                                "description".to_owned(),
                                GraphValue::String(description.clone()),
                            ),
                        ])
                    })
                    .collect());
            }
            if cypher.contains("`Hypothesis`") {
                return Ok(self
                    .hypotheses
                    .iter()
                    .map(|(id, statement)| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            (
                                "statement".to_owned(),
                                GraphValue::String(statement.clone()),
                            ),
                        ])
                    })
                    .collect());
            }
        }

        if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
            let relation = relation_from_query(cypher)?;
            return Ok(self
                .edges
                .iter()
                .filter(|(kind, _, _)| *kind == relation)
                .map(|(_, from, to)| {
                    GraphRow::from([
                        ("from_id".to_owned(), GraphValue::String(from.clone())),
                        ("to_id".to_owned(), GraphValue::String(to.clone())),
                    ])
                })
                .collect());
        }

        if cypher.contains("RETURN count(rel) AS count;") {
            let relation = relation_from_query(cypher)?;
            let incoming = cypher.contains("<-[rel:");
            let id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            let count = self
                .edges
                .iter()
                .filter(|(kind, from, to)| {
                    *kind == relation && if incoming { to == id } else { from == id }
                })
                .count();
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(i64::try_from(count).unwrap_or(0)),
            )])]);
        }

        if cypher.contains("RETURN count(d) AS count;") {
            let topic = match params.get("topic") {
                Some(GraphValue::String(topic)) => topic,
                _ => return Err(query_error("missing topic param").into()),
            };
            let count = self
                .decisions
                .values()
                .filter(|(_, _, topics)| topics.iter().any(|value| value == topic))
                .count();
            return Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(i64::try_from(count).unwrap_or(0)),
            )])]);
        }

        if cypher.contains("MATCH (d:`Decision` {id: $id}) RETURN d.id AS id") {
            let id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            if let Some((title, rationale, topics)) = self.decisions.get(id) {
                return Ok(vec![GraphRow::from([
                    ("id".to_owned(), GraphValue::String(id.clone())),
                    ("title".to_owned(), GraphValue::String(title.clone())),
                    (
                        "rationale".to_owned(),
                        GraphValue::String(rationale.clone()),
                    ),
                    (
                        "topic_keys".to_owned(),
                        GraphValue::StringList(topics.clone()),
                    ),
                ])]);
            }
            return Ok(Vec::new());
        }

        if cypher.contains("WHERE $topic IN d.topic_keys") {
            let topic = match params.get("topic") {
                Some(GraphValue::String(topic)) => topic,
                _ => return Err(query_error("missing topic param").into()),
            };
            let mut rows = self
                .decisions
                .iter()
                .filter(|(_, (_, _, topics))| topics.iter().any(|value| value == topic))
                .map(|(id, (title, rationale, topics))| {
                    GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id.clone())),
                        ("title".to_owned(), GraphValue::String(title.clone())),
                        (
                            "rationale".to_owned(),
                            GraphValue::String(rationale.clone()),
                        ),
                        (
                            "topic_keys".to_owned(),
                            GraphValue::StringList(topics.clone()),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                let l = left.get("id");
                let r = right.get("id");
                format!("{l:?}").cmp(&format!("{r:?}"))
            });
            return Ok(rows);
        }

        for (relation, alias) in [
            (RelationKind::HasOption, "option_id"),
            (RelationKind::Chose, "option_id"),
            (RelationKind::BasedOn, "evidence_id"),
            (RelationKind::Assumes, "hypothesis_id"),
        ] {
            if cypher.contains(&format!("[:`{}`]", relation.table_name())) {
                let decision_id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let mut ids = self
                    .edges
                    .iter()
                    .filter(|(kind, from, _)| *kind == relation && from == decision_id)
                    .map(|(_, _, to)| to.clone())
                    .collect::<Vec<_>>();
                ids.sort();
                return Ok(ids
                    .into_iter()
                    .map(|value| GraphRow::from([(alias.to_owned(), GraphValue::String(value))]))
                    .collect());
            }
        }

        if cypher.contains("[r:`") && cypher.contains("AS event_origin") {
            let id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            let relation = relation_from_query(cypher)?;
            let incoming = cypher.contains("<-[r:`");
            let mut neighbors: Vec<String> = self
                .edges
                .iter()
                .filter(|(kind, from, to)| {
                    *kind == relation && if incoming { to == id } else { from == id }
                })
                .map(
                    |(_, from, to)| {
                        if incoming {
                            from.clone()
                        } else {
                            to.clone()
                        }
                    },
                )
                .collect();
            neighbors.sort();
            return Ok(neighbors
                .into_iter()
                .map(|other_id| {
                    GraphRow::from([
                        ("id".to_owned(), GraphValue::String(other_id)),
                        ("event_origin".to_owned(), GraphValue::Null),
                    ])
                })
                .collect());
        }

        if cypher.contains("[:`SUPERSEDES`]->(other:`Decision`)") {
            let decision_id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            let mut ids = self
                .edges
                .iter()
                .filter(|(kind, from, _)| *kind == RelationKind::Supersedes && from == decision_id)
                .map(|(_, _, to)| to.clone())
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                .collect());
        }

        if cypher.contains("(other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision`") {
            let decision_id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            let mut ids = self
                .edges
                .iter()
                .filter(|(kind, _, to)| *kind == RelationKind::Supersedes && to == decision_id)
                .map(|(_, from, _)| from.clone())
                .collect::<Vec<_>>();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                .collect());
        }

        Err(query_error(format!("unsupported query in fixture: {cypher}")).into())
    }

    fn wipe(&self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn get_decision_returns_neighbors_and_derived_status() -> Result<()> {
    let graph = FixtureGraph::sample();
    let response = get_decision(&graph, "d1")?;
    assert_eq!(response.result_count, 1);
    assert!(!response.truncated);
    let decision = response.data.expect("decision exists");
    assert_eq!(decision.id, "d1");
    assert_eq!(decision.status, DecisionStatus::Accepted);
    assert_eq!(decision.chosen_option_id.as_deref(), Some("o2"));
    assert_eq!(decision.option_ids, vec!["o1".to_owned(), "o2".to_owned()]);
    assert_eq!(decision.evidence_ids, vec!["e1".to_owned()]);
    assert_eq!(decision.hypotheses.len(), 1);
    assert_eq!(decision.hypotheses[0].id, "h1");
    assert_eq!(decision.hypotheses[0].status, HypothesisStatus::Supported);
    Ok(())
}

#[test]
fn get_relevant_decisions_filters_by_status() -> Result<()> {
    let graph = FixtureGraph::sample();
    let response = get_relevant_decisions(&graph, "infra", Some(DecisionStatus::Rejected))?;
    assert_eq!(response.result_count, 1);
    assert_eq!(response.data.len(), 1);
    assert_eq!(response.data[0].id, "d2");
    assert_eq!(response.data[0].status, DecisionStatus::Rejected);
    Ok(())
}

#[test]
fn search_decisions_matches_title_rationale_topic_status_and_actor() -> Result<()> {
    let graph = FixtureGraph::sample();
    let response = search_decisions(
        &graph,
        &SearchDecisionRequest {
            query: Some("queue".to_owned()),
            topic_keys: vec!["infra".to_owned()],
            statuses: vec![DecisionStatus::Accepted],
            actor_ids: vec!["actor:2".to_owned()],
            sources: vec!["agent".to_owned()],
            limit: 10,
            cursor: None,
        },
    )?;

    assert_eq!(response.result_count, 1);
    let result = &response.data.items[0];
    assert_eq!(result.decision.id, "d1");
    assert_eq!(result.decision.status, DecisionStatus::Accepted);
    assert_eq!(result.rank, 1);
    assert!(result.matched_fields.contains(&"decision.title".to_owned()));
    assert_eq!(result.graph_context.actor_ids, vec!["actor:1", "actor:2"]);

    let rationale_response = search_decisions(
        &graph,
        &SearchDecisionRequest {
            query: Some("simplicity".to_owned()),
            statuses: vec![DecisionStatus::Rejected],
            limit: 10,
            ..SearchDecisionRequest::default()
        },
    )?;
    assert_eq!(rationale_response.data.items[0].decision.id, "d2");
    assert!(rationale_response.data.items[0]
        .matched_fields
        .contains(&"decision.rationale".to_owned()));
    Ok(())
}

#[test]
fn search_decisions_matches_graph_context_text() -> Result<()> {
    let graph = FixtureGraph::sample();
    for (query, field, kind) in [
        ("Kuzu", "evidence.content", NodeKind::Evidence),
        ("p95", "hypothesis.statement", NodeKind::Hypothesis),
        ("async", "option.label", NodeKind::Option),
    ] {
        let response = search_decisions(
            &graph,
            &SearchDecisionRequest {
                query: Some(query.to_owned()),
                limit: 10,
                ..SearchDecisionRequest::default()
            },
        )?;

        assert_eq!(response.result_count, 1, "query {query}");
        let result = &response.data.items[0];
        assert_eq!(result.decision.id, "d1");
        assert_eq!(result.rank, 3);
        assert!(result.matched_fields.contains(&field.to_owned()));
        assert!(
            result
                .graph_context
                .matched_nodes
                .iter()
                .any(|node| node.kind == kind && node.field == field),
            "matched node missing for {field}"
        );
    }
    Ok(())
}

#[test]
fn search_decisions_paginates_in_deterministic_order() -> Result<()> {
    let graph = FixtureGraph::sample();
    let first = search_decisions(
        &graph,
        &SearchDecisionRequest {
            topic_keys: vec!["infra".to_owned()],
            limit: 1,
            ..SearchDecisionRequest::default()
        },
    )?;

    assert_eq!(first.result_count, 1);
    assert!(first.truncated);
    assert_eq!(first.data.total_matches, 2);
    assert_eq!(first.data.items[0].decision.id, "d1");
    assert_eq!(first.data.next_cursor.as_deref(), Some("1"));

    let second = search_decisions(
        &graph,
        &SearchDecisionRequest {
            topic_keys: vec!["infra".to_owned()],
            limit: 1,
            cursor: first.data.next_cursor,
            ..SearchDecisionRequest::default()
        },
    )?;

    assert_eq!(second.result_count, 1);
    assert!(!second.truncated);
    assert_eq!(second.data.items[0].decision.id, "d2");
    assert_eq!(second.data.next_cursor, None);
    Ok(())
}

#[test]
fn get_supersession_chain_walks_both_directions() -> Result<()> {
    let mut graph = FixtureGraph::sample();
    graph
        .edges
        .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
    graph
        .edges
        .insert((RelationKind::Supersedes, "d3".to_owned(), "d2".to_owned()));
    graph.decisions.insert(
        "d3".to_owned(),
        (
            "Newest".to_owned(),
            "latest".to_owned(),
            vec!["infra".to_owned()],
        ),
    );

    let chain = get_supersession_chain(&graph, "d2")?;
    assert_eq!(
        chain.data.decision_ids,
        vec!["d1".to_owned(), "d2".to_owned(), "d3".to_owned()]
    );
    assert_eq!(chain.data.input_index, 1);
    Ok(())
}

#[test]
fn get_decision_neighborhood_returns_full_one_hop() -> Result<()> {
    let graph = FixtureGraph::sample();
    let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

    assert!(response.data.root.present);
    assert_eq!(response.data.root.id, "d1");
    assert_eq!(response.data.root.kind, NodeKind::Decision);

    let edge_relations: Vec<RelationKind> = response
        .data
        .edges
        .iter()
        .map(|edge| edge.relation)
        .collect();
    assert!(edge_relations.contains(&RelationKind::ProposedBy));
    assert!(edge_relations.contains(&RelationKind::AcceptedBy));
    assert!(edge_relations.contains(&RelationKind::HasOption));
    assert!(edge_relations.contains(&RelationKind::Chose));
    assert!(edge_relations.contains(&RelationKind::BasedOn));
    assert!(edge_relations.contains(&RelationKind::Assumes));
    // SUPPORTS arrives via 2-hop from the visible hypothesis h1 <- e1
    assert!(edge_relations.contains(&RelationKind::Supports));

    let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
    for expected in ["d1", "actor:1", "actor:2", "o1", "o2", "e1", "h1"] {
        assert!(node_ids.contains(&expected), "missing node {expected}");
    }

    let root_node = response
        .data
        .nodes
        .iter()
        .find(|n| n.id == "d1")
        .expect("root node present");
    assert_eq!(root_node.decision_status, Some(DecisionStatus::Accepted));

    let hypothesis_node = response
        .data
        .nodes
        .iter()
        .find(|n| n.id == "h1")
        .expect("hypothesis node present");
    assert_eq!(
        hypothesis_node.hypothesis_status,
        Some(HypothesisStatus::Supported)
    );

    let mut sorted = response.data.edges.clone();
    sorted.sort_by(|a, b| {
        (a.relation, &a.from, &a.to, a.event_origin).cmp(&(
            b.relation,
            &b.from,
            &b.to,
            b.event_origin,
        ))
    });
    assert_eq!(sorted, response.data.edges, "edges must be deterministic");

    Ok(())
}

#[test]
fn get_decision_neighborhood_filters_by_relation() -> Result<()> {
    let graph = FixtureGraph::sample();
    let request = NeighborhoodRequest::with_relations([RelationKind::ProposedBy]);
    let response = get_decision_neighborhood(&graph, "d1", &request)?;

    for edge in &response.data.edges {
        assert_eq!(edge.relation, RelationKind::ProposedBy);
    }
    let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(node_ids.contains(&"actor:1"));
    assert!(!node_ids.contains(&"o1"), "options filtered out");
    assert!(!node_ids.contains(&"e1"), "evidence filtered out");
    Ok(())
}

#[test]
fn get_decision_neighborhood_handles_missing_decision() -> Result<()> {
    let graph = FixtureGraph::sample();
    let response =
        get_decision_neighborhood(&graph, "no-such-decision", &NeighborhoodRequest::all())?;

    assert!(!response.data.root.present);
    assert!(response.data.nodes.is_empty());
    assert!(response.data.edges.is_empty());
    assert_eq!(response.result_count, 0);
    Ok(())
}

#[test]
fn get_decision_neighborhood_reports_branched_supersession() -> Result<()> {
    let mut graph = FixtureGraph::sample();
    graph.decisions.insert(
        "branch_a".to_owned(),
        ("A".to_owned(), "rationale".to_owned(), Vec::new()),
    );
    graph.decisions.insert(
        "branch_b".to_owned(),
        ("B".to_owned(), "rationale".to_owned(), Vec::new()),
    );
    graph.edges.insert((
        RelationKind::Supersedes,
        "d1".to_owned(),
        "branch_a".to_owned(),
    ));
    graph.edges.insert((
        RelationKind::Supersedes,
        "d1".to_owned(),
        "branch_b".to_owned(),
    ));

    let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

    let supersedes_targets: Vec<&str> = response
        .data
        .edges
        .iter()
        .filter(|edge| edge.relation == RelationKind::Supersedes && edge.from == "d1")
        .map(|edge| edge.to.as_str())
        .collect();
    assert!(supersedes_targets.contains(&"branch_a"));
    assert!(supersedes_targets.contains(&"branch_b"));
    Ok(())
}

#[test]
fn get_decision_neighborhood_includes_refuting_evidence_via_hypothesis() -> Result<()> {
    let mut graph = FixtureGraph::sample();
    graph
        .edges
        .insert((RelationKind::Refutes, "e2".to_owned(), "h1".to_owned()));

    let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

    let refutes_edges: Vec<&NeighborEdge> = response
        .data
        .edges
        .iter()
        .filter(|edge| edge.relation == RelationKind::Refutes)
        .collect();
    assert_eq!(refutes_edges.len(), 1);
    assert_eq!(refutes_edges[0].from, "e2");
    assert_eq!(refutes_edges[0].to, "h1");

    let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(node_ids.contains(&"e2"), "refuting evidence reached via h1");

    let hypothesis_node = response
        .data
        .nodes
        .iter()
        .find(|n| n.id == "h1")
        .expect("hypothesis node present");
    assert_eq!(
        hypothesis_node.hypothesis_status,
        Some(HypothesisStatus::Refuted)
    );
    Ok(())
}

#[test]
fn get_decision_neighborhood_rejects_empty_id() {
    let graph = FixtureGraph::sample();
    let error = get_decision_neighborhood(&graph, "   ", &NeighborhoodRequest::all())
        .expect_err("empty id rejected");
    assert!(format!("{error}").contains("decision_id"));
}

#[test]
fn get_supersession_chain_detects_cycle() {
    let mut graph = FixtureGraph::sample();
    graph
        .edges
        .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
    graph
        .edges
        .insert((RelationKind::Supersedes, "d1".to_owned(), "d2".to_owned()));

    let error = get_supersession_chain(&graph, "d1").expect_err("cycle should fail");
    assert!(format!("{error}").contains("cycle detected"));
}
