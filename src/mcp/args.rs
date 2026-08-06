//! Shared JSON-RPC arg-extraction helpers for MCP tools.
//!
//! Used by both the stdio transport (`mcp.rs`) and the HTTP transport (`api.rs`).
//! Functions return `Result<_, (i32, String)>` — the raw JSON-RPC (code, message)
//! pair — so callers in `api.rs` (which use `McpToolResult = Result<Value, (i32, String)>`)
//! pay no conversion overhead.  The stdio transport bridges via
//! `From<(i32, String)> for RpcError`.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

pub(crate) const INVALID_PARAMS: i32 = -32602;

/// Default description injected when an MCP option object omits `description`.
pub(crate) fn default_option_description(label: &str) -> String {
    format!("Option generated from MCP value '{label}'")
}

pub(crate) fn require_string(
    args: &Map<String, Value>,
    field: &str,
) -> Result<String, (i32, String)> {
    match args.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err((
            INVALID_PARAMS,
            format!("`{field}` must be a non-empty string"),
        )),
        Some(_) => Err((INVALID_PARAMS, format!("`{field}` must be a string"))),
        None => Err((INVALID_PARAMS, format!("missing `{field}`"))),
    }
}

pub(crate) fn optional_string(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, (i32, String)> {
    match args.get(field) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) | None | Some(Value::Null) => Ok(None),
        Some(_) => Err((INVALID_PARAMS, format!("`{field}` must be a string"))),
    }
}

pub(crate) fn require_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        Some(Value::Array(items)) => collect_strings(items, field),
        Some(_) => Err((
            INVALID_PARAMS,
            format!("`{field}` must be an array of strings"),
        )),
        None => Err((INVALID_PARAMS, format!("missing `{field}`"))),
    }
}

pub(crate) fn optional_string_array(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => collect_strings(items, field),
        Some(_) => Err((
            INVALID_PARAMS,
            format!("`{field}` must be an array of strings"),
        )),
    }
}

pub(crate) fn optional_option_labels(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Vec<String>, (i32, String)> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| match item {
                Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
                Value::Object(map) => map
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        (
                            INVALID_PARAMS,
                            format!("`{field}[{index}].label` must be a non-empty string"),
                        )
                    }),
                _ => Err((
                    INVALID_PARAMS,
                    format!(
                        "`{field}[{index}]` must be a non-empty string or an object with a non-empty label"
                    ),
                )),
            })
            .collect(),
        Some(_) => Err((INVALID_PARAMS, format!("`{field}` must be an array"))),
    }
}

pub(crate) fn optional_usize(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Option<usize>, (i32, String)> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_u64() else {
                return Err((
                    INVALID_PARAMS,
                    format!("`{field}` must be a non-negative integer"),
                ));
            };
            usize::try_from(value)
                .map(Some)
                .map_err(|error| (INVALID_PARAMS, format!("`{field}` is too large: {error}")))
        }
        Some(_) => Err((INVALID_PARAMS, format!("`{field}` must be an integer"))),
    }
}

pub(crate) fn optional_datetime(
    args: &Map<String, Value>,
    field: &str,
) -> Result<Option<DateTime<Utc>>, (i32, String)> {
    let Some(value) = optional_string(args, field)? else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| {
            (
                INVALID_PARAMS,
                format!("`{field}` must be RFC3339: {error}"),
            )
        })
}

pub(crate) fn collect_strings(items: &[Value], field: &str) -> Result<Vec<String>, (i32, String)> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(s) if !s.trim().is_empty() => Ok(s.clone()),
            _ => Err((
                INVALID_PARAMS,
                format!("`{field}[{index}]` must be a non-empty string"),
            )),
        })
        .collect()
}
