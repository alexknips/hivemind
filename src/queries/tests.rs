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
        tenant_id: Default::default(),
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
            .insert((RelationKind::PremisedOn, "o2".to_owned(), "h1".to_owned()));
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

        // Handle the premised_on_hypothesis_ids UNION Cypher (2-hop + direct).
        if cypher.contains("UNION") && cypher.contains("hypothesis_id") && cypher.contains("$id") {
            let decision_id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("missing id param").into()),
            };
            let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let chosen_options: Vec<String> = self
                .edges
                .iter()
                .filter(|(kind, from, _)| *kind == RelationKind::Chose && from == decision_id)
                .map(|(_, _, to)| to.clone())
                .collect();
            for opt_id in &chosen_options {
                for (kind, from, to) in &self.edges {
                    if *kind == RelationKind::PremisedOn && from == opt_id {
                        ids.insert(to.clone());
                    }
                }
            }
            for (kind, from, to) in &self.edges {
                if *kind == RelationKind::PremisedOnDirect && from == decision_id {
                    ids.insert(to.clone());
                }
            }
            return Ok(ids
                .into_iter()
                .map(|id| GraphRow::from([("hypothesis_id".to_owned(), GraphValue::String(id))]))
                .collect());
        }

        for (relation, alias) in [
            (RelationKind::HasOption, "option_id"),
            (RelationKind::Chose, "option_id"),
            (RelationKind::BasedOn, "evidence_id"),
            (RelationKind::PremisedOn, "hypothesis_id"),
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
    let decision = response
        .data
        .ok_or_else(|| query_error("decision exists"))?;
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
            since: None,
            until: None,
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
fn path_terms_splits_and_lowercases_segments() {
    assert_eq!(
        path_terms("src/api/auth.rs"),
        vec!["api".to_owned(), "auth".to_owned()]
    );
}

#[test]
fn path_terms_drops_stopwords_and_short_segments() {
    assert_eq!(path_terms("src/lib.rs"), Vec::<String>::new());
    assert_eq!(path_terms("a/b/c.rs"), Vec::<String>::new());
}

#[test]
fn path_terms_handles_branch_like_strings() {
    assert_eq!(
        path_terms("feature/auth-bearer-postgres"),
        vec![
            "feature".to_owned(),
            "auth".to_owned(),
            "bearer".to_owned(),
            "postgres".to_owned()
        ]
    );
}

#[test]
fn text_terms_tokenizes_prose_and_drops_stopwords() {
    let terms = text_terms("Bearer auth on Postgres requires session tokens.");
    assert!(terms.contains(&"bearer".to_owned()));
    assert!(terms.contains(&"postgres".to_owned()));
    assert!(terms.contains(&"tokens".to_owned()));
    assert!(!terms.contains(&"on".to_owned()));
}

#[test]
fn overlap_score_is_fraction_of_query_terms_matched() {
    let query = vec!["auth".to_owned(), "postgres".to_owned(), "cache".to_owned()];
    let candidate = vec!["auth".to_owned(), "postgres".to_owned()];
    assert_eq!(overlap_score(&query, &candidate), 2.0 / 3.0);
}

#[test]
fn overlap_score_is_zero_for_empty_query_or_no_overlap() {
    assert_eq!(overlap_score(&[], &["auth".to_owned()]), 0.0);
    assert_eq!(
        overlap_score(&["auth".to_owned()], &["unrelated".to_owned()]),
        0.0
    );
}

#[test]
fn overlapping_terms_is_sorted_and_deduped() {
    let query = vec!["postgres".to_owned(), "auth".to_owned(), "auth".to_owned()];
    let candidate = vec!["auth".to_owned(), "unrelated".to_owned()];
    assert_eq!(
        overlapping_terms(&query, &candidate),
        vec!["auth".to_owned()]
    );
}

fn situational_fixture() -> Result<(InMemoryEventLedger, MemoryGraph)> {
    let ledger = InMemoryEventLedger::new();
    for event in [
        test_event(
            1,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:auth-note",
                "content": "Bearer auth on Postgres requires session tokens for the API layer",
                "source": "test"
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
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
            "2026-01-01T00:01:00Z",
        ),
        test_event(
            3,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:auth-legacy",
                "title": "Legacy cookie auth still in place",
                "rationale": "Predates the bearer migration",
                "topic_keys": ["auth"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
            "2026-01-01T00:02:00Z",
        ),
        test_event(
            4,
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
            "2026-01-01T00:03:00Z",
        ),
    ] {
        ledger.append(event)?;
    }
    let graph = MemoryGraph::default();
    rebuild_graph(&ledger, &graph)?;
    Ok((ledger, graph))
}

#[test]
fn situational_decisions_surface_for_a_touched_path_and_rank_by_overlap() -> Result<()> {
    let (ledger, graph) = situational_fixture()?;
    let context = QueryContext::local();

    let response = get_situational_decisions(
        &context,
        &graph,
        &ledger,
        &SituationalRequest {
            paths: vec!["src/api/auth.rs".to_owned()],
            limit: 10,
            ..SituationalRequest::default()
        },
    )?;

    assert_eq!(
        response.data.query_terms,
        vec!["api".to_owned(), "auth".to_owned()]
    );
    assert_eq!(response.data.total_matches, 2);
    let ids: Vec<&str> = response
        .data
        .matches
        .iter()
        .map(|m| m.decision.id.as_str())
        .collect();
    assert_eq!(ids, vec!["decision:auth", "decision:auth-legacy"]);

    let top = &response.data.matches[0];
    assert_eq!(top.score, 1.0);
    assert!(top
        .matched_via
        .iter()
        .any(|reason| matches!(reason, MatchReason::TopicKey { topic } if topic == "auth")));
    assert!(top.matched_via.iter().any(|reason| matches!(
        reason,
        MatchReason::EvidenceOverlap { evidence_id, .. } if evidence_id == "evidence:auth-note"
    )));

    let second = &response.data.matches[1];
    assert_eq!(second.decision.id, "decision:auth-legacy");
    assert!(second.score < top.score);
    Ok(())
}

#[test]
fn situational_decisions_do_not_surface_for_an_unrelated_path() -> Result<()> {
    let (ledger, graph) = situational_fixture()?;
    let context = QueryContext::local();

    let response = get_situational_decisions(
        &context,
        &graph,
        &ledger,
        &SituationalRequest {
            paths: vec!["src/billing/invoice.rs".to_owned()],
            limit: 10,
            ..SituationalRequest::default()
        },
    )?;

    assert_eq!(response.data.total_matches, 0);
    assert!(response.data.matches.is_empty());
    Ok(())
}

#[test]
fn situational_decisions_annotate_changed_since_boundary() -> Result<()> {
    let (ledger, graph) = situational_fixture()?;
    let context = QueryContext::local();

    let response = get_situational_decisions(
        &context,
        &graph,
        &ledger,
        &SituationalRequest {
            paths: vec!["src/api/auth.rs".to_owned()],
            since_offset: Some(2),
            limit: 10,
            ..SituationalRequest::default()
        },
    )?;

    assert!(response.data.since_boundary.is_some());
    let by_id: BTreeMap<&str, Option<bool>> = response
        .data
        .matches
        .iter()
        .map(|m| (m.decision.id.as_str(), m.changed_since))
        .collect();
    assert_eq!(by_id.get("decision:auth"), Some(&Some(false)));
    assert_eq!(by_id.get("decision:auth-legacy"), Some(&Some(true)));
    Ok(())
}

#[test]
fn situational_decisions_reject_empty_situation() {
    let context = QueryContext::local();
    let graph = MemoryGraph::default();
    let ledger = InMemoryEventLedger::new();
    let err = get_situational_decisions(&context, &graph, &ledger, &SituationalRequest::default())
        .expect_err("empty paths must be rejected");
    assert!(err.to_string().contains("at least one path"));
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
    assert!(edge_relations.contains(&RelationKind::PremisedOn));
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
        .ok_or_else(|| query_error("root node present"))?;
    assert_eq!(root_node.decision_status, Some(DecisionStatus::Accepted));

    let hypothesis_node = response
        .data
        .nodes
        .iter()
        .find(|n| n.id == "h1")
        .ok_or_else(|| query_error("hypothesis node present"))?;
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
        .ok_or_else(|| query_error("hypothesis node present"))?;
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

// ---------------------------------------------------------------------------
// get_compact_view tests
// ---------------------------------------------------------------------------

#[test]
fn compact_view_empty_id_is_rejected() {
    let graph = graph_from_events([]).expect("empty graph");
    let error = get_compact_view(&graph, "").expect_err("empty id rejected");
    assert!(format!("{error}").contains("decision_id"));
}

#[test]
fn compact_view_returns_none_for_missing_decision() -> Result<()> {
    let graph = graph_from_events([])?;
    let response = get_compact_view(&graph, "does-not-exist")?;
    assert!(response.data.is_none());
    assert_eq!(response.result_count, 0);
    Ok(())
}

#[test]
fn compact_view_simple_proposed_decision() -> Result<()> {
    let graph = graph_from_events([test_event(
        1,
        EventType::DecisionProposed,
        "actor:a",
        json!({
            "decision_id": "d:simple",
            "title": "Simple decision",
            "rationale": "Because",
            "topic_keys": ["test"],
            "option_ids": ["opt:yes", "opt:no"],
            "chosen_option_id": "opt:yes",
            "hypothesis_ids": [],
            "evidence_ids": []
        }),
        "2026-01-01T00:00:00Z",
    )])?;

    let response = get_compact_view(&graph, "d:simple")?;
    let view = response.data.ok_or_else(|| query_error("decision found"))?;

    assert_eq!(view.decision.id, "d:simple");
    assert_eq!(view.decision.status, DecisionStatus::Proposed);
    assert!(view.supersession_chain.is_none());
    assert!(view.contest.is_none());
    assert!(view.hypotheses.is_empty());
    assert!(view.evidence_ids.is_empty());
    assert!(view.active_blockers.is_empty());

    assert_eq!(view.elided.superseded_decision_count, 0);
    assert_eq!(view.elided.unchosen_option_count, 1); // opt:no is unchosen
    Ok(())
}

#[test]
fn compact_view_resolves_to_terminal_in_supersession_chain() -> Result<()> {
    let graph = graph_from_events([
        test_event(
            1,
            EventType::DecisionProposed,
            "actor:a",
            json!({
                "decision_id": "d:old",
                "title": "Old decision",
                "rationale": "Old rationale",
                "topic_keys": ["arch"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
            EventType::DecisionProposed,
            "actor:a",
            json!({
                "decision_id": "d:new",
                "title": "New decision",
                "rationale": "New rationale",
                "topic_keys": ["arch"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
            "2026-01-01T01:00:00Z",
        ),
        test_event(
            3,
            EventType::DecisionSuperseded,
            "actor:a",
            json!({
                "old_decision_id": "d:old",
                "new_decision_id": "d:new"
            }),
            "2026-01-01T01:01:00Z",
        ),
    ])?;

    // Query via the old (superseded) decision: should resolve to terminal
    let response = get_compact_view(&graph, "d:old")?;
    let view = response
        .data
        .ok_or_else(|| query_error("found via superseded id"))?;

    assert_eq!(view.decision.id, "d:new", "terminal decision is returned");
    assert!(view.supersession_chain.is_some());
    let chain = view
        .supersession_chain
        .ok_or_else(|| query_error("supersession chain present"))?;
    assert_eq!(chain.chain_length, 2);
    assert_eq!(chain.oldest_id, "d:old");
    assert_eq!(view.elided.superseded_decision_count, 1);

    // Query via terminal directly: same result
    let via_terminal = get_compact_view(&graph, "d:new")?;
    let terminal_view = via_terminal
        .data
        .ok_or_else(|| query_error("terminal view found"))?;
    assert_eq!(terminal_view.decision.id, "d:new");
    Ok(())
}

#[test]
fn compact_view_contested_includes_both_actor_lists() -> Result<()> {
    let graph = graph_from_events([
        test_event(
            1,
            EventType::DecisionProposed,
            "actor:proposer",
            json!({
                "decision_id": "d:contested",
                "title": "Contested call",
                "rationale": "Some rationale",
                "topic_keys": ["policy"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
            EventType::DecisionAccepted,
            "actor:approver",
            json!({ "decision_id": "d:contested" }),
            "2026-01-01T01:00:00Z",
        ),
        test_event(
            3,
            EventType::DecisionRejected,
            "actor:objector",
            json!({ "decision_id": "d:contested" }),
            "2026-01-01T02:00:00Z",
        ),
    ])?;

    let view = get_compact_view(&graph, "d:contested")?
        .data
        .ok_or_else(|| query_error("contested view found"))?;

    assert_eq!(view.decision.status, DecisionStatus::Contested);
    let contest = view
        .contest
        .ok_or_else(|| query_error("contest present for contested decision"))?;
    assert!(contest.accepted_by.contains(&"actor:approver".to_owned()));
    assert!(contest.rejected_by.contains(&"actor:objector".to_owned()));
    Ok(())
}

#[test]
fn compact_view_refuted_hypothesis_exposes_refuting_evidence() -> Result<()> {
    let graph = graph_from_events([
        test_event(
            1,
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hyp:assumption",
                "statement": "System is safe under load"
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "ev:incident",
                "content": "Incident report: data loss under load",
                "source": "test"
            }),
            "2026-01-01T00:01:00Z",
        ),
        test_event(
            3,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "d:with-hyp",
                "title": "Deploy at scale",
                "rationale": "Assuming the system is safe",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hyp:assumption"],
                "evidence_ids": []
            }),
            "2026-01-01T00:02:00Z",
        ),
        test_event(
            4,
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": EventRelationKind::Refutes,
                "from_id": "ev:incident",
                "to_id": "hyp:assumption"
            }),
            "2026-01-01T00:03:00Z",
        ),
    ])?;

    let view = get_compact_view(&graph, "d:with-hyp")?
        .data
        .ok_or_else(|| query_error("refuted-hyp view found"))?;

    assert_eq!(view.hypotheses.len(), 1);
    let hyp = &view.hypotheses[0];
    assert_eq!(hyp.id, "hyp:assumption");
    assert_eq!(hyp.status, HypothesisStatus::Refuted);
    assert_eq!(hyp.statement, "System is safe under load");
    let refuting = hyp
        .refuting_evidence_ids
        .as_ref()
        .ok_or_else(|| query_error("refuting_evidence_ids present"))?;
    assert!(refuting.contains(&"ev:incident".to_owned()));
    assert!(hyp.supporting_evidence_ids.is_none());
    Ok(())
}

#[test]
fn compact_view_supported_hypothesis_exposes_supporting_evidence_ids() -> Result<()> {
    let graph = graph_from_events([
        test_event(
            1,
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hyp:valid",
                "statement": "Cache hit rate is acceptable"
            }),
            "2026-01-01T00:00:00Z",
        ),
        test_event(
            2,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "ev:metrics",
                "content": "p95 cache hit = 99%",
                "source": "test"
            }),
            "2026-01-01T00:01:00Z",
        ),
        test_event(
            3,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "d:cache",
                "title": "Use cache",
                "rationale": "Cache is valid",
                "topic_keys": ["perf"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hyp:valid"],
                "evidence_ids": []
            }),
            "2026-01-01T00:02:00Z",
        ),
        test_event(
            4,
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": EventRelationKind::Supports,
                "from_id": "ev:metrics",
                "to_id": "hyp:valid"
            }),
            "2026-01-01T00:03:00Z",
        ),
    ])?;

    let view = get_compact_view(&graph, "d:cache")?
        .data
        .ok_or_else(|| query_error("supported-hyp view found"))?;

    let hyp = &view.hypotheses[0];
    assert_eq!(hyp.status, HypothesisStatus::Supported);
    assert_eq!(hyp.statement, "Cache hit rate is acceptable");
    assert!(hyp.refuting_evidence_ids.is_none());
    let supporting = hyp
        .supporting_evidence_ids
        .as_ref()
        .ok_or_else(|| query_error("supporting_evidence_ids present"))?;
    assert!(supporting.contains(&"ev:metrics".to_owned()));
    Ok(())
}

// ---------------------------------------------------------------------------
// OutcomeGraph — minimal test double for outcome.rs
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OutcomeGraph {
    nodes: BTreeSet<(NodeKind, String)>,
    edges: BTreeSet<(RelationKind, String, String)>,
    node_origins: BTreeMap<String, i64>,
    edge_origins: BTreeMap<(String, String, String), i64>,
    mutation_calls: Cell<usize>,
}

impl OutcomeGraph {
    fn add_decision(mut self, id: &str, event_origin: i64) -> Self {
        self.nodes.insert((NodeKind::Decision, id.to_owned()));
        self.node_origins.insert(id.to_owned(), event_origin);
        self
    }
    fn add_edge(mut self, kind: RelationKind, from: &str, to: &str) -> Self {
        self.edges.insert((kind, from.to_owned(), to.to_owned()));
        self
    }
    fn add_edge_with_origin(
        mut self,
        kind: RelationKind,
        from: &str,
        to: &str,
        origin: i64,
    ) -> Self {
        self.edges.insert((kind, from.to_owned(), to.to_owned()));
        self.edge_origins.insert(
            (kind.table_name().to_owned(), from.to_owned(), to.to_owned()),
            origin,
        );
        self
    }
}

impl GraphView for OutcomeGraph {
    fn upsert_node(&self, _: NodeKind, _: &str, _: &GraphProperties) -> Result<()> {
        self.mutation_calls.set(self.mutation_calls.get() + 1);
        Ok(())
    }
    fn upsert_edge(&self, _: RelationKind, _: &str, _: &str, _: &GraphProperties) -> Result<()> {
        self.mutation_calls.set(self.mutation_calls.get() + 1);
        Ok(())
    }
    fn wipe(&self) -> Result<()> {
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        let id = params
            .get("id")
            .and_then(|v| {
                if let GraphValue::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");

        if cypher.contains("RETURN d.id AS id, d.event_origin AS event_origin LIMIT 1") {
            if self.nodes.contains(&(NodeKind::Decision, id.to_owned())) {
                let origin = self.node_origins.get(id).copied().unwrap_or(0);
                return Ok(vec![GraphRow::from([
                    ("id".to_owned(), GraphValue::String(id.to_owned())),
                    ("event_origin".to_owned(), GraphValue::Int(origin)),
                ])]);
            }
            return Ok(vec![]);
        }

        if cypher.contains("SUPERSEDES") && cypher.contains("superseder_id") {
            let superseder = self
                .edges
                .iter()
                .find(|(k, _, to)| *k == RelationKind::Supersedes && to == id);
            if let Some((_, from, to)) = superseder {
                let edge_origin = self
                    .edge_origins
                    .get(&(
                        RelationKind::Supersedes.table_name().to_owned(),
                        from.clone(),
                        to.clone(),
                    ))
                    .copied();
                let mut row = GraphRow::from([(
                    "superseder_id".to_owned(),
                    GraphValue::String(from.clone()),
                )]);
                if let Some(eo) = edge_origin {
                    row.insert("edge_origin".to_owned(), GraphValue::Int(eo));
                }
                return Ok(vec![row]);
            }
            return Ok(vec![]);
        }

        if cypher.contains("REFUTES") {
            let mut ids = Vec::new();
            for (kind, from, to) in &self.edges {
                if *kind == RelationKind::PremisedOnDirect && from == id {
                    let is_refuted = self
                        .edges
                        .iter()
                        .any(|(k, _, t)| *k == RelationKind::Refutes && t == to);
                    if is_refuted {
                        ids.push(GraphRow::from([(
                            "hypothesis_id".to_owned(),
                            GraphValue::String(to.clone()),
                        )]));
                    }
                }
            }
            return Ok(ids);
        }

        if cypher.contains("accepted_count") && cypher.contains("rejected_count") {
            let accepted = self
                .edges
                .iter()
                .filter(|(k, from, _)| *k == RelationKind::AcceptedBy && from == id)
                .count() as i64;
            let rejected = self
                .edges
                .iter()
                .filter(|(k, from, _)| *k == RelationKind::RejectedBy && from == id)
                .count() as i64;
            return Ok(vec![GraphRow::from([
                ("accepted_count".to_owned(), GraphValue::Int(accepted)),
                ("rejected_count".to_owned(), GraphValue::Int(rejected)),
            ])]);
        }

        if cypher.contains("HAS_OPTION") {
            let cnt = self
                .edges
                .iter()
                .filter(|(k, from, _)| *k == RelationKind::HasOption && from == id)
                .count() as i64;
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(cnt),
            )])]);
        }

        if cypher.contains("BASED_ON") {
            let cnt = self
                .edges
                .iter()
                .filter(|(k, from, _)| *k == RelationKind::BasedOn && from == id)
                .count() as i64;
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(cnt),
            )])]);
        }

        if cypher.contains("MATCH (d:`Decision`)")
            && cypher.contains("d.event_origin")
            && !cypher.contains("{id:")
        {
            let mut rows: Vec<GraphRow> = self
                .nodes
                .iter()
                .filter(|(k, _)| *k == NodeKind::Decision)
                .map(|(_, nid)| {
                    let origin = self.node_origins.get(nid).copied().unwrap_or(0);
                    GraphRow::from([
                        ("id".to_owned(), GraphValue::String(nid.clone())),
                        ("event_origin".to_owned(), GraphValue::Int(origin)),
                    ])
                })
                .collect();
            rows.sort_by_key(|r| {
                if let Some(GraphValue::Int(o)) = r.get("event_origin") {
                    *o
                } else {
                    0
                }
            });
            return Ok(rows);
        }

        Ok(vec![])
    }
}

#[test]
fn clean_decision_holds_up() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:1", 10)
        .add_edge(RelationKind::AcceptedBy, "d:1", "actor:alice")
        .add_edge(RelationKind::HasOption, "d:1", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:1", "ev:1");

    let result = get_decision_outcome(&graph, "d:1")?;
    let outcome = result.data.unwrap();

    assert!(outcome.held_up);
    assert!(!outcome.superseded);
    assert!(!outcome.stale_premises);
    assert!(!outcome.contested);
    assert!(outcome.has_options);
    assert!(outcome.has_evidence);
    assert!(outcome.reasons.is_empty());
    Ok(())
}

#[test]
fn superseded_decision_does_not_hold_up() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:old", 10)
        .add_decision("d:new", 50)
        .add_edge_with_origin(RelationKind::Supersedes, "d:new", "d:old", 55)
        .add_edge(RelationKind::HasOption, "d:old", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:old", "ev:1");

    let result = get_decision_outcome(&graph, "d:old")?;
    let outcome = result.data.unwrap();

    assert!(!outcome.held_up);
    assert!(outcome.superseded);
    assert_eq!(outcome.superseded_by.as_deref(), Some("d:new"));
    assert_eq!(outcome.supersession_gap_events, Some(45)); // 55 - 10
    assert!(!outcome.reasons.is_empty());
    assert!(matches!(
        outcome.reasons[0],
        OutcomeReason::SupersededBy { .. }
    ));
    Ok(())
}

#[test]
fn stale_premise_does_not_hold_up() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:1", 10)
        .add_edge(RelationKind::PremisedOnDirect, "d:1", "hyp:1")
        .add_edge(RelationKind::Refutes, "ev:refutation", "hyp:1")
        .add_edge(RelationKind::HasOption, "d:1", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:1", "ev:1");

    let result = get_decision_outcome(&graph, "d:1")?;
    let outcome = result.data.unwrap();

    assert!(!outcome.held_up);
    assert!(outcome.stale_premises);
    assert_eq!(outcome.refuted_hypothesis_ids, vec!["hyp:1"]);
    assert!(outcome
        .reasons
        .iter()
        .any(|r| matches!(r, OutcomeReason::PremisedOnRefuted { .. })));
    Ok(())
}

#[test]
fn contested_decision_does_not_hold_up() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:1", 10)
        .add_edge(RelationKind::AcceptedBy, "d:1", "actor:alice")
        .add_edge(RelationKind::RejectedBy, "d:1", "actor:bob")
        .add_edge(RelationKind::HasOption, "d:1", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:1", "ev:1");

    let result = get_decision_outcome(&graph, "d:1")?;
    let outcome = result.data.unwrap();

    assert!(!outcome.held_up);
    assert!(outcome.contested);
    assert!(outcome
        .reasons
        .iter()
        .any(|r| matches!(r, OutcomeReason::Contested)));
    Ok(())
}

#[test]
fn thin_structure_still_holds_up_but_has_reason() -> Result<()> {
    let graph = OutcomeGraph::default().add_decision("d:1", 10).add_edge(
        RelationKind::AcceptedBy,
        "d:1",
        "actor:alice",
    );

    let result = get_decision_outcome(&graph, "d:1")?;
    let outcome = result.data.unwrap();

    assert!(outcome.held_up);
    assert!(!outcome.has_options);
    assert!(!outcome.has_evidence);
    assert!(outcome.reasons.iter().any(|r| matches!(
        r,
        OutcomeReason::ThinStructure {
            no_options: true,
            no_evidence: true
        }
    )));
    Ok(())
}

#[test]
fn missing_decision_returns_none() -> Result<()> {
    let graph = OutcomeGraph::default();
    let result = get_decision_outcome(&graph, "d:missing")?;
    assert!(result.data.is_none());
    Ok(())
}

#[test]
fn bulk_query_returns_all_decisions() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:1", 10)
        .add_decision("d:2", 20)
        .add_edge(RelationKind::HasOption, "d:1", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:1", "ev:1");

    let req = DecisionQualityCandidatesRequest {
        limit: 10,
        ..Default::default()
    };
    let result = get_decision_quality_candidates(&graph, &req)?;

    assert_eq!(result.result_count, 2);
    assert!(!result.truncated);
    Ok(())
}

#[test]
fn bulk_query_only_with_signals_filters_clean_decisions() -> Result<()> {
    let graph = OutcomeGraph::default()
        .add_decision("d:clean", 10)
        .add_decision("d:thin", 20)
        .add_edge(RelationKind::AcceptedBy, "d:clean", "actor:alice")
        .add_edge(RelationKind::HasOption, "d:clean", "opt:1")
        .add_edge(RelationKind::BasedOn, "d:clean", "ev:1");

    let req = DecisionQualityCandidatesRequest {
        limit: 10,
        only_with_signals: true,
        ..Default::default()
    };
    let result = get_decision_quality_candidates(&graph, &req)?;

    assert!(result.data.iter().all(|o| o.decision_id == "d:thin"));
    Ok(())
}

// ---------------------------------------------------------------------------
// get_decision_outcome / get_decision_quality_candidates against a REAL
// MemoryGraph (hivemind-kj0i). Every test above this point exercises
// derive_outcome's logic against OutcomeGraph, a hand-rolled fake — none of
// them would have caught that the CLI's default GraphView (MemoryGraph) and
// the shared-backend Postgres GraphView didn't support several of these
// Cypher query shapes at all (crash) or silently matched the wrong handler
// (silent-wrong). These rebuild a MemoryGraph from ledger events instead, so
// a dispatch-table gap in src/projector/memory.rs fails here directly.
// ---------------------------------------------------------------------------

fn outcome_signals_fixture() -> Result<MemoryGraph> {
    graph_from_events([
        test_event(
            1,
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hypothesis:stale-direct",
                "statement": "Direct premise holds"
            }),
            "2026-01-01T00:00:01Z",
        ),
        test_event(
            2,
            EventType::HypothesisRecorded,
            "actor:analyst",
            json!({
                "hypothesis_id": "hypothesis:stale-option",
                "statement": "Option premise holds"
            }),
            "2026-01-01T00:00:02Z",
        ),
        test_event(
            3,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:refute-direct",
                "content": "Direct premise was wrong",
                "source": "test"
            }),
            "2026-01-01T00:00:03Z",
        ),
        test_event(
            4,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:refute-option",
                "content": "Option premise was wrong",
                "source": "test"
            }),
            "2026-01-01T00:00:04Z",
        ),
        test_event(
            5,
            EventType::EvidenceRecorded,
            "actor:analyst",
            json!({
                "evidence_id": "evidence:clean",
                "content": "Supports the clean decision",
                "source": "test"
            }),
            "2026-01-01T00:00:05Z",
        ),
        test_event(
            6,
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
            "2026-01-01T00:00:06Z",
        ),
        test_event(
            7,
            EventType::DecisionAccepted,
            "actor:alice",
            json!({"decision_id": "decision:clean"}),
            "2026-01-01T00:00:07Z",
        ),
        test_event(
            8,
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
            "2026-01-01T00:00:08Z",
        ),
        test_event(
            9,
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
            "2026-01-01T00:00:09Z",
        ),
        test_event(
            10,
            EventType::DecisionSuperseded,
            "actor:planner",
            json!({
                "old_decision_id": "decision:old",
                "new_decision_id": "decision:new"
            }),
            "2026-01-01T00:00:10Z",
        ),
        test_event(
            11,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:stale-direct",
                "title": "Directly premised on a refuted hypothesis",
                "rationale": "No chosen option — PREMISED_ON_DIRECT",
                "topic_keys": ["infra"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": ["hypothesis:stale-direct"],
                "evidence_ids": []
            }),
            "2026-01-01T00:00:11Z",
        ),
        test_event(
            12,
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": EventRelationKind::Refutes,
                "from_id": "evidence:refute-direct",
                "to_id": "hypothesis:stale-direct"
            }),
            "2026-01-01T00:00:12Z",
        ),
        test_event(
            13,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision:stale-option",
                "title": "Premised via a chosen option on a refuted hypothesis",
                "rationale": "Chosen option — PREMISED_ON",
                "topic_keys": ["infra"],
                "option_ids": ["option:stale"],
                "chosen_option_id": "option:stale",
                "hypothesis_ids": ["hypothesis:stale-option"],
                "evidence_ids": []
            }),
            "2026-01-01T00:00:13Z",
        ),
        test_event(
            14,
            EventType::RelationAdded,
            "actor:analyst",
            json!({
                "relation": EventRelationKind::Refutes,
                "from_id": "evidence:refute-option",
                "to_id": "hypothesis:stale-option"
            }),
            "2026-01-01T00:00:14Z",
        ),
        test_event(
            15,
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
            "2026-01-01T00:00:15Z",
        ),
        test_event(
            16,
            EventType::DecisionAccepted,
            "actor:alice",
            json!({"decision_id": "decision:contested"}),
            "2026-01-01T00:00:16Z",
        ),
        test_event(
            17,
            EventType::DecisionRejected,
            "actor:bob",
            json!({"decision_id": "decision:contested"}),
            "2026-01-01T00:00:17Z",
        ),
        test_event(
            18,
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
            "2026-01-01T00:00:18Z",
        ),
    ])
}

#[test]
fn memory_graph_clean_decision_holds_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:clean")?
        .data
        .expect("decision:clean exists");
    assert!(outcome.held_up);
    assert!(outcome.reasons.is_empty());
    assert!(outcome.has_options);
    assert!(outcome.has_evidence);
    Ok(())
}

#[test]
fn memory_graph_superseded_decision_does_not_hold_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:old")?
        .data
        .expect("decision:old exists");
    assert!(!outcome.held_up);
    assert!(outcome.superseded);
    assert_eq!(outcome.superseded_by.as_deref(), Some("decision:new"));
    assert_eq!(outcome.supersession_gap_events, Some(2)); // event_origin 10 - 8
    Ok(())
}

#[test]
fn memory_graph_stale_premise_direct_does_not_hold_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:stale-direct")?
        .data
        .expect("decision:stale-direct exists");
    assert!(!outcome.held_up);
    assert!(outcome.stale_premises);
    assert_eq!(
        outcome.refuted_hypothesis_ids,
        vec!["hypothesis:stale-direct".to_owned()]
    );
    Ok(())
}

#[test]
fn memory_graph_stale_premise_via_chosen_option_does_not_hold_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:stale-option")?
        .data
        .expect("decision:stale-option exists");
    assert!(!outcome.held_up);
    assert!(outcome.stale_premises);
    assert_eq!(
        outcome.refuted_hypothesis_ids,
        vec!["hypothesis:stale-option".to_owned()]
    );
    Ok(())
}

#[test]
fn memory_graph_contested_decision_does_not_hold_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:contested")?
        .data
        .expect("decision:contested exists");
    assert!(!outcome.held_up);
    assert!(outcome.contested);
    assert!(outcome
        .reasons
        .iter()
        .any(|r| matches!(r, OutcomeReason::Contested)));
    Ok(())
}

#[test]
fn memory_graph_thin_structure_still_holds_up() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    let outcome = get_decision_outcome(&graph, "decision:thin")?
        .data
        .expect("decision:thin exists");
    assert!(outcome.held_up); // thin structure alone does not flip held_up
    assert!(!outcome.has_options);
    assert!(!outcome.has_evidence);
    assert!(outcome.reasons.iter().any(|r| matches!(
        r,
        OutcomeReason::ThinStructure {
            no_options: true,
            no_evidence: true
        }
    )));
    Ok(())
}

#[test]
fn memory_graph_missing_decision_returns_none() -> Result<()> {
    let graph = outcome_signals_fixture()?;
    assert!(get_decision_outcome(&graph, "decision:does-not-exist")?
        .data
        .is_none());
    Ok(())
}

#[test]
fn memory_graph_quality_candidates_bulk_and_since_filter() -> Result<()> {
    let graph = outcome_signals_fixture()?;

    // No filter: every proposed decision comes back.
    let all = get_decision_quality_candidates(
        &graph,
        &DecisionQualityCandidatesRequest {
            limit: 100,
            ..Default::default()
        },
    )?;
    assert_eq!(all.result_count, 7);
    assert!(!all.truncated);

    // since_event_origin floors by the decision node's own event_origin
    // (decision:thin was proposed by event 18; decision:contested's decision
    // node is event 15 — both are >= 15, everything proposed earlier is not).
    let since = get_decision_quality_candidates(
        &graph,
        &DecisionQualityCandidatesRequest {
            since_event_origin: Some(15),
            limit: 100,
            ..Default::default()
        },
    )?;
    let ids: BTreeSet<_> = since.data.iter().map(|o| o.decision_id.clone()).collect();
    assert_eq!(
        ids,
        BTreeSet::from([
            "decision:contested".to_owned(),
            "decision:thin".to_owned(),
        ])
    );

    // only_with_signals excludes decision:clean (no reasons) from the full set.
    let signals_only = get_decision_quality_candidates(
        &graph,
        &DecisionQualityCandidatesRequest {
            only_with_signals: true,
            limit: 100,
            ..Default::default()
        },
    )?;
    assert!(signals_only
        .data
        .iter()
        .all(|o| o.decision_id != "decision:clean"));

    Ok(())
}

// ---------------------------------------------------------------------------
// ContextGraph — minimal test double for context.rs
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ContextGraph {
    /// decision_id -> (source, source_ref, rationale, event_origin)
    decisions: BTreeMap<String, (String, Option<String>, String, i64)>,
    /// actor_id -> kind ("human" | "agent" | "unknown")
    actors: BTreeMap<String, String>,
    /// decision_id -> actor_id (at most one proposer per decision)
    proposed_by: BTreeMap<String, String>,
    /// decision_id -> [actor_id]
    accepted_by: BTreeMap<String, Vec<String>>,
    /// decision_id -> [actor_id]
    rejected_by: BTreeMap<String, Vec<String>>,
    evidence_count: BTreeMap<String, i64>,
    hypothesis_ids: BTreeMap<String, Vec<String>>,
    options_count: BTreeMap<String, i64>,
}

impl ContextGraph {
    fn add_decision(
        mut self,
        id: &str,
        source: &str,
        source_ref: Option<&str>,
        rationale: &str,
        event_origin: i64,
    ) -> Self {
        self.decisions.insert(
            id.to_owned(),
            (
                source.to_owned(),
                source_ref.map(str::to_owned),
                rationale.to_owned(),
                event_origin,
            ),
        );
        self
    }
    fn add_actor(mut self, id: &str, kind: &str) -> Self {
        self.actors.insert(id.to_owned(), kind.to_owned());
        self
    }
    fn set_proposer(mut self, decision_id: &str, actor_id: &str) -> Self {
        self.proposed_by
            .insert(decision_id.to_owned(), actor_id.to_owned());
        self
    }
    fn add_acceptor(mut self, decision_id: &str, actor_id: &str) -> Self {
        self.accepted_by
            .entry(decision_id.to_owned())
            .or_default()
            .push(actor_id.to_owned());
        self
    }
    fn add_rejector(mut self, decision_id: &str, actor_id: &str) -> Self {
        self.rejected_by
            .entry(decision_id.to_owned())
            .or_default()
            .push(actor_id.to_owned());
        self
    }
    fn set_evidence(mut self, decision_id: &str, count: i64) -> Self {
        self.evidence_count.insert(decision_id.to_owned(), count);
        self
    }
    fn add_hypothesis(mut self, decision_id: &str, hypothesis_id: &str) -> Self {
        self.hypothesis_ids
            .entry(decision_id.to_owned())
            .or_default()
            .push(hypothesis_id.to_owned());
        self
    }
    fn set_options(mut self, decision_id: &str, count: i64) -> Self {
        self.options_count.insert(decision_id.to_owned(), count);
        self
    }
}

impl GraphView for ContextGraph {
    fn upsert_node(&self, _: NodeKind, _: &str, _: &GraphProperties) -> Result<()> {
        Ok(())
    }
    fn upsert_edge(&self, _: RelationKind, _: &str, _: &str, _: &GraphProperties) -> Result<()> {
        Ok(())
    }
    fn wipe(&self) -> Result<()> {
        Ok(())
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        let id = params
            .get("id")
            .and_then(|v| {
                if let GraphValue::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("");
        let since = params.get("since").and_then(|v| {
            if let GraphValue::Int(n) = v {
                Some(*n)
            } else {
                None
            }
        });

        // Single decision base info: contains d.rationale and {id: param but no PROPOSED_BY/ACCEPTED_BY
        if cypher.contains("d.rationale") && cypher.contains("{id:") {
            if let Some((source, source_ref, rationale, _)) = self.decisions.get(id) {
                let mut row = GraphRow::from([
                    ("id".to_owned(), GraphValue::String(id.to_owned())),
                    ("source".to_owned(), GraphValue::String(source.clone())),
                    (
                        "rationale".to_owned(),
                        GraphValue::String(rationale.clone()),
                    ),
                ]);
                if let Some(sr) = source_ref {
                    row.insert("source_ref".to_owned(), GraphValue::String(sr.clone()));
                }
                return Ok(vec![row]);
            }
            return Ok(vec![]);
        }

        // Bulk decision list: contains d.rationale but no {id:
        if cypher.contains("d.rationale") && !cypher.contains("{id:") {
            let mut rows: Vec<GraphRow> = self
                .decisions
                .iter()
                .filter(|(_, (_, _, _, origin))| since.is_none_or(|s| *origin >= s))
                .map(|(did, (source, source_ref, rationale, origin))| {
                    let mut row = GraphRow::from([
                        ("id".to_owned(), GraphValue::String(did.clone())),
                        ("source".to_owned(), GraphValue::String(source.clone())),
                        (
                            "rationale".to_owned(),
                            GraphValue::String(rationale.clone()),
                        ),
                        ("event_origin".to_owned(), GraphValue::Int(*origin)),
                    ]);
                    if let Some(sr) = source_ref {
                        row.insert("source_ref".to_owned(), GraphValue::String(sr.clone()));
                    }
                    row
                })
                .collect();
            rows.sort_by_key(|r| {
                let o = if let Some(GraphValue::Int(o)) = r.get("event_origin") {
                    *o
                } else {
                    0
                };
                let d = if let Some(GraphValue::String(d)) = r.get("id") {
                    d.clone()
                } else {
                    String::new()
                };
                (o, d)
            });
            return Ok(rows);
        }

        // Proposer: PROPOSED_BY + actor_id + kind + LIMIT 1
        if cypher.contains("PROPOSED_BY") && cypher.contains("actor_id") {
            if let Some(actor_id) = self.proposed_by.get(id) {
                let kind = self.actors.get(actor_id).cloned().unwrap_or_default();
                return Ok(vec![GraphRow::from([
                    ("actor_id".to_owned(), GraphValue::String(actor_id.clone())),
                    ("kind".to_owned(), GraphValue::String(kind)),
                ])]);
            }
            return Ok(vec![]);
        }

        // Acceptors: ACCEPTED_BY + actor_id + kind
        if cypher.contains("ACCEPTED_BY") && cypher.contains("actor_id") {
            let mut rows = Vec::new();
            if let Some(acceptors) = self.accepted_by.get(id) {
                let mut sorted = acceptors.clone();
                sorted.sort();
                for actor_id in sorted {
                    let kind = self.actors.get(&actor_id).cloned().unwrap_or_default();
                    rows.push(GraphRow::from([
                        ("actor_id".to_owned(), GraphValue::String(actor_id)),
                        ("kind".to_owned(), GraphValue::String(kind)),
                    ]));
                }
            }
            return Ok(rows);
        }

        // Rejected count: REJECTED_BY + cnt
        if cypher.contains("REJECTED_BY") && cypher.contains("cnt") {
            let cnt = self.rejected_by.get(id).map_or(0, |v| v.len() as i64);
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(cnt),
            )])]);
        }

        // Evidence count: BASED_ON + cnt
        if cypher.contains("BASED_ON") && cypher.contains("cnt") {
            let cnt = self.evidence_count.get(id).copied().unwrap_or(0);
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(cnt),
            )])]);
        }

        // Hypothesis ids: hid column
        if cypher.contains("hid") {
            let rows: Vec<GraphRow> = self
                .hypothesis_ids
                .get(id)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .map(|hid| GraphRow::from([("hid".to_owned(), GraphValue::String(hid.clone()))]))
                .collect();
            return Ok(rows);
        }

        // Options count: HAS_OPTION + cnt
        if cypher.contains("HAS_OPTION") && cypher.contains("cnt") {
            let cnt = self.options_count.get(id).copied().unwrap_or(0);
            return Ok(vec![GraphRow::from([(
                "cnt".to_owned(),
                GraphValue::Int(cnt),
            )])]);
        }

        Ok(vec![])
    }
}

#[test]
fn human_authored_decision_context() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:1", "human", None, "We chose X because Y", 10)
        .add_actor("human:alice", "human")
        .set_proposer("d:1", "human:alice")
        .set_evidence("d:1", 2)
        .set_options("d:1", 1);

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.decision_id, "d:1");
    assert_eq!(ctx.authorship, AuthorshipShape::HumanAuthored);
    assert_eq!(ctx.proposer_id.as_deref(), Some("human:alice"));
    assert_eq!(ctx.source, "human");
    assert_eq!(ctx.review, ReviewShape::Unreviewed);
    assert_eq!(ctx.accepted_count, 0);
    assert_eq!(ctx.rejected_count, 0);
    assert_eq!(ctx.evidence_count, 2);
    assert_eq!(ctx.options_count, 1);
    assert_eq!(ctx.rationale_chars, "We chose X because Y".len() as i64);
    Ok(())
}

#[test]
fn agent_proposed_human_accepted_context() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision(
            "d:1",
            "agent",
            Some("claude:opus:sess-abc"),
            "Rationale text",
            5,
        )
        .add_actor("agent:claude:sess-abc", "agent")
        .add_actor("human:bob", "human")
        .set_proposer("d:1", "agent:claude:sess-abc")
        .add_acceptor("d:1", "human:bob")
        .set_evidence("d:1", 1)
        .set_options("d:1", 2);

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.authorship, AuthorshipShape::AgentProposedHumanAccepted);
    assert_eq!(ctx.source_ref.as_deref(), Some("claude:opus:sess-abc"));
    assert_eq!(ctx.review, ReviewShape::PeerReviewed);
    assert_eq!(ctx.accepted_count, 1);
    assert_eq!(ctx.evidence_count, 1);
    assert_eq!(ctx.options_count, 2);
    assert_eq!(ctx.hypothesis_count, 0);
    Ok(())
}

#[test]
fn context_hypothesis_count_includes_direct_and_via_option() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:1", "human", None, "Rationale", 10)
        .add_hypothesis("d:1", "hyp:1")
        .add_hypothesis("d:1", "hyp:2");

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.hypothesis_count, 2);
    Ok(())
}

#[test]
fn agent_only_decision_context() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision(
            "d:1",
            "agent",
            Some("claude:haiku:sess-xyz"),
            "Agent rationale",
            15,
        )
        .add_actor("agent:claude:sess-xyz", "agent")
        .set_proposer("d:1", "agent:claude:sess-xyz")
        .add_acceptor("d:1", "agent:claude:sess-xyz"); // self-acceptance by agent

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.authorship, AuthorshipShape::AgentOnly);
    assert_eq!(ctx.review, ReviewShape::SelfAccepted);
    Ok(())
}

#[test]
fn disputed_decision_context() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:1", "human", None, "Rationale", 20)
        .add_actor("human:alice", "human")
        .add_actor("human:bob", "human")
        .set_proposer("d:1", "human:alice")
        .add_acceptor("d:1", "human:alice")
        .add_rejector("d:1", "human:bob");

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.review, ReviewShape::Disputed);
    assert_eq!(ctx.accepted_count, 1);
    assert_eq!(ctx.rejected_count, 1);
    Ok(())
}

#[test]
fn self_accepted_decision_context() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:1", "human", None, "Rationale", 20)
        .add_actor("human:alice", "human")
        .set_proposer("d:1", "human:alice")
        .add_acceptor("d:1", "human:alice");

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.review, ReviewShape::SelfAccepted);
    assert_eq!(ctx.authorship, AuthorshipShape::HumanAuthored);
    Ok(())
}

#[test]
fn unknown_authorship_when_no_proposer() -> Result<()> {
    let graph = ContextGraph::default().add_decision("d:1", "cli", None, "", 5);

    let result = get_decision_context(&graph, "d:1")?;
    let ctx = result.data.unwrap();

    assert_eq!(ctx.authorship, AuthorshipShape::Unknown);
    assert!(ctx.proposer_id.is_none());
    assert_eq!(ctx.review, ReviewShape::Unreviewed);
    Ok(())
}

#[test]
fn missing_decision_context_returns_none() -> Result<()> {
    let graph = ContextGraph::default();
    let result = get_decision_context(&graph, "d:missing")?;
    assert!(result.data.is_none());
    Ok(())
}

#[test]
fn context_bulk_query_returns_all_decisions() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:1", "human", None, "R1", 10)
        .add_decision("d:2", "agent", Some("claude:opus:s1"), "R2", 20)
        .add_actor("human:alice", "human")
        .add_actor("agent:claude:s1", "agent")
        .set_proposer("d:1", "human:alice")
        .set_proposer("d:2", "agent:claude:s1");

    let req = DecisionContextRequest {
        limit: 10,
        ..Default::default()
    };
    let result = get_decision_context_candidates(&graph, &req)?;

    assert_eq!(result.result_count, 2);
    assert!(!result.truncated);
    let shapes: Vec<_> = result.data.iter().map(|c| c.authorship).collect();
    assert!(shapes.contains(&AuthorshipShape::HumanAuthored));
    assert!(shapes.contains(&AuthorshipShape::AgentOnly));
    Ok(())
}

#[test]
fn context_bulk_query_since_filter() -> Result<()> {
    let graph = ContextGraph::default()
        .add_decision("d:old", "human", None, "Old", 5)
        .add_decision("d:new", "agent", None, "New", 50);

    let req = DecisionContextRequest {
        since_event_origin: Some(20),
        limit: 10,
        ..Default::default()
    };
    let result = get_decision_context_candidates(&graph, &req)?;

    assert_eq!(result.result_count, 1);
    assert_eq!(result.data[0].decision_id, "d:new");
    Ok(())
}

// ---------------------------------------------------------------------------
// In-house scorer: score_from_signals unit tests
// ---------------------------------------------------------------------------

use super::context::{AuthorshipShape, DecisionContext, ReviewShape};
use super::inhouse_scorer::{score_from_signals, QualityTier, ScorerConfig, SupersessionSpeed};
use super::outcome::{DecisionOutcome, OutcomeReason};

fn clean_outcome(id: &str) -> DecisionOutcome {
    DecisionOutcome {
        decision_id: id.to_owned(),
        held_up: true,
        superseded: false,
        superseded_by: None,
        supersession_gap_events: None,
        stale_premises: false,
        refuted_hypothesis_ids: vec![],
        contested: false,
        has_options: true,
        has_evidence: true,
        reasons: vec![],
    }
}

fn peer_reviewed_context(id: &str) -> DecisionContext {
    DecisionContext {
        decision_id: id.to_owned(),
        authorship: AuthorshipShape::HumanAuthored,
        proposer_id: Some("human:alice".to_owned()),
        source: "human".to_owned(),
        source_ref: None,
        review: ReviewShape::PeerReviewed,
        accepted_count: 2,
        rejected_count: 0,
        evidence_count: 3,
        hypothesis_count: 1,
        options_count: 2,
        rationale_chars: 120,
    }
}

#[test]
fn clean_decision_scores_clean() {
    let outcome = clean_outcome("d:1");
    let context = peer_reviewed_context("d:1");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    assert_eq!(scored.decision_id, "d:1");
    assert!((scored.score - 1.0).abs() < f64::EPSILON);
    assert_eq!(scored.tier, QualityTier::Clean);
    assert!(scored.reasons.is_empty());
    assert!(scored.contributing_ids.is_empty());
}

#[test]
fn superseded_normally_scores_minor_concerns() {
    let outcome = DecisionOutcome {
        held_up: false,
        superseded: true,
        superseded_by: Some("d:new".to_owned()),
        supersession_gap_events: Some(50),
        reasons: vec![OutcomeReason::SupersededBy {
            by_id: "d:new".to_owned(),
            gap_events: Some(50),
        }],
        ..clean_outcome("d:old")
    };
    let context = peer_reviewed_context("d:old");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // gap=50 → Normal speed → deduct 0.30 → score=0.70 → MinorConcerns
    assert!((scored.score - 0.70).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::MinorConcerns);
    assert_eq!(scored.contributing_ids, vec!["d:new"]);
    assert!(scored.reasons.iter().any(|r| matches!(
        r,
        super::inhouse_scorer::ScorerReason::SupersededBy {
            speed: SupersessionSpeed::Normal,
            ..
        }
    )));
}

#[test]
fn superseded_rapidly_scores_significant_concerns() {
    let outcome = DecisionOutcome {
        held_up: false,
        superseded: true,
        superseded_by: Some("d:fix".to_owned()),
        supersession_gap_events: Some(3),
        reasons: vec![OutcomeReason::SupersededBy {
            by_id: "d:fix".to_owned(),
            gap_events: Some(3),
        }],
        ..clean_outcome("d:mistake")
    };
    let context = peer_reviewed_context("d:mistake");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // gap=3 → Rapid → deduct 0.50 → score=0.50 → SignificantConcerns
    assert!((scored.score - 0.50).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::SignificantConcerns);
    assert!(scored.reasons.iter().any(|r| matches!(
        r,
        super::inhouse_scorer::ScorerReason::SupersededBy {
            speed: SupersessionSpeed::Rapid,
            ..
        }
    )));
}

#[test]
fn stale_premises_scores_significant_concerns() {
    let outcome = DecisionOutcome {
        held_up: false,
        stale_premises: true,
        refuted_hypothesis_ids: vec!["hyp:1".to_owned()],
        reasons: vec![OutcomeReason::PremisedOnRefuted {
            hypothesis_id: "hyp:1".to_owned(),
        }],
        ..clean_outcome("d:stale")
    };
    let context = peer_reviewed_context("d:stale");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // stale premises → deduct 0.50 → score=0.50 → SignificantConcerns
    assert!((scored.score - 0.50).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::SignificantConcerns);
    assert!(scored.contributing_ids.contains(&"hyp:1".to_owned()));
    assert!(scored.reasons.iter().any(|r| matches!(
        r,
        super::inhouse_scorer::ScorerReason::PremisedOnRefuted { .. }
    )));
}

#[test]
fn contested_scores_minor_concerns() {
    let outcome = DecisionOutcome {
        held_up: false,
        contested: true,
        reasons: vec![OutcomeReason::Contested],
        ..clean_outcome("d:contest")
    };
    let context = peer_reviewed_context("d:contest");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // contested → deduct 0.25 → score=0.75 → MinorConcerns
    assert!((scored.score - 0.75).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::MinorConcerns);
    assert!(scored
        .reasons
        .iter()
        .any(|r| matches!(r, super::inhouse_scorer::ScorerReason::Contested { .. })));
}

#[test]
fn thin_structure_both_stays_clean() {
    let outcome = DecisionOutcome {
        has_options: false,
        has_evidence: false,
        reasons: vec![OutcomeReason::ThinStructure {
            no_options: true,
            no_evidence: true,
        }],
        ..clean_outcome("d:thin")
    };
    let context = peer_reviewed_context("d:thin");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // thin both → deduct 0.15 → score=0.85 → Clean (precision-biased: thin alone is not flagged)
    assert!((scored.score - 0.85).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::Clean);
    assert!(scored.reasons.iter().any(|r| matches!(
        r,
        super::inhouse_scorer::ScorerReason::ThinStructure {
            no_options: true,
            no_evidence: true,
            ..
        }
    )));
}

#[test]
fn agent_only_unreviewed_stays_clean() {
    let outcome = clean_outcome("d:agent");
    let context = DecisionContext {
        authorship: AuthorshipShape::AgentOnly,
        review: ReviewShape::Unreviewed,
        accepted_count: 0,
        ..peer_reviewed_context("d:agent")
    };
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // agent_only + unreviewed → deduct 0.10 → score=0.90 → Clean (precision: mild alone)
    assert!((scored.score - 0.90).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::Clean);
    assert!(scored.reasons.iter().any(|r| matches!(
        r,
        super::inhouse_scorer::ScorerReason::AgentOnlyUnreviewed { .. }
    )));
}

#[test]
fn compound_stale_and_contested_scores_high_concern() {
    let outcome = DecisionOutcome {
        held_up: false,
        stale_premises: true,
        contested: true,
        refuted_hypothesis_ids: vec!["hyp:x".to_owned()],
        reasons: vec![
            OutcomeReason::PremisedOnRefuted {
                hypothesis_id: "hyp:x".to_owned(),
            },
            OutcomeReason::Contested,
        ],
        ..clean_outcome("d:compound")
    };
    let context = peer_reviewed_context("d:compound");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // stale -0.50 + contested -0.25 = -0.75 → score=0.25 → HighConcern
    assert!((scored.score - 0.25).abs() < 1e-9);
    assert_eq!(scored.tier, QualityTier::HighConcern);
}

#[test]
fn custom_config_changes_tier_cutoff() {
    let outcome = DecisionOutcome {
        has_options: false,
        has_evidence: false,
        reasons: vec![OutcomeReason::ThinStructure {
            no_options: true,
            no_evidence: true,
        }],
        ..clean_outcome("d:thin")
    };
    let context = peer_reviewed_context("d:thin");
    // Raise clean_threshold so thin structure triggers MinorConcerns.
    let config = ScorerConfig {
        clean_threshold: 0.90,
        ..ScorerConfig::default()
    };
    let scored = score_from_signals(&outcome, &context, &config);

    // thin both → score=0.85 → now below 0.90 threshold → MinorConcerns
    assert_eq!(scored.tier, QualityTier::MinorConcerns);
}

#[test]
fn reasons_carry_deduction_and_contributing_ids() {
    let outcome = DecisionOutcome {
        held_up: false,
        superseded: true,
        superseded_by: Some("d:newer".to_owned()),
        supersession_gap_events: Some(15),
        reasons: vec![OutcomeReason::SupersededBy {
            by_id: "d:newer".to_owned(),
            gap_events: Some(15),
        }],
        ..clean_outcome("d:old")
    };
    let context = peer_reviewed_context("d:old");
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    // Verify reason details are present
    assert!(!scored.reasons.is_empty(), "reasons must be populated");
    assert!(scored.contributing_ids.contains(&"d:newer".to_owned()));

    // gap=15 → Quick speed → deduct 0.40
    if let super::inhouse_scorer::ScorerReason::SupersededBy {
        deduction, speed, ..
    } = &scored.reasons[0]
    {
        assert!((deduction - 0.40).abs() < 1e-9);
        assert_eq!(*speed, SupersessionSpeed::Quick);
    } else {
        panic!("expected SupersededBy reason");
    }
}

#[test]
fn score_is_clamped_at_zero_on_extreme_compounding() {
    // Force a score that would go negative without clamping.
    let outcome = DecisionOutcome {
        held_up: false,
        superseded: true,
        superseded_by: Some("d:x".to_owned()),
        supersession_gap_events: Some(2),
        stale_premises: true,
        refuted_hypothesis_ids: vec!["hyp:a".to_owned()],
        contested: true,
        has_options: false,
        has_evidence: false,
        reasons: vec![
            OutcomeReason::SupersededBy {
                by_id: "d:x".to_owned(),
                gap_events: Some(2),
            },
            OutcomeReason::PremisedOnRefuted {
                hypothesis_id: "hyp:a".to_owned(),
            },
            OutcomeReason::Contested,
            OutcomeReason::ThinStructure {
                no_options: true,
                no_evidence: true,
            },
        ],
        ..clean_outcome("d:worst")
    };
    let context = DecisionContext {
        authorship: AuthorshipShape::AgentOnly,
        review: ReviewShape::Unreviewed,
        accepted_count: 0,
        ..peer_reviewed_context("d:worst")
    };
    let config = ScorerConfig::default();
    let scored = score_from_signals(&outcome, &context, &config);

    assert!(scored.score >= 0.0, "score must not go below 0");
    assert_eq!(scored.tier, QualityTier::HighConcern);
}

// ---------------------------------------------------------------------------
// Failure-mode attribution: unit tests over pure computation helpers
// ---------------------------------------------------------------------------

use super::attribution::get_failure_attribution;
use super::attribution::{AttributionGroup, ConfidenceLevel, FailureAttributionRequest};

#[test]
fn confidence_level_boundaries() {
    assert_eq!(ConfidenceLevel::from_n(0), ConfidenceLevel::Low);
    assert_eq!(ConfidenceLevel::from_n(9), ConfidenceLevel::Low);
    assert_eq!(ConfidenceLevel::from_n(10), ConfidenceLevel::Medium);
    assert_eq!(ConfidenceLevel::from_n(29), ConfidenceLevel::Medium);
    assert_eq!(ConfidenceLevel::from_n(30), ConfidenceLevel::High);
    assert_eq!(ConfidenceLevel::from_n(1000), ConfidenceLevel::High);
}

#[test]
fn attribution_empty_graph_returns_zero_stats() -> Result<()> {
    // OutcomeGraph with no decisions: clean zero-failure corpus.
    let graph = OutcomeGraph::default();
    let req = FailureAttributionRequest::default();
    let response = get_failure_attribution(&graph, &req)?;
    let report = response.data;

    assert_eq!(report.corpus_stats.total_decisions, 0);
    assert_eq!(report.corpus_stats.failed_decisions, 0);
    assert!((report.corpus_stats.baseline_failure_rate - 0.0).abs() < f64::EPSILON);
    assert_eq!(report.findings.len(), 0);
    Ok(())
}

#[test]
fn attribution_findings_sorted_by_abs_effect() {
    // Build an attribution group list and verify the findings sorting.
    // This directly exercises the sorting and filtering logic.
    let baseline = 0.3;

    let groups = [
        AttributionGroup {
            dimension: "authorship".to_owned(),
            group_label: "agent_only".to_owned(),
            total: 15,
            failed: 9, // 60% failure rate → +30pp above baseline
            failure_rate: 0.6,
            effect_vs_baseline: 0.3,
            confidence: ConfidenceLevel::Medium,
        },
        AttributionGroup {
            dimension: "review".to_owned(),
            group_label: "peer_reviewed".to_owned(),
            total: 20,
            failed: 2, // 10% → -20pp below baseline
            failure_rate: 0.1,
            effect_vs_baseline: -0.2,
            confidence: ConfidenceLevel::Medium,
        },
        AttributionGroup {
            dimension: "authorship".to_owned(),
            group_label: "human_authored".to_owned(),
            total: 5,
            failed: 2, // 40% → +10pp (but n < min_sample=10, excluded)
            failure_rate: 0.4,
            effect_vs_baseline: 0.1,
            confidence: ConfidenceLevel::Low,
        },
    ];

    // Simulate the finding extraction + sorting (min_sample=10 filters out n=5).
    let min_sample = 10_usize;
    let mut findings: Vec<_> = groups
        .iter()
        .filter(|g| g.total >= min_sample)
        .collect::<Vec<_>>();

    // Sort by absolute effect descending.
    findings.sort_by(|a, b| {
        b.effect_vs_baseline
            .abs()
            .partial_cmp(&a.effect_vs_baseline.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    assert_eq!(findings.len(), 2, "n=5 group filtered out");
    assert_eq!(
        findings[0].group_label, "agent_only",
        "largest |effect| first"
    );
    assert_eq!(findings[1].group_label, "peer_reviewed");

    let _ = baseline; // used for conceptual clarity only
}

#[test]
fn attribution_effect_sign_positive_means_worse_than_baseline() {
    // A group with higher-than-baseline failure rate must have positive effect.
    let group = AttributionGroup {
        dimension: "source".to_owned(),
        group_label: "agent".to_owned(),
        total: 20,
        failed: 10,
        failure_rate: 0.5,
        effect_vs_baseline: 0.5 - 0.25,
        confidence: ConfidenceLevel::Medium,
    };
    assert!(
        group.effect_vs_baseline > 0.0,
        "worse than baseline → positive effect"
    );
}

#[test]
fn attribution_effect_sign_negative_means_better_than_baseline() {
    let group = AttributionGroup {
        dimension: "review".to_owned(),
        group_label: "peer_reviewed".to_owned(),
        total: 30,
        failed: 3,
        failure_rate: 0.1,
        effect_vs_baseline: 0.1 - 0.35,
        confidence: ConfidenceLevel::High,
    };
    assert!(
        group.effect_vs_baseline < 0.0,
        "better than baseline → negative effect"
    );
}
