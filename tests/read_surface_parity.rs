//! `why` and `verify` give the same answer through every surface (hivemind-ctok).
//!
//! One resolver reads the description for the CLI, the HTTP API and the MCP server, and the
//! plugin's `why.sh` / `verify.sh` call the CLI. Each of them still calls the resolver itself and
//! builds its own reply, and nothing but this test stops one from answering with another decision,
//! listing where the others answer, or leaving off the `close_match` line that says what the
//! decision lacks. The questions are the ones the after-deploy check asked when the resolver
//! answered an unrelated decision in one call (hivemind-tfde) or gave a different answer to the
//! same words as `recall` (hivemind-3lko), put to a ledger shaped like the one they were asked of:
//! decisions whose long rationales hold the question's words by chance, next to the decision whose
//! title says them.

use std::collections::HashMap;
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

/// (label, title, topic keys, rationale). The labels are what the checks below say; the decision
/// ids are made when the ledger is seeded.
const DECISIONS: &[(&str, &str, &str, &str)] = &[
    (
        "hosted",
        "The paid shape is picked after the comparison and the testers' feedback; the waitlist price question replaces interviews",
        "pricing,packaging,hosted,waitlist",
        "The paid shape is chosen after the comparison run and the first testers' feedback, not after interviews. The waitlist price question replaces the interviews, because asking a price is cheaper than scheduling a call. Until then the hosted cell stays on one node and nobody is charged. The comparison covers a hosted plan with self-hosting free against one Pro plan valid hosted or self-hosted.",
    ),
    (
        "paywall",
        "Nothing is paywalled until after the own-use trial; self-hosting stays free, a paid offer is hosted service and support",
        "pricing,packaging,licensing,self-hosting,hosted",
        "Read as: nothing is paywalled until after the own-use trial, and the self-hosted version stays free, so any future paid offer is the hosted service and support, not a self-host licence. A hosted plan with self-hosting free is one candidate shape; one Pro plan valid hosted or self-hosted is the other. The trial decides which plan to charge for, and the hosted cell is not billed during it. The maintainer sets the price after the trial.",
    ),
    (
        "gate",
        "Triage gate built against the System One shape, local model default, opt-in for the hosted classifier",
        "classifier,triage-gate,privacy,self-hosted",
        "The triage gate sits in front of the classifier that turns transcripts into decision records. It answers yes or no questions: is there a decision here at all, does it bind something or cost to undo, which existing question does this answer, which project. Today a model call per excerpt is spent to learn that most excerpts hold nothing, capped at 150 calls a day. A local model is the default; the product is still called by its working name in the gate prompts, and nothing is sent to the hosted classifier without opt-in.",
    ),
    (
        "name",
        "The product's new name is Upheld, home domain upheld.sh, replacing the working name (not Standing, not Decisis)",
        "product-name,naming,rename,domain",
        "A, Upheld. Home domain upheld.sh; the domain is bought by the maintainer; nothing is registered or reserved; the rename itself is not planned or started. Why the working name goes: it is overloaded and taken, on the main registries and with dozens of repositories of that exact name. Options: A = Upheld, a decision is upheld or it is not; B = Standing; C = Decisis. The site and the docs move to the new domain at the rename, one word at a time.",
    ),
    (
        "slugs-browser",
        "Decision links use title slugs worked out the same in every browser, not slugs remembered per browser",
        "ui,links,routing",
        "The server keeps no slug, and titles can change. Showing a slug remembered by one browser would let two browsers give one link to different decisions, so a shared link could open the wrong decision. Deterministic slugs open the same decision everywhere. Old links survive a retitle through the id suffix.",
    ),
    (
        "slugs-server",
        "UI decision links use the server's stable slug; title-derived slugs only when the server sends none",
        "ui,links,slugs",
        "The server now assigns each decision a slug once at proposal and keeps it through a retitle, and serves it on the graph route. Deriving links from titles in the browser moved a link on every retitle and needed a per-browser memory to keep old links working. The UI takes the server's slug first and works a slug out from the title only for decisions the server sent none for, such as the public demo snapshot.",
    ),
    (
        "flow-titles",
        "Flow node titles are laid out in screen space so none overprints: nudge, then leader line, else hide",
        "ui,flow,labels,layout",
        "Titles are overlays anchored above each node; on a phone fourteen titles share a narrow band. A greedy placer keeps each title on its spot if free, else the nearest free nudge with a faint leader line back to its node, else hides it; key titles first, then birth order.",
    ),
    (
        "flow-card",
        "On a phone, keep the Flow info card's room only while a card is pinned",
        "ui,flow,labels,phone",
        "The fixed room for the info card keeps titles from printing over any card, and costs nothing on a desktop, but on a small phone canvas it is a quarter of the canvas and every title that loses its spot is hidden.",
    ),
    (
        "demo-copy",
        "Public demo data: commit a byte-exact copy of the generator export, checked by recorded SHA-256",
        "ui,public-demo,example-data",
        "The demo build must work without a hivemind checkout (CI, the site rebuild), so the generator's graph and briefs files are committed rather than read from hivemind at build time. A sync script copies them byte for byte and records the source commit and each file's hash; a test fails if a copy no longer matches.",
    ),
    (
        "demo-runtime",
        "The public demo reads its example snapshot at runtime as static files in the server's own shapes",
        "ui,public-demo,snapshot",
        "The read-only public demo build fetches the graph and briefs files from a snapshot folder at runtime, mapped by the same code that reads a live server. The generator exports exactly what the server returns, so nothing is derived in the browser.",
    ),
    (
        "site-label",
        "Site graphs label the supersedes edge 'replaces' and draw coming-next parts solid with a COMING NEXT mark",
        "website,graphs",
        "The bead lists replaces among the product's graph words and the page says replace in plain words everywhere, while the app's graph reads supersedes; the site uses replaces and the claims file records the difference. Coming-next parts are merged and built, so they are drawn solid with a COMING NEXT mark; only planned parts are dashed and marked PLANNED.",
    ),
    (
        "site-chain",
        "Site graph 'one assumption falls' shows refutation marking only direct dependents; the chain goes stale on replacement",
        "website,graphs,honesty",
        "The product marks a decision as not holding when an assumption it rests on directly is refuted, one hop. A decision that follows from such a decision is not marked until the decision in between is replaced, and then it reads stale. Drawing the whole chain lighting up on the refutation alone would show something neither built nor planned.",
    ),
    (
        "slack-author",
        "Slack reaction and shortcut captures: the message's author decides, whoever triggered it records",
        "slack,attribution,capture",
        "When someone reacts to or runs the shortcut on another person's Slack message, the words and the choice are the author's, so the author is recorded as the decider and the person who triggered the capture as the recorder. Recording the reactor as decider would claim a person accepted a decision someone else worded.",
    ),
    (
        "framing",
        "Interview and search-term framing leads with who decided what, why, and whether it held up",
        "positioning,framing",
        "The lead line for the interviews and search terms is who decided what, why, and whether it held up. A person who saves a message, adds a mark or counts votes is not the one who decided; the framing says the person, not the tool.",
    ),
    (
        "agent-name",
        "The UI names every agent 'an agent', never by the tool or role it ran as",
        "ui,naming",
        "An agent's actor id names the tool that ran it and its role in that tool: internal names a reader cannot make sense of. What a reader needs is whether a person or an agent decided, so every agent reads alike.",
    ),
    (
        "option-letters",
        "UI letters a shortened option from its recorded label, else its id, only when the options run A, B, C",
        "ui,decision-page,options",
        "The preview API shortens option labels and sends the recorded label only where the option was captured with a label of its own. The id is text the capturing agent wrote, not a guess, so reading it keeps the rule that no letter is invented.",
    ),
    (
        "status-history",
        "Decision page status history: one row per logged status change; the demo snapshot keeps its undated rows",
        "ui,status,decision-page",
        "The server's status-events read dates every proposal, acceptance, rejection and supersession and names who made it and the status it led to, so each history row is one logged change. Nothing is derived in the browser. The public demo's snapshot carries only the graph and the briefs, so it keeps the rows it showed before.",
    ),
    (
        "verdict-note",
        "A superseded decision that was contested says so on a second line of its one verdict note, naming who rejected it",
        "decision-page,verdict",
        "The decision page showed one verdict note and chose superseded before contested, so a contested first vote that was later superseded hid its disagreement. Keeping one note with a second line adds the contest without a new layout element and keeps the supersession first.",
    ),
    (
        "deck",
        "Pitch deck: the value and who it is for instead of install steps, graphs over the held-up shot, still eight slides",
        "pitch-deck,positioning",
        "The install commands go; the deck says the value and who it is for, described by values. Each of the eight slides is made fuller instead of adding new ones.",
    ),
    (
        "listing",
        "Listing order after proof: Claude Code plugin first, then MCP Registry, then community lists",
        "channels,distribution",
        "All three listings follow the own-use trial, in the order the plugin, the registry, the community lists. The registry is the next step right after having shown that it actually works.",
    ),
    (
        "related-layer",
        "UI shows possibly-related decisions as an opt-in Graph layer: dashed, no arrowhead, first five per decision",
        "ui,graph",
        "The layer is off by default, drawn dashed, muted and without arrowheads, joining the selected decision to the first page of five suggestions, so none reads as a recorded relation.",
    ),
    (
        "no-people",
        "The graph draws no people; who decided shows under the selected decision's title",
        "ui,graph,actors",
        "Showing relation words at rest would put the server's passive 'proposed by' and 'accepted by' on every actor edge, and there is no active verb that reads decision to actor along an arrow that runs newer to older. Who decided shows on the decision itself.",
    ),
];

/// What a question must come to, on every surface.
#[derive(Debug)]
enum Expect {
    /// Answered in one call with this decision.
    Answers(&'static str),
    /// Not answered: a list of candidates, the first of them this decision (the one asked about).
    ListsFirst(&'static str),
    /// Not answered: a list, whichever decision it starts with (no decision is about the question).
    Lists,
    /// The decision asked about comes first, whether the reply answers with it or lists it first.
    Leads(&'static str),
    /// No decision matches.
    NoMatch,
}

/// (verb, question, expected). The first six are the questions hivemind-tfde was filed on, the
/// next three the ones hivemind-3lko was, and the last one asks about something never decided.
const CASES: &[(&str, &str, Expect)] = &[
    // hivemind-tfde: no decision is about Postgres or a per-seat charge, and the ones that share
    // some of the words must not be named as the answer.
    (
        "why",
        "why did we choose Postgres for the hosted cell?",
        Expect::Lists,
    ),
    (
        "why",
        "why do we charge per seat for the hosted plan?",
        Expect::Lists,
    ),
    (
        "why",
        "where do decision page addresses come from now, the browser or the server?",
        Expect::ListsFirst("slugs-browser"),
    ),
    (
        "why",
        "how are decision URLs built so every browser gets the same one?",
        Expect::Answers("slugs-browser"),
    ),
    (
        "why",
        "what word do the site's diagrams put on the arrow from a new decision to the one it replaced?",
        Expect::Leads("site-label"),
    ),
    (
        "why",
        "why is the demo's data checked into the UI repo instead of read from hivemind when it builds?",
        Expect::Answers("demo-copy"),
    ),
    // hivemind-3lko: what recall names, why and verify name.
    (
        "why",
        "how are Flow node titles kept from overprinting?",
        Expect::Leads("flow-titles"),
    ),
    (
        "why",
        "how do we keep links to a decision page stable across browsers?",
        Expect::Leads("slugs-server"),
    ),
    (
        "verify",
        "is the product still called Upheld",
        Expect::Answers("name"),
    ),
    ("why", "what is the weather on mars", Expect::NoMatch),
];

/// What a reply comes to, whichever surface it came through.
#[derive(Debug, PartialEq)]
enum Resolution {
    Answered(String),
    Listed(Vec<String>),
    NoMatch,
}

struct Labels(HashMap<String, &'static str>);

impl Labels {
    fn of(&self, id: &str) -> TestResult<String> {
        self.0
            .get(id)
            .map(|label| (*label).to_owned())
            .ok_or_else(|| {
                format!("a reply names a decision the ledger was not seeded with: {id}").into()
            })
    }

    /// Reads a reply's envelope the way a caller does: a candidate list, no match, or the
    /// decision it is about (`root` for `why`, `decision_id` for `verify`).
    fn resolution(&self, envelope: &Value) -> TestResult<Resolution> {
        let data = &envelope["data"];
        match data["outcome"].as_str() {
            Some("not_found") => Ok(Resolution::NoMatch),
            Some("ambiguous") => {
                let candidates = data["candidates"]
                    .as_array()
                    .ok_or("a list has candidates")?;
                let ids = candidates
                    .iter()
                    .map(|candidate| {
                        let id = candidate["decision_id"]
                            .as_str()
                            .ok_or("a candidate has an id")?;
                        self.of(id)
                    })
                    .collect::<TestResult<Vec<String>>>()?;
                Ok(Resolution::Listed(ids))
            }
            _ => {
                let id = data["root"]["id"]
                    .as_str()
                    .or_else(|| data["decision_id"].as_str())
                    .ok_or_else(|| format!("an answer names its decision: {envelope}"))?;
                Ok(Resolution::Answered(self.of(id)?))
            }
        }
    }
}

fn seed(dir: &Path) -> TestResult<Labels> {
    let path = dir.to_str().ok_or("the ledger path is utf-8")?;
    let mut labels = HashMap::new();
    for (label, title, topics, rationale) in DECISIONS {
        let id = run(&Cli::parse_from([
            "hivemind",
            "--actor",
            "agent:test:parity",
            "--hivemind-dir",
            path,
            "emit",
            "decision.proposed",
            "--title",
            title,
            "--rationale",
            rationale,
            "--topic-keys",
            topics,
            "--options",
            "A,B",
            "--chose",
            "A",
        ]))?;
        labels.insert(id.trim().to_owned(), *label);
    }
    Ok(Labels(labels))
}

/// An envelope without the one field that differs on every call.
fn without_latency(mut envelope: Value) -> Value {
    if let Some(fields) = envelope.as_object_mut() {
        fields.remove("latency_ms");
    }
    envelope
}

fn via_cli(dir: &Path, verb: &str, question: &str) -> TestResult<Value> {
    let path = dir.to_str().ok_or("the ledger path is utf-8")?;
    let output = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        path,
        "query",
        verb,
        question,
    ]))?;
    Ok(serde_json::from_str(&output)?)
}

/// The plugin's script, which finds the binary and the ledger the way a user's session does.
fn via_plugin(dir: &Path, verb: &str, question: &str) -> TestResult<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(root.join(format!("plugins/hivemind-context/scripts/{verb}.sh")))
        .arg(question)
        .current_dir(root)
        .env("HIVEMIND_CAPTURE_BIN", env!("CARGO_BIN_EXE_hivemind"))
        .env("HIVEMIND_DIR", dir)
        .env("CLAUDE_PROJECT_DIR", root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "{verb}.sh failed: {}",
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

async fn via_http(dir: &Path, verb: &str, question: &str) -> TestResult<(StatusCode, Value)> {
    let request = Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/decisions/{verb}?description={}",
            percent_encoded(question)
        ))
        .header("x-hivemind-actor", "agent:test:parity")
        .body(Body::empty())?;
    let response = app(dir).oneshot(request).await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok((status, serde_json::from_slice(&bytes)?))
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
            .arg("read-surface-parity")
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

    /// The reply of a read tool: the same envelope the CLI prints.
    fn call(&mut self, tool: &str, description: &str) -> TestResult<Value> {
        let reply = self.request(
            "tools/call",
            Some(json!({"name": tool, "arguments": {"description": description}})),
        )?;
        if reply["result"]["isError"] == Value::Bool(true) {
            return Err(format!("{tool} failed: {reply}").into());
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

fn mcp_tool(verb: &str) -> &'static str {
    if verb == "why" {
        "get_decision_neighborhood"
    } else {
        "get_decision_outcome"
    }
}

fn check(expected: &Expect, got: &Resolution) -> bool {
    match (expected, got) {
        (Expect::Answers(label), Resolution::Answered(answered)) => answered == label,
        (Expect::ListsFirst(label), Resolution::Listed(ids)) => {
            ids.first() == Some(&(*label).to_owned())
        }
        (Expect::Lists, Resolution::Listed(ids)) => !ids.is_empty(),
        (Expect::Leads(label), Resolution::Answered(answered)) => answered == label,
        (Expect::Leads(label), Resolution::Listed(ids)) => {
            ids.first() == Some(&(*label).to_owned())
        }
        (Expect::NoMatch, Resolution::NoMatch) => true,
        _ => false,
    }
}

#[tokio::test]
async fn why_and_verify_answer_the_same_through_the_cli_the_plugin_http_and_mcp() -> TestResult<()>
{
    let dir = unique_temp_dir("read-surface-parity");
    let labels = seed(&dir)?;
    let mut mcp = McpServer::start(&dir)?;

    for (verb, question, expected) in CASES {
        let cli = without_latency(via_cli(&dir, verb, question)?);
        let plugin = without_latency(via_plugin(&dir, verb, question)?);
        let from_mcp = without_latency(mcp.call(mcp_tool(verb), question)?);
        let (status, http) = via_http(&dir, verb, question).await?;

        let resolution = labels.resolution(&cli)?;
        assert!(
            check(expected, &resolution),
            "{verb} {question:?}: expected {expected:?}, the CLI came to {resolution:?}: {cli}"
        );

        // The plugin runs the CLI, so its reply is the CLI's, and MCP puts the same envelope in
        // its structured content.
        assert_eq!(
            plugin, cli,
            "{verb} {question:?}: the plugin's reply differs"
        );
        assert_eq!(from_mcp, cli, "{verb} {question:?}: the MCP reply differs");

        match resolution {
            // The one difference between the surfaces by design: HTTP says "nothing matches" with
            // a 404 and an error body, where the CLI and MCP print a successful `not_found`.
            Resolution::NoMatch => {
                assert_eq!(status, StatusCode::NOT_FOUND, "{verb} {question:?}: {http}");
                assert_eq!(http["error"]["code"], "not_found", "{http}");
            }
            _ => {
                assert_eq!(status, StatusCode::OK, "{verb} {question:?}: {http}");
                assert_eq!(
                    without_latency(http),
                    cli,
                    "{verb} {question:?}: the HTTP reply differs"
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
