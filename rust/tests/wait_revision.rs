use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::Duration;

struct Fixture {
    repo: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
}

fn fixture() -> Fixture {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    fs::write(repo.path().join("README.md"), "test\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-qm", "init"]);
    let home = repo.path().join(".home");
    fs::create_dir_all(&home).unwrap();
    let bin = repo.path().join("fake-omp.sh");
    let script = r#"#!/bin/sh
set -eu
case "${1:-}" in
  models) printf '{"models":[]}\n'; exit 0 ;;
  usage) printf '{"providers":[]}\n'; exit 0 ;;
esac
cfg="${LYRA_WORKER_CONFIG:?}"
python3 - "$cfg" <<'PY'
import json, os, sys, time
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
time.sleep(1.0)
result = {
    "schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"],
    "claimId": cfg["claimId"], "status": "done", "summary": "slow done",
    "verification": "ok", "evidence": [], "blockers": [],
    "finishedAt": "2026-01-01T00:00:00.000Z",
}
tmp = ch["result"] + ".tmp"
json.dump(result, open(tmp, "w"))
os.replace(tmp, ch["result"])
PY
"#;
    fs::write(&bin, script).unwrap();
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin, perms).unwrap();
    Fixture { repo, home, bin }
}

fn run(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lyra"))
        .args(args)
        .current_dir(fixture.repo.path())
        .env("LYRA_HOME", &fixture.home)
        .env("LYRA_OMP_BIN", &fixture.bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn okay_json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn start(fixture: &Fixture) -> serde_json::Value {
    let init = run(fixture, &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = run(fixture, &["add", "Slow worker"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    okay_json(&run(
        fixture,
        &[
            "conduct",
            "start",
            "--backend",
            "headless",
            "--run-id",
            "run-wait",
            "--json",
            "--concise",
        ],
    ))
}

#[test]
fn wait_timeout_does_not_stop_an_active_run() {
    let fixture = fixture();
    let started = start(&fixture);
    assert_eq!(started["status"], "running");
    assert!(started["revision"].as_u64().unwrap() >= 1);

    let waited = okay_json(&run(
        &fixture,
        &[
            "conduct",
            "wait",
            "--run",
            "run-wait",
            "--timeout-ms",
            "100",
            "--interval-ms",
            "10",
            "--backend",
            "headless",
            "--json",
            "--concise",
        ],
    ));
    assert_eq!(waited["timedOut"], true);
    assert_eq!(waited["status"], "running");

    let report = okay_json(&run(
        &fixture,
        &["report", "--run", "run-wait", "--json", "--concise"],
    ));
    assert_eq!(report["status"], "running");
}

#[test]
fn wait_settles_a_slow_result_and_increments_revision() {
    let fixture = fixture();
    start(&fixture);
    let before = okay_json(&run(
        &fixture,
        &[
            "conduct",
            "wait",
            "--run",
            "run-wait",
            "--timeout-ms",
            "100",
            "--interval-ms",
            "10",
            "--backend",
            "headless",
            "--json",
            "--concise",
        ],
    ));
    let revision = before["revision"].as_u64().unwrap();

    sleep(Duration::from_millis(1200));
    let after = okay_json(&run(
        &fixture,
        &[
            "conduct",
            "wait",
            "--run",
            "run-wait",
            "--after-revision",
            &revision.to_string(),
            "--timeout-ms",
            "2000",
            "--interval-ms",
            "50",
            "--backend",
            "headless",
            "--json",
            "--concise",
        ],
    ));
    assert_eq!(after["status"], "done");
    assert_eq!(after["timedOut"], false);
    assert!(after["revision"].as_u64().unwrap() > revision);
    assert_eq!(after["tickets"][0]["status"], "done");
}

#[test]
fn wait_returns_immediately_when_the_revision_is_newer() {
    let fixture = fixture();
    start(&fixture);
    let current = okay_json(&run(
        &fixture,
        &["report", "--run", "run-wait", "--json", "--concise"],
    ));
    let revision = current["revision"].as_u64().unwrap();
    let waited = okay_json(&run(
        &fixture,
        &[
            "conduct",
            "wait",
            "--run",
            "run-wait",
            "--after-revision",
            &revision.saturating_sub(1).to_string(),
            "--timeout-ms",
            "2000",
            "--interval-ms",
            "50",
            "--backend",
            "headless",
            "--json",
            "--concise",
        ],
    ));
    assert_eq!(waited["timedOut"], false);
    assert_eq!(waited["revision"], revision);
}

#[test]
fn old_state_files_without_revision_load_as_zero() {
    let fixture = fixture();
    let state = start(&fixture);
    assert!(state["revision"].as_u64().unwrap() >= 1);
    let runs = fixture.home.join("runs");
    let project = fs::read_dir(&runs).unwrap().next().unwrap().unwrap().path();
    let run_dir = project.join("run-wait");
    let path = run_dir.join("state.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("revision");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(serde_json::to_string_pretty(&value).unwrap().as_bytes())
        .unwrap();
    let report = okay_json(&run(
        &fixture,
        &["report", "--run", "run-wait", "--json", "--concise"],
    ));
    assert_eq!(report["revision"], 0);
}
