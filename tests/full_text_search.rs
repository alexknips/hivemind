use std::fs;
use std::path::Path;

use clap::Parser;
use hivemind::cli::{run, Cli};
use rusqlite::{params, Connection, OptionalExtension};

#[allow(dead_code)]
#[path = "support/seed_data.rs"]
mod seed_data;

use seed_data::{seed_to_dir, unique_temp_dir, TestResult};

#[test]
fn query_search_rebuilds_fts_index_from_seed_ledger() -> TestResult<()> {
    let first = unique_temp_dir("fts-first");
    let second = unique_temp_dir("fts-second");
    seed_to_dir(&first)?;
    seed_to_dir(&second)?;

    let first_output = run_seed_search(&first)?;
    let second_output = run_seed_search(&second)?;
    assert_eq!(decision_ids(&first_output), vec!["decision-020"]);
    assert_eq!(decision_ids(&first_output), decision_ids(&second_output));
    assert_fts_table_exists(&first)?;

    let _ = fs::remove_dir_all(&first);
    let _ = fs::remove_dir_all(&second);
    Ok(())
}

fn run_seed_search(seed_dir: &Path) -> TestResult<serde_json::Value> {
    let output = run(&Cli::parse_from([
        "hivemind",
        "--hivemind-dir",
        seed_dir.to_str().ok_or("seed path must be utf-8")?,
        "query",
        "search",
        "--q",
        "delta-mirror",
        "--limit",
        "5",
    ]))?;
    Ok(serde_json::from_str(&output)?)
}

fn decision_ids(output: &serde_json::Value) -> Vec<&str> {
    output["data"]["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| item["decision"]["id"].as_str().expect("decision id string"))
        .collect()
}

fn assert_fts_table_exists(seed_dir: &Path) -> TestResult<()> {
    let connection = Connection::open(seed_dir.join("ledger.sqlite"))?;
    let table_name: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params!["decision_search_fts"],
            |row| row.get(0),
        )
        .optional()?;
    assert_eq!(table_name.as_deref(), Some("decision_search_fts"));
    Ok(())
}
