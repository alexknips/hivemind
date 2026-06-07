// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only checks.
#[cfg(test)]
use super::*;
use crate::events::{EventSource, TenantId};
use crate::ledger::EventLedger;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::other(message.into()).into()
}

fn unique_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("hivemind-http-{label}-{}", uuid::Uuid::new_v4()));
    dir
}

fn test_config(dir: &PathBuf) -> TestResult<HttpConfig> {
    Ok(HttpConfig::new(dir, "127.0.0.1:0".parse()?))
}

async fn rpc_call(
    app: Router,
    body: Value,
    tenant: Option<&str>,
    actor: Option<&str>,
) -> TestResult<(StatusCode, Value)> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/rpc")
        .header("Content-Type", "application/json");
    if let Some(tenant) = tenant {
        builder = builder.header(TENANT_HEADER, tenant);
    }
    if let Some(actor) = actor {
        builder = builder.header(ACTOR_HEADER, actor);
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_string()))?)
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let body = serde_json::from_slice::<Value>(&bytes)?;
    Ok((status, body))
}

fn check_eq<T>(actual: T, expected: T, label: &str) -> TestResult
where
    T: std::fmt::Debug + PartialEq,
{
    if actual == expected {
        return Ok(());
    }
    Err(test_error(format!(
        "{label}: expected {expected:?}, got {actual:?}"
    )))
}

fn check_contains(haystack: &str, needle: &str, label: &str) -> TestResult {
    if haystack.contains(needle) {
        return Ok(());
    }
    Err(test_error(format!(
        "{label}: expected {haystack:?} to contain {needle:?}"
    )))
}

fn json_path<'a>(value: &'a Value, path: &[&str]) -> TestResult<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current
            .get(*segment)
            .ok_or_else(|| test_error("missing JSON path segment"))?;
    }
    Ok(current)
}

fn json_string<'a>(value: &'a Value, path: &[&str]) -> TestResult<&'a str> {
    json_path(value, path)?
        .as_str()
        .ok_or_else(|| test_error(format!("JSON path {} is not a string", path.join("."))))
}

#[tokio::test]
async fn rpc_capture_then_get_round_trips_with_header_context() -> TestResult {
    let dir = unique_dir("roundtrip");
    let app = router(test_config(&dir)?);

    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "emit.decision.capture",
        "params": {
            "title": "Expose HTTP API",
            "rationale": "Agents need a third thin transport over the same commands",
            "topic_keys": ["api"],
            "options": [
                {"label": "json-rpc"},
                {"label": "rest"}
            ],
            "chosen_option_label": "json-rpc"
        }
    });
    let (status, response) = rpc_call(
        app.clone(),
        capture,
        Some("tenant:http"),
        Some("agent:http:test"),
    )
    .await?;
    check_eq(status, StatusCode::OK, "capture status")?;
    let decision_id = json_string(&response, &["result", "value"])?.to_owned();

    let fetch = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "query.get_decision",
        "params": { "decision_id": decision_id }
    });
    let (status, response) =
        rpc_call(app, fetch, Some("tenant:http"), Some("agent:http:test")).await?;
    check_eq(status, StatusCode::OK, "fetch status")?;
    check_eq(
        json_string(&response, &["result", "data", "id"])?,
        decision_id.as_str(),
        "fetched decision id",
    )?;
    check_eq(
        json_string(&response, &["result", "data", "title"])?,
        "Expose HTTP API",
        "fetched decision title",
    )?;

    let ledger = SqliteEventLedger::open(&dir)?;
    let tenant_id = TenantId::new("tenant:http")?;
    let events = ledger.read_for_tenant(&tenant_id, 0, 16)?;
    let proposal = events
        .iter()
        .find(|event| event.event_type == crate::events::EventType::DecisionProposed)
        .ok_or_else(|| test_error("missing proposal event"))?;
    check_eq(
        proposal.tenant_id.as_str(),
        "tenant:http",
        "proposal tenant",
    )?;
    check_eq(
        proposal.actor_id.as_str(),
        "agent:http:test",
        "proposal actor",
    )?;
    check_eq(proposal.source, EventSource::Api, "proposal source")?;
    check_eq(
        proposal.source_ref.as_deref(),
        Some("agent:http:test"),
        "proposal source_ref",
    )?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn rpc_requires_actor_and_tenant_headers() -> TestResult {
    let dir = unique_dir("auth");
    let app = router(test_config(&dir)?);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "capture_evidence",
        "params": { "content": "auth boundary evidence" }
    });

    let (status, response) =
        rpc_call(app.clone(), request.clone(), None, Some("agent:http:test")).await?;
    check_eq(status, StatusCode::UNAUTHORIZED, "missing tenant status")?;
    check_eq(
        json_path(&response, &["error", "code"])?,
        &json!(JSONRPC_UNAUTHORIZED),
        "missing tenant error",
    )?;

    let (status, response) = rpc_call(app, request, Some("tenant:http"), None).await?;
    check_eq(status, StatusCode::UNAUTHORIZED, "missing actor status")?;
    check_eq(
        json_path(&response, &["error", "code"])?,
        &json!(JSONRPC_UNAUTHORIZED),
        "missing actor error",
    )?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn rpc_rejects_payload_actor_that_differs_from_header_actor() -> TestResult {
    let dir = unique_dir("actor-mismatch");
    let app = router(test_config(&dir)?);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "capture_evidence",
        "params": {
            "actor_id": "agent:http:payload",
            "content": "mismatched actor"
        }
    });

    let (status, response) =
        rpc_call(app, request, Some("tenant:http"), Some("agent:http:header")).await?;
    check_eq(status, StatusCode::OK, "actor mismatch status")?;
    check_eq(
        json_path(&response, &["error", "code"])?,
        &json!(JSONRPC_INVALID_PARAMS),
        "actor mismatch code",
    )?;
    check_contains(
        json_string(&response, &["error", "message"])?,
        "X-HiveMind-Actor",
        "actor mismatch message",
    )?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn rpc_queries_are_tenant_scoped() -> TestResult {
    let dir = unique_dir("tenant");
    let app = router(test_config(&dir)?);
    let capture = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "capture_decision",
        "params": {
            "title": "Tenant scoped decision",
            "rationale": "HTTP must not leak across tenants",
            "topic_keys": ["tenant"],
            "options": ["scoped"]
        }
    });
    let (_, response) = rpc_call(
        app.clone(),
        capture,
        Some("tenant:a"),
        Some("agent:http:test"),
    )
    .await?;
    let decision_id = json_string(&response, &["result", "decision_id"])?.to_owned();

    let fetch = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "get_decision",
        "params": { "decision_id": decision_id }
    });
    let (status, response) =
        rpc_call(app, fetch, Some("tenant:b"), Some("agent:http:test")).await?;
    check_eq(status, StatusCode::OK, "tenant scoped query status")?;
    check_eq(
        json_path(&response, &["result", "data"])?,
        &Value::Null,
        "tenant scoped query data",
    )?;

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
