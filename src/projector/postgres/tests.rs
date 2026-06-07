// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::commands::{CommandContext, Commands, DecisionProposalEventUuids};
use crate::events::{EventProvenance, TenantId};
use crate::ledger::PostgresEventLedger;
use crate::projector::rebuild_graph_for_tenant;
use crate::queries::{
    get_decision, get_relevant_decisions, search_decisions, DecisionStatus, SearchDecisionRequest,
};
use crate::ProjectorError;
use crate::Result;

use super::PostgresGraphView;

const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

#[test]
fn postgres_projection_answers_existing_query_surface() -> Result<()> {
    with_postgres_projection("projection-query", |ledger, graph, tenant_id| {
        seed_decision(
            ledger,
            tenant_id,
            "decision:postgres-query",
            "Postgres projection read path",
            "Durable projection keeps layer two queries deterministic",
            "postgres",
        )?;

        rebuild_graph_for_tenant(ledger, tenant_id, graph)?;

        let decision = get_decision(graph, "decision:postgres-query")?
            .data
            .ok_or_else(|| test_error("decision missing from Postgres projection"))?;
        if decision.status != DecisionStatus::Accepted {
            return Err(test_error("accepted decision status was not derived"));
        }
        if decision.option_ids != vec!["option:postgres-query".to_owned()] {
            return Err(test_error("decision options were not projected"));
        }
        if decision.evidence_ids != vec!["evidence:postgres-query".to_owned()] {
            return Err(test_error("decision evidence was not projected"));
        }
        if decision
            .hypotheses
            .first()
            .map(|hypothesis| hypothesis.id.as_str())
            != Some("hypothesis:postgres-query")
        {
            return Err(test_error("decision hypothesis was not projected"));
        }

        let relevant = get_relevant_decisions(graph, "postgres", None)?.data;
        if relevant
            .iter()
            .map(|decision| decision.id.as_str())
            .collect::<Vec<_>>()
            != vec!["decision:postgres-query"]
        {
            return Err(test_error("topic query did not return projected decision"));
        }

        let search = search_decisions(
            graph,
            &SearchDecisionRequest {
                query: Some("durable deterministic".to_owned()),
                limit: 10,
                ..SearchDecisionRequest::default()
            },
        )?
        .data;
        if search
            .items
            .iter()
            .map(|item| item.decision.id.as_str())
            .collect::<Vec<_>>()
            != vec!["decision:postgres-query"]
        {
            return Err(test_error("graph search did not see Postgres projection"));
        }

        Ok(())
    })
}

#[test]
fn postgres_projection_isolates_tenants() -> Result<()> {
    let Some(database_url) = test_database_url() else {
        eprintln!("skipping Postgres projection test; set {TEST_DATABASE_URL_ENV}");
        return Ok(());
    };

    let tenant_a = unique_tenant("projection-tenant-a")?;
    let tenant_b = unique_tenant("projection-tenant-b")?;
    let ledger_a =
        PostgresEventLedger::connect_with_pool_size(&database_url, tenant_a.as_str(), 4)?;
    let ledger_b = ledger_a.for_tenant(tenant_b.as_str())?;
    let graph_a = PostgresGraphView::connect_with_pool_size(&database_url, tenant_a.as_str(), 4)?;
    let graph_b = graph_a.for_tenant(tenant_b.as_str())?;

    seed_decision(
        &ledger_a,
        &tenant_a,
        "decision:tenant-a",
        "Tenant A decision",
        "Tenant A rationale",
        "tenant-a",
    )?;
    seed_decision(
        &ledger_b,
        &tenant_b,
        "decision:tenant-b",
        "Tenant B decision",
        "Tenant B rationale",
        "tenant-b",
    )?;

    rebuild_graph_for_tenant(&ledger_a, &tenant_a, &graph_a)?;
    rebuild_graph_for_tenant(&ledger_b, &tenant_b, &graph_b)?;

    if get_decision(&graph_a, "decision:tenant-a")?.data.is_none() {
        return Err(test_error("tenant A decision missing from tenant A graph"));
    }
    if get_decision(&graph_b, "decision:tenant-b")?.data.is_none() {
        return Err(test_error("tenant B decision missing from tenant B graph"));
    }
    if get_decision(&graph_a, "decision:tenant-b")?.data.is_some() {
        return Err(test_error("tenant A graph leaked tenant B decision"));
    }
    if get_decision(&graph_b, "decision:tenant-a")?.data.is_some() {
        return Err(test_error("tenant B graph leaked tenant A decision"));
    }

    Ok(())
}

fn with_postgres_projection(
    prefix: &str,
    test: impl FnOnce(&PostgresEventLedger, &PostgresGraphView, &TenantId) -> Result<()>,
) -> Result<()> {
    let Some(database_url) = test_database_url() else {
        eprintln!("skipping Postgres projection test; set {TEST_DATABASE_URL_ENV}");
        return Ok(());
    };

    let tenant_id = unique_tenant(prefix)?;
    let ledger = PostgresEventLedger::connect_with_pool_size(&database_url, tenant_id.as_str(), 4)?;
    let graph = PostgresGraphView::connect_with_pool_size(&database_url, tenant_id.as_str(), 4)?;
    test(&ledger, &graph, &tenant_id)
}

fn seed_decision(
    ledger: &PostgresEventLedger,
    tenant_id: &TenantId,
    decision_id: &str,
    title: &str,
    rationale: &str,
    topic: &str,
) -> Result<()> {
    let commands = Commands::new_with_context(
        ledger,
        CommandContext::new(
            tenant_id.clone(),
            EventProvenance::api(Some("postgres-projection-test".to_owned())),
        ),
    );
    let suffix = decision_id
        .rsplit(':')
        .next()
        .ok_or_else(|| test_error("decision id missing suffix"))?;
    let evidence_id = format!("evidence:{suffix}");
    let hypothesis_id = format!("hypothesis:{suffix}");
    let option_id = format!("option:{suffix}");

    commands.record_evidence_with_id(
        "actor:alice",
        &evidence_id,
        "Postgres projector preserves evidence content",
        None,
        Uuid::new_v4(),
    )?;
    commands.record_hypothesis_with_id(
        "actor:alice",
        &hypothesis_id,
        "Postgres graph projection is query-compatible",
        Uuid::new_v4(),
    )?;
    commands.record_option_with_id(
        "actor:alice",
        &option_id,
        "Use Postgres projection",
        "Materialize decision graph rows in Postgres",
    )?;
    commands.propose_decision_with_id(
        "actor:alice",
        decision_id,
        title,
        rationale,
        &[topic.to_owned()],
        std::slice::from_ref(&option_id),
        Some(option_id.as_str()),
        std::slice::from_ref(&hypothesis_id),
        std::slice::from_ref(&evidence_id),
        DecisionProposalEventUuids {
            proposal: Uuid::new_v4(),
            has_option: vec![Uuid::new_v4()],
            chose: Some(Uuid::new_v4()),
            assumes: vec![Uuid::new_v4()],
            based_on: vec![Uuid::new_v4()],
        },
    )?;
    commands.accept_decision(decision_id, "actor:bob")?;
    Ok(())
}

fn test_database_url() -> Option<String> {
    std::env::var(TEST_DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn unique_tenant(prefix: &str) -> Result<TenantId> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    TenantId::new(format!(
        "tenant:test:{prefix}:{nanos}:{}",
        std::process::id()
    ))
    .map_err(|error| test_error(format!("invalid tenant id: {error}")))
}

fn test_error(message: impl Into<String>) -> crate::HivemindError {
    ProjectorError::Projection(message.into()).into()
}
