//! Builds a fresh HiveMind ledger through the real capture verbs
//! (`hivemind::cli::run`, the same command layer the `hivemind` binary uses — never
//! hand-written fixture JSON, see hivemind-hc-yds and hu-0pf) and exports the two files the
//! public demo's read-only UI needs: `GET /v1/graph`'s response body, verbatim, and one
//! `GET /v1/decisions/verify?id=` response per decision.
//!
//! `cargo test --test example_data_generator` runs on every PR via the existing `rust` CI
//! job (no extra CI wiring) and only checks the freshly-built snapshot's shape: decision and
//! project counts, that both stories that must stay stale-and-visible (a superseded decision,
//! a contested one) are still there, and that briefs.json covers every decision in graph.json.
//! It writes nothing.
//!
//! `cargo test --test example_data_generator -- --bless` does the same build, then also
//! writes demos/example-data/snapshot/{graph,briefs}.json — the files this repo commits.
//! Decision ids and timestamps are freshly generated on every run (real capture, not a fixed
//! fixture), so `--bless` is how the snapshot is regenerated after the story below changes;
//! there is no byte-stable golden diff for this one (contrast tests/golden.rs, whose seed data
//! writes deterministic ids).

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use hivemind::api::{create_router, ApiConfig};
use hivemind::cli::{run, Cli};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SNAPSHOT_DIR: &str = "demos/example-data/snapshot";

fn main() {
    if let Err(error) = run_harness() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_harness() -> TestResult<()> {
    let bless = parse_args()?;

    let scratch = unique_temp_dir("example-data-generator");
    let hivemind_dir = scratch.join("hivemind");
    std::fs::create_dir_all(&hivemind_dir)?;

    build_story(&hivemind_dir)?;

    let runtime = tokio::runtime::Runtime::new()?;
    let (graph, briefs) = runtime.block_on(export_snapshot(&hivemind_dir))?;

    let cleanup = std::fs::remove_dir_all(&scratch);
    // Checked before writing: a shape that fails the check is never blessed into the
    // committed snapshot.
    check_snapshot(&graph, &briefs)?;

    if bless {
        write_snapshot(&graph, &briefs)?;
        println!("blessed {SNAPSHOT_DIR}/graph.json and {SNAPSHOT_DIR}/briefs.json");
    }

    cleanup?;
    println!(
        "example-data generator: {} decisions, {} briefs — shape OK{}",
        array_len(&graph["decisions"]),
        object_len(&briefs),
        if bless { " (blessed)" } else { "" }
    );
    Ok(())
}

fn parse_args() -> TestResult<bool> {
    let mut bless = false;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bless" => bless = true,
            "--nocapture" | "--include-ignored" | "--ignored" | "--exact" | "--quiet"
            | "--show-output" => {}
            "--test-threads" | "--skip" | "--format" => {
                let _ = args.next();
            }
            "--help" | "-h" => {
                println!("Usage: cargo test --test example_data_generator -- [--bless]");
                std::process::exit(0);
            }
            other if other.starts_with("--test-threads=") => {}
            other if other.starts_with("--skip=") => {}
            other if other.starts_with("--format=") => {}
            other if !other.starts_with('-') => {}
            other => return Err(format!("unknown example_data_generator argument: {other}").into()),
        }
    }
    Ok(bless)
}

fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hivemind-{label}-{}", uuid::Uuid::new_v4()))
}

// ---------------------------------------------------------------------------
// The story — three fictional projects, captured through the real CLI verbs.
// ---------------------------------------------------------------------------

fn build_story(dir: &Path) -> TestResult<()> {
    register_projects(dir)?;
    trailkeeper_sqlite_to_postgres(dir)?;
    ridewell_cache_and_retries(dir)?;
    twelve_angry_men(dir)?;
    Ok(())
}

fn register_projects(dir: &Path) -> TestResult<()> {
    hm(
        dir,
        "human:priya",
        &[
            "project",
            "register",
            "trailkeeper",
            "--display-name",
            "TrailKeeper",
            "--purpose",
            "A hiking-log app for planning and recording backcountry trips",
        ],
    )?;
    hm(
        dir,
        "human:maya",
        &[
            "project",
            "register",
            "ridewell-api",
            "--display-name",
            "Ridewell API",
            "--purpose",
            "Backend services for the Ridewell driver-dispatch platform",
        ],
    )?;
    hm(
        dir,
        "human:maya",
        &[
            "project",
            "register",
            "ridewell-mobile",
            "--display-name",
            "Ridewell Mobile",
            "--purpose",
            "The Ridewell driver mobile app",
        ],
    )?;
    hm(
        dir,
        "human:maya",
        &[
            "project",
            "link",
            "--from",
            "ridewell-mobile",
            "--to",
            "ridewell-api",
            "--kind",
            "depends_on",
        ],
    )?;
    hm(
        dir,
        "human:juror-foreman",
        &[
            "project",
            "register",
            "12-angry-men",
            "--display-name",
            "12 Angry Men",
            "--purpose",
            "A jury's deliberation over a single murder trial verdict",
        ],
    )?;
    declare_topics(dir)
}

/// A registered project's captures may only use topic keys the project declared, so each story's
/// keys are declared through the real verb before its first capture uses them. (The supersedes
/// in the stories name no keys and inherit their predecessor's.)
fn declare_topics(dir: &Path) -> TestResult<()> {
    let vocabularies: [(&str, &str, &[&str]); 4] = [
        (
            "human:priya",
            "trailkeeper",
            &["infrastructure", "trailkeeper"],
        ),
        (
            "human:maya",
            "ridewell-api",
            &["infrastructure", "ridewell"],
        ),
        (
            "human:maya",
            "ridewell-mobile",
            &["mobile", "reliability", "ridewell"],
        ),
        (
            "human:juror-foreman",
            "12-angry-men",
            &["12-angry-men", "verdict"],
        ),
    ];
    for (actor, handle, keys) in vocabularies {
        let mut args = vec!["project", "declare-topic", handle];
        args.extend_from_slice(keys);
        hm(dir, actor, &args)?;
    }
    Ok(())
}

/// Story: an SQLite -> Postgres supersede, as a single-device assumption stops holding.
fn trailkeeper_sqlite_to_postgres(dir: &Path) -> TestResult<()> {
    let sqlite_decision = id_of(&hm(
        dir,
        "human:priya",
        &[
            "emit",
            "decision.capture",
            "--actor-id",
            "human:priya",
            "--source",
            "human",
            "--title",
            "Store hike logs in local SQLite on the phone",
            "--rationale",
            "TrailKeeper v1 is single-device: one hiker records one trip on one phone. SQLite \
             needs no server, keeps the app fully usable with no signal on trail, and keeps \
             hosting cost at zero while we find out if anyone wants this at all.",
            "--topic-keys",
            "infrastructure,trailkeeper",
            "--options",
            "Local SQLite,Local JSON files,A hosted database from day one",
            "--chose",
            "Local SQLite",
            "--rests-on-assumption",
            "Each hiker uses TrailKeeper from a single phone; nobody needs the same trip log \
             on two devices",
            "--project",
            "trailkeeper",
            "--question",
            "Where do we store a hiker's trip logs?",
            "--quote",
            "Let's not stand up a backend before we know anyone wants this.",
        ],
    )?)?;

    hm(
        dir,
        "human:priya",
        &[
            "supersede",
            "--old",
            &sqlite_decision,
            "--title",
            "Move hike log storage to a hosted Postgres, synced across devices",
            "--rationale",
            "The single-device assumption behind local SQLite doesn't hold: the beta survey \
             shows most hikers already use TrailKeeper on their phone and the web dashboard, \
             and a fifth of them lost photos or edits made on the other device. Postgres with \
             a per-user sync log lets both surfaces read and write the same trip; we accept \
             running one small managed instance in exchange for not losing anyone's trip \
             again.",
            "--options",
            "Hosted Postgres with a per-user sync log,Keep SQLite and add a manual export/import,Firebase Realtime Database",
            "--chose",
            "Hosted Postgres with a per-user sync log",
            "--rests-on-evidence",
            "Beta survey, Sept 2026: 61% of 140 respondents use TrailKeeper on both phone and \
             web; 22% reported a trip missing photos or edits made on the other device",
            "--evidence-source",
            "trailkeeper beta survey, 2026-09",
        ],
    )?;
    Ok(())
}

/// Stories: an agent-chosen retry count, and a cache-by-id decision resting on "ids never
/// change" that goes stale — across the Ridewell API and Mobile projects, so the staleness is
/// visible across a project boundary (mobile's decision follows from the API's).
fn ridewell_cache_and_retries(dir: &Path) -> TestResult<()> {
    let cache_by_driver_id = hm(
        dir,
        "human:maya",
        &[
            "emit",
            "decision.capture",
            "--actor-id",
            "human:maya",
            "--source",
            "human",
            "--title",
            "Cache driver profiles in memory, keyed by the driver's numeric id",
            "--rationale",
            "Driver-profile reads dominate dispatch-service load, about 70% of calls. An \
             in-memory LRU keyed by driver_id avoids a database round trip on every dispatch \
             and is simple to reason about, since driver_id is the primary key login already \
             returns.",
            "--topic-keys",
            "infrastructure,ridewell",
            "--options",
            "In-memory LRU keyed by driver_id,Redis cache keyed by driver_id,No cache (read through on every request)",
            "--chose",
            "In-memory LRU keyed by driver_id",
            "--rests-on-assumption",
            "Driver ids are assigned once at signup and never change for the lifetime of the \
             account",
            "--project",
            "ridewell-api",
            "--question",
            "How do we avoid a database round trip on every dispatch lookup?",
        ],
    )?;
    let cache_by_driver_id_id = id_of(&cache_by_driver_id)?;
    let ids_never_change_hypothesis = first_rests_on_id(&cache_by_driver_id)?;

    hm(
        dir,
        "agent:codex:ridewell-sync",
        &[
            "emit",
            "decision.capture",
            "--actor-id",
            "agent:codex:ridewell-sync",
            "--source",
            "agent",
            "--title",
            "Cache the driver's id on the phone at login and skip the profile lookup on cold \
             start",
            "--rationale",
            "Cold start currently blocks on a profile fetch before the map can render. The API \
             already treats driver_id as stable once assigned, so storing it in local device \
             storage at login and reusing it on every cold start removes that fetch with no \
             correctness cost.",
            "--topic-keys",
            "ridewell,mobile",
            "--options",
            "Store driver_id locally and skip the cold-start lookup,Always re-fetch the profile on cold start,Cache the whole profile object with a 1-hour TTL",
            "--chose",
            "Store driver_id locally and skip the cold-start lookup",
            "--rests-on-decision",
            &cache_by_driver_id_id,
            "--project",
            "ridewell-mobile",
            "--question",
            "How do we avoid blocking cold start on a profile fetch?",
        ],
    )?;

    hm(
        dir,
        "agent:claude:ridewell-mobile-ci",
        &[
            "emit",
            "decision.capture",
            "--actor-id",
            "agent:claude:ridewell-mobile-ci",
            "--source",
            "agent",
            "--title",
            "Retry a failed location-ping upload up to 5 times with exponential backoff, capped \
             at 30 seconds",
            "--rationale",
            "Field logs from the last two weeks show sync failures during cell-tower handoff \
             mostly clear within 4 attempts. A fixed short retry gives up too early on a flaky \
             handoff; an unbounded retry can spin all day and drains the phone's battery mid-\
             shift.",
            "--topic-keys",
            "ridewell,mobile,reliability",
            "--options",
            "3 retries fixed 5s gap,5 retries exponential backoff capped at 30s,Unlimited retries with a circuit breaker",
            "--chose",
            "5 retries exponential backoff capped at 30s",
            "--project",
            "ridewell-mobile",
            "--bet",
            "Capping at 5 retries with backoff will clear at least 90% of handoff failures \
             without a manual retry",
            "--would-change-if",
            "Manual retry reports do not drop after this ships",
            "--check-by",
            "2026-11-15",
        ],
    )?;

    let migration_incident = id_of(&hm(
        dir,
        "human:maya",
        &[
            "emit",
            "evidence.recorded",
            "--actor-id",
            "human:maya",
            "--source",
            "human",
            "--content",
            "Incident review, 2026-09-20: migrating dispatch to the new fleet-partner platform \
             reassigned driver_id for about 1,200 accounts during the backfill; two drivers \
             were shown another driver's cached profile for several minutes before the cache \
             expired.",
            "--source-ref",
            "ridewell incident review, 2026-09-20",
        ],
    )?)?;

    hm(
        dir,
        "human:maya",
        &[
            "emit",
            "relation.added",
            "--kind",
            "refutes",
            "--from",
            &migration_incident,
            "--to",
            &ids_never_change_hypothesis,
        ],
    )?;

    hm(
        dir,
        "human:maya",
        &[
            "supersede",
            "--old",
            &cache_by_driver_id_id,
            "--title",
            "Cache driver profiles keyed by a stable external UUID instead of the mutable \
             numeric id",
            "--rationale",
            "The 2026-09-20 platform migration reassigned numeric driver ids for over a \
             thousand accounts and briefly served two drivers each other's cached profile. The \
             dispatch service already receives a separate UUID from the identity provider that \
             survives a backend migration; keying the cache on that UUID removes the failure \
             mode the numeric-id assumption depended on.",
            "--options",
            "In-memory LRU keyed by the identity-provider UUID,Redis cache keyed by the UUID,Drop the profile cache entirely",
            "--chose",
            "In-memory LRU keyed by the identity-provider UUID",
            "--evidence",
            &migration_incident,
        ],
    )?;
    Ok(())
}

/// Story: 12 Angry Men — a contested verdict, revised as new evidence surfaces, still
/// contested after the revision (one juror never comes around; disagreement is never
/// silently resolved).
fn twelve_angry_men(dir: &Path) -> TestResult<()> {
    let guilty = id_of(&hm(
        dir,
        "human:juror-foreman",
        &[
            "emit",
            "decision.capture",
            "--actor-id",
            "human:juror-foreman",
            "--source",
            "human",
            "--title",
            "The jury finds the defendant guilty",
            "--rationale",
            "Eleven of twelve jurors voted guilty on the first ballot. The knife matches the \
             defendant's own description of a rare weapon, the downstairs neighbor testified \
             he heard the defendant threaten to kill his father and then heard the body fall, \
             and the woman across the street said she saw the stabbing through the windows of \
             a passing elevated train.",
            "--topic-keys",
            "verdict,12-angry-men",
            "--options",
            "Guilty,Not guilty,Hung jury (cannot agree)",
            "--chose",
            "Guilty",
            "--rests-on-evidence",
            "Trial testimony: the downstairs neighbor says he heard 'I'm gonna kill you' \
             followed by a body hitting the floor, one second after a train passed",
            "--evidence-source",
            "trial transcript, witness: downstairs neighbor",
            "--project",
            "12-angry-men",
            "--question",
            "How does the jury find the defendant?",
            "--quote",
            "That's eleven guilty. Somebody's got a reasonable doubt?",
        ],
    )?)?;

    hm(
        dir,
        "human:juror-8",
        &[
            "disagree",
            "--decision",
            &guilty,
            "--reason",
            "The knife the defendant claimed was one-of-a-kind is not unique at all — I bought \
             an identical one two blocks from his house for six dollars. If I could find one \
             so quickly, so could he, and his alibi becomes possible again.",
        ],
    )?;

    let reenactment_timing = id_of(&hm(
        dir,
        "human:juror-8",
        &[
            "emit",
            "evidence.recorded",
            "--actor-id",
            "human:juror-8",
            "--source",
            "human",
            "--content",
            "Re-enactment in the jury room: walking from the bed to the front door in the \
             defendant's apartment takes 41 seconds using his own cane, not the 15 seconds the \
             old man upstairs claimed for hearing the threat, running to the door, and seeing \
             the defendant flee.",
            "--source-ref",
            "jury-room re-enactment, timed by juror 8",
        ],
    )?)?;

    let eyewitness_glasses = id_of(&hm(
        dir,
        "human:juror-9",
        &[
            "emit",
            "evidence.recorded",
            "--actor-id",
            "human:juror-9",
            "--source",
            "human",
            "--content",
            "The woman across the street testified she saw the stabbing through the windows of \
             a passing elevated train, but she was also seen with fresh marks on her nose from \
             eyeglasses — she would not have been wearing them in bed, and without them an \
             identification across the street at night is unreliable.",
            "--source-ref",
            "trial transcript, cross-examination of eyewitness",
        ],
    )?)?;

    let not_guilty = new_decision_id_of(&hm(
        dir,
        "human:juror-foreman",
        &[
            "supersede",
            "--old",
            &guilty,
            "--title",
            "The jury finds the defendant not guilty",
            "--rationale",
            "A second knife identical to the one in evidence turned up two blocks from the \
             apartment, undercutting the claim it was one of a kind. The re-enactment showed \
             the upstairs witness could not have reached his door in time to see what he \
             described. And the only eyewitness needed glasses she was not wearing in bed. \
             None of this proves innocence, but it is enough reasonable doubt that all twelve \
             of us can no longer say guilty beyond it.",
            "--options",
            "Guilty,Not guilty",
            "--chose",
            "Not guilty",
            "--evidence",
            &format!("{reenactment_timing},{eyewitness_glasses}"),
        ],
    )?)?;

    hm(
        dir,
        "human:juror-3",
        &[
            "disagree",
            "--decision",
            &not_guilty,
            "--reason",
            "Beyond a reasonable doubt doesn't mean giving him the benefit of every maybe. I \
             still believe the boy did it. I'm entitled to that as long as I have a reason.",
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI helpers
// ---------------------------------------------------------------------------

/// Runs one `hivemind` command through the real command layer (`hivemind::cli::run`) — the
/// same code path the built binary uses, not a hand-written ledger write. `actor` is always
/// explicit (never the environment-derived default), so this generator can never pick up this
/// city's own actor identity, session ids, or rig name.
fn hm(dir: &Path, actor: &str, args: &[&str]) -> TestResult<Value> {
    let mut argv: Vec<String> = vec![
        "hivemind".to_owned(),
        "--json".to_owned(),
        "--hivemind-dir".to_owned(),
        dir.display().to_string(),
        "--actor".to_owned(),
        actor.to_owned(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let cli = Cli::parse_from(argv);
    let output = run(&cli).map_err(|error| format!("{args:?} failed: {error}"))?;
    Ok(serde_json::from_str(&output)?)
}

fn id_of(v: &Value) -> TestResult<String> {
    v["value"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("expected a string \"value\" (created id) in {v}").into())
}

/// A `supersede` response has no top-level "value" (that's the `emit` envelope shape); the
/// replacement decision's id is `new_decision_id`.
fn new_decision_id_of(v: &Value) -> TestResult<String> {
    v["new_decision_id"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("expected \"new_decision_id\" in {v}").into())
}

/// The id of the hypothesis a capture created inline via `--rests-on-assumption`, from that
/// capture's own response (`rests_on[0].id`) — so a later step can refute the same node instead
/// of recording a fresh, disconnected assumption.
fn first_rests_on_id(v: &Value) -> TestResult<String> {
    v["rests_on"][0]["id"]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("expected rests_on[0].id in {v}").into())
}

// ---------------------------------------------------------------------------
// API export — the same axum router the `hivemind serve` binary runs, called in-process
// (tower's oneshot, no TCP bind) so the exported JSON is exactly what a real GET returns.
// ---------------------------------------------------------------------------

async fn export_snapshot(hivemind_dir: &Path) -> TestResult<(Value, Value)> {
    let config = ApiConfig {
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
    };
    let router = create_router(&config);

    let graph = call_json(&router, "/v1/graph").await?;

    let decisions = graph["decisions"]
        .as_array()
        .ok_or("graph.decisions should be an array")?;

    let mut briefs = serde_json::Map::new();
    for decision in decisions {
        let id = decision["id"]
            .as_str()
            .ok_or("each graph decision should have a string id")?;
        let brief = call_json(&router, &format!("/v1/decisions/verify?id={id}")).await?;
        briefs.insert(id.to_owned(), brief);
    }

    Ok((graph, Value::Object(briefs)))
}

async fn call_json(router: &axum::Router, path: &str) -> TestResult<Value> {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())?;
    let response = router.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = response.into_body().collect().await?.to_bytes();
    if status != StatusCode::OK {
        return Err(format!(
            "GET {path} returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

// ---------------------------------------------------------------------------
// Shape check — this is what CI actually enforces on every PR. It does not diff against the
// committed snapshot byte-for-byte (decision ids and timestamps are freshly generated on every
// real capture); it enforces the demo's contract: every decision has a brief, staleness stays
// visible, and the three named projects (plus the courtroom) are present.
// ---------------------------------------------------------------------------

fn check_snapshot(graph: &Value, briefs: &Value) -> TestResult<()> {
    let decisions = graph["decisions"]
        .as_array()
        .ok_or("graph.decisions should be an array")?;
    if decisions.len() != 8 {
        return Err(format!(
            "expected 8 decisions in the example-data story, found {}",
            decisions.len()
        )
        .into());
    }

    let briefs_obj = briefs
        .as_object()
        .ok_or("briefs snapshot should be a JSON object")?;
    if briefs_obj.len() != decisions.len() {
        return Err(format!(
            "briefs.json should have one entry per decision ({} decisions, {} briefs)",
            decisions.len(),
            briefs_obj.len()
        )
        .into());
    }
    for decision in decisions {
        let id = decision["id"].as_str().unwrap_or_default();
        if !briefs_obj.contains_key(id) {
            return Err(format!("briefs.json is missing decision {id}").into());
        }
    }

    let project_labels: std::collections::BTreeSet<&str> = graph["nodes"]
        .as_array()
        .ok_or("graph.nodes should be an array")?
        .iter()
        .filter(|n| n["kind"] == "Project")
        .filter_map(|n| n["label"].as_str())
        .collect();
    for expected in [
        "TrailKeeper",
        "Ridewell API",
        "Ridewell Mobile",
        "12 Angry Men",
    ] {
        if !project_labels.contains(expected) {
            return Err(format!(
                "expected project {expected:?} in the example-data story, found {project_labels:?}"
            )
            .into());
        }
    }

    let statuses: Vec<&str> = decisions
        .iter()
        .filter_map(|d| d["status"].as_str())
        .collect();
    if !statuses.contains(&"superseded") {
        return Err(
            "expected at least one superseded decision (staleness must stay visible)".into(),
        );
    }
    if !statuses.contains(&"contested") {
        return Err("expected at least one contested decision (disagreement must survive)".into());
    }

    Ok(())
}

fn write_snapshot(graph: &Value, briefs: &Value) -> TestResult<()> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let snapshot_dir = manifest_dir.join(SNAPSHOT_DIR);
    std::fs::create_dir_all(&snapshot_dir)?;
    std::fs::write(
        snapshot_dir.join("graph.json"),
        format!("{}\n", serde_json::to_string_pretty(graph)?),
    )?;
    std::fs::write(
        snapshot_dir.join("briefs.json"),
        format!("{}\n", serde_json::to_string_pretty(briefs)?),
    )?;
    Ok(())
}

fn array_len(v: &Value) -> usize {
    v.as_array().map(Vec::len).unwrap_or(0)
}

fn object_len(v: &Value) -> usize {
    v.as_object().map(serde_json::Map::len).unwrap_or(0)
}
