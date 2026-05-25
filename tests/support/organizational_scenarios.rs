use chrono::{DateTime, Duration, Utc};
use hivemind::events::{Event, EventSource, EventType, RelationKind};
use serde_json::json;
use uuid::Uuid;

const ORG_BASE_UNIX_SECONDS: i64 = 1_767_312_000;

#[derive(Clone, Debug)]
pub struct OrganizationalScenario {
    pub name: &'static str,
    pub topic: &'static str,
    pub events: Vec<Event>,
    pub final_decision_id: &'static str,
    pub final_evidence_id: &'static str,
    pub final_hypothesis_id: &'static str,
    pub contested_decision_id: Option<&'static str>,
    pub rejected_decision_id: Option<&'static str>,
    pub supersession_probe_id: Option<&'static str>,
    pub supersession_chain: &'static [&'static str],
}

pub fn scenarios() -> Vec<OrganizationalScenario> {
    vec![
        production_incident_response(),
        remote_db_architecture_choice(),
        product_launch_readiness(),
        security_vulnerability_triage(),
        hiring_capacity_planning(),
    ]
}

pub fn scenario_events() -> Vec<Event> {
    scenarios()
        .into_iter()
        .flat_map(|scenario| scenario.events)
        .collect()
}

fn production_incident_response() -> OrganizationalScenario {
    let mut builder = ScenarioBuilder::new("production-incident-response", 1);
    builder.evidence(
        "org:incident:evidence:error-spike",
        "agent:monitoring",
        "Checkout errors rose above the incident threshold for the new pricing flag.",
    );
    builder.evidence(
        "org:incident:evidence:rollback-health",
        "agent:sre",
        "Canary rollback restored checkout success rate in the affected region.",
    );
    builder.evidence(
        "org:incident:evidence:customer-impact",
        "human:support-lead",
        "Enterprise customers opened priority cases while the hotfix window slipped.",
    );
    builder.hypothesis(
        "org:incident:hypothesis:flag-regression",
        "The new pricing flag is causing the checkout regression.",
        "agent:sre",
    );
    builder.hypothesis(
        "org:incident:hypothesis:hotfix-fast-enough",
        "A hotfix can land before customer impact expands.",
        "human:oncall",
    );
    builder.supports(
        "org:incident:evidence:error-spike",
        "org:incident:hypothesis:flag-regression",
        "agent:sre",
    );
    builder.refutes(
        "org:incident:evidence:customer-impact",
        "org:incident:hypothesis:hotfix-fast-enough",
        "human:support-lead",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:incident:decision:declare",
        actor_id: "agent:monitoring",
        title: "Declare pricing-flag incident",
        rationale: "Automated monitoring saw a sustained checkout failure spike.",
        topic_keys: &["org.incident", "operations"],
        option_ids: &["org:incident:option:declare", "org:incident:option:watch"],
        chosen_option_id: "org:incident:option:declare",
        hypothesis_ids: &["org:incident:hypothesis:flag-regression"],
        evidence_ids: &["org:incident:evidence:error-spike"],
    });
    builder.accept("org:incident:decision:declare", "human:oncall");
    builder.decision(DecisionSpec {
        decision_id: "org:incident:decision:hotfix",
        actor_id: "human:oncall",
        title: "Attempt hotfix before rollback",
        rationale: "A targeted patch might preserve the feature launch.",
        topic_keys: &["org.incident", "operations"],
        option_ids: &["org:incident:option:hotfix", "org:incident:option:rollback"],
        chosen_option_id: "org:incident:option:hotfix",
        hypothesis_ids: &["org:incident:hypothesis:hotfix-fast-enough"],
        evidence_ids: &[
            "org:incident:evidence:error-spike",
            "org:incident:evidence:customer-impact",
        ],
    });
    builder.accept("org:incident:decision:hotfix", "human:product-owner");
    builder.reject("org:incident:decision:hotfix", "agent:sre");
    builder.decision(DecisionSpec {
        decision_id: "org:incident:decision:rollback",
        actor_id: "agent:sre",
        title: "Rollback pricing flag and queue remediation",
        rationale:
            "Rollback evidence resolves the immediate risk and preserves remediation context.",
        topic_keys: &["org.incident", "operations", "customer-comms"],
        option_ids: &[
            "org:incident:option:disable-flag",
            "org:incident:option:regional-patch",
        ],
        chosen_option_id: "org:incident:option:disable-flag",
        hypothesis_ids: &[
            "org:incident:hypothesis:flag-regression",
            "org:incident:hypothesis:hotfix-fast-enough",
        ],
        evidence_ids: &[
            "org:incident:evidence:rollback-health",
            "org:incident:evidence:customer-impact",
        ],
    });
    builder.accept("org:incident:decision:rollback", "human:oncall");
    builder.supersede(
        "org:incident:decision:hotfix",
        "org:incident:decision:rollback",
        "agent:sre",
    );

    OrganizationalScenario {
        name: "production-incident-response",
        topic: "org.incident",
        events: builder.events,
        final_decision_id: "org:incident:decision:rollback",
        final_evidence_id: "org:incident:evidence:rollback-health",
        final_hypothesis_id: "org:incident:hypothesis:flag-regression",
        contested_decision_id: None,
        rejected_decision_id: None,
        supersession_probe_id: Some("org:incident:decision:hotfix"),
        supersession_chain: &[
            "org:incident:decision:hotfix",
            "org:incident:decision:rollback",
        ],
    }
}

fn remote_db_architecture_choice() -> OrganizationalScenario {
    let mut builder = ScenarioBuilder::new("remote-db-architecture-choice", 2);
    builder.evidence(
        "org:remote-db:evidence:shared-state",
        "human:product-stakeholder",
        "Non-developer users need shared state across support shifts.",
    );
    builder.evidence(
        "org:remote-db:evidence:security-review",
        "human:security-reviewer",
        "Security review rejects direct database writes from every client.",
    );
    builder.evidence(
        "org:remote-db:evidence:ops-latency",
        "agent:ops",
        "Hosted Postgres meets the latency and backup constraints for the pilot.",
    );
    builder.hypothesis(
        "org:remote-db:hypothesis:embedded-sufficient",
        "A local embedded store is enough for the first shared pilot.",
        "agent:coding",
    );
    builder.hypothesis(
        "org:remote-db:hypothesis:service-owned-writes",
        "A service-owned write path can meet security and operational constraints.",
        "human:architect",
    );
    builder.refutes(
        "org:remote-db:evidence:shared-state",
        "org:remote-db:hypothesis:embedded-sufficient",
        "human:product-stakeholder",
    );
    builder.supports(
        "org:remote-db:evidence:security-review",
        "org:remote-db:hypothesis:service-owned-writes",
        "human:security-reviewer",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:remote-db:decision:embedded-prototype",
        actor_id: "agent:coding",
        title: "Keep prototype storage embedded",
        rationale: "The local store is simplest while the data model is still moving.",
        topic_keys: &["org.remote-db", "architecture"],
        option_ids: &[
            "org:remote-db:option:embedded",
            "org:remote-db:option:remote-service",
        ],
        chosen_option_id: "org:remote-db:option:embedded",
        hypothesis_ids: &["org:remote-db:hypothesis:embedded-sufficient"],
        evidence_ids: &["org:remote-db:evidence:shared-state"],
    });
    builder.accept("org:remote-db:decision:embedded-prototype", "agent:coding");
    builder.decision(DecisionSpec {
        decision_id: "org:remote-db:decision:service-owned-writes",
        actor_id: "human:architect",
        title: "Move writes behind a shared remote service",
        rationale: "Shared state and security review both require service ownership.",
        topic_keys: &["org.remote-db", "architecture", "security"],
        option_ids: &[
            "org:remote-db:option:direct-clients",
            "org:remote-db:option:service-owned-writes",
        ],
        chosen_option_id: "org:remote-db:option:service-owned-writes",
        hypothesis_ids: &["org:remote-db:hypothesis:service-owned-writes"],
        evidence_ids: &[
            "org:remote-db:evidence:shared-state",
            "org:remote-db:evidence:security-review",
        ],
    });
    builder.accept(
        "org:remote-db:decision:service-owned-writes",
        "human:security-reviewer",
    );
    builder.supersede(
        "org:remote-db:decision:embedded-prototype",
        "org:remote-db:decision:service-owned-writes",
        "human:architect",
    );
    builder.decision(
        DecisionSpec {
            decision_id: "org:remote-db:decision:postgres-pilot",
            actor_id: "agent:ops",
            title: "Use hosted Postgres for the short-run backend",
            rationale: "It satisfies the shared-service migration plan without introducing graph storage early.",
            topic_keys: &["org.remote-db", "architecture", "migration"],
            option_ids: &[
                "org:remote-db:option:postgres",
                "org:remote-db:option:mysql",
            ],
            chosen_option_id: "org:remote-db:option:postgres",
            hypothesis_ids: &["org:remote-db:hypothesis:service-owned-writes"],
            evidence_ids: &["org:remote-db:evidence:ops-latency"],
        },
    );
    builder.accept("org:remote-db:decision:postgres-pilot", "human:architect");

    OrganizationalScenario {
        name: "remote-db-architecture-choice",
        topic: "org.remote-db",
        events: builder.events,
        final_decision_id: "org:remote-db:decision:postgres-pilot",
        final_evidence_id: "org:remote-db:evidence:ops-latency",
        final_hypothesis_id: "org:remote-db:hypothesis:service-owned-writes",
        contested_decision_id: None,
        rejected_decision_id: None,
        supersession_probe_id: Some("org:remote-db:decision:embedded-prototype"),
        supersession_chain: &[
            "org:remote-db:decision:embedded-prototype",
            "org:remote-db:decision:service-owned-writes",
        ],
    }
}

fn product_launch_readiness() -> OrganizationalScenario {
    let mut builder = ScenarioBuilder::new("product-launch-readiness", 3);
    builder.evidence(
        "org:launch:evidence:qa-failures",
        "agent:qa",
        "Regression automation still fails checkout and billing smoke tests.",
    );
    builder.evidence(
        "org:launch:evidence:user-research",
        "agent:analytics",
        "Beta cohorts complete activation when the risky bulk import is hidden.",
    );
    builder.evidence(
        "org:launch:evidence:legal-caveat",
        "human:legal",
        "Launch copy needs a compliance caveat before broad availability.",
    );
    builder.hypothesis(
        "org:launch:hypothesis:launch-ready",
        "The full public launch is ready this week.",
        "human:pm",
    );
    builder.hypothesis(
        "org:launch:hypothesis:phased-rollout",
        "A phased beta can capture value while containing support and compliance risk.",
        "agent:analytics",
    );
    builder.refutes(
        "org:launch:evidence:qa-failures",
        "org:launch:hypothesis:launch-ready",
        "agent:qa",
    );
    builder.supports(
        "org:launch:evidence:user-research",
        "org:launch:hypothesis:phased-rollout",
        "agent:analytics",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:launch:decision:readiness",
        actor_id: "human:pm",
        title: "Treat launch readiness as green",
        rationale: "The core workflow works for the beta cohort.",
        topic_keys: &["org.launch", "product"],
        option_ids: &["org:launch:option:ready", "org:launch:option:not-ready"],
        chosen_option_id: "org:launch:option:ready",
        hypothesis_ids: &["org:launch:hypothesis:launch-ready"],
        evidence_ids: &["org:launch:evidence:user-research"],
    });
    builder.accept("org:launch:decision:readiness", "human:pm");
    builder.reject("org:launch:decision:readiness", "agent:qa");
    builder.decision(DecisionSpec {
        decision_id: "org:launch:decision:launch-now",
        actor_id: "human:pm",
        title: "Launch publicly now",
        rationale:
            "The launch window is available but quality and caveat evidence are still unresolved.",
        topic_keys: &["org.launch", "product"],
        option_ids: &["org:launch:option:public-now", "org:launch:option:delay"],
        chosen_option_id: "org:launch:option:public-now",
        hypothesis_ids: &["org:launch:hypothesis:launch-ready"],
        evidence_ids: &[
            "org:launch:evidence:qa-failures",
            "org:launch:evidence:legal-caveat",
        ],
    });
    builder.reject("org:launch:decision:launch-now", "human:legal");
    builder.decision(DecisionSpec {
        decision_id: "org:launch:decision:phased-rollout",
        actor_id: "agent:analytics",
        title: "Ship phased beta with scoped messaging",
        rationale: "Cutting bulk import and adding the caveat preserves value without hiding risk.",
        topic_keys: &["org.launch", "product", "support"],
        option_ids: &[
            "org:launch:option:phased-beta",
            "org:launch:option:full-delay",
        ],
        chosen_option_id: "org:launch:option:phased-beta",
        hypothesis_ids: &["org:launch:hypothesis:phased-rollout"],
        evidence_ids: &[
            "org:launch:evidence:user-research",
            "org:launch:evidence:legal-caveat",
        ],
    });
    builder.accept("org:launch:decision:phased-rollout", "human:support");

    OrganizationalScenario {
        name: "product-launch-readiness",
        topic: "org.launch",
        events: builder.events,
        final_decision_id: "org:launch:decision:phased-rollout",
        final_evidence_id: "org:launch:evidence:legal-caveat",
        final_hypothesis_id: "org:launch:hypothesis:phased-rollout",
        contested_decision_id: Some("org:launch:decision:readiness"),
        rejected_decision_id: Some("org:launch:decision:launch-now"),
        supersession_probe_id: None,
        supersession_chain: &[],
    }
}

fn security_vulnerability_triage() -> OrganizationalScenario {
    let mut builder = ScenarioBuilder::new("security-vulnerability-triage", 4);
    builder.evidence(
        "org:security:evidence:scanner-hit",
        "agent:scanner",
        "Static scanner flagged an authorization bypass in export links.",
    );
    builder.evidence(
        "org:security:evidence:exploit-repro",
        "human:security",
        "Manual reproduction accesses another tenant's export without authentication.",
    );
    builder.evidence(
        "org:security:evidence:customer-exposure",
        "human:customer-success",
        "Customer exposure logs show two enterprise tenants may be affected.",
    );
    builder.hypothesis(
        "org:security:hypothesis:auth-required",
        "The export endpoint still requires tenant authentication.",
        "agent:maintainer",
    );
    builder.hypothesis(
        "org:security:hypothesis:flagged-patch-safe",
        "A feature-flagged patch can close exposure without breaking exports.",
        "human:security",
    );
    builder.supports(
        "org:security:evidence:scanner-hit",
        "org:security:hypothesis:auth-required",
        "agent:scanner",
    );
    builder.refutes(
        "org:security:evidence:exploit-repro",
        "org:security:hypothesis:auth-required",
        "human:security",
    );
    builder.supports(
        "org:security:evidence:customer-exposure",
        "org:security:hypothesis:flagged-patch-safe",
        "human:customer-success",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:security:decision:severity-low",
        actor_id: "agent:maintainer",
        title: "Classify export issue as low severity",
        rationale: "The first read assumed authentication was still enforced.",
        topic_keys: &["org.security", "security"],
        option_ids: &["org:security:option:low", "org:security:option:critical"],
        chosen_option_id: "org:security:option:low",
        hypothesis_ids: &["org:security:hypothesis:auth-required"],
        evidence_ids: &["org:security:evidence:scanner-hit"],
    });
    builder.accept("org:security:decision:severity-low", "agent:maintainer");
    builder.decision(DecisionSpec {
        decision_id: "org:security:decision:severity-critical",
        actor_id: "human:security",
        title: "Escalate export issue to critical severity",
        rationale: "Exploit reproduction refutes the low-severity assumption.",
        topic_keys: &["org.security", "security", "legal"],
        option_ids: &["org:security:option:critical", "org:security:option:medium"],
        chosen_option_id: "org:security:option:critical",
        hypothesis_ids: &["org:security:hypothesis:auth-required"],
        evidence_ids: &[
            "org:security:evidence:exploit-repro",
            "org:security:evidence:customer-exposure",
        ],
    });
    builder.accept("org:security:decision:severity-critical", "human:legal");
    builder.supersede(
        "org:security:decision:severity-low",
        "org:security:decision:severity-critical",
        "human:security",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:security:decision:coordinated-notice",
        actor_id: "human:legal",
        title: "Notify affected customers after patch deployment",
        rationale: "Legal and customer success need a consistent disclosure package.",
        topic_keys: &["org.security", "legal", "customer-comms"],
        option_ids: &[
            "org:security:option:notify-now",
            "org:security:option:notify-after-patch",
        ],
        chosen_option_id: "org:security:option:notify-after-patch",
        hypothesis_ids: &["org:security:hypothesis:flagged-patch-safe"],
        evidence_ids: &["org:security:evidence:customer-exposure"],
    });
    builder.accept("org:security:decision:coordinated-notice", "human:legal");
    builder.reject(
        "org:security:decision:coordinated-notice",
        "human:customer-success",
    );

    OrganizationalScenario {
        name: "security-vulnerability-triage",
        topic: "org.security",
        events: builder.events,
        final_decision_id: "org:security:decision:severity-critical",
        final_evidence_id: "org:security:evidence:exploit-repro",
        final_hypothesis_id: "org:security:hypothesis:auth-required",
        contested_decision_id: Some("org:security:decision:coordinated-notice"),
        rejected_decision_id: None,
        supersession_probe_id: Some("org:security:decision:severity-low"),
        supersession_chain: &[
            "org:security:decision:severity-low",
            "org:security:decision:severity-critical",
        ],
    }
}

fn hiring_capacity_planning() -> OrganizationalScenario {
    let mut builder = ScenarioBuilder::new("hiring-capacity-planning", 5);
    builder.evidence(
        "org:capacity:evidence:roadmap-gap",
        "agent:planning",
        "Roadmap forecast shows two quarters of backend capacity shortfall.",
    );
    builder.evidence(
        "org:capacity:evidence:budget-window",
        "human:finance",
        "Budget can support one full-time hire but not two contractors.",
    );
    builder.evidence(
        "org:capacity:evidence:contractor-ramp",
        "human:recruiting",
        "Recent contractors took longer to ramp on regulated workflows.",
    );
    builder.hypothesis(
        "org:capacity:hypothesis:contractor-covers-gap",
        "A contractor can cover the next milestone without onboarding drag.",
        "human:manager",
    );
    builder.hypothesis(
        "org:capacity:hypothesis:fulltime-payoff",
        "A full-time backend hire pays off after the first onboarding month.",
        "agent:team-lead",
    );
    builder.refutes(
        "org:capacity:evidence:contractor-ramp",
        "org:capacity:hypothesis:contractor-covers-gap",
        "human:recruiting",
    );
    builder.supports(
        "org:capacity:evidence:budget-window",
        "org:capacity:hypothesis:fulltime-payoff",
        "human:finance",
    );
    builder.decision(DecisionSpec {
        decision_id: "org:capacity:decision:defer-hire",
        actor_id: "human:finance",
        title: "Defer hiring until next planning cycle",
        rationale: "Budget timing is tight and headcount can wait if scope shrinks.",
        topic_keys: &["org.capacity", "planning"],
        option_ids: &["org:capacity:option:defer", "org:capacity:option:hire-now"],
        chosen_option_id: "org:capacity:option:defer",
        hypothesis_ids: &["org:capacity:hypothesis:contractor-covers-gap"],
        evidence_ids: &["org:capacity:evidence:budget-window"],
    });
    builder.reject("org:capacity:decision:defer-hire", "agent:team-lead");
    builder.decision(DecisionSpec {
        decision_id: "org:capacity:decision:contractor",
        actor_id: "human:manager",
        title: "Use a contractor for backend capacity",
        rationale: "Contracting keeps the team flexible while recruiting starts.",
        topic_keys: &["org.capacity", "planning"],
        option_ids: &[
            "org:capacity:option:contractor",
            "org:capacity:option:fulltime",
        ],
        chosen_option_id: "org:capacity:option:contractor",
        hypothesis_ids: &["org:capacity:hypothesis:contractor-covers-gap"],
        evidence_ids: &[
            "org:capacity:evidence:roadmap-gap",
            "org:capacity:evidence:contractor-ramp",
        ],
    });
    builder.accept("org:capacity:decision:contractor", "human:manager");
    builder.reject("org:capacity:decision:contractor", "agent:team-lead");
    builder.decision(DecisionSpec {
        decision_id: "org:capacity:decision:fulltime-hire",
        actor_id: "agent:planning",
        title: "Open one full-time backend role with a focused loop",
        rationale: "Roadmap pressure and budget evidence favor durable capacity.",
        topic_keys: &["org.capacity", "planning", "hiring"],
        option_ids: &[
            "org:capacity:option:fulltime",
            "org:capacity:option:scope-cut",
        ],
        chosen_option_id: "org:capacity:option:fulltime",
        hypothesis_ids: &["org:capacity:hypothesis:fulltime-payoff"],
        evidence_ids: &[
            "org:capacity:evidence:roadmap-gap",
            "org:capacity:evidence:budget-window",
        ],
    });
    builder.accept("org:capacity:decision:fulltime-hire", "human:manager");
    builder.decision(DecisionSpec {
        decision_id: "org:capacity:decision:onboarding-plan",
        actor_id: "agent:team-lead",
        title: "Pair new hire with regulated workflow owner",
        rationale: "The onboarding plan addresses the capacity risk without losing domain context.",
        topic_keys: &["org.capacity", "hiring"],
        option_ids: &[
            "org:capacity:option:paired-onboarding",
            "org:capacity:option:self-serve-onboarding",
        ],
        chosen_option_id: "org:capacity:option:paired-onboarding",
        hypothesis_ids: &["org:capacity:hypothesis:fulltime-payoff"],
        evidence_ids: &["org:capacity:evidence:contractor-ramp"],
    });
    builder.accept("org:capacity:decision:onboarding-plan", "human:recruiting");

    OrganizationalScenario {
        name: "hiring-capacity-planning",
        topic: "org.capacity",
        events: builder.events,
        final_decision_id: "org:capacity:decision:fulltime-hire",
        final_evidence_id: "org:capacity:evidence:budget-window",
        final_hypothesis_id: "org:capacity:hypothesis:fulltime-payoff",
        contested_decision_id: Some("org:capacity:decision:contractor"),
        rejected_decision_id: Some("org:capacity:decision:defer-hire"),
        supersession_probe_id: None,
        supersession_chain: &[],
    }
}

struct ScenarioBuilder {
    name: &'static str,
    scenario_index: u128,
    events: Vec<Event>,
}

struct DecisionSpec<'a> {
    decision_id: &'a str,
    actor_id: &'a str,
    title: &'a str,
    rationale: &'a str,
    topic_keys: &'a [&'a str],
    option_ids: &'a [&'a str],
    chosen_option_id: &'a str,
    hypothesis_ids: &'a [&'a str],
    evidence_ids: &'a [&'a str],
}

impl ScenarioBuilder {
    fn new(name: &'static str, scenario_index: u128) -> Self {
        Self {
            name,
            scenario_index,
            events: Vec::new(),
        }
    }

    fn evidence(&mut self, evidence_id: &str, actor_id: &str, content: &str) {
        self.push(
            EventType::EvidenceRecorded,
            actor_id,
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": self.name
            }),
        );
    }

    fn hypothesis(&mut self, hypothesis_id: &str, statement: &str, actor_id: &str) {
        self.push(
            EventType::HypothesisRecorded,
            actor_id,
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
        );
    }

    fn decision(&mut self, spec: DecisionSpec<'_>) {
        self.push(
            EventType::DecisionProposed,
            spec.actor_id,
            json!({
                "decision_id": spec.decision_id,
                "title": spec.title,
                "rationale": spec.rationale,
                "topic_keys": spec.topic_keys,
                "option_ids": spec.option_ids,
                "chosen_option_id": spec.chosen_option_id,
                "hypothesis_ids": spec.hypothesis_ids,
                "evidence_ids": spec.evidence_ids
            }),
        );
    }

    fn accept(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
        );
    }

    fn reject(&mut self, decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
        );
    }

    fn supersede(&mut self, old_decision_id: &str, new_decision_id: &str, actor_id: &str) {
        self.push(
            EventType::DecisionSuperseded,
            actor_id,
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id
            }),
        );
    }

    fn supports(&mut self, evidence_id: &str, hypothesis_id: &str, actor_id: &str) {
        self.relation(RelationKind::Supports, evidence_id, hypothesis_id, actor_id);
    }

    fn refutes(&mut self, evidence_id: &str, hypothesis_id: &str, actor_id: &str) {
        self.relation(RelationKind::Refutes, evidence_id, hypothesis_id, actor_id);
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
        );
    }

    fn push(&mut self, event_type: EventType, actor_id: &str, payload: serde_json::Value) {
        let sequence = self.events.len() + 1;
        let sequence_u128 = u128::try_from(sequence).unwrap_or(u128::MAX);
        self.events.push(Event {
            tenant_id: Default::default(),
            event_id: None,
            event_uuid: Uuid::from_u128((self.scenario_index << 96) | sequence_u128),
            correlation_id: Some(format!("org-scenario:{}", self.name)),
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some(self.name.to_owned()),
            payload,
            ts: Some(scenario_timestamp(self.scenario_index, sequence)),
        });
    }
}

fn scenario_timestamp(scenario_index: u128, sequence: usize) -> DateTime<Utc> {
    let scenario_seconds = i64::try_from(scenario_index).unwrap_or(0) * 600;
    let sequence_seconds = i64::try_from(sequence).unwrap_or(0);
    DateTime::from_timestamp(ORG_BASE_UNIX_SECONDS, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        + Duration::seconds(scenario_seconds + sequence_seconds)
}
