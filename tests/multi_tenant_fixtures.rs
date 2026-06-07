use hivemind::events::EventType;
use hivemind::ledger::{EventLedger, InMemoryEventLedger};

#[path = "support/multi_tenant.rs"]
mod multi_tenant;

use multi_tenant::{
    assert_ledger_tenant_isolation, assert_projected_tenant_isolation, require, require_eq,
    require_str_eq, seed_multi_tenant_ledger, MultiTenantSeed, SqliteMultiTenantFixture,
    DECISIONS_PER_TENANT,
};

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn multi_tenant_seed_dataset_has_three_tenants_with_ten_decisions_each() -> TestResult<()> {
    let seed = MultiTenantSeed::new("shape-test")?;

    require_eq(seed.namespace(), "shape-test", "fixture namespace")?;
    require_eq(seed.tenants().len(), 3, "tenant count")?;
    require_eq(
        seed.tenant("acme").map(|tenant| tenant.display_name()),
        Some("Acme Robotics"),
        "tenant lookup",
    )?;

    const EXPECTED_TITLES: [(&str, &str); 3] = [
        ("acme", "Acme Robotics fixture decision 001"),
        ("bravo", "Bravo Health fixture decision 001"),
        ("cygnus", "Cygnus Finance fixture decision 001"),
    ];
    for tenant in seed.tenants() {
        require(
            tenant
                .tenant_id()
                .as_str()
                .starts_with("tenant:shape-test:"),
            "tenant id includes namespace",
        )?;
        let expected_title = EXPECTED_TITLES
            .iter()
            .find(|(slug, _)| *slug == tenant.slug())
            .map(|(_, title)| *title)
            .ok_or("missing expected title")?;
        let actual_title = tenant.expected_decision_title(1);
        require_str_eq(
            actual_title.as_str(),
            expected_title,
            "tenant decision title",
        )?;
    }

    Ok(())
}

#[test]
fn in_memory_fixture_seeds_tenants_with_overlapping_entity_ids() -> TestResult<()> {
    let ledger = InMemoryEventLedger::new();
    let seed = seed_multi_tenant_ledger(&ledger)?;

    for tenant in seed.tenants() {
        let events = ledger.read_for_tenant(tenant.tenant_id(), 0, 10_000)?;
        let decision_count = events
            .iter()
            .filter(|event| event.event_type == EventType::DecisionProposed)
            .count();
        require_eq(
            decision_count,
            DECISIONS_PER_TENANT,
            "tenant decision count",
        )?;
        require(
            events.iter().any(|event| {
                event.event_type == EventType::DecisionProposed
                    && event
                        .payload
                        .get("decision_id")
                        .and_then(|value| value.as_str())
                        == Some("decision-001")
            }),
            "tenant includes overlapping shared decision id",
        )?;
    }

    assert_ledger_tenant_isolation(&ledger, &seed)?;
    Ok(())
}

#[test]
fn sqlite_fixture_asserts_ledger_and_projection_isolation() -> TestResult<()> {
    let fixture = SqliteMultiTenantFixture::create("tenant-isolation")?;
    let seed = fixture.seed()?;

    require(
        fixture.path().join("ledger.sqlite").exists(),
        "sqlite fixture created ledger file",
    )?;
    assert_ledger_tenant_isolation(fixture.ledger(), &seed)?;
    assert_projected_tenant_isolation(fixture.ledger(), &seed)?;

    Ok(())
}

#[cfg(feature = "shared-backend-postgres")]
#[test]
fn postgres_fixture_asserts_ledger_and_projection_isolation() -> TestResult<()> {
    if let Some(fixture) =
        multi_tenant::postgres::PostgresMultiTenantFixture::connect("postgres-fixture")?
    {
        require_eq(fixture.seed().tenants().len(), 3, "postgres tenant count")?;
        require(
            !fixture.ledger().tenant_id().is_empty(),
            "postgres fixture tenant id is set",
        )?;
        fixture.assert_isolated()?;
    }

    Ok(())
}
