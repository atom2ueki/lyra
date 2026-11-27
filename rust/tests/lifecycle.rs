use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lyra::conductor::{
    MonitorOptions, RunStatus, StartOptions, close_run, monitor_run, run_dir_for, start_run,
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
ID="${LYRA_TICKET_ID:?}"
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

case "$SUBJECT" in
  Alpha)
    printf 'alpha\n' > "$WORKTREE/alpha.txt"
    ;;
  Beta)
    printf 'beta\n' > "$WORKTREE/beta.txt"
    ;;
  Gamma)
    if [ "$ATTEMPT" = "1" ]; then fail_result; fi
    printf 'gamma\n' > "$WORKTREE/gamma.txt"
    ;;
  Blocked)
    needs_input_result
    ;;
  *)
    printf 'unknown\n' > "$WORKTREE/unknown.txt"
    ;;
esac

git -C "$WORKTREE" add -A
git -C "$WORKTREE" commit -qm "$ID attempt $ATTEMPT" >/dev/null
printf 'status: done\nsummary: %s complete\nverification: file committed\nblockers:\n' "$ID" > "$RESULT"
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
build = ["sh", "{script}", "{{task}}"]
"#
        ),
    )
    .unwrap();
}

fn start(repo: &Path, store: &Path) -> lyra::conductor::RunState {
    let config = ProjectConfig::load(repo).unwrap();
    start_run(StartOptions {
        repo: repo.to_path_buf(),
        store: store.to_path_buf(),
        project_id: "proj-test".to_string(),
        config,
        max_parallel: None,
        max_seconds: None,
        ticket_ids: Vec::new(),
        base_ref: None,
        dry_run: false,
        run_id: Some("run-lifecycle".to_string()),
    })
    .unwrap()
}

fn wait_for_settle(run_dir: &Path, timeout: Duration) -> lyra::conductor::RunState {
    let deadline = Instant::now() + timeout;
    loop {
        let state = monitor_run(MonitorOptions {
            run_dir: run_dir.to_path_buf(),
            once: true,
            interval_ms: 10,
            timeout_ms: Some(200),
            max_parallel: None,
        })
        .unwrap();
        if matches!(
            state.status,
            RunStatus::Done | RunStatus::Blocked | RunStatus::Stopped | RunStatus::Closed
        ) {
            return state;
        }
        assert!(Instant::now() < deadline, "run did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn complex_dag_runs_in_parallel_settles_and_retries() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let alpha = new_ticket(&store, "Alpha", vec![]);
    let beta = new_ticket(&store, "Beta", vec![alpha.as_str()]);
    let gamma = new_ticket(&store, "Gamma", vec![beta.as_str()]);

    let started = start(repo.path(), &store);
    assert_eq!(started.run_id, "run-lifecycle");
    let state = wait_for_settle(Path::new(&started.run_dir), Duration::from_secs(20));

    assert_eq!(state.status, RunStatus::Done, "{state:#?}");
    assert_eq!(state.backend, "process");
    for id in [&alpha, &beta, &gamma] {
        let ticket = state.tickets.get(id).unwrap();
        assert_eq!(ticket.status, lyra::conductor::RunTicketStatus::Done);
        assert_eq!(ticket.attempt, if id == &gamma { 2 } else { 1 });
    }
    let tickets = list_tickets(&store).unwrap();
    assert!(
        tickets
            .iter()
            .all(|ticket| ticket.status == TicketStatus::Done)
    );

    let run_dir = PathBuf::from(&state.run_dir);
    assert!(run_dir.join(format!("workers/{alpha}/worker.log")).exists());
    assert!(run_dir.join(format!("workers/{alpha}/RESULT.md")).exists());
    assert!(
        run_dir
            .join(format!("worktrees/{alpha}/alpha.txt"))
            .exists()
    );
    assert!(
        run_dir
            .join(format!("worktrees/{gamma}/gamma.txt"))
            .exists()
    );

    let closed = close_run(&run_dir).unwrap();
    assert_eq!(closed.status, RunStatus::Closed);
    assert!(!run_dir.join(format!("worktrees/{alpha}")).exists());
    assert!(!run_dir.join(format!("worktrees/{gamma}")).exists());
}

#[test]
fn needs_input_parks_the_ticket_and_keeps_its_worktree() {
    let repo = repo();
    let script = worker_script(repo.path());
    write_config(repo.path(), &script);
    let store = store_dir_for(repo.path());
    ensure_store(&store).unwrap();
    let blocked = new_ticket(&store, "Blocked", vec![]);

    let started = start(repo.path(), &store);
    let state = wait_for_settle(Path::new(&started.run_dir), Duration::from_secs(10));
    let ticket = state.tickets.get(&blocked).unwrap();
    assert_eq!(ticket.status, lyra::conductor::RunTicketStatus::Blocked);
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
fn run_dir_is_stable_for_a_run_id() {
    let repo = repo();
    let expected = run_dir_for(repo.path(), "run-fixed");
    assert_eq!(run_dir_for(repo.path(), "run-fixed"), expected);
    assert!(expected.ends_with("run-fixed"));
}
