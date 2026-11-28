use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lyra::conductor::{
    MonitorOptions, RunStatus, RunTicketStatus, StartOptions, approve_ticket, close_run,
    monitor_run, reject_ticket, run_dir_for, start_run,
};
use lyra::project::store_dir_for;
use lyra::project_config::ProjectConfig;
use lyra::schema::TicketStatus;
use lyra::store::{NewTicket, create_ticket, ensure_store, list_tickets};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {}", args.join(" "));
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    fs::write(dir.path().join("README.md"), "test\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-qm", "init"]);
    dir
}

fn worker_script(dir: &Path) -> PathBuf {
    let path = dir.join("fake-worker.sh");
    let script = r#"#!/bin/sh
set -eu
MODE="${1:-build}"
SUBJECT="${LYRA_TICKET_SUBJECT:?}"
ATTEMPT="${LYRA_ATTEMPT:-1}"
RESULT="${LYRA_RESULT_FILE:?}"
WORKTREE="${LYRA_WORKTREE:?}"

fail_result() {
  printf 'status: failed\nsummary: simulated failure on attempt %s\nverification: none\nblockers:\n' "$ATTEMPT" > "$RESULT"
  exit 0
}

needs_input_result() {
  printf 'status: needs_input\nsummary: need a credential\nverification: none\nblockers:\n' > "$RESULT"
  exit 0
}

if [ "$MODE" = "plan" ]; then
  if [ "$SUBJECT" = "Blocked" ]; then needs_input_result; fi
  printf '# Plan for %s\n\n1. Inspect the repository\n2. Implement\n3. Verify\n' "$SUBJECT" > "$LYRA_PLAN_FILE"
  printf 'status: done\nsummary: plan ready for %s\nverification: plan only, no code changed\nblockers:\n' "$SUBJECT" > "$RESULT"
  exit 0
fi

case "$SUBJECT" in
  Alpha)
    printf 'alpha %s\n' "$ATTEMPT" > "$WORKTREE/alpha.txt"
    ;;
  Beta)
    printf 'beta %s\n' "$ATTEMPT" > "$WORKTREE/beta.txt"
    ;;
  Gamma)
    MARKER="${RESULT}.first-build"
    if [ ! -f "$MARKER" ]; then
      : > "$MARKER"
      fail_result
    fi
    printf 'gamma %s\n' "$ATTEMPT" > "$WORKTREE/gamma.txt"
    ;;
  Blocked)
    needs_input_result
    ;;
  *)
    printf 'unknown\n' > "$WORKTREE/unknown.txt"
    ;;
esac

git -C "$WORKTREE" add -A
git -C "$WORKTREE" commit -qm "$SUBJECT attempt $ATTEMPT" >/dev/null
printf 'status: done\nsummary: %s complete\nverification: file committed\nblockers:\n' "$SUBJECT" > "$RESULT"
"#;
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn new_ticket(store: &Path, subject: &str, blocked_by: Vec<&str>) -> String {
    create_ticket(
        store,
        NewTicket {
            project_id: "proj-test".to_string(),
            subject: subject.to_string(),
            prompt: format!("Do {subject}"),
            blocked_by: blocked_by.into_iter().map(str::to_string).collect(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            parent: None,
            origin: Some("test".to_string()),
            harness: None,
        },
    )
    .unwrap()
    .id
}

fn write_config(repo: &Path, script: &Path) {
    let script = script.display();
    fs::write(
        repo.join("lyra.toml"),
        format!(
            r#"[project]
base_ref = "main"
max_parallel = 2
max_seconds = 30

[worker]
plan = ["sh", "{script}", "plan"]
build = ["sh", "{script}", "build"]
"#
        ),
    )
    .unwrap();
}

fn start_selected(repo: &Path, store: &Path, ticket_ids: Vec<String>) -> lyra::conductor::RunState {
    let config = ProjectConfig::load(repo).unwrap();
    start_run(StartOptions {
        repo: repo.to_path_buf(),
        store: store.to_path_buf(),
        project_id: "proj-test".to_string(),
        config,
        max_parallel: None,
        max_seconds: None,
        ticket_ids,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-lifecycle".to_string()),
    })
    .unwrap()
}

fn start(repo: &Path, store: &Path) -> lyra::conductor::RunState {
    start_selected(repo, store, Vec::new())
}

fn monitor(run_dir: &Path) -> lyra::conductor::RunState {
    monitor_run(MonitorOptions {
        run_dir: run_dir.to_path_buf(),
        once: true,
        interval_ms: 10,
        timeout_ms: Some(200),
        max_parallel: None,
    })
    .unwrap()
}

fn gate_ids(state: &lyra::conductor::RunState, status: RunTicketStatus) -> Vec<String> {
    let mut ids: Vec<String> = state
        .tickets
        .iter()
        .filter(|(_, ticket)| ticket.status == status)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Drive until every gate has been approved. This is the human loop in the
/// acceptance test: plan gate -> build -> review gate -> merge.
fn drive(run_dir: &Path, timeout: Duration) -> lyra::conductor::RunState {
    let deadline = Instant::now() + timeout;
    loop {
        let state = monitor(run_dir);
        match state.status {
            RunStatus::Done | RunStatus::Blocked | RunStatus::Stopped | RunStatus::Closed => {
                return state;
            }
            RunStatus::Awaiting => {
                let plans = gate_ids(&state, RunTicketStatus::AwaitingPlan);
                let reviews = gate_ids(&state, RunTicketStatus::AwaitingReview);
                if plans.is_empty() && reviews.is_empty() {
                    panic!("awaiting with no gate: {state:#?}");
                }
                for id in plans {
                    approve_ticket(run_dir, &id, true).unwrap();
                }
                for id in reviews {
                    approve_ticket(run_dir, &id, true).unwrap();
                }
            }
            _ => {
                assert!(Instant::now() < deadline, "run did not settle: {state:#?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Wait for a specific gate, auto-approving earlier plan gates.
fn wait_for_gate(
    run_dir: &Path,
    target: RunTicketStatus,
    timeout: Duration,
) -> lyra::conductor::RunState {
    let deadline = Instant::now() + timeout;
    loop {
        let state = monitor(run_dir);
        if state.status == RunStatus::Awaiting {
            let target_ids = gate_ids(&state, target);
            if !target_ids.is_empty() {
                return state;
            }
            for id in gate_ids(&state, RunTicketStatus::AwaitingPlan) {
                approve_ticket(run_dir, &id, true).unwrap();
            }
        }
        if matches!(
            state.status,
            RunStatus::Done | RunStatus::Blocked | RunStatus::Stopped | RunStatus::Closed
        ) {
            panic!("run settled before {target:?}: {state:#?}");
        }
        assert!(Instant::now() < deadline, "run did not reach {target:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn complex_dag_plan_review_retry_and_merge() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let alpha = new_ticket(&store, "Alpha", vec![]);
    let beta = new_ticket(&store, "Beta", vec![alpha.as_str()]);
    let gamma = new_ticket(&store, "Gamma", vec![beta.as_str()]);

    let started = start(repo.path(), &store);
    let state = drive(Path::new(&started.run_dir), Duration::from_secs(30));

    assert_eq!(state.status, RunStatus::Done, "{state:#?}");
    assert_eq!(state.backend, "process");
    for (id, attempts) in [(&alpha, 2), (&beta, 2), (&gamma, 3)] {
        let ticket = state.tickets.get(id).unwrap();
        assert_eq!(ticket.status, RunTicketStatus::Done);
        assert_eq!(ticket.attempt, attempts, "{id} attempts");
    }
    let tickets = list_tickets(&store).unwrap();
    assert!(
        tickets
            .iter()
            .all(|ticket| ticket.status == TicketStatus::Done)
    );

    let run_dir = PathBuf::from(&state.run_dir);
    for id in [&alpha, &beta, &gamma] {
        let worker = run_dir.join("workers").join(id);
        assert!(worker.join("PLAN.md").exists());
        assert!(worker.join("ARTIFACT.json").exists());
        assert!(worker.join("artifacts/planning-1.json").exists());
        assert!(worker.join("artifacts/building-2.json").exists());
        assert!(worker.join("RESULT.md").exists());
        let artifact: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(worker.join("ARTIFACT.json")).unwrap())
                .unwrap();
        assert_eq!(artifact["runId"], "run-lifecycle");
        assert_eq!(artifact["ticketId"], id.as_str());
        assert!(artifact["phase"].is_string());
        assert!(artifact["command"].is_array());
    }
    let alpha_artifact: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join(format!("workers/{alpha}/ARTIFACT.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(alpha_artifact["exitCode"], 0);
    assert!(run_dir.join(format!("workers/{alpha}/DIFF.patch")).exists());
    assert!(run_dir.join(format!("workers/{alpha}/PLAN.md")).exists());

    let closed = close_run(&run_dir).unwrap();
    assert_eq!(closed.status, RunStatus::Closed);
    assert!(!run_dir.join(format!("worktrees/{alpha}")).exists());
}

#[test]
fn plan_needs_input_parks_and_keeps_the_worktree() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let blocked = new_ticket(&store, "Blocked", vec![]);

    let started = start(repo.path(), &store);
    let state = drive(Path::new(&started.run_dir), Duration::from_secs(10));
    let ticket = state.tickets.get(&blocked).unwrap();
    assert_eq!(ticket.status, RunTicketStatus::Blocked);
    assert_eq!(state.status, RunStatus::Blocked);

    let stored = list_tickets(&store).unwrap();
    let stored = stored.iter().find(|ticket| ticket.id == blocked).unwrap();
    assert!(stored.suspension.as_ref().unwrap().needs_input);

    let run_dir = PathBuf::from(&state.run_dir);
    close_run(&run_dir).unwrap();
    assert!(
        run_dir.join(format!("worktrees/{blocked}")).exists(),
        "a needs_input worktree must be kept for resume"
    );
}

#[test]
fn review_reject_reruns_build_with_feedback() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let alpha = new_ticket(&store, "Alpha", vec![]);

    let started = start(repo.path(), &store);
    let gate = wait_for_gate(
        Path::new(&started.run_dir),
        RunTicketStatus::AwaitingReview,
        Duration::from_secs(15),
    );
    assert_eq!(gate.tickets[&alpha].status, RunTicketStatus::AwaitingReview);
    let first_attempt = gate.tickets[&alpha].attempt;

    let rejected = reject_ticket(
        Path::new(&started.run_dir),
        &alpha,
        "change alpha.txt before approval",
    )
    .unwrap();
    assert_eq!(rejected.tickets[&alpha].status, RunTicketStatus::Running);
    let gate = wait_for_gate(
        Path::new(&started.run_dir),
        RunTicketStatus::AwaitingReview,
        Duration::from_secs(15),
    );
    assert_eq!(gate.tickets[&alpha].attempt, first_attempt + 1);
    assert!(
        Path::new(&gate.run_dir)
            .join("workers")
            .join(&alpha)
            .join("FEEDBACK.md")
            .exists()
    );

    approve_ticket(Path::new(&started.run_dir), &alpha, true).unwrap();
    let state = drive(Path::new(&started.run_dir), Duration::from_secs(15));
    assert_eq!(state.status, RunStatus::Done);
    assert_eq!(state.tickets[&alpha].status, RunTicketStatus::Done);
}

#[test]
fn merge_conflict_is_detected_without_landing_the_branch() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let alpha = new_ticket(&store, "Alpha", vec![]);

    let started = start(repo.path(), &store);
    wait_for_gate(
        Path::new(&started.run_dir),
        RunTicketStatus::AwaitingReview,
        Duration::from_secs(15),
    );

    // Move the base branch with a conflicting change after the build.
    fs::write(repo.path().join("alpha.txt"), "main version\n").unwrap();
    git(repo.path(), &["add", "alpha.txt"]);
    git(repo.path(), &["commit", "-qm", "conflict on main"]);

    let error = approve_ticket(Path::new(&started.run_dir), &alpha, true).unwrap_err();
    assert!(format!("{error:#}").contains("merge conflict"), "{error:#}");
    let state = lyra::conductor::load_state(Path::new(&started.run_dir)).unwrap();
    assert_eq!(
        state.tickets[&alpha].status,
        RunTicketStatus::AwaitingReview
    );
    assert_eq!(
        state.tickets[&alpha].phase.as_deref(),
        Some("merge_conflict")
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("alpha.txt")).unwrap(),
        "main version\n"
    );
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "merge must be aborted"
    );
}

#[test]
fn explicit_selection_does_not_launch_other_ready_tickets() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let selected = new_ticket(&store, "Alpha", vec![]);
    let other = new_ticket(&store, "Beta", vec![]);

    let started = start_selected(repo.path(), &store, vec![selected.clone()]);
    let state = drive(Path::new(&started.run_dir), Duration::from_secs(20));
    assert_eq!(state.status, RunStatus::Done);
    assert!(state.tickets.contains_key(&selected));
    assert!(!state.tickets.contains_key(&other));
    let stored = list_tickets(&store).unwrap();
    let other = stored.iter().find(|ticket| ticket.id == other).unwrap();
    assert_eq!(other.status, TicketStatus::Todo);
    assert_eq!(other.attempts, 0);
}

#[test]
fn run_dir_is_stable_for_a_run_id() {
    let repo = repo();
    let expected = run_dir_for(repo.path(), "run-fixed");
    assert_eq!(run_dir_for(repo.path(), "run-fixed"), expected);
    assert!(expected.ends_with("run-fixed"));
}
