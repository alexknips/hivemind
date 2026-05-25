use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use hivemind::events::{Event, EventSource, EventType, RelationKind};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use serde_json::json;
use uuid::Uuid;

pub type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SEED_BASE_UNIX_SECONDS: i64 = 1_767_225_600;

pub fn seed_to_dir(seed_dir: &Path) -> TestResult<()> {
    fs::create_dir_all(seed_dir)?;
    let ledger = SqliteEventLedger::open(seed_dir)?;

    for event in seed_events() {
        ledger.append(event)?;
    }

    Ok(())
}

pub fn seed_events() -> Vec<Event> {
    let mut builder = SeedBuilder::default();

    for (evidence_id, content) in [
        (
            "evidence-001",
            "Seed evidence 001: embedded projection replay drift packet",
        ),
        (
            "evidence-002",
            "Seed evidence 002: CLI automation output stayed stable",
        ),
        (
            "evidence-003",
            "Seed evidence 003: audit trail reconstructs provenance",
        ),
        (
            "evidence-004",
            "Seed evidence 004: packet-capture refutes projection freshness",
        ),
        (
            "evidence-005",
            "Seed evidence 005: empty result pages render without fallback rows",
        ),
        (
            "evidence-006",
            "Seed evidence 006: packet-capture shows delta mirror search coverage",
        ),
    ] {
        builder.evidence(evidence_id, content);
    }

    builder.hypothesis(
        "hypothesis-001",
        "The embedded graph projection remains deterministic after replay",
    );
    builder.hypothesis(
        "hypothesis-002",
        "CLI command output remains stable enough for automation",
    );
    builder.hypothesis(
        "hypothesis-003",
        "Decision provenance is sufficient for later audits",
    );
    builder.hypothesis(
        "hypothesis-004",
        "Hypothesis text needle keeps search coverage anchored",
    );

    let topics = ["architecture", "operations", "security", "product"];
    for index in 1..=30 {
        let topic = match topics.get((index - 1) % topics.len()) {
            Some(topic) => *topic,
            None => "architecture",
        };
        let hypotheses = match index {
            5 | 6 => vec!["hypothesis-001"],
            7..=10 => vec!["hypothesis-002"],
            11..=15 => vec!["hypothesis-003"],
            21 => vec!["hypothesis-004"],
            _ => Vec::new(),
        };
        let evidence = match index {
            5 | 6 => vec!["evidence-001"],
            7..=10 => vec!["evidence-002"],
            11..=15 => vec!["evidence-003"],
            20 => vec!["evidence-006"],
            21 => vec!["evidence-005"],
            _ => Vec::new(),
        };
        builder.decision(index, topic, &hypotheses, &evidence);
    }

    builder.accept("decision-003", "actor:reviewer");
    builder.accept("decision-004", "actor:alice");
    builder.reject("decision-004", "actor:bob");
    builder.supersede("decision-001", "decision-002");
    builder.supersede("decision-002", "decision-003");
    builder.supersede("decision-016", "decision-018");
    builder.supersede("decision-016", "decision-019");
    builder.relation(
        RelationKind::Supports,
        "evidence-001",
        "hypothesis-001",
        "actor:analyst",
    );
    builder.relation(
        RelationKind::Refutes,
        "evidence-004",
        "hypothesis-001",
        "actor:auditor",
    );
    builder.relation(
        RelationKind::Supports,
        "evidence-002",
        "hypothesis-002",
        "actor:analyst",
    );

    builder.events
}

pub fn canonical_ledger_export(seed_dir: &Path) -> TestResult<Vec<u8>> {
    let ledger = SqliteEventLedger::open(seed_dir)?;
    let events = ledger.read(0, 10_000)?;
    Ok(serde_json::to_vec_pretty(&events)?)
}

pub fn reset_seed_dir(seed_dir: &Path) -> TestResult<()> {
    if seed_dir.as_os_str().is_empty() || seed_dir == Path::new("/") {
        return Err("refusing to reset unsafe seed directory".into());
    }

    if seed_dir.exists() {
        return Err(format!(
            "seed directory already exists: {}; remove it before running the seed test",
            seed_dir.display()
        )
        .into());
    }

    Ok(())
}

pub fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-seed-{label}-{}", Uuid::new_v4()))
}

pub fn payload_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(|value| value.as_str())
}

#[derive(Default)]
struct SeedBuilder {
    events: Vec<Event>,
}

impl SeedBuilder {
    fn evidence(&mut self, evidence_id: &str, content: &str) {
        self.push(
            EventType::EvidenceRecorded,
            "actor:researcher",
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": "seed"
            }),
            None,
        );
    }

    fn hypothesis(&mut self, hypothesis_id: &str, statement: &str) {
        self.push(
            EventType::HypothesisRecorded,
            "actor:researcher",
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
            None,
        );
    }

    fn decision(
        &mut self,
        index: usize,
        topic: &str,
        hypothesis_ids: &[&str],
        evidence_ids: &[&str],
    ) {
        let decision_id = format!("decision-{index:03}");
        let (option_a, option_b) = match index {
            20 => (
                "option-020-manual-reconciliation".to_owned(),
                "option-020-delta-mirror".to_owned(),
            ),
            _ => (
                format!("option-{index:03}-a"),
                format!("option-{index:03}-b"),
            ),
        };
        self.push(
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": decision_id,
                "title": format!("Seed decision {index:03}"),
                "rationale": format!("Deterministic rationale for seed decision {index:03}"),
                "topic_keys": [topic, "slice-one"],
                "option_ids": [option_a, option_b],
                "chosen_option_id": option_b,
                "hypothesis_ids": hypothesis_ids,
                "evidence_ids": evidence_ids
            }),
            None,
        );
    }

    fn accept(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );
    }

    fn reject(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );
    }

    fn supersede(&mut self, old_decision_id: &str, new_decision_id: &str) {
        self.push(
            EventType::DecisionSuperseded,
            "actor:architect",
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id
            }),
            None,
        );
    }

    fn relation(&mut self, relation: RelationKind, from_id: &str, to_id: &str, actor_id: &str) {
        self.push(
            EventType::RelationAdded,
            actor_id,
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id
            }),
            None,
        );
    }

    fn push(
        &mut self,
        event_type: EventType,
        actor_id: &str,
        payload: serde_json::Value,
        causation_event_id: Option<u64>,
    ) {
        let sequence = self.events.len() + 1;
        self.events.push(Event {
            tenant_id: Default::default(),
            event_id: None,
            event_uuid: Uuid::from_u128(u128::try_from(sequence).unwrap_or(u128::MAX)),
            correlation_id: Some("seed-dataset-v1".to_owned()),
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some("seed-dataset-v1".to_owned()),
            payload,
            ts: Some(seed_timestamp(sequence)),
        });
    }
}

fn seed_timestamp(sequence: usize) -> DateTime<Utc> {
    let sequence_seconds = i64::try_from(sequence).unwrap_or(i64::MAX - SEED_BASE_UNIX_SECONDS);
    DateTime::from_timestamp(SEED_BASE_UNIX_SECONDS, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        + Duration::seconds(sequence_seconds)
}
