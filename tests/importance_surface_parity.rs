//! "Which decisions carry the most impact" answers the same through every surface
//! (hivemind-bbnw.9).
//!
//! One ranking reads the graph for the CLI, the HTTP API, the stdio MCP server and the HTTP MCP
//! endpoint, and the plugin's `importance.sh` calls the CLI. Each of them still builds its own
//! request and its own reply, and nothing but this test stops one from listing another order,
//! leaving off who decided, turning `not_assessed` into a number, or paging differently. The
//! ledger is shaped like the one the question is asked of: a decision a person decided with
//! others resting on it, one an agent decided within that person's delegation, one nobody has
//! decided, and ones nothing rests on.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;

use hivemind::cli::{run, Cli};

#[allow(dead_code)]
#[path = "support/seed_data.rs"]
mod seed_data;

use seed_data::{unique_temp_dir, TestResult};

const RECORDER: &str = "agent:test:parity";
const PERSON: &str = "human:alex.knips@example.com";

/// (label, title, how it was decided). Recorded in this order, so a later one is newer.
const DECISIONS: &[(&str, &str, Decided)] = &[
    (
        "root",
        "The ledger is the only source of truth for what was decided",
        Decided::ByPerson,
    ),
    (
        "mid",
        "Reads derive status from the ledger instead of storing it",
        Decided::ByAgentWithinDelegation,
    ),
    (
        "leaf",
        "Status derivation is a pure function of the events",
        Decided::Nobody,
    ),
    (
        "lone",
        "The demo snapshot is regenerated on every release",
        Decided::ByAgentAlone,
    ),
    (
        "second-leaf",
        "Derived status is never cached across restarts",
        Decided::Nobody,
    ),
];

/// (follower, premise): `mid` follows from `root`, `leaf` and `second-leaf` from `mid`.
const FOLLOWS: &[(&str, &str)] = &[("mid", "root"), ("leaf", "mid"), ("second-leaf", "mid")];

#[derive(Clone, Copy, Debug)]
enum Decided {
    ByPerson,
    ByAgentWithinDelegation,
    ByAgentAlone,
    Nobody,
}

struct Seeded {
    /// Decision id by label.
    ids: Vec<(&'static str, String)>,
}

impl Seeded {
    fn id(&self, label: &str) -> &str {
        self.ids
            .iter()
            .find(|(candidate, _)| *candidate == label)
            .map(|(_, id)| id.as_str())
            .unwrap_or_else(|| panic!("no decision labelled {label}")) // ubs:ignore: test-only; panicking is correct in tests
    }

    fn label(&self, id: &str) -> &str {
        self.ids
            .iter()
            .find(|(_, candidate)| candidate == id)
            .map(|(label, _)| *label)
            .unwrap_or_else(|| {
                panic!("a reply names a decision the ledger was not seeded with: {id}")
            }) // ubs:ignore: test-only; panicking is correct in tests
    }
}

fn seed(dir: &Path) -> TestResult<Seeded> {
    let path = dir.to_str().ok_or("the ledger path is utf-8")?;
    let mut ids = Vec::new();
    for (label, title, decided) in DECISIONS {
        let mut argv = vec![
            "hivemind",
            "--actor",
            RECORDER,
            "--hivemind-dir",
            path,
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            "Recorded for the importance parity check, long enough to read on its own",
            "--topic-keys",
            "parity",
            "--options",
            "A,B",
            "--chose",
            "A",
        ];
        match decided {
            Decided::ByPerson => argv.extend(["--decided-by", PERSON]),
            Decided::ByAgentWithinDelegation => argv.extend(["--delegated-by", PERSON]),
            Decided::ByAgentAlone => {}
            Decided::Nobody => argv.push("--still-proposed"),
        }
        let id = run(&Cli::parse_from(argv))?;
        ids.push((*label, id.trim().to_owned()));
    }
    let seeded = Seeded { ids };
    for (follower, premise) in FOLLOWS {
        run(&Cli::parse_from([
            "hivemind",
            "--actor",
            RECORDER,
            "--hivemind-dir",
            path,
            "emit",
            "relation.added",
            "--kind",
            "follows_from",
            "--from",
            seeded.id(follower),
            "--to",
            seeded.id(premise),
        ]))?;
    }
    Ok(seeded)
}

/// An envelope without the one field that differs on every call.
fn without_latency(mut envelope: Value) -> Value {
    if let Some(fields) = envelope.as_object_mut() {
        fields.remove("latency_ms");
    }
    envelope
}

/// The request as each surface spells it: `limit`, `cursor` and `include_not_in_force`.
#[derive(Clone, Debug, Default)]
struct Ask {
    limit: Option<usize>,
    cursor: Option<String>,
    include_not_in_force: bool,
}

impl Ask {
    fn cli_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(limit) = self.limit {
            args.extend(["--limit".to_owned(), limit.to_string()]);
        }
        if let Some(cursor) = &self.cursor {
            args.extend(["--cursor".to_owned(), cursor.clone()]);
        }
        if self.include_not_in_force {
            args.push("--include-not-in-force".to_owned());
        }
        args
    }

    fn query_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(limit) = self.limit {
            parts.push(format!("limit={limit}"));
        }
        if let Some(cursor) = &self.cursor {
            parts.push(format!("cursor={}", percent_encoded(cursor)));
        }
        if self.include_not_in_force {
            parts.push("include_not_in_force=true".to_owned());
        }
        parts.join("&")
    }

    fn tool_arguments(&self) -> Value {
        let mut arguments = json!({});
        if let Some(limit) = self.limit {
            arguments["limit"] = json!(limit);
        }
        if let Some(cursor) = &self.cursor {
            arguments["cursor"] = json!(cursor);
        }
        if self.include_not_in_force {
            arguments["include_not_in_force"] = json!(true);
        }
        arguments
    }
}

fn via_cli(dir: &Path, ask: &Ask) -> TestResult<Value> {
    let path = dir.to_str().ok_or("the ledger path is utf-8")?;
    let mut argv = vec![
        "hivemind".to_owned(),
        "--hivemind-dir".to_owned(),
        path.to_owned(),
        "query".to_owned(),
        "rank_decisions_by_importance".to_owned(),
    ];
    argv.extend(ask.cli_args());
    let output = run(&Cli::parse_from(argv))?;
    Ok(serde_json::from_str(&output)?)
}

/// The plugin's script, which finds the binary and the ledger the way a user's session does.
fn via_plugin(dir: &Path, ask: &Ask) -> TestResult<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(root.join("plugins/hivemind-context/scripts/importance.sh"))
        .args(ask.cli_args())
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "importance.sh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn app(hivemind_dir: &Path) -> axum::Router {
    let config = hivemind::api::ApiConfig {
        hivemind_dir: hivemind_dir.to_path_buf(),
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 0,
        allow_unauthenticated_remote: false,
        api_key: None,
        database_url: None,
        admin_key: None,
        workos_domain: None,
        workos_issuer: None,
        workos_jwks_url: None,
        workos_audience: None,
        spa_dir: None,
        cors_origins: vec![],
        slack_client_id: None,
        slack_client_secret: None,
        slack_signing_secret: None,
        slack_api_base_url: None,
    };
    hivemind::api::create_router(&config)
}

/// `text` as a query-string value.
fn percent_encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn via_http(dir: &Path, ask: &Ask) -> TestResult<(StatusCode, Value)> {
    let query = ask.query_string();
    let uri = if query.is_empty() {
        "/v1/decisions/importance".to_owned()
    } else {
        format!("/v1/decisions/importance?{query}")
    };
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-hivemind-actor", RECORDER)
        .body(Body::empty())?;
    let response = app(dir).oneshot(request).await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// The same tool over the HTTP MCP endpoint.
async fn via_http_mcp(dir: &Path, ask: &Ask) -> TestResult<Value> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "rank_decisions_by_importance",
            "arguments": ask.tool_arguments(),
        },
    });
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("x-hivemind-actor", RECORDER)
        .body(Body::from(serde_json::to_string(&body)?))?;
    let response = app(dir).oneshot(request).await?;
    let bytes = response.into_body().collect().await?.to_bytes();
    let reply: Value = serde_json::from_slice(&bytes)?;
    if reply["result"]["isError"] == Value::Bool(true) {
        return Err(format!("rank_decisions_by_importance failed over HTTP MCP: {reply}").into());
    }
    Ok(reply["result"]["structuredContent"].clone())
}

/// The MCP server, spoken to over its stdio the way a client does.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpServer {
    fn start(dir: &Path) -> TestResult<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
            .arg("--hivemind-dir")
            .arg(dir)
            .arg("mcp")
            .arg("--session-id")
            .arg("importance-surface-parity")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("the server has a stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the server has a stdout")?);
        let mut server = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        server.request("initialize", None)?;
        Ok(server)
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> TestResult<Value> {
        self.next_id += 1;
        let mut request = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method});
        if let Some(params) = params {
            request["params"] = params;
        }
        writeln!(self.stdin, "{request}")?;
        self.stdin.flush()?;
        let mut line = String::new();
        self.stdout.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim())?)
    }

    /// The reply of the read tool: the same envelope the CLI prints.
    fn rank(&mut self, ask: &Ask) -> TestResult<Value> {
        let reply = self.request(
            "tools/call",
            Some(json!({
                "name": "rank_decisions_by_importance",
                "arguments": ask.tool_arguments(),
            })),
        )?;
        if reply["result"]["isError"] == Value::Bool(true) {
            return Err(format!("rank_decisions_by_importance failed: {reply}").into());
        }
        Ok(reply["result"]["structuredContent"].clone())
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every surface's reply to `ask`, which must all be the CLI's.
async fn answer_alike(dir: &Path, mcp: &mut McpServer, ask: &Ask) -> TestResult<Value> {
    let cli = without_latency(via_cli(dir, ask)?);
    let plugin = without_latency(via_plugin(dir, ask)?);
    let stdio = without_latency(mcp.rank(ask)?);
    let http_mcp = without_latency(via_http_mcp(dir, ask).await?);
    let (status, http) = via_http(dir, ask).await?;

    assert_eq!(status, StatusCode::OK, "{ask:?}: {http}");
    assert_eq!(plugin, cli, "{ask:?}: the plugin's reply differs");
    assert_eq!(stdio, cli, "{ask:?}: the stdio MCP reply differs");
    assert_eq!(http_mcp, cli, "{ask:?}: the HTTP MCP reply differs");
    assert_eq!(
        without_latency(http),
        cli,
        "{ask:?}: the HTTP reply differs"
    );
    Ok(cli)
}

fn labels<'a>(seeded: &'a Seeded, reply: &Value) -> TestResult<Vec<&'a str>> {
    reply["data"]["decisions"]
        .as_array()
        .ok_or("the reply lists decisions")?
        .iter()
        .map(|row| -> TestResult<&str> {
            let id = row["decision_id"].as_str().ok_or("a row has an id")?;
            Ok(seeded.label(id))
        })
        .collect()
}

fn row<'a>(seeded: &Seeded, reply: &'a Value, label: &str) -> TestResult<&'a Value> {
    let id = seeded.id(label);
    Ok(reply["data"]["decisions"]
        .as_array()
        .ok_or("the reply lists decisions")?
        .iter()
        .find(|row| row["decision_id"] == json!(id))
        .ok_or_else(|| format!("{label} is not listed: {reply}"))?)
}

#[tokio::test]
async fn the_ranking_answers_the_same_through_the_cli_the_plugin_http_and_mcp() -> TestResult<()> {
    let dir = unique_temp_dir("importance-surface-parity");
    let seeded = seed(&dir)?;
    let mut mcp = McpServer::start(&dir)?;

    let reply = answer_alike(&dir, &mut mcp, &Ask::default()).await?;

    // root has three decisions resting on it (mid directly; leaf and second-leaf through mid),
    // mid has two. Nothing rests on the rest, which follow newest first.
    assert_eq!(
        labels(&seeded, &reply)?,
        ["root", "mid", "second-leaf", "lone", "leaf"]
    );
    assert_eq!(reply["data"]["ranked_total"], json!(2));
    assert_eq!(reply["data"]["not_assessed_total"], json!(3));
    assert_eq!(reply["truncated"], json!(false));

    let root = row(&seeded, &reply, "root")?;
    assert_eq!(root["rank"], json!(1));
    assert_eq!(root["importance"], json!("ranked"));
    assert_eq!(root["rests_on_it"]["direct"], json!(1));
    assert_eq!(root["rests_on_it"]["through_chains"], json!(3));
    assert_eq!(root["decided_by"]["kind"], json!("person"));
    assert_eq!(root["decided_by"]["deciders"][0]["id"], json!(PERSON));
    assert_eq!(root["decided_by"]["deciders"][0]["kind"], json!("human"));
    assert_eq!(root["recorded_by"], json!(RECORDER));

    let mid = row(&seeded, &reply, "mid")?;
    assert_eq!(mid["rank"], json!(2));
    assert_eq!(mid["decided_by"]["kind"], json!("agent_within_delegation"));
    assert_eq!(mid["decided_by"]["delegated_by"], json!(PERSON));

    // Not assessed is a reading, never a number; and the recorder is not the decider.
    let leaf = row(&seeded, &reply, "leaf")?;
    assert_eq!(leaf["importance"], json!("not_assessed"));
    assert!(leaf.get("rank").is_none(), "{leaf}");
    assert_eq!(leaf["decided_by"]["kind"], json!("none_recorded"));
    assert_eq!(leaf["recorded_by"], json!(RECORDER));
    let lone = row(&seeded, &reply, "lone")?;
    assert_eq!(lone["decided_by"]["kind"], json!("agent"));

    // Who decided the ranked decisions: one person, one agent within that person's delegation.
    assert_eq!(
        reply["data"]["ranked_decided_by"],
        json!({
            "person": 1,
            "agent": 0,
            "agent_within_delegation": 1,
            "mixed": 0,
            "unknown": 0,
            "none_recorded": 0,
        })
    );
    assert_eq!(
        reply["data"]["left_out"],
        json!({"superseded": 0, "rejected": 0})
    );

    // Importance is its own axis: no score, no tier, no grade anywhere in the reply.
    let text = reply.to_string();
    for word in ["\"score\"", "\"tier\"", "\"grade\"", "\"composite\""] {
        assert!(!text.contains(word), "{word} in {text}");
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn pages_and_flags_mean_the_same_on_every_surface() -> TestResult<()> {
    let dir = unique_temp_dir("importance-surface-parity-pages");
    let seeded = seed(&dir)?;
    let mut mcp = McpServer::start(&dir)?;

    let first = answer_alike(
        &dir,
        &mut mcp,
        &Ask {
            limit: Some(2),
            ..Ask::default()
        },
    )
    .await?;
    assert_eq!(labels(&seeded, &first)?, ["root", "mid"]);
    assert_eq!(first["truncated"], json!(true));
    let cursor = first["data"]["next_cursor"]
        .as_str()
        .ok_or("a truncated page says where to resume")?
        .to_owned();

    let second = answer_alike(
        &dir,
        &mut mcp,
        &Ask {
            limit: Some(2),
            cursor: Some(cursor),
            ..Ask::default()
        },
    )
    .await?;
    assert_eq!(labels(&seeded, &second)?, ["second-leaf", "lone"]);
    let cursor = second["data"]["next_cursor"]
        .as_str()
        .ok_or("another page follows")?
        .to_owned();

    let last = answer_alike(
        &dir,
        &mut mcp,
        &Ask {
            limit: Some(2),
            cursor: Some(cursor),
            ..Ask::default()
        },
    )
    .await?;
    assert_eq!(labels(&seeded, &last)?, ["leaf"]);
    assert_eq!(last["truncated"], json!(false));
    assert!(last["data"].get("next_cursor").is_none(), "{last}");

    // The flag is read alike. Nothing here is superseded or rejected, so it changes nothing.
    let everything = answer_alike(
        &dir,
        &mut mcp,
        &Ask {
            include_not_in_force: true,
            ..Ask::default()
        },
    )
    .await?;
    assert_eq!(
        everything["data"]["left_out"],
        json!({"superseded": 0, "rejected": 0})
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
