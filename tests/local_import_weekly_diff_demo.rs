use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use hivemind::cli::{run, Cli};
use serde_json::Value;
use uuid::Uuid;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn local_import_plus_weekly_diff_demo_proves_decisions_added_window() -> TestResult<()> {
    let scratch = TempDir::new("local-import-weekly-diff-demo")?;
    let hivemind_dir = scratch.path().join("hivemind");
    let fixtures_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m2_weekly_diff_demo");
    let last_week_dir = fixtures_root.join("last_week");
    let this_week_dir = fixtures_root.join("this_week");

    let first_import = run_cli_json(
        &hivemind_dir,
        vec![
            "--actor".to_owned(),
            "importer:last-week".to_owned(),
            "import".to_owned(),
            "documents".to_owned(),
            last_week_dir.display().to_string(),
        ],
    )?;
    assert_eq!(
        first_import["summary"]["blocks_imported"],
        Value::from(1u64),
        "last-week fixture should import one decision block",
    );

    let recent_after_first = run_cli_json(
        &hivemind_dir,
        vec![
            "query".to_owned(),
            "get_recent_activity".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
        ],
    )?;
    let boundary_offset = recent_after_first["data"]["items"][0]["event_origin"]
        .as_u64()
        .ok_or("recent activity should expose latest event_origin")?;

    let second_import = run_cli_json(
        &hivemind_dir,
        vec![
            "--actor".to_owned(),
            "importer:this-week".to_owned(),
            "import".to_owned(),
            "documents".to_owned(),
            this_week_dir.display().to_string(),
        ],
    )?;
    assert_eq!(
        second_import["summary"]["blocks_imported"],
        Value::from(1u64),
        "this-week fixture should import one decision block",
    );
    let this_week_run_id = second_import["import_run_id"]
        .as_str()
        .ok_or("this-week import should report an import_run_id")?
        .to_owned();

    let diff = run_cli_json(
        &hivemind_dir,
        vec![
            "query".to_owned(),
            "get_decisions_added_since".to_owned(),
            "--since-offset".to_owned(),
            boundary_offset.to_string(),
            "--limit".to_owned(),
            "50".to_owned(),
        ],
    )?;

    assert_eq!(
        diff["data"]["total_added"],
        Value::from(1u64),
        "only the this-week decision should appear as added",
    );
    assert_eq!(
        diff["data"]["total_changed_existing"],
        Value::from(0u64),
        "the last-week decision must not appear under changed_existing",
    );

    let added = diff["data"]["added_decisions"]
        .as_array()
        .ok_or("added_decisions should be an array")?;
    assert_eq!(added.len(), 1, "exactly one decision should be in window");
    let entry = &added[0];

    let decision_id = entry["decision_id"]
        .as_str()
        .ok_or("added entry needs decision_id")?
        .to_owned();
    assert!(
        decision_id.contains("decision:document:"),
        "decision id should carry document namespace, got {decision_id}"
    );

    let creation = &entry["creation"];
    assert_eq!(
        creation["source"], "document",
        "imported decisions must include source=document provenance",
    );
    assert_eq!(creation["actor_id"], "actor:bob");
    assert_eq!(creation["import_run_id"], Value::String(this_week_run_id));
    assert!(
        creation["source_ref"].as_str().is_some_and(|raw| {
            let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
            parsed["source"] == "document"
                && parsed["block_id"] == "weekly-cache-eviction"
                && parsed["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("cache_eviction.md"))
        }),
        "source_ref must carry document path, block id, and import_run_id",
    );

    let added_topics: BTreeSet<String> = entry["topic_keys"]
        .as_array()
        .ok_or("topic_keys should be an array")?
        .iter()
        .filter_map(|topic| topic.as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(
        added_topics,
        BTreeSet::from(["caching".to_owned(), "performance".to_owned()]),
    );

    let last_week_present_in_window = added
        .iter()
        .chain(
            diff["data"]["changed_existing_decisions"]
                .as_array()
                .into_iter()
                .flatten(),
        )
        .any(|item| {
            item["decision_id"]
                .as_str()
                .is_some_and(|id| id.contains("last-week-import-policy"))
        });
    assert!(
        !last_week_present_in_window,
        "last-week decision must not appear inside a window that starts after its import",
    );

    let resolved_since = diff["data"]["resolved_since"]["offset"]
        .as_u64()
        .ok_or("resolved_since must include offset")?;
    assert_eq!(
        resolved_since, boundary_offset,
        "resolved_since must echo the boundary offset",
    );

    assert!(
        diff["data"]["ledger_range"]["to_offset_inclusive"]
            .as_u64()
            .ok_or("ledger_range needs to_offset_inclusive")?
            > boundary_offset,
        "ledger range should extend past the boundary after second import",
    );

    Ok(())
}

fn run_cli_json(hivemind_dir: &Path, args: Vec<String>) -> TestResult<Value> {
    let mut argv = vec![
        "hivemind".to_owned(),
        "--json".to_owned(),
        "--hivemind-dir".to_owned(),
        hivemind_dir.display().to_string(),
    ];
    argv.extend(args);

    let cli = Cli::parse_from(argv);
    let output = run(&cli)?;
    Ok(serde_json::from_str(&output)?)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> TestResult<Self> {
        let path = std::env::temp_dir().join(format!("hivemind-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
