//! `scripts/cell-update.sh` and the env file a cell is reconciled from
//! (hivemind-2mgj).
//!
//! A self-hosted cell is often reconciled by `docker compose up -d` — a systemd
//! timer, a cron job — which reads `HIVEMIND_IMAGE` from the compose project's
//! env file. An update script that set it only for its own restart was undone
//! by the next reconciliation, which recreated the container on `:latest`. The
//! script therefore pins the sha-tagged image it built in that env file, and
//! keeps the pin only when the new container came up healthy.
//!
//! `docker` and `curl` are stand-ins on `PATH` that log what they were asked
//! and answer from environment variables; the script itself, `git` and `awk`
//! run for real.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tempfile::TempDir;

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const FAKE_DOCKER: &str = r#"#!/bin/sh
echo "$*" >> "$SHIM_LOG"
if [ "$1" = compose ]; then
  echo "HIVEMIND_IMAGE=${HIVEMIND_IMAGE-unset}" >> "$SHIM_LOG"
fi
"#;

const FAKE_CURL: &str = r#"#!/bin/sh
for url; do :; done
case "$url" in
  */v1/health) [ "${SHIM_HEALTHY:-1}" = 1 ] ;;
  */v1/version) printf '{"sha":"%s"}' "$SHIM_VERSION_SHA" ;;
esac
"#;

/// The tests write executables and then run them; a `fork` from a sibling test
/// thread in between can hold the write descriptor open and fail the `exec`
/// with ETXTBSY. Running the tests one at a time rules that out.
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct Run {
    /// `/v1/health` never answers.
    unhealthy: bool,
    /// What `/v1/version` reports instead of the sha just built.
    version_sha: Option<String>,
    /// `HIVEMIND_COMPOSE_FILE`; unset runs the script's default.
    compose_file: Option<String>,
    /// `HIVEMIND_ENV_FILE`.
    env_file: Option<PathBuf>,
}

struct Cell {
    _serial: MutexGuard<'static, ()>,
    root: TempDir,
    short_sha: String,
}

impl Cell {
    fn new() -> TestResult<Self> {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let root = TempDir::new()?;
        let cell = Self {
            _serial: serial,
            short_sha: String::new(),
            root,
        };
        for dir in [cell.repo(), cell.dir(), cell.bin()] {
            fs::create_dir(dir)?;
        }
        for name in ["docker-compose.yml", "docker-compose.local-agents.yml"] {
            fs::write(cell.dir().join(name), "")?;
        }
        for (name, body) in [("docker", FAKE_DOCKER), ("curl", FAKE_CURL)] {
            let path = cell.bin().join(name);
            fs::write(&path, body)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        git(&cell.repo(), &["init", "-q"])?;
        git(
            &cell.repo(),
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        )?;
        let short_sha = git(&cell.repo(), &["rev-parse", "--short=12", "HEAD"])?;
        Ok(Self { short_sha, ..cell })
    }

    /// The checkout the script runs from.
    fn repo(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    /// The compose project directory, apart from the checkout as on a real cell.
    fn dir(&self) -> PathBuf {
        self.root.path().join("cell")
    }

    fn bin(&self) -> PathBuf {
        self.root.path().join("bin")
    }

    fn log(&self) -> PathBuf {
        self.root.path().join("docker.log")
    }

    fn env_file(&self) -> PathBuf {
        self.dir().join(".env")
    }

    fn image(&self) -> String {
        format!("hivemind:{}", self.short_sha)
    }

    /// Both compose files, as `HIVEMIND_COMPOSE_FILE` takes them.
    fn compose_files(&self) -> String {
        format!(
            "{}:{}",
            self.dir().join("docker-compose.yml").display(),
            self.dir().join("docker-compose.local-agents.yml").display()
        )
    }

    fn write_env(&self, contents: &str, mode: u32) -> TestResult<()> {
        write_env_file(&self.env_file(), contents, mode)
    }

    fn run(&self, run: Run) -> TestResult<Output> {
        let path = std::env::join_paths(std::iter::once(self.bin()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))?;
        let mut command = Command::new("sh");
        command
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cell-update.sh"))
            .arg("HEAD")
            .current_dir(self.repo())
            .env("PATH", path)
            .env("SHIM_LOG", self.log())
            .env("SHIM_HEALTHY", if run.unhealthy { "0" } else { "1" })
            .env(
                "SHIM_VERSION_SHA",
                run.version_sha.as_deref().unwrap_or(&self.short_sha),
            )
            .env("HIVEMIND_HEALTH_RETRIES", "1")
            .env_remove("HIVEMIND_IMAGE")
            .env_remove("HIVEMIND_PORT")
            .env_remove("HIVEMIND_HEALTH_URL")
            .env_remove("HIVEMIND_VERSION_URL")
            .env_remove("HIVEMIND_COMPOSE_FILE")
            .env_remove("HIVEMIND_ENV_FILE");
        if let Some(compose_file) = &run.compose_file {
            command.env("HIVEMIND_COMPOSE_FILE", compose_file);
        }
        if let Some(env_file) = &run.env_file {
            command.env("HIVEMIND_ENV_FILE", env_file);
        }
        Ok(command.output()?)
    }

    /// A run against the cell's two compose files.
    fn update(&self) -> TestResult<Output> {
        self.run(Run {
            compose_file: Some(self.compose_files()),
            ..Run::default()
        })
    }

    fn docker_calls(&self) -> TestResult<Vec<String>> {
        Ok(fs::read_to_string(self.log())?
            .lines()
            .map(str::to_owned)
            .collect())
    }
}

fn git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git").args(args).current_dir(dir).output()?;
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn write_env_file(path: &Path, contents: &str, mode: u32) -> TestResult<()> {
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn mode_of(path: &Path) -> TestResult<u32> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn update_pins_the_built_image_where_the_reconciler_reads_it() -> TestResult<()> {
    let cell = Cell::new()?;
    // No trailing newline, an old pin, and a mode looser than 0600: the pin
    // replaces the old line in place and nothing else about the file changes.
    cell.write_env(
        "POSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE=hivemind:000000000000\nHIVEMIND_PORT=8080",
        0o640,
    )?;

    let output = cell.update()?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(cell.env_file())?,
        format!(
            "POSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE={}\nHIVEMIND_PORT=8080\n",
            cell.image()
        )
    );
    assert_eq!(mode_of(&cell.env_file())?, 0o640);

    // The restart goes through every compose file and the same env file the
    // pin was written to — the reconciler's view, not just the script's.
    let calls = cell.docker_calls()?;
    let build = calls
        .iter()
        .find(|call| call.starts_with("build "))
        .unwrap();
    assert!(build.contains(&format!("-t {} ", cell.image())), "{build}");
    let dir = cell.dir();
    assert!(
        calls.contains(&format!(
            "compose -f {d}/docker-compose.yml -f {d}/docker-compose.local-agents.yml \
             --env-file {d}/.env up -d --no-deps hivemind",
            d = dir.display()
        )),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("HIVEMIND_IMAGE={}", cell.image())),
        "{calls:?}"
    );
    Ok(())
}

#[test]
fn update_collapses_stale_pins_and_leaves_comments_alone() -> TestResult<()> {
    let cell = Cell::new()?;
    cell.write_env(
        "export HIVEMIND_IMAGE=a\nPOSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE = b\n\
         # HIVEMIND_IMAGE=commented\n",
        0o600,
    )?;

    let output = cell.update()?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(cell.env_file())?,
        format!(
            "HIVEMIND_IMAGE={}\nPOSTGRES_PASSWORD=s3cret\n# HIVEMIND_IMAGE=commented\n",
            cell.image()
        )
    );
    Ok(())
}

#[test]
fn update_appends_the_pin_when_the_env_file_has_none() -> TestResult<()> {
    let cell = Cell::new()?;
    cell.write_env("# the cell\nPOSTGRES_PASSWORD=s3cret\n", 0o600)?;

    let output = cell.update()?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(cell.env_file())?,
        format!(
            "# the cell\nPOSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE={}\n",
            cell.image()
        )
    );
    Ok(())
}

#[test]
fn update_creates_the_env_file_owner_only_when_there_is_none() -> TestResult<()> {
    let cell = Cell::new()?;
    assert!(!cell.env_file().exists());

    let output = cell.update()?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(cell.env_file())?,
        format!("HIVEMIND_IMAGE={}\n", cell.image())
    );
    assert_eq!(mode_of(&cell.env_file())?, 0o600);
    Ok(())
}

#[test]
fn updating_twice_to_the_same_ref_leaves_the_env_file_unchanged() -> TestResult<()> {
    let cell = Cell::new()?;
    cell.write_env("POSTGRES_PASSWORD=s3cret\n", 0o600)?;

    assert!(cell.update()?.status.success());
    let first = fs::read_to_string(cell.env_file())?;
    assert!(cell.update()?.status.success());

    assert_eq!(fs::read_to_string(cell.env_file())?, first);
    assert_eq!(first.matches("HIVEMIND_IMAGE=").count(), 1);
    Ok(())
}

#[test]
fn default_run_pins_next_to_the_default_compose_file() -> TestResult<()> {
    let cell = Cell::new()?;

    let output = cell.run(Run::default())?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(cell.repo().join(".env"))?,
        format!("HIVEMIND_IMAGE={}\n", cell.image())
    );
    assert!(
        cell.docker_calls()?.contains(
            &"compose -f docker-compose.yml --env-file ./.env up -d --no-deps hivemind".to_owned()
        ),
        "{:?}",
        cell.docker_calls()?
    );
    Ok(())
}

#[test]
fn env_file_override_is_pinned_and_handed_to_compose() -> TestResult<()> {
    let cell = Cell::new()?;
    let elsewhere = cell.root.path().join("elsewhere.env");
    write_env_file(&elsewhere, "POSTGRES_PASSWORD=s3cret\n", 0o600)?;

    let output = cell.run(Run {
        compose_file: Some(cell.compose_files()),
        env_file: Some(elsewhere.clone()),
        ..Run::default()
    })?;
    assert!(output.status.success(), "{}", stderr(&output));

    assert_eq!(
        fs::read_to_string(&elsewhere)?,
        format!(
            "POSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE={}\n",
            cell.image()
        )
    );
    assert!(!cell.env_file().exists());
    assert!(
        cell.docker_calls()?
            .iter()
            .any(|call| call.contains(&format!("--env-file {} up", elsewhere.display()))),
        "{:?}",
        cell.docker_calls()?
    );
    Ok(())
}

#[test]
fn unhealthy_update_puts_the_env_file_back_as_it_was() -> TestResult<()> {
    let cell = Cell::new()?;
    let before =
        "POSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE=hivemind:000000000000\nHIVEMIND_PORT=8080";
    cell.write_env(before, 0o640)?;

    let output = cell.run(Run {
        unhealthy: true,
        compose_file: Some(cell.compose_files()),
        ..Run::default()
    })?;

    assert!(!output.status.success());
    assert!(stderr(&output).contains("did not become healthy"));
    // The container was replaced; the env file is what a reconciler will
    // converge it back to, so it must not keep naming the unhealthy image.
    assert_eq!(fs::read_to_string(cell.env_file())?, before);
    assert_eq!(mode_of(&cell.env_file())?, 0o640);
    Ok(())
}

#[test]
fn unhealthy_update_removes_an_env_file_it_created() -> TestResult<()> {
    let cell = Cell::new()?;

    let output = cell.run(Run {
        unhealthy: true,
        compose_file: Some(cell.compose_files()),
        ..Run::default()
    })?;

    assert!(!output.status.success());
    assert!(!cell.env_file().exists());
    Ok(())
}

#[test]
fn update_whose_version_does_not_match_puts_the_env_file_back() -> TestResult<()> {
    let cell = Cell::new()?;
    let before = "POSTGRES_PASSWORD=s3cret\nHIVEMIND_IMAGE=hivemind:000000000000\n";
    cell.write_env(before, 0o600)?;

    let output = cell.run(Run {
        version_sha: Some("ffffffffffff".to_owned()),
        compose_file: Some(cell.compose_files()),
        ..Run::default()
    })?;

    assert!(!output.status.success());
    assert!(stderr(&output).contains("does not mention"));
    assert_eq!(fs::read_to_string(cell.env_file())?, before);
    Ok(())
}
