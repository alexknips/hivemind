use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use hivemind::events::{Event, EventSource, EventType, RelationKind, TenantId};
use hivemind::ledger::{EventLedger, SqliteEventLedger};
use hivemind::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use hivemind::queries::get_decision;
use serde_json::json;
use uuid::Uuid;

pub type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const DECISIONS_PER_TENANT: usize = 10;
pub const SHARED_DECISION_ID: &str = "decision-001";

const MULTI_TENANT_BASE_UNIX_SECONDS: i64 = 1_767_916_800;
const EVIDENCE_IDS: [&str; 3] = ["evidence-001", "evidence-002", "evidence-003"];
const HYPOTHESIS_IDS: [&str; 2] = ["hypothesis-001", "hypothesis-002"];

#[derive(Clone, Debug)]
pub struct MultiTenantSeed {
    namespace: String,
    tenants: Vec<SeedTenant>,
}

impl MultiTenantSeed {
    pub fn new(namespace: impl Into<String>) -> TestResult<Self> {
        let namespace = namespace.into();
        if namespace.trim().is_empty() {
            return Err("multi-tenant fixture namespace must not be empty".into());
        }

        let tenants = [
            ("acme", "Acme Robotics"),
            ("bravo", "Bravo Health"),
            ("cygnus", "Cygnus Finance"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (slug, display_name))| {
            SeedTenant::new(index + 1, namespace.as_str(), slug, display_name)
        })
        .collect::<TestResult<Vec<_>>>()?;

        Ok(Self { namespace, tenants })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn tenants(&self) -> &[SeedTenant] {
        &self.tenants
    }

    pub fn tenant(&self, slug: &str) -> Option<&SeedTenant> {
        self.tenants.iter().find(|tenant| tenant.slug == slug)
    }
}

#[derive(Clone, Debug)]
pub struct SeedTenant {
    index: usize,
    slug: &'static str,
    display_name: &'static str,
    tenant_id: TenantId,
    topic_key: String,
}

impl SeedTenant {
    fn new(
        index: usize,
        namespace: &str,
        slug: &'static str,
        display_name: &'static str,
    ) -> TestResult<Self> {
        Ok(Self {
            index,
            slug,
            display_name,
            tenant_id: TenantId::new(format!("tenant:{namespace}:{slug}"))?,
            topic_key: format!("tenant.{slug}.shared-backend"),
        })
    }

    pub fn slug(&self) -> &'static str {
        self.slug
    }

    pub fn display_name(&self) -> &'static str {
        self.display_name
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    pub fn topic_key(&self) -> &str {
        &self.topic_key
    }

    pub fn expected_decision_title(&self, decision_index: usize) -> String {
        format!("{} fixture decision {decision_index:03}", self.display_name)
    }
}

#[derive(Debug)]
pub struct SqliteMultiTenantFixture {
    dir: PathBuf,
    ledger: SqliteEventLedger,
}

impl SqliteMultiTenantFixture {
    pub fn create(label: &str) -> TestResult<Self> {
        let dir = unique_temp_dir(label);
        let ledger = SqliteEventLedger::open(&dir)?;
        Ok(Self { dir, ledger })
    }

    pub fn seed(&self) -> TestResult<MultiTenantSeed> {
        seed_multi_tenant_ledger_with_namespace(&self.ledger, "sqlite-fixture")
    }

    pub fn ledger(&self) -> &SqliteEventLedger {
        &self.ledger
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for SqliteMultiTenantFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub fn seed_multi_tenant_ledger<L: EventLedger + ?Sized>(
    ledger: &L,
) -> TestResult<MultiTenantSeed> {
    seed_multi_tenant_ledger_with_namespace(ledger, "default")
}

pub fn seed_multi_tenant_ledger_with_namespace<L: EventLedger + ?Sized>(
    ledger: &L,
    namespace: &str,
) -> TestResult<MultiTenantSeed> {
    let seed = MultiTenantSeed::new(namespace)?;
    for tenant in seed.tenants() {
        seed_tenant(ledger, tenant)?;
    }
    Ok(seed)
}

pub fn seed_tenant<L: EventLedger + ?Sized>(ledger: &L, tenant: &SeedTenant) -> TestResult<()> {
    for event in tenant_events(tenant) {
        ledger.append_for_tenant(tenant.tenant_id(), event)?;
    }
    Ok(())
}

pub fn project_tenant_graph<L: EventLedger>(
    ledger: &L,
    tenant: &SeedTenant,
) -> TestResult<MemoryGraph> {
    let graph = MemoryGraph::default();
    rebuild_graph_for_tenant(ledger, tenant.tenant_id(), &graph)?;
    Ok(graph)
}

pub fn assert_ledger_tenant_isolation<L: EventLedger + ?Sized>(
    ledger: &L,
    seed: &MultiTenantSeed,
) -> TestResult<()> {
    for tenant in seed.tenants() {
        let events = ledger.read_for_tenant(tenant.tenant_id(), 0, 10_000)?;
        require(!events.is_empty(), "tenant should have seeded events")?;
        require(
            events
                .iter()
                .all(|event| event.tenant_id == *tenant.tenant_id()),
            "tenant read returned another tenant's event",
        )?;
        require_eq(
            decision_count(&events),
            DECISIONS_PER_TENANT,
            "tenant decision count",
        )?;
        require(
            events.iter().any(|event| {
                decision_title(event).as_deref() == Some(tenant.expected_decision_title(1).as_str())
            }),
            "tenant missing expected shared decision title",
        )?;

        for other in seed.tenants() {
            if other.slug() != tenant.slug() {
                require(
                    events.iter().all(|event| {
                        decision_title(event).as_deref()
                            != Some(other.expected_decision_title(1).as_str())
                    }),
                    "tenant read leaked another decision title",
                )?;
            }
        }
    }

    Ok(())
}

pub fn assert_projected_tenant_isolation<L: EventLedger>(
    ledger: &L,
    seed: &MultiTenantSeed,
) -> TestResult<()> {
    for tenant in seed.tenants() {
        let graph = project_tenant_graph(ledger, tenant)?;
        let Some(decision) = get_decision(&graph, SHARED_DECISION_ID)?.data else {
            return Err("missing projected decision".into());
        };
        let expected_title = tenant.expected_decision_title(1);
        require_eq(
            &decision.title,
            &expected_title,
            "projected tenant decision title",
        )?;

        for other in seed.tenants() {
            if other.slug() != tenant.slug() {
                require_ne(
                    &decision.title,
                    &other.expected_decision_title(1),
                    "projected tenant decision leaked another title",
                )?;
            }
        }
    }

    Ok(())
}

#[cfg(feature = "shared-backend-postgres")]
pub mod postgres {
    use super::{
        assert_ledger_tenant_isolation, assert_projected_tenant_isolation,
        seed_multi_tenant_ledger_with_namespace, unique_namespace, MultiTenantSeed, TestResult,
    };
    use hivemind::ledger::PostgresEventLedger;

    pub const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

    pub struct PostgresMultiTenantFixture {
        ledger: PostgresEventLedger,
        seed: MultiTenantSeed,
    }

    impl PostgresMultiTenantFixture {
        pub fn connect(label: &str) -> TestResult<Option<Self>> {
            let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
            else {
                eprintln!("skipping Postgres multi-tenant fixture; set {TEST_DATABASE_URL_ENV}");
                return Ok(None);
            };

            let namespace = unique_namespace(label);
            let ledger = PostgresEventLedger::connect_with_pool_size(
                &database_url,
                format!("tenant:{namespace}:acme"),
                4,
            )?;
            let seed = seed_multi_tenant_ledger_with_namespace(&ledger, &namespace)?;
            Ok(Some(Self { ledger, seed }))
        }

        pub fn ledger(&self) -> &PostgresEventLedger {
            &self.ledger
        }

        pub fn seed(&self) -> &MultiTenantSeed {
            &self.seed
        }

        pub fn assert_isolated(&self) -> TestResult<()> {
            assert_ledger_tenant_isolation(&self.ledger, &self.seed)?;
            assert_projected_tenant_isolation(&self.ledger, &self.seed)
        }
    }
}

fn tenant_events(tenant: &SeedTenant) -> Vec<Event> {
    let mut builder = TenantEventBuilder::new(tenant);

    for (index, evidence_id) in EVIDENCE_IDS.into_iter().enumerate() {
        builder.evidence(index + 1, evidence_id);
    }

    for (index, hypothesis_id) in HYPOTHESIS_IDS.into_iter().enumerate() {
        builder.hypothesis(index + 1, hypothesis_id);
    }

    builder.relation(RelationKind::Supports, "evidence-001", "hypothesis-001");
    builder.relation(RelationKind::Refutes, "evidence-002", "hypothesis-001");
    builder.relation(RelationKind::Supports, "evidence-003", "hypothesis-002");

    for index in 1..=DECISIONS_PER_TENANT {
        let hypothesis_id = if index <= 5 {
            "hypothesis-001"
        } else {
            "hypothesis-002"
        };
        let evidence_id = match index % 3 {
            1 => "evidence-001",
            2 => "evidence-002",
            _ => "evidence-003",
        };
        builder.decision(index, hypothesis_id, evidence_id);
    }

    builder.accept(
        "decision-001",
        format!("human:{}:owner", tenant.slug()).as_str(),
    );
    builder.accept(
        "decision-002",
        format!("human:{}:owner", tenant.slug()).as_str(),
    );
    builder.reject(
        "decision-002",
        format!("agent:{}:reviewer", tenant.slug()).as_str(),
    );
    builder.reject(
        "decision-005",
        format!("agent:{}:reviewer", tenant.slug()).as_str(),
    );
    builder.supersede("decision-003", "decision-004");

    builder.events
}

fn decision_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| event.event_type == EventType::DecisionProposed)
        .count()
}

fn decision_title(event: &Event) -> Option<String> {
    if event.event_type != EventType::DecisionProposed {
        return None;
    }
    event
        .payload
        .get("title")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
}

struct TenantEventBuilder<'a> {
    tenant: &'a SeedTenant,
    events: Vec<Event>,
}

impl<'a> TenantEventBuilder<'a> {
    fn new(tenant: &'a SeedTenant) -> Self {
        Self {
            tenant,
            events: Vec::new(),
        }
    }

    fn evidence(&mut self, index: usize, evidence_id: &str) {
        self.push(
            EventType::EvidenceRecorded,
            format!("agent:{}:researcher", self.tenant.slug()).as_str(),
            json!({
                "evidence_id": evidence_id,
                "content": format!(
                    "{} evidence {index:03}: tenant-scoped backend fixture marker",
                    self.tenant.display_name()
                ),
                "source": "multi-tenant-fixture"
            }),
            None,
        );
    }

    fn hypothesis(&mut self, index: usize, hypothesis_id: &str) {
        self.push(
            EventType::HypothesisRecorded,
            format!("human:{}:architect", self.tenant.slug()).as_str(),
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": format!(
                    "{} hypothesis {index:03}: tenant scope remains isolated",
                    self.tenant.display_name()
                )
            }),
            None,
        );
    }

    fn decision(&mut self, index: usize, hypothesis_id: &str, evidence_id: &str) {
        let decision_id = format!("decision-{index:03}");
        let option_a = format!("option-{index:03}-a");
        let option_b = format!("option-{index:03}-b");
        let actor_id = format!("agent:{}:planner", self.tenant.slug());

        self.push(
            EventType::DecisionProposed,
            actor_id.as_str(),
            json!({
                "decision_id": decision_id,
                "title": self.tenant.expected_decision_title(index),
                "rationale": format!(
                    "{} rationale {index:03}: preserve tenant provenance and evidence links",
                    self.tenant.display_name()
                ),
                "topic_keys": [self.tenant.topic_key(), "multi-tenant-fixture"],
                "option_ids": [&option_a, &option_b],
                "chosen_option_id": &option_b,
                "hypothesis_ids": [hypothesis_id],
                "evidence_ids": [evidence_id]
            }),
            None,
        );
        self.relation_from_decision(RelationKind::HasOption, &decision_id, &option_a);
        self.relation_from_decision(RelationKind::HasOption, &decision_id, &option_b);
        self.relation_from_decision(RelationKind::Chose, &decision_id, &option_b);
        self.relation_from_decision(RelationKind::Assumes, &decision_id, hypothesis_id);
        self.relation_from_decision(RelationKind::BasedOn, &decision_id, evidence_id);
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
            format!("human:{}:architect", self.tenant.slug()).as_str(),
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id
            }),
            None,
        );
    }

    fn relation(&mut self, relation: RelationKind, from_id: &str, to_id: &str) {
        self.push(
            EventType::RelationAdded,
            format!("agent:{}:researcher", self.tenant.slug()).as_str(),
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id
            }),
            None,
        );
    }

    fn relation_from_decision(&mut self, relation: RelationKind, decision_id: &str, to_id: &str) {
        self.push(
            EventType::RelationAdded,
            format!("agent:{}:planner", self.tenant.slug()).as_str(),
            json!({
                "relation": relation,
                "from_id": decision_id,
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
            tenant_id: self.tenant.tenant_id().clone(),
            event_id: None,
            event_uuid: Uuid::from_u128(u128::try_from(sequence).unwrap_or(u128::MAX)),
            correlation_id: Some(format!("multi-tenant-fixture:{}", self.tenant.slug())),
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some("multi-tenant-fixture".to_owned()),
            payload,
            ts: Some(seed_timestamp(self.tenant.index, sequence)),
        });
    }
}

fn seed_timestamp(tenant_index: usize, sequence: usize) -> DateTime<Utc> {
    let tenant_seconds = i64::try_from(tenant_index).unwrap_or(0) * 900;
    let sequence_seconds = i64::try_from(sequence).unwrap_or(0);
    DateTime::from_timestamp(MULTI_TENANT_BASE_UNIX_SECONDS, 0)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        + Duration::seconds(tenant_seconds + sequence_seconds)
}

fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-multi-tenant-{label}-{}", Uuid::new_v4()))
}

#[cfg(feature = "shared-backend-postgres")]
fn unique_namespace(label: &str) -> String {
    format!("{label}:{}", Uuid::new_v4())
}

pub fn require(condition: bool, message: impl Into<String>) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

pub fn require_eq<T>(actual: T, expected: T, label: impl Into<String>) -> TestResult<()>
where
    T: std::fmt::Debug + PartialEq,
{
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{}: expected {expected:?}, got {actual:?}", label.into()).into())
    }
}

pub fn require_str_eq(actual: &str, expected: &str, label: impl Into<String>) -> TestResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{}: expected {expected:?}, got {actual:?}", label.into()).into())
    }
}

fn require_ne<T>(actual: &T, unexpected: &T, label: impl Into<String>) -> TestResult<()>
where
    T: std::fmt::Debug + PartialEq,
{
    if actual != unexpected {
        Ok(())
    } else {
        Err(format!("{}: unexpected {actual:?}", label.into()).into())
    }
}
