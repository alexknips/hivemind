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

const MCP_SETUP_PATH: &str = "website/src/content/docs/guides/mcp-setup.md";

fn main() {
    let check_mode = std::env::args().any(|a| a == "--check");

    let tool_count = tool_definitions().len();
    let mcp_md = generate_mcp_reference();
    let cli_missing = check_cli_completeness();

    let mcp_path = "website/src/content/docs/reference/mcp-tools.md";

    let setup_text = std::fs::read_to_string(MCP_SETUP_PATH).unwrap_or_else(|e| {
        eprintln!("Cannot read {MCP_SETUP_PATH}: {e}");
        std::process::exit(1)
    });

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

        let stale_mentions = stale_tool_count_mentions(&setup_text, tool_count);
        if !stale_mentions.is_empty() {
            eprintln!(
                "STALE: {MCP_SETUP_PATH} has tool-count mentions that don't match \
                 tool_definitions() ({tool_count} tools):"
            );
            for mention in &stale_mentions {
                eprintln!("{mention}");
            }
            eprintln!("Run: cargo run --bin generate-reference");
            failed = true;
        } else {
            println!(
                "OK: {MCP_SETUP_PATH} tool-count mentions match tool_definitions() \
                 ({tool_count} tools)."
            );
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

        let fixed_setup = fix_tool_count_mentions(&setup_text, tool_count);
        if fixed_setup != setup_text {
            std::fs::write(MCP_SETUP_PATH, &fixed_setup).unwrap_or_else(|e| {
                eprintln!("Cannot write {MCP_SETUP_PATH}: {e}");
                std::process::exit(1)
            });
            println!("Updated: {MCP_SETUP_PATH} tool-count mentions -> {tool_count}");
        } else {
            println!(
                "OK: {MCP_SETUP_PATH} tool-count mentions already match ({tool_count} tools)."
            );
        }
    }
}

// ---------------------------------------------------------------------------
// MCP setup guide tool-count guard
// ---------------------------------------------------------------------------
//
// mcp-setup.md is hand-written prose (unlike mcp-tools.md, which is fully
// generated), so it can't just be overwritten wholesale. Instead we scan it
// for numbers immediately followed by "tool"/"tools" (e.g. "14 tools", "12
// HiveMind tools") and treat each as an assertion about the total tool count
// that must match `tool_definitions().len()`.

/// A number in `mcp-setup.md` immediately followed (within two words) by
/// "tool" or "tools" — treated as an assertion about the total tool count.
struct ToolCountMention {
    line: usize,
    /// Byte range of the digit run within the file (suffix punctuation like
    /// "14." is preserved; only the digits are replaced on fix).
    start: usize,
    end: usize,
    value: usize,
}

fn find_tool_count_mentions(text: &str) -> Vec<ToolCountMention> {
    let mut mentions = Vec::new();
    let mut line_offset = 0usize;

    for (line_idx, line) in text.split('\n').enumerate() {
        let words = word_offsets(line);
        for (i, &(w_off, word)) in words.iter().enumerate() {
            let digits_len = word.bytes().take_while(u8::is_ascii_digit).count();
            if digits_len == 0 {
                continue;
            }
            // Reject ordinals ("14th") and other letter-suffixed tokens; only
            // punctuation may trail the digits.
            if !word[digits_len..].chars().all(|c| c.is_ascii_punctuation()) {
                continue;
            }
            let window_end = (i + 3).min(words.len());
            let window_start = (i + 1).min(window_end);
            let followed_by_tool = words[window_start..window_end].iter().any(|&(_, w)| {
                let stripped: String = w.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                let lower = stripped.to_ascii_lowercase();
                lower == "tool" || lower == "tools"
            });
            if !followed_by_tool {
                continue;
            }
            let Ok(value) = word[..digits_len].parse::<usize>() else {
                continue;
            };
            mentions.push(ToolCountMention {
                line: line_idx + 1,
                start: line_offset + w_off,
                end: line_offset + w_off + digits_len,
                value,
            });
        }
        line_offset += line.len() + 1; // +1 for the '\n' split delimiter
    }

    mentions
}

/// Whitespace-delimited tokens in `line` paired with their byte offset
/// within `line`.
fn word_offsets(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, &line[s..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push((s, &line[s..]));
    }
    out
}

fn stale_tool_count_mentions(text: &str, expected: usize) -> Vec<String> {
    find_tool_count_mentions(text)
        .into_iter()
        .filter(|m| m.value != expected)
        .map(|m| {
            format!(
                "  line {}: found {} tool(s), expected {expected}",
                m.line, m.value
            )
        })
        .collect()
}

fn fix_tool_count_mentions(text: &str, expected: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for mention in find_tool_count_mentions(text) {
        if mention.value == expected {
            continue;
        }
        out.push_str(&text[last..mention.start]);
        out.push_str(&expected.to_string());
        last = mention.end;
    }
    out.push_str(&text[last..]);
    out
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
    out.push_str(
        "The Markdown decision-log export (`hivemind export --format markdown --out <dir>`) is\nCLI-only and has no MCP tool: its output is a directory tree, not a single result an\nMCP call can return.\n\n",
    );
    writeln!(
        out,
        "See [MCP Setup](../../guides/mcp-setup/) to configure your client."
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
