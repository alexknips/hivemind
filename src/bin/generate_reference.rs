//! Reference doc generator for HiveMind MCP tools and CLI commands.
//!
//! Generates `website/src/content/docs/reference/mcp-tools.md` from
//! `mcp::tool_definitions()` and validates that `cli.md` documents every
//! `EmitCommand` and `QueryCommand` variant.
//!
//! Usage:
//!   cargo run --bin generate-reference            # write / update docs
//!   cargo run --bin generate-reference -- --check  # exit 1 if docs are stale
//!
//! The canonical source of truth is `src/mcp.rs::tool_definitions()` for MCP
//! and `src/cli/args.rs` (via clap) for the CLI. Re-run this binary after
//! changing either to keep the checked-in docs in sync.

use std::fmt::Write as FmtWrite;

use clap::CommandFactory;
use hivemind::cli::Cli;
use hivemind::mcp::tool_definitions;
use serde_json::Value;

// Write tools modify the ledger; everything else is a read (or layer-3) tool.
const WRITE_TOOLS: &[&str] = &[
    "capture_decision",
    "capture_evidence",
    "capture_hypothesis",
    "disagree_decision",
    "supersede_decision",
];

fn main() {
    let check_mode = std::env::args().any(|a| a == "--check");

    let mcp_md = generate_mcp_reference();
    let cli_missing = check_cli_completeness();

    let mcp_path = "website/src/content/docs/reference/mcp-tools.md";

    if check_mode {
        let current = std::fs::read_to_string(mcp_path).unwrap_or_else(|e| {
            eprintln!("Cannot read {mcp_path}: {e}");
            std::process::exit(1)
        });
        let mut failed = false;

        if current != mcp_md {
            eprintln!(
                "STALE: {mcp_path} does not match tool_definitions().\n\
                 Run: cargo run --bin generate-reference"
            );
            failed = true;
        } else {
            println!("OK: {mcp_path} matches tool_definitions().");
        }

        if !cli_missing.is_empty() {
            eprintln!(
                "INCOMPLETE: cli.md is missing documentation for {} CLI subcommand(s):",
                cli_missing.len()
            );
            for name in &cli_missing {
                eprintln!("  - {name}");
            }
            eprintln!(
                "Add each missing subcommand to\n\
                 website/src/content/docs/reference/cli.md"
            );
            failed = true;
        } else {
            println!("OK: cli.md documents all emit and query subcommands.");
        }

        if failed {
            std::process::exit(1);
        }
    } else {
        std::fs::write(mcp_path, &mcp_md).unwrap_or_else(|e| {
            eprintln!("Cannot write {mcp_path}: {e}");
            std::process::exit(1)
        });
        println!("Generated: {mcp_path}");

        if !cli_missing.is_empty() {
            eprintln!(
                "WARNING: cli.md is missing {} subcommand(s). Add them manually:",
                cli_missing.len()
            );
            for name in &cli_missing {
                eprintln!("  - {name}");
            }
        } else {
            println!("OK: cli.md documents all emit and query subcommands.");
        }
    }
}

// ---------------------------------------------------------------------------
// MCP reference generator
// ---------------------------------------------------------------------------

fn generate_mcp_reference() -> String {
    let tools = tool_definitions();
    let count = tools.len();
    let mut out = String::new();

    writeln!(out, "---").unwrap();
    writeln!(out, "title: MCP Tools").unwrap();
    writeln!(
        out,
        "description: Reference for all {count} tools exposed by the HiveMind MCP server."
    )
    .unwrap();
    writeln!(out, "---").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "The HiveMind MCP server exposes {count} tools. Write tools append events to the"
    )
    .unwrap();
    writeln!(
        out,
        "ledger and require an explicit `actor_id`. Read tools query the graph and never"
    )
    .unwrap();
    writeln!(
        out,
        "write. Layer-3 tools add ranked summaries or compact views."
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "See [MCP Setup](/guides/mcp-setup/) to configure your client."
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "---").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "## Write tools").unwrap();
    writeln!(out).unwrap();

    for tool in &tools {
        let name = tool["name"].as_str().unwrap_or_default();
        if WRITE_TOOLS.contains(&name) {
            append_tool_section(&mut out, tool);
        }
    }

    writeln!(out, "## Read tools").unwrap();
    writeln!(out).unwrap();

    for tool in &tools {
        let name = tool["name"].as_str().unwrap_or_default();
        if !WRITE_TOOLS.contains(&name) {
            append_tool_section(&mut out, tool);
        }
    }

    append_error_section(&mut out);
    out
}

fn append_tool_section(out: &mut String, tool: &Value) {
    let name = tool["name"].as_str().unwrap_or_default();
    let description = tool["description"].as_str().unwrap_or_default();
    let schema = &tool["inputSchema"];
    let required_arr = schema["required"].as_array();
    let required_names: Vec<&str> = required_arr
        .iter()
        .flat_map(|a| a.iter())
        .filter_map(|v| v.as_str())
        .collect();
    let properties = schema["properties"].as_object();

    writeln!(out, "### `{name}`").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "{description}").unwrap();
    writeln!(out).unwrap();

    if let Some(props) = properties {
        if !props.is_empty() {
            writeln!(out, "**Parameters:**").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "| Parameter | Type | Required | Description |").unwrap();
            writeln!(out, "|-----------|------|----------|-------------|").unwrap();

            // Required parameters first, then optional.
            for pass in [true, false] {
                for (param, def) in props.iter() {
                    let is_req = required_names.contains(&param.as_str());
                    if is_req != pass {
                        continue;
                    }
                    let type_str = format_type(def);
                    let req_marker = if is_req { "✓" } else { "—" };
                    let desc = def["description"].as_str().unwrap_or("");
                    writeln!(out, "| `{param}` | {type_str} | {req_marker} | {desc} |").unwrap();
                }
            }
            writeln!(out).unwrap();
        }
    }

    writeln!(out, "---").unwrap();
    writeln!(out).unwrap();
}

fn format_type(def: &Value) -> String {
    match def["type"].as_str() {
        Some("string") => "string".to_string(),
        Some("integer") => "integer".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("object") => "object".to_string(),
        Some("array") => {
            let item_type = match def["items"]["type"].as_str() {
                Some("string") => "string",
                Some("integer") => "integer",
                Some("object") => "object",
                _ => "any",
            };
            format!("{item_type}[]")
        }
        _ => "any".to_string(),
    }
}

fn append_error_section(out: &mut String) {
    writeln!(out, "## Error handling").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "All tools return a standard error envelope on failure:"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "```json").unwrap();
    writeln!(out, "{{").unwrap();
    writeln!(out, r#"  "error": {{"#).unwrap();
    writeln!(out, r#"    "code": "ACTOR_REQUIRED","#).unwrap();
    writeln!(
        out,
        r#"    "message": "actor_id is required for all write operations""#
    )
    .unwrap();
    writeln!(out, "  }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out, "```").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "Common error codes:").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "| Code | Meaning |").unwrap();
    writeln!(out, "|------|---------|").unwrap();
    writeln!(
        out,
        "| `ACTOR_REQUIRED` | Write tool called without `actor_id` |"
    )
    .unwrap();
    writeln!(
        out,
        "| `DECISION_NOT_FOUND` | ID does not exist in the ledger |"
    )
    .unwrap();
    writeln!(
        out,
        "| `SUPERSESSION_CYCLE` | `supersedes_id` would create a cycle |"
    )
    .unwrap();
    writeln!(
        out,
        "| `INVALID_TOPIC_KEY` | Topic key contains invalid characters |"
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// CLI completeness check
// ---------------------------------------------------------------------------

/// Return the names of emit + query subcommands not found in cli.md.
fn check_cli_completeness() -> Vec<String> {
    let cli_path = "website/src/content/docs/reference/cli.md";
    let cli_md = match std::fs::read_to_string(cli_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Cannot read {cli_path}: {e}");
            return vec![];
        }
    };

    let root = Cli::command();
    let mut missing = Vec::new();

    for top_name in ["emit", "query"] {
        let top_cmd = root
            .get_subcommands()
            .find(|c| c.get_name() == top_name)
            .unwrap_or_else(|| {
                eprintln!("toplevel '{top_name}' subcommand not found");
                std::process::exit(1)
            });

        for sub in top_cmd.get_subcommands() {
            let name = sub.get_name();
            // Skip internal / rarely-documented sub-subcommands.
            if should_skip_cli_check(top_name, name) {
                continue;
            }
            // Accept canonical name or any alias — some subcommands are
            // documented under a user-friendly alias (e.g. `recent` for
            // `recent_decisions`).
            let aliases: Vec<&str> = sub.get_all_aliases().collect();
            let found = cli_md.contains(name) || aliases.iter().any(|a| cli_md.contains(a));
            if !found {
                missing.push(format!("{top_name} {name}"));
            }
        }
    }

    missing
}

/// True for subcommand names that are intentionally omitted from the public
/// reference (internal / low-level / deprecated entries).
fn should_skip_cli_check(top: &str, name: &str) -> bool {
    // Low-level primitives not part of the user-facing reference:
    if top == "emit" {
        return matches!(
            name,
            "option.recorded" | "relation.added" | "relation.attach_evidence"
        );
    }
    // get_blocker_notification_candidates: internal scheduler surface
    if top == "query" {
        return matches!(name, "get_blocker_notification_candidates");
    }
    false
}
