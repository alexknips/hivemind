use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{Event, EventId, EventProvenance, EventType, RelationKind};
use crate::ledger::EventLedger;
use crate::Result;

pub type DecisionId = String;
pub type EvidenceId = String;
pub type HypothesisId = String;
pub type OptionId = String;

pub const MAX_TOPIC_KEY_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct DecisionProposalEventUuids {
    pub proposal: Uuid,
    pub has_option: Vec<Uuid>,
    pub chose: Option<Uuid>,
    pub assumes: Vec<Uuid>,
    pub based_on: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionProposalEventIds {
    pub proposal_event_id: EventId,
    pub relation_event_ids: Vec<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeOutcome {
    pub new_decision_id: DecisionId,
    pub proposal_event_id: EventId,
    pub relation_event_ids: Vec<EventId>,
    pub superseded_event_id: EventId,
}

pub struct Commands<'a, L: EventLedger> {
    ledger: &'a L,
    provenance: EventProvenance,
    state: Mutex<CommandState>,
}

#[derive(Default)]
struct CommandState {
    option_ids: HashSet<OptionId>,
}

#[derive(Debug, Clone)]
struct DecisionProposalSnapshot {
    event_id: EventId,
    decision_id: DecisionId,
    actor_id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    option_ids: Vec<String>,
    chosen_option_id: Option<String>,
    hypothesis_ids: Vec<String>,
    evidence_ids: Vec<String>,
}

impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new(ledger: &'a L) -> Self {
        Self::new_with_provenance(ledger, EventProvenance::cli())
    }

    pub fn new_with_provenance(ledger: &'a L, provenance: EventProvenance) -> Self {
        Self {
            ledger,
            provenance,
            state: Mutex::new(CommandState::default()),
        }
    }

    pub fn record_evidence(&self, actor_id: &str, content: &str) -> Result<EvidenceId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("content", content)?;

        let evidence_id = generate_entity_id("evidence");
        self.record_evidence_with_id(actor_id, &evidence_id, content, None, Uuid::new_v4())?;
        Ok(evidence_id)
    }

    pub fn record_evidence_with_id(
        &self,
        actor_id: &str,
        evidence_id: &str,
        content: &str,
        source: Option<&str>,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("evidence_id", evidence_id)?;
        require_non_empty("content", content)?;
        require_optional_non_empty("source", source)?;

        let event = self.event_with_uuid(
            EventType::EvidenceRecorded,
            actor_id,
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": source
            }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    pub fn record_hypothesis(&self, actor_id: &str, statement: &str) -> Result<HypothesisId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("statement", statement)?;

        let hypothesis_id = generate_entity_id("hypothesis");
        self.record_hypothesis_with_id(actor_id, &hypothesis_id, statement, Uuid::new_v4())?;
        Ok(hypothesis_id)
    }

    pub fn record_hypothesis_with_id(
        &self,
        actor_id: &str,
        hypothesis_id: &str,
        statement: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;
        require_non_empty("statement", statement)?;

        let event = self.event_with_uuid(
            EventType::HypothesisRecorded,
            actor_id,
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    pub fn record_option(
        &self,
        actor_id: &str,
        label: &str,
        description: &str,
    ) -> Result<OptionId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("label", label)?;
        require_non_empty("description", description)?;

        let option_id = generate_option_id(label);
        self.record_option_with_id(actor_id, &option_id, label, description)?;
        Ok(option_id)
    }

    pub fn record_option_with_id(
        &self,
        actor_id: &str,
        option_id: &str,
        label: &str,
        description: &str,
    ) -> Result<OptionId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("option_id", option_id)?;
        require_non_empty("label", label)?;
        require_non_empty("description", description)?;

        let mut state = self.lock_state()?;
        state.option_ids.insert(option_id.to_owned());
        Ok(option_id.to_owned())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_decision(
        &self,
        actor_id: &str,
        title: &str,
        rationale: &str,
        topic_keys: &[String],
        option_ids: &[String],
        chosen_option_id: Option<&str>,
        hypothesis_ids: &[String],
        evidence_ids: &[String],
    ) -> Result<DecisionId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("title", title)?;
        require_non_empty("rationale", rationale)?;

        if option_ids.is_empty() {
            return Err(CommandError::Validation("option_ids must not be empty".to_owned()).into());
        }

        let normalized_topic_keys: Vec<String> = topic_keys
            .iter()
            .map(|topic| normalize_topic_key(topic))
            .filter(|topic| !topic.is_empty())
            .collect();

        if normalized_topic_keys.is_empty() {
            return Err(CommandError::Validation(
                "topic_keys must contain at least one non-empty normalized key".to_owned(),
            )
            .into());
        }

        {
            let state = self.lock_state()?;
            for option_id in option_ids {
                if !state.option_ids.contains(option_id) {
                    return Err(CommandError::Invariant(format!(
                        "option does not exist: {option_id}"
                    ))
                    .into());
                }
            }
        }

        if let Some(chosen_option_id) = chosen_option_id {
            let chosen_option_is_candidate = option_ids.iter().any(|option_id| {
                // ubs:ignore: option IDs are public decision graph IDs, not secrets.
                same_identifier(option_id, chosen_option_id)
            });
            if !chosen_option_is_candidate {
                return Err(CommandError::Validation(
                    "chosen_option_id must be one of option_ids".to_owned(),
                )
                .into());
            }
        }

        for hypothesis_id in hypothesis_ids {
            if !self.hypothesis_exists(hypothesis_id)? {
                return Err(CommandError::Invariant(format!(
                    "hypothesis does not exist: {hypothesis_id}"
                ))
                .into());
            }
        }

        for evidence_id in evidence_ids {
            if !self.evidence_exists(evidence_id)? {
                return Err(CommandError::Invariant(format!(
                    "evidence does not exist: {evidence_id}"
                ))
                .into());
            }
        }

        let event_uuids = DecisionProposalEventUuids {
            proposal: Uuid::new_v4(),
            has_option: repeat_uuid(option_ids.len()),
            chose: chosen_option_id.map(|_| Uuid::new_v4()),
            assumes: repeat_uuid(hypothesis_ids.len()),
            based_on: repeat_uuid(evidence_ids.len()),
        };
        let decision_id = generate_entity_id("decision");

        self.propose_decision_with_id(
            actor_id,
            &decision_id,
            title,
            rationale,
            topic_keys,
            option_ids,
            chosen_option_id,
            hypothesis_ids,
            evidence_ids,
            event_uuids,
        )?;

        Ok(decision_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_decision_with_id(
        &self,
        actor_id: &str,
        decision_id: &str,
        title: &str,
        rationale: &str,
        topic_keys: &[String],
        option_ids: &[String],
        chosen_option_id: Option<&str>,
        hypothesis_ids: &[String],
        evidence_ids: &[String],
        event_uuids: DecisionProposalEventUuids,
    ) -> Result<DecisionProposalEventIds> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("title", title)?;
        require_non_empty("rationale", rationale)?;

        if option_ids.is_empty() {
            return Err(CommandError::Validation("option_ids must not be empty".to_owned()).into());
        }

        if event_uuids.has_option.len() != option_ids.len() {
            return Err(CommandError::Validation(
                "has_option event UUID count must match option_ids".to_owned(),
            )
            .into());
        }

        if event_uuids.assumes.len() != hypothesis_ids.len() {
            return Err(CommandError::Validation(
                "assumes event UUID count must match hypothesis_ids".to_owned(),
            )
            .into());
        }

        if event_uuids.based_on.len() != evidence_ids.len() {
            return Err(CommandError::Validation(
                "based_on event UUID count must match evidence_ids".to_owned(),
            )
            .into());
        }

        if chosen_option_id.is_some() != event_uuids.chose.is_some() {
            return Err(CommandError::Validation(
                "chose event UUID must be present exactly when chosen_option_id is present"
                    .to_owned(),
            )
            .into());
        }

        let normalized_topic_keys: Vec<String> = topic_keys
            .iter()
            .map(|topic| normalize_topic_key(topic))
            .filter(|topic| !topic.is_empty())
            .collect();

        if normalized_topic_keys.is_empty() {
            return Err(CommandError::Validation(
                "topic_keys must contain at least one non-empty normalized key".to_owned(),
            )
            .into());
        }

        {
            let state = self.lock_state()?;
            for option_id in option_ids {
                if !state.option_ids.contains(option_id) {
                    return Err(CommandError::Invariant(format!(
                        "option does not exist: {option_id}"
                    ))
                    .into());
                }
            }
        }

        if let Some(chosen_option_id) = chosen_option_id {
            let chosen_option_is_candidate = option_ids.iter().any(|option_id| {
                // ubs:ignore: option IDs are public decision graph IDs, not secrets.
                same_identifier(option_id, chosen_option_id)
            });
            if !chosen_option_is_candidate {
                return Err(CommandError::Validation(
                    "chosen_option_id must be one of option_ids".to_owned(),
                )
                .into());
            }
        }

        for hypothesis_id in hypothesis_ids {
            if !self.hypothesis_exists(hypothesis_id)? {
                return Err(CommandError::Invariant(format!(
                    "hypothesis does not exist: {hypothesis_id}"
                ))
                .into());
            }
        }

        for evidence_id in evidence_ids {
            if !self.evidence_exists(evidence_id)? {
                return Err(CommandError::Invariant(format!(
                    "evidence does not exist: {evidence_id}"
                ))
                .into());
            }
        }

        let root_event = self.event_with_uuid(
            EventType::DecisionProposed,
            actor_id,
            json!({
                "decision_id": decision_id,
                "title": title,
                "rationale": rationale,
                "topic_keys": normalized_topic_keys,
                "option_ids": option_ids,
                "chosen_option_id": chosen_option_id,
                "hypothesis_ids": hypothesis_ids,
                "evidence_ids": evidence_ids,
            }),
            None,
            event_uuids.proposal,
        );

        let root_event_id = self.ledger.append(root_event)?;
        let mut relation_event_ids = Vec::new();

        for (option_id, event_uuid) in option_ids.iter().zip(event_uuids.has_option) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                actor_id,
                root_event_id,
                RelationKind::HasOption,
                decision_id,
                option_id,
                event_uuid,
            )?);
        }

        if let (Some(chosen_option_id), Some(event_uuid)) = (chosen_option_id, event_uuids.chose) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                actor_id,
                root_event_id,
                RelationKind::Chose,
                decision_id,
                chosen_option_id,
                event_uuid,
            )?);
        }

        for (hypothesis_id, event_uuid) in hypothesis_ids.iter().zip(event_uuids.assumes) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                actor_id,
                root_event_id,
                RelationKind::Assumes,
                decision_id,
                hypothesis_id,
                event_uuid,
            )?);
        }

        for (evidence_id, event_uuid) in evidence_ids.iter().zip(event_uuids.based_on) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                actor_id,
                root_event_id,
                RelationKind::BasedOn,
                decision_id,
                evidence_id,
                event_uuid,
            )?);
        }

        Ok(DecisionProposalEventIds {
            proposal_event_id: root_event_id,
            relation_event_ids,
        })
    }

    pub fn accept_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        self.accept_decision_with_uuid(decision_id, actor_id, Uuid::new_v4())
    }

    pub fn accept_decision_with_uuid(
        &self,
        decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionRejected)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    pub fn reject_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        self.reject_decision_with_uuid(decision_id, actor_id, Uuid::new_v4())
    }

    pub fn disagree(&self, actor_id: &str, decision_id: &str, reason: &str) -> Result<EventId> {
        self.disagree_with_uuid(actor_id, decision_id, reason, Uuid::new_v4())
    }

    pub fn disagree_with_uuid(
        &self,
        actor_id: &str,
        decision_id: &str,
        reason: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("reason", reason)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if let Some(existing_event_id) =
            self.find_decision_event_id(decision_id, actor_id, EventType::DecisionRejected)?
        {
            return Ok(existing_event_id);
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionAccepted)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            EventType::DecisionRejected,
            actor_id,
            json!({
                "decision_id": decision_id,
                "reason": reason,
            }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    pub fn reject_decision_with_uuid(
        &self,
        decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionAccepted)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    pub fn supersede_decision(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        self.supersede_decision_with_uuid(
            old_decision_id,
            new_decision_id,
            actor_id,
            Uuid::new_v4(),
        )
    }

    pub fn supersede_decision_with_uuid(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("old_decision_id", old_decision_id)?;
        require_non_empty("new_decision_id", new_decision_id)?;

        if same_identifier(old_decision_id, new_decision_id) {
            return Err(CommandError::Validation(
                "old_decision_id and new_decision_id must be different".to_owned(),
            )
            .into());
        }

        if !self.decision_exists(old_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {old_decision_id}"
            ))
            .into());
        }

        if !self.decision_exists(new_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {new_decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            EventType::DecisionSuperseded,
            actor_id,
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id,
            }),
            None,
            event_uuid,
        );

        self.ledger.append(event)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn supersede(
        &self,
        actor_id: &str,
        old_decision_id: &str,
        new_title: &str,
        new_rationale: &str,
        topic_keys: &[String],
        option_labels: &[String],
        chosen_option_label: Option<&str>,
        hypothesis_ids: &[String],
        evidence_ids: &[String],
    ) -> Result<SupersedeOutcome> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("old_decision_id", old_decision_id)?;
        require_non_empty("new_title", new_title)?;
        require_non_empty("new_rationale", new_rationale)?;
        require_optional_non_empty("chosen_option_label", chosen_option_label)?;

        let old_decision = self
            .decision_proposal_snapshot(old_decision_id)?
            .ok_or_else(|| {
                CommandError::Invariant(format!("decision does not exist: {old_decision_id}"))
            })?;
        let effective_topic_keys =
            effective_topic_keys(topic_keys, old_decision.topic_keys.as_slice())?;
        let option_labels = effective_option_labels(new_title, option_labels, chosen_option_label)?;
        let option_ids = deterministic_supersede_option_ids(
            actor_id,
            old_decision_id,
            new_title,
            new_rationale,
            &option_labels,
        );
        let chosen_label = chosen_option_label.map(str::trim);
        let chosen_option_id = chosen_label
            .map(|label| {
                option_labels
                    .iter()
                    .position(|option_label| option_label == label)
                    .map(|index| option_ids[index].clone())
                    .ok_or_else(|| {
                        CommandError::Validation(
                            "chosen_option_label must be one of option_labels".to_owned(),
                        )
                    })
            })
            .transpose()?;

        if let Some(existing) = self.find_matching_supersede(
            actor_id,
            old_decision_id,
            new_title,
            new_rationale,
            &effective_topic_keys,
            &option_ids,
            chosen_option_id.as_deref(),
            hypothesis_ids,
            evidence_ids,
        )? {
            return Ok(existing);
        }

        for (option_label, option_id) in option_labels.iter().zip(&option_ids) {
            let mut option_description = String::with_capacity(
                "Option generated from supersede value ''".len() + option_label.len(),
            );
            let _ = write!(
                option_description,
                "Option generated from supersede value '{option_label}'"
            );
            self.record_option_with_id(actor_id, option_id, option_label, &option_description)?;
        }

        let new_decision_id = generate_entity_id("decision");
        let proposal_event_ids = self.propose_decision_with_id(
            actor_id,
            &new_decision_id,
            new_title,
            new_rationale,
            &effective_topic_keys,
            &option_ids,
            chosen_option_id.as_deref(),
            hypothesis_ids,
            evidence_ids,
            DecisionProposalEventUuids {
                proposal: Uuid::new_v4(),
                has_option: repeat_uuid(option_ids.len()),
                chose: chosen_option_id.as_ref().map(|_| Uuid::new_v4()),
                assumes: repeat_uuid(hypothesis_ids.len()),
                based_on: repeat_uuid(evidence_ids.len()),
            },
        )?;
        let superseded_event_id = self.supersede_decision_with_uuid(
            old_decision_id,
            &new_decision_id,
            actor_id,
            Uuid::new_v4(),
        )?;

        Ok(SupersedeOutcome {
            new_decision_id,
            proposal_event_id: proposal_event_ids.proposal_event_id,
            relation_event_ids: proposal_event_ids.relation_event_ids,
            superseded_event_id,
        })
    }

    pub fn attach_evidence(
        &self,
        decision_id: &str,
        evidence_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        self.attach_evidence_with_uuid(decision_id, evidence_id, actor_id, Uuid::new_v4())
    }

    pub fn attach_evidence_with_uuid(
        &self,
        decision_id: &str,
        evidence_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("evidence_id", evidence_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }
        if !self.evidence_exists(evidence_id)? {
            return Err(
                CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into(),
            );
        }

        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::BasedOn,
            decision_id,
            evidence_id,
            event_uuid,
        )
    }

    pub fn assume_hypothesis_with_uuid(
        &self,
        decision_id: &str,
        hypothesis_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }
        if !self.hypothesis_exists(hypothesis_id)? {
            return Err(CommandError::Invariant(format!(
                "hypothesis does not exist: {hypothesis_id}"
            ))
            .into());
        }

        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::Assumes,
            decision_id,
            hypothesis_id,
            event_uuid,
        )
    }

    pub fn relate_evidence_to_hypothesis(
        &self,
        evidence_id: &str,
        hypothesis_id: &str,
        relation_kind: RelationKind,
        actor_id: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("evidence_id", evidence_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;

        if !matches!(
            relation_kind,
            RelationKind::Supports | RelationKind::Refutes
        ) {
            return Err(CommandError::Validation(
                "relation kind must be supports or refutes".to_owned(),
            )
            .into());
        }

        if !self.evidence_exists(evidence_id)? {
            return Err(
                CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into(),
            );
        }
        if !self.hypothesis_exists(hypothesis_id)? {
            return Err(CommandError::Invariant(format!(
                "hypothesis does not exist: {hypothesis_id}"
            ))
            .into());
        }

        if let Some(existing_event_id) =
            self.find_relation_event_id(relation_kind, evidence_id, hypothesis_id)?
        {
            return Ok(existing_event_id);
        }

        self.append_relation_event(actor_id, 0, relation_kind, evidence_id, hypothesis_id)
    }

    fn append_relation_event(
        &self,
        actor_id: &str,
        root_event_id: EventId,
        relation: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<EventId> {
        self.append_relation_event_with_uuid(
            actor_id,
            root_event_id,
            relation,
            from_id,
            to_id,
            Uuid::new_v4(),
        )
    }

    fn append_relation_event_with_uuid(
        &self,
        actor_id: &str,
        root_event_id: EventId,
        relation: RelationKind,
        from_id: &str,
        to_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        let event = self.event_with_uuid(
            EventType::RelationAdded,
            actor_id,
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id,
            }),
            if root_event_id == 0 {
                None
            } else {
                Some(root_event_id)
            },
            event_uuid,
        );

        self.ledger.append(event)
    }

    fn event_with_uuid(
        &self,
        event_type: EventType,
        actor_id: &str,
        payload: serde_json::Value,
        causation_event_id: Option<EventId>,
        event_uuid: Uuid,
    ) -> Event {
        Event {
            event_id: None,
            event_uuid,
            correlation_id: None,
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            source: self.provenance.source,
            source_ref: self.provenance.source_ref.clone(),
            payload,
            ts: Some(Utc::now()),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, CommandState>> {
        self.state.lock().map_err(|error| {
            CommandError::Invariant(format!("commands state lock poisoned: {error}")).into()
        })
    }

    fn evidence_exists(&self, evidence_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: evidence IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "evidence_id", evidence_id);
            event.event_type == EventType::EvidenceRecorded && has_matching_id
        })
    }

    fn hypothesis_exists(&self, hypothesis_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            let has_matching_id =
                // ubs:ignore: hypothesis IDs are public ledger IDs, not timing-sensitive secrets.
                payload_value_matches(event, "hypothesis_id", hypothesis_id);
            event.event_type == EventType::HypothesisRecorded && has_matching_id
        })
    }

    fn decision_exists(&self, decision_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
            event.event_type == EventType::DecisionProposed && has_matching_id
        })
    }

    fn actor_has_decision_event(
        &self,
        decision_id: &str,
        actor_id: &str,
        decision_event_type: EventType,
    ) -> Result<bool> {
        Ok(self
            .find_decision_event_id(decision_id, actor_id, decision_event_type)?
            .is_some())
    }

    fn find_decision_event_id(
        &self,
        decision_id: &str,
        actor_id: &str,
        decision_event_type: EventType,
    ) -> Result<Option<EventId>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                // ubs:ignore: actor IDs are public ledger IDs, not timing-sensitive secrets.
                let has_matching_actor = same_identifier(event.actor_id.as_str(), actor_id);
                // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
                let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
                if event.event_type == decision_event_type && has_matching_actor && has_matching_id
                {
                    return Ok(event.event_id);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    fn decision_proposal_snapshot(
        &self,
        decision_id: &str,
    ) -> Result<Option<DecisionProposalSnapshot>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::DecisionProposed {
                    continue;
                }
                let Some(snapshot) = decision_proposal_snapshot_from_event(event) else {
                    continue;
                };
                if same_identifier(snapshot.decision_id.as_str(), decision_id) {
                    return Ok(Some(snapshot));
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn find_matching_supersede(
        &self,
        actor_id: &str,
        old_decision_id: &str,
        new_title: &str,
        new_rationale: &str,
        topic_keys: &[String],
        option_ids: &[String],
        chosen_option_id: Option<&str>,
        hypothesis_ids: &[String],
        evidence_ids: &[String],
    ) -> Result<Option<SupersedeOutcome>> {
        let mut proposals = HashMap::new();
        let mut superseded_events = Vec::new();
        let mut relation_event_ids_by_causation: HashMap<EventId, Vec<EventId>> = HashMap::new();
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                break;
            }

            for event in &events {
                match event.event_type {
                    EventType::DecisionProposed => {
                        if let Some(snapshot) = decision_proposal_snapshot_from_event(event) {
                            proposals.insert(snapshot.decision_id.clone(), snapshot);
                        }
                    }
                    EventType::DecisionSuperseded => {
                        let Some(event_id) = event.event_id else {
                            continue;
                        };
                        let Some(old_id) = payload_value_as_str(event, "old_decision_id") else {
                            continue;
                        };
                        let Some(new_id) = payload_value_as_str(event, "new_decision_id") else {
                            continue;
                        };
                        // ubs:ignore: actor and decision IDs are public graph IDs.
                        if same_identifier(event.actor_id.as_str(), actor_id)
                            && same_identifier(old_id, old_decision_id)
                        {
                            superseded_events.push((event_id, new_id.to_owned()));
                        }
                    }
                    EventType::RelationAdded => {
                        if let (Some(causation_event_id), Some(event_id)) =
                            (event.causation_event_id, event.event_id)
                        {
                            relation_event_ids_by_causation
                                .entry(causation_event_id)
                                .or_default()
                                .push(event_id);
                        }
                    }
                    _ => {}
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                break;
            }
        }

        for (superseded_event_id, new_decision_id) in superseded_events {
            let Some(proposal) = proposals.get(&new_decision_id) else {
                continue;
            };
            // ubs:ignore: actor and decision IDs are public graph IDs.
            if !same_identifier(proposal.actor_id.as_str(), actor_id) {
                continue;
            }
            if proposal.title == new_title
                && proposal.rationale == new_rationale
                && proposal.topic_keys == topic_keys
                && proposal.option_ids == option_ids
                && proposal.chosen_option_id.as_deref() == chosen_option_id
                && proposal.hypothesis_ids == hypothesis_ids
                && proposal.evidence_ids == evidence_ids
            {
                let relation_event_ids = relation_event_ids_by_causation
                    .get(&proposal.event_id)
                    .cloned()
                    .unwrap_or_default();
                return Ok(Some(SupersedeOutcome {
                    new_decision_id,
                    proposal_event_id: proposal.event_id,
                    relation_event_ids,
                    superseded_event_id,
                }));
            }
        }

        Ok(None)
    }

    fn find_relation_event_id(
        &self,
        relation_kind: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<Option<EventId>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;
        let relation_name = relation_kind_name(relation_kind);

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::RelationAdded {
                    continue;
                }

                let same_relation = payload_value_matches(event, "relation", relation_name);
                let same_from = payload_value_matches(event, "from_id", from_id);
                let same_to = payload_value_matches(event, "to_id", to_id);
                if same_relation && same_from && same_to {
                    return Ok(event.event_id);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    fn scan_events(&self, predicate: impl Fn(&Event) -> bool) -> Result<bool> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(false);
            }

            for event in &events {
                if predicate(event) {
                    return Ok(true);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(false);
            }
        }
    }
}

pub fn normalize_topic_key(input: &str) -> String {
    let transliterated = deunicode::deunicode(input);
    let mut normalized = String::with_capacity(transliterated.len());
    let mut last_was_separator = false;

    for character in transliterated.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            last_was_separator = false;
            continue;
        }

        if normalized.is_empty() || last_was_separator {
            continue;
        }

        normalized.push('-');
        last_was_separator = true;
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.len() > MAX_TOPIC_KEY_LEN {
        normalized.truncate(MAX_TOPIC_KEY_LEN);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }

    normalized
}

fn effective_topic_keys(requested: &[String], fallback: &[String]) -> Result<Vec<String>> {
    let normalized_requested = normalize_topic_keys(requested);
    let effective = if normalized_requested.is_empty() {
        fallback.to_vec()
    } else {
        normalized_requested
    };

    if effective.is_empty() {
        Err(CommandError::Validation(
            "topic_keys must contain at least one non-empty normalized key".to_owned(),
        )
        .into())
    } else {
        Ok(effective)
    }
}

fn normalize_topic_keys(topic_keys: &[String]) -> Vec<String> {
    topic_keys
        .iter()
        .map(|topic| normalize_topic_key(topic))
        .filter(|topic| !topic.is_empty())
        .collect()
}

fn effective_option_labels(
    new_title: &str,
    option_labels: &[String],
    chosen_option_label: Option<&str>,
) -> Result<Vec<String>> {
    let mut labels = Vec::with_capacity(option_labels.len().max(1));
    for option_label in option_labels {
        let trimmed = option_label.trim();
        if trimmed.is_empty() {
            return Err(CommandError::Validation(
                "option_labels must not contain empty values".to_owned(),
            )
            .into());
        }
        labels.push(trimmed.to_owned());
    }

    if labels.is_empty() {
        let fallback = chosen_option_label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| new_title.trim());
        labels.push(fallback.to_owned());
    }

    Ok(labels)
}

fn deterministic_supersede_option_ids(
    actor_id: &str,
    old_decision_id: &str,
    new_title: &str,
    new_rationale: &str,
    option_labels: &[String],
) -> Vec<String> {
    option_labels
        .iter()
        .enumerate()
        .map(|(index, option_label)| {
            let stable_name =
                format!("{actor_id}\0{old_decision_id}\0{new_title}\0{new_rationale}\0{index}\0{option_label}");
            format!("option-{}", Uuid::new_v5(&Uuid::NAMESPACE_URL, stable_name.as_bytes()))
        })
        .collect()
}

fn decision_proposal_snapshot_from_event(event: &Event) -> Option<DecisionProposalSnapshot> {
    let event_id = event.event_id?;
    Some(DecisionProposalSnapshot {
        event_id,
        decision_id: payload_value_as_str(event, "decision_id")?.to_owned(),
        actor_id: event.actor_id.clone(),
        title: payload_value_as_str(event, "title")?.to_owned(),
        rationale: payload_value_as_str(event, "rationale")?.to_owned(),
        topic_keys: payload_string_list(event, "topic_keys"),
        option_ids: payload_string_list(event, "option_ids"),
        chosen_option_id: payload_value_as_str(event, "chosen_option_id").map(str::to_owned),
        hypothesis_ids: payload_string_list(event, "hypothesis_ids"),
        evidence_ids: payload_string_list(event, "evidence_ids"),
    })
}

fn payload_string_list(event: &Event, key: &str) -> Vec<String> {
    event
        .payload
        .get(key)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn payload_value_as_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(|value| value.as_str())
}

fn payload_value_matches(event: &Event, key: &str, expected: &str) -> bool {
    payload_value_as_str(event, key).is_some_and(|actual| same_identifier(actual, expected))
}

fn same_identifier(left: &str, right: &str) -> bool {
    left.eq(right)
}

const fn relation_kind_name(relation_kind: RelationKind) -> &'static str {
    match relation_kind {
        RelationKind::BasedOn => "BASED_ON",
        RelationKind::HasOption => "HAS_OPTION",
        RelationKind::Chose => "CHOSE",
        RelationKind::Assumes => "ASSUMES",
        RelationKind::Supports => "SUPPORTS",
        RelationKind::Refutes => "REFUTES",
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

fn require_optional_non_empty(field: &'static str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

fn generate_entity_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn generate_option_id(label: &str) -> String {
    let slug = normalize_topic_key(label);
    let slug = if slug.is_empty() { "option" } else { &slug };
    format!("option-{slug}-{}", Uuid::new_v4())
}

fn repeat_uuid(count: usize) -> Vec<Uuid> {
    (0..count).map(|_| Uuid::new_v4()).collect()
}

#[cfg(test)]
mod tests;
