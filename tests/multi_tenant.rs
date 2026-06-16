use hivemind::ledger::{EventLedger, InMemoryEventLedger};

#[path = "support/multi_tenant.rs"]
mod multi_tenant;

use multi_tenant::{
    assert_graph_completeness, assert_tenant_completeness, assert_tenant_isolation, seed_tenant,
    TestResult,
};

const TENANT_ALPHA: &str = "alpha-corp";
const TENANT_BETA: &str = "beta-startup";
const TENANT_GAMMA: &str = "gamma-labs";

fn seeded_ledger() -> TestResult<(
    InMemoryEventLedger,
    multi_tenant::TenantDataset,
    multi_tenant::TenantDataset,
    multi_tenant::TenantDataset,
)> {
    let ledger = InMemoryEventLedger::new();
    let alpha = seed_tenant(&ledger, TENANT_ALPHA)?;
    let beta = seed_tenant(&ledger, TENANT_BETA)?;
    let gamma = seed_tenant(&ledger, TENANT_GAMMA)?;
    Ok((ledger, alpha, beta, gamma))
}

#[test]
fn each_tenant_has_ten_decisions() -> TestResult<()> {
    let (_, alpha, beta, gamma) = seeded_ledger()?;
    assert_eq!(
        // ubs:ignore
        alpha.decision_ids.len(),
        10,
        "alpha should have 10 decisions"
    );
    assert_eq!(beta.decision_ids.len(), 10, "beta should have 10 decisions"); // ubs:ignore
    assert_eq!(
        // ubs:ignore
        gamma.decision_ids.len(),
        10,
        "gamma should have 10 decisions"
    );
    Ok(())
}

#[test]
fn each_tenant_can_read_its_own_events() -> TestResult<()> {
    let (ledger, alpha, beta, gamma) = seeded_ledger()?;
    assert_tenant_completeness(&ledger, &alpha)?;
    assert_tenant_completeness(&ledger, &beta)?;
    assert_tenant_completeness(&ledger, &gamma)?;
    Ok(())
}

#[test]
fn alpha_events_are_isolated_from_beta_and_gamma() -> TestResult<()> {
    let (ledger, alpha, beta, gamma) = seeded_ledger()?;
    assert_tenant_isolation(&ledger, &alpha, &beta)?;
    assert_tenant_isolation(&ledger, &alpha, &gamma)?;
    Ok(())
}

#[test]
fn beta_events_are_isolated_from_alpha_and_gamma() -> TestResult<()> {
    let (ledger, alpha, beta, gamma) = seeded_ledger()?;
    assert_tenant_isolation(&ledger, &beta, &alpha)?;
    assert_tenant_isolation(&ledger, &beta, &gamma)?;
    Ok(())
}

#[test]
fn gamma_events_are_isolated_from_alpha_and_beta() -> TestResult<()> {
    let (ledger, alpha, beta, gamma) = seeded_ledger()?;
    assert_tenant_isolation(&ledger, &gamma, &alpha)?;
    assert_tenant_isolation(&ledger, &gamma, &beta)?;
    Ok(())
}

#[test]
fn rebuilt_graph_for_each_tenant_contains_only_own_decisions() -> TestResult<()> {
    let (ledger, alpha, beta, gamma) = seeded_ledger()?;
    assert_graph_completeness(&ledger, &alpha)?;
    assert_graph_completeness(&ledger, &beta)?;
    assert_graph_completeness(&ledger, &gamma)?;
    Ok(())
}

#[test]
fn each_tenant_has_evidence_and_hypothesis() -> TestResult<()> {
    let (_, alpha, beta, gamma) = seeded_ledger()?;
    for dataset in [&alpha, &beta, &gamma] {
        assert!(
            // ubs:ignore
            !dataset.evidence_id.is_empty(),
            "tenant '{}' should have an evidence_id",
            dataset.name,
        );
        assert!(
            // ubs:ignore
            !dataset.hypothesis_id.is_empty(),
            "tenant '{}' should have a hypothesis_id",
            dataset.name,
        );
        assert!(
            // ubs:ignore
            dataset.evidence_id.starts_with("evidence-"),
            "evidence_id for '{}' should start with 'evidence-'",
            dataset.name,
        );
        assert!(
            // ubs:ignore
            dataset.hypothesis_id.starts_with("hypothesis-"),
            "hypothesis_id for '{}' should start with 'hypothesis-'",
            dataset.name,
        );
    }
    Ok(())
}

#[test]
fn tenant_ids_are_distinct() -> TestResult<()> {
    let (_, alpha, beta, gamma) = seeded_ledger()?;
    assert_ne!(alpha.tenant_id, beta.tenant_id); // ubs:ignore
    assert_ne!(beta.tenant_id, gamma.tenant_id); // ubs:ignore
    assert_ne!(alpha.tenant_id, gamma.tenant_id); // ubs:ignore
    Ok(())
}

#[test]
fn local_tenant_is_isolated_from_named_tenants() -> TestResult<()> {
    use hivemind::commands::{CommandContext, Commands};
    use hivemind::events::{EventProvenance, TenantId};

    let ledger = InMemoryEventLedger::new();

    let alpha = seed_tenant(&ledger, TENANT_ALPHA)?;

    let local_context = CommandContext::local(EventProvenance::cli());
    let local_cmds = Commands::new_with_context(&ledger, local_context);
    let local_opt = local_cmds.record_option(
        "human:local-planner",
        "Local option A",
        "Local decision option A",
    )?;
    let local_opt_b = local_cmds.record_option(
        "human:local-planner",
        "Local option B",
        "Local decision option B",
    )?;
    local_cmds.propose_decision(
        "human:local-planner",
        "Local decision: pick infra",
        "Only the local tenant should see this",
        &["architecture".to_owned()],
        &[local_opt, local_opt_b.clone()],
        Some(&local_opt_b),
        &[],
        &[],
    )?;

    let local_dataset = multi_tenant::TenantDataset {
        tenant_id: TenantId::local(),
        name: "local",
        decision_ids: vec!["local-decision-placeholder".to_owned()],
        evidence_id: String::new(),
        hypothesis_id: String::new(),
    };

    // Alpha should not see local's events.
    let alpha_events = ledger.read_for_tenant(&alpha.tenant_id, 0, 1_000)?;
    for event in &alpha_events {
        assert_eq!(
            // ubs:ignore
            event.tenant_id,
            alpha.tenant_id,
            "local event leaked into alpha event stream",
        );
    }

    // Local should not see alpha's decisions.
    let local_events = ledger.read_for_tenant(&TenantId::local(), 0, 1_000)?;
    let local_decision_ids: std::collections::HashSet<&str> = local_events
        .iter()
        .filter_map(|e| e.payload.get("decision_id").and_then(|v| v.as_str()))
        .collect();
    for alpha_id in &alpha.decision_ids {
        assert!(
            // ubs:ignore
            !local_decision_ids.contains(alpha_id.as_str()),
            "alpha decision '{}' leaked into local tenant stream",
            alpha_id,
        );
    }

    drop(local_dataset);
    Ok(())
}
