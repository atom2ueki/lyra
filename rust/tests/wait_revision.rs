//! `wait_run` semantics: revision-gated returns and timeout-without-stop.
//! Run-level coverage lives here at library level (Q20: the binary ships no
//! invisible-worker backend; runs are driven with the fake spawn backend).

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use common::FakeSpawnBackend;
use lyra::conductor::{RunStatus, StartOptions, WaitOptions, run_dir_for, start_run, wait_run};
use lyra::config::{ApprovalPosture, LyraConfig, MergeMode, WorkerLayout};
use lyra::store::{NewTicket, create_ticket, ensure_store};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    fs::write(dir.path().join("README.md"), "# test\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-qm", "init"]);
    dir
}

fn slow_omp(dir: &Path, sleep_secs: f64) -> std::path::PathBuf {
    let path = dir.join(format!("fake-omp-{sleep_secs}.sh"));
    let script = format!(
        r#"#!/bin/sh
set -eu
case "${{1:-}}" in --version) echo "fake-omp 1.0"; exit 0 ;; esac
cfg="${{LYRA_WORKER_CONFIG:?}}"
python3 - "$cfg" <<'PY'
import json, os, sys, time
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
time.sleep({sleep_secs})
result = {{
    "schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"],
    "claimId": cfg["claimId"], "status": "done", "summary": "slow done",
    "verification": "ok", "evidence": [], "blockers": [],
    "finishedAt": "2026-01-01T00:00:00.000Z",
}}
tmp = ch["result"] + ".tmp"
json.dump(result, open(tmp, "w"))
os.replace(tmp, ch["result"])
PY
"#
    );
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn start_slow_run(repo: &Path, store: &Path, sleep_secs: f64, run_id: &str) {
    create_ticket(
        store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Slow worker".to_string(),
            prompt: "Do it slowly.".to_string(),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            parent: None,
            origin: Some("test".to_string()),
            harness: Some("omp".to_string()),
        },
    )
    .unwrap();
    unsafe { std::env::set_var("LYRA_OMP_BIN", slow_omp(repo, sleep_secs)) };
    let state = start_run(StartOptions {
        repo: repo.to_path_buf(),
        store: store.to_path_buf(),
        project_id: "test-project".to_string(),
        config: LyraConfig {
            max_parallel: 1,
            max_per_provider: Default::default(),
            approval_posture: ApprovalPosture::Ask,
            merge: MergeMode::Auto,
            pane_backend: None,
            worker_layout: WorkerLayout::Auto,
            enabled_harnesses: vec!["omp".to_string()],
        },
        backend: Some(Box::new(FakeSpawnBackend::new())),
        max_parallel: None,
        max_seconds: Some(30),
        ticket_ids: Vec::new(),
        base_ref: None,
        dry_run: false,
        run_id: Some(run_id.to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    assert_eq!(state.status, RunStatus::Running);
}

#[test]
fn wait_returns_once_the_revision_advances() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    start_slow_run(dir.path(), &store, 1.0, "run-wait-advance");
    let run_dir = run_dir_for(dir.path(), "run-wait-advance");

    let outcome = wait_run(WaitOptions {
        run_dir: run_dir.clone(),
        backend: Some(Box::new(FakeSpawnBackend::new())),
        after_revision: Some(1),
        timeout_ms: 5000,
        interval_ms: 25,
        max_parallel: None,
    })
    .unwrap();
    assert!(!outcome.timed_out);
    assert!(outcome.state.revision > 1);
    let settled = wait_run(WaitOptions {
        run_dir,
        backend: Some(Box::new(FakeSpawnBackend::new())),
        after_revision: None,
        timeout_ms: 15000,
        interval_ms: 50,
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(settled.state.status, RunStatus::Done);
}

#[test]
fn wait_timeout_does_not_stop_an_active_run() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    start_slow_run(dir.path(), &store, 5.0, "run-wait-timeout");
    let run_dir = run_dir_for(dir.path(), "run-wait-timeout");

    let outcome = wait_run(WaitOptions {
        run_dir: run_dir.clone(),
        backend: Some(Box::new(FakeSpawnBackend::new())),
        after_revision: Some(2),
        timeout_ms: 300,
        interval_ms: 50,
        max_parallel: None,
    })
    .unwrap();
    assert!(outcome.timed_out);
    assert_eq!(outcome.state.status, RunStatus::Running);

    let settled = wait_run(WaitOptions {
        run_dir,
        backend: Some(Box::new(FakeSpawnBackend::new())),
        after_revision: None,
        timeout_ms: 15000,
        interval_ms: 50,
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(settled.state.status, RunStatus::Done);
}
