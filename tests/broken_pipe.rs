//! Regression test for hivemind-t75k: a reader that closes the pipe early
//! (what `| head` does) must not crash hivemind. Rust's `println!` panics on
//! a write failure; main.rs writes with `writeln!` and swallows `BrokenPipe`
//! instead. This spawns the real binary and closes its stdout without
//! reading, the strictest form of "the reader went away first".

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn unique_dir(label: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "hivemind-broken-pipe-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    dir
}

#[test]
fn closing_stdout_early_does_not_panic() {
    let hivemind_dir = unique_dir("recent-decisions");
    let _ = std::fs::create_dir_all(&hivemind_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hivemind"))
        .arg("--hivemind-dir")
        .arg(&hivemind_dir)
        .arg("query")
        .arg("recent_decisions")
        .arg("--since")
        .arg("2020-01-01T00:00:00Z")
        .arg("--limit")
        .arg("500")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hivemind"); // ubs:ignore: test-only; panicking is correct in tests

    // Drop our end of the stdout pipe without reading anything, simulating a
    // reader (like `head`) that has already gone away before hivemind writes.
    drop(child.stdout.take().expect("stdout")); // ubs:ignore: test-only; panicking is correct in tests

    let output = child.wait_with_output().expect("wait for hivemind"); // ubs:ignore: test-only; panicking is correct in tests
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("panicked"),
        "hivemind panicked on a closed stdout pipe instead of exiting quietly: {stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "broken-pipe panic text leaked to stderr: {stderr}"
    );
}
