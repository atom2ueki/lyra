use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

use lyra::conductor::{
    MonitorOptions, RunStatus, RunTicketStatus, StartOptions, close_run, monitor_run, run_dir_for,
    start_run,
};
use lyra::config::{ApprovalPosture, LyraConfig, MergeMode};
use lyra::pane::headless::HeadlessBackend;
use lyra::schema::TicketStatus;
use lyra::store::{NewTicket, create_ticket, ensure_store, list_tickets};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("run git");
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

fn fake_omp(dir: &Path) -> PathBuf {
    let path = dir.join("fake-omp.sh");
    let script = r#"#!/bin/sh
set -eu
cfg="${LYRA_WORKER_CONFIG:?}"
python3 - "$cfg" <<'PY'
import json, os, sys
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
json.dump({"phase": "working", "detail": "fake worker"}, open(ch["state"], "w"))
result = {
    "schemaVersion": 1,
    "ticketId": cfg["ticketId"],
    "runId": cfg["runId"],
    "claimId": cfg["claimId"],
    "status": "done",
    "summary": "fake implementation complete",
    "verification": "fake checks passed",
    "evidence": [],
    "blockers": [],
    "finishedAt": "2026-01-01T00:00:00.000Z",
}
tmp = ch["result"] + ".tmp"
json.dump(result, open(tmp, "w"))
os.replace(tmp, ch["result"])
PY
"#;
    fs::write(&path, script).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn conductor_runs_a_headless_omp_worker_and_closes() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let ticket = create_ticket(
        &store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Implement feature".to_string(),
            prompt: "Implement the feature.".to_string(),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: Some("It works".to_string()),
            plan: Some("Do it".to_string()),
            parent: None,
            origin: Some("test".to_string()),
            harness: Some("omp".to_string()),
        },
    )
    .unwrap();
    let bin = fake_omp(dir.path());
    unsafe { std::env::set_var("LYRA_OMP_BIN", &bin) };
    let config = LyraConfig {
        max_parallel: 2,
        max_per_provider: HashMap::new(),
        approval_posture: ApprovalPosture::Ask,
        merge: MergeMode::Auto,
    };
    let started = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config,
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: None,
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-test".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    assert_eq!(started.status, RunStatus::Running);
    assert_eq!(
        started.tickets.get(&ticket.id).unwrap().status,
        RunTicketStatus::Running
    );
    let run_dir = run_dir_for(dir.path(), "run-test");
    let state = monitor_run(MonitorOptions {
        run_dir: run_dir.clone(),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(5000),
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(state.status, RunStatus::Done);
    assert_eq!(
        state.tickets.get(&ticket.id).unwrap().status,
        RunTicketStatus::Done
    );
    let tickets = list_tickets(&store).unwrap();
    assert_eq!(tickets[0].status, TicketStatus::Done);
    assert!(Path::new(&state.tickets[&ticket.id].worktree).exists());
    let closed = close_run(&run_dir).unwrap();
    assert_eq!(closed.status, RunStatus::Closed);
    assert!(!Path::new(&closed.tickets[&ticket.id].worktree).exists());
}

#[test]
fn suspended_ticket_resumes_in_a_new_run() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let ticket = create_ticket(
        &store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Resume me".to_string(),
            prompt: "Do the work.".to_string(),
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
    let bin = dir.path().join("fake-omp.sh");
    let script = r#"#!/bin/sh
set -eu
cfg="${LYRA_WORKER_CONFIG:?}"
python3 - "$cfg" <<'PY'
import json, os, sys
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
json.dump({"phase": "working"}, open(ch["state"], "w"))
done = os.environ.get("FAKE_DONE") == "1"
result = {
    "schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"],
    "claimId": cfg["claimId"], "status": "done" if done else "needs_input",
    "summary": "done" if done else "need a decision",
    "verification": "", "evidence": [], "blockers": [],
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
    unsafe {
        std::env::set_var("LYRA_OMP_BIN", &bin);
        std::env::set_var("FAKE_DONE", "0");
    }
    let config = LyraConfig {
        max_parallel: 1,
        max_per_provider: HashMap::new(),
        approval_posture: ApprovalPosture::Ask,
        merge: MergeMode::Auto,
    };
    let _first = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config: config.clone(),
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: None,
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-first".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    let first_run = run_dir_for(dir.path(), "run-first");
    let first_state = monitor_run(MonitorOptions {
        run_dir: first_run.clone(),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(5000),
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(first_state.status, RunStatus::Blocked);
    assert_eq!(
        first_state.tickets[&ticket.id].status,
        RunTicketStatus::Blocked
    );
    let suspended = list_tickets(&store).unwrap();
    assert_eq!(suspended[0].status, TicketStatus::Todo);
    assert!(suspended[0].suspension.is_some());
    let worktree = suspended[0].suspension.as_ref().unwrap().worktree.clone();
    assert!(Path::new(&worktree).exists());

    unsafe { std::env::set_var("FAKE_DONE", "1") };
    let second = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config,
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: None,
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-second".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    assert_eq!(second.tickets[&ticket.id].worktree, worktree);
    let second_state = monitor_run(MonitorOptions {
        run_dir: run_dir_for(dir.path(), "run-second"),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(5000),
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(second_state.status, RunStatus::Done);
    assert_eq!(
        second_state.tickets[&ticket.id].status,
        RunTicketStatus::Done
    );
    let finished = list_tickets(&store).unwrap();
    assert_eq!(finished[0].status, TicketStatus::Done);
}

#[test]
fn conductor_schedules_a_four_ticket_dag_two_at_a_time() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let mut ids = Vec::new();
    for (subject, blocked_by) in [
        ("first", Vec::<String>::new()),
        ("second", Vec::<String>::new()),
        ("third", Vec::<String>::new()),
        ("fourth", Vec::<String>::new()),
    ] {
        let ticket = create_ticket(
            &store,
            NewTicket {
                project_id: "test-project".to_string(),
                subject: subject.to_string(),
                prompt: format!("Do {subject}"),
                blocked_by,
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
        ids.push(ticket.id);
    }
    let third = lyra::store::read_ticket(&store, &ids[2]).unwrap().unwrap();
    let fourth = lyra::store::read_ticket(&store, &ids[3]).unwrap().unwrap();
    assert!(third.blocked_by.is_empty() && fourth.blocked_by.is_empty());
    // Rebuild the graph with explicit dependencies: T-003 <- T-001, T-004 <- T-002.
    for (id, blocker) in [(&ids[2], &ids[0]), (&ids[3], &ids[1])] {
        let path = store.join("tasks").join(format!("{id}.json"));
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["blockedBy"] = serde_json::json!([blocker]);
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }
    let bin = fake_omp(dir.path());
    unsafe { std::env::set_var("LYRA_OMP_BIN", &bin) };
    let config = LyraConfig {
        max_parallel: 2,
        max_per_provider: HashMap::new(),
        approval_posture: ApprovalPosture::Ask,
        merge: MergeMode::Auto,
    };
    let started = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config,
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: Some(2),
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-dag".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    let running = started
        .tickets
        .values()
        .filter(|ticket| ticket.status == RunTicketStatus::Running)
        .count();
    assert_eq!(running, 2);
    assert!(!started.tickets.contains_key(&ids[2]));
    assert!(!started.tickets.contains_key(&ids[3]));
    let state = monitor_run(MonitorOptions {
        run_dir: run_dir_for(dir.path(), "run-dag"),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(5000),
        max_parallel: Some(2),
    })
    .unwrap();
    assert_eq!(state.status, RunStatus::Done);
    for id in &ids {
        assert_eq!(state.tickets[id].status, RunTicketStatus::Done);
    }
}

#[test]
fn conductor_applies_model_policy_and_writes_worker_roles() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let ticket = create_ticket(
        &store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Model me".to_string(),
            prompt: "Use the selected model.".to_string(),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: Some("implementation".to_string()),
            acceptance: None,
            plan: None,
            parent: None,
            origin: Some("test".to_string()),
            harness: Some("omp".to_string()),
        },
    )
    .unwrap();
    let bin = fake_omp(dir.path());
    unsafe { std::env::set_var("LYRA_OMP_BIN", &bin) };
    let mut classes = HashMap::new();
    classes.insert("implementation".to_string(), vec!["p/a:high".to_string()]);
    let policy = lyra::models::ModelPolicy {
        classes,
        roles: HashMap::new(),
        limits: lyra::models::ModelLimits {
            providers: HashMap::new(),
        },
    };
    let models = vec![lyra::models::ModelInfo {
        provider: "p".to_string(),
        id: "a".to_string(),
        selector: "p/a".to_string(),
        context_window: None,
        max_tokens: None,
        reasoning: None,
        input: None,
    }];
    let state = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config: LyraConfig {
            max_parallel: 1,
            max_per_provider: HashMap::new(),
            approval_posture: ApprovalPosture::Ask,
            merge: MergeMode::Auto,
        },
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: None,
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-model".to_string()),
        model_policy: Some(policy),
        models: Some(models),
        usage: Some(Vec::new()),
    })
    .unwrap();
    assert_eq!(state.tickets[&ticket.id].model.as_deref(), Some("p/a:high"));
    // The run directory is project-scoped; resolve it from state instead of guessing.
    let worker = PathBuf::from(&state.run_dir)
        .join("workers")
        .join(&ticket.id);
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(worker.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["model"], "p/a:high");
    let roles = fs::read_to_string(worker.join("roles.yml")).unwrap();
    assert!(roles.contains("p/a:high"));
    assert!(worker.join("TASK.md").exists());
}

#[test]
fn discovered_blocker_runs_and_parent_resumes() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let parent = create_ticket(
        &store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Parent".to_string(),
            prompt: "Do parent".to_string(),
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
    let bin = dir.path().join("fake-omp.sh");
    let script = format!(
        r#"#!/bin/sh
set -eu
cfg="${{LYRA_WORKER_CONFIG:?}}"
python3 - "$cfg" <<'PY'
import json, os, sys, time, uuid
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
json.dump({{"phase": "working"}}, open(ch["state"], "w"))
parent = {parent:?}
if cfg["ticketId"] != parent and not cfg.get("continuation"):
    # A blocker discovered by the parent is itself a real ticket; finish it.
    result = {{"schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "done", "summary": "blocker done", "verification": "ok", "evidence": [], "blockers": [], "finishedAt": "2026-01-01T00:00:00.000Z"}}
elif cfg.get("continuation"):
    result = {{"schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "done", "summary": "parent resumed", "verification": "ok", "evidence": [], "blockers": [], "finishedAt": "2026-01-01T00:00:00.000Z"}}
else:
    request_id = str(uuid.uuid4())
    json.dump({{"requestId": request_id, "kind": "blocker", "reason": "need helper", "subject": "Helper", "prompt": "Do helper"}}, open(os.path.join(ch["requests"], request_id + ".json"), "w"))
    response = None
    deadline = time.time() + 5
    while time.time() < deadline:
        path = os.path.join(ch["responses"], request_id + ".json")
        if os.path.exists(path):
            response = json.load(open(path)); break
        time.sleep(0.05)
    result = {{"schemaVersion": 1, "ticketId": cfg["ticketId"], "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "blocked", "summary": "waiting", "verification": "", "evidence": [], "blockers": [response["ticketId"]], "finishedAt": "2026-01-01T00:00:00.000Z"}}
tmp = ch["result"] + ".tmp"
json.dump(result, open(tmp, "w"))
os.replace(tmp, ch["result"])
PY
"#,
        parent = parent.id
    );
    fs::write(&bin, script).unwrap();
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin, perms).unwrap();
    unsafe { std::env::set_var("LYRA_OMP_BIN", &bin) };
    let config = LyraConfig {
        max_parallel: 2,
        max_per_provider: HashMap::new(),
        approval_posture: ApprovalPosture::Ask,
        merge: MergeMode::Auto,
    };
    let _started = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config,
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: Some(2),
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-discovery".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    let state = monitor_run(MonitorOptions {
        run_dir: run_dir_for(dir.path(), "run-discovery"),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(8000),
        max_parallel: Some(2),
    })
    .unwrap();
    assert_eq!(state.status, RunStatus::Done);
    let tickets = list_tickets(&store).unwrap();
    assert_eq!(tickets.len(), 2);
    assert!(
        tickets
            .iter()
            .all(|ticket| ticket.status == TicketStatus::Done)
    );
    assert_eq!(state.tickets[&parent.id].status, RunTicketStatus::Done);
}

#[test]
fn provider_cap_overflows_to_the_next_candidate() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let mut ids = Vec::new();
    for index in 0..4 {
        let ticket = create_ticket(
            &store,
            NewTicket {
                project_id: "test-project".to_string(),
                subject: format!("ticket {index}"),
                prompt: "work".to_string(),
                blocked_by: Vec::new(),
                priority: 2,
                labels: Vec::new(),
                model_class: Some("implementation".to_string()),
                acceptance: None,
                plan: None,
                parent: None,
                origin: Some("test".to_string()),
                harness: Some("omp".to_string()),
            },
        )
        .unwrap();
        ids.push(ticket.id);
    }
    let bin = fake_omp(dir.path());
    unsafe { std::env::set_var("LYRA_OMP_BIN", &bin) };
    let mut classes = HashMap::new();
    classes.insert(
        "implementation".to_string(),
        vec!["dgx/a:high".to_string(), "other/b:high".to_string()],
    );
    let policy = lyra::models::ModelPolicy {
        classes,
        roles: HashMap::new(),
        limits: lyra::models::ModelLimits {
            providers: HashMap::new(),
        },
    };
    let models = vec![
        lyra::models::ModelInfo {
            provider: "dgx".to_string(),
            id: "a".to_string(),
            selector: "dgx/a".to_string(),
            context_window: None,
            max_tokens: None,
            reasoning: None,
            input: None,
        },
        lyra::models::ModelInfo {
            provider: "other".to_string(),
            id: "b".to_string(),
            selector: "other/b".to_string(),
            context_window: None,
            max_tokens: None,
            reasoning: None,
            input: None,
        },
    ];
    let state = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config: LyraConfig {
            max_parallel: 4,
            max_per_provider: HashMap::from([("dgx".to_string(), 2)]),
            approval_posture: ApprovalPosture::Ask,
            merge: MergeMode::Auto,
        },
        backend: Some(Box::new(HeadlessBackend::new())),
        max_parallel: Some(4),
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-cap".to_string()),
        model_policy: Some(policy),
        models: Some(models),
        usage: Some(Vec::new()),
    })
    .unwrap();
    let mut dgx = 0;
    let mut other = 0;
    for id in &ids {
        match state.tickets[id].model.as_deref() {
            Some("dgx/a:high") => dgx += 1,
            Some("other/b:high") => other += 1,
            value => panic!("unexpected model for {id}: {value:?}"),
        }
    }
    assert_eq!(dgx, 2);
    assert_eq!(other, 2);
}

#[derive(Default)]
struct RecordingBackend {
    env: Mutex<HashMap<String, String>>,
    started: Mutex<Vec<String>>,
}

impl lyra::pane::PaneBackend for RecordingBackend {
    fn name(&self) -> &str {
        "recording"
    }

    fn capabilities(&self) -> lyra::pane::Capabilities {
        lyra::pane::Capabilities {
            splits: true,
            start_agent: true,
            send_text: true,
            close: true,
            ..lyra::pane::Capabilities::default()
        }
    }

    fn anchor(&self) -> anyhow::Result<lyra::pane::PaneHandle> {
        Ok(lyra::pane::PaneHandle {
            backend: "recording".to_string(),
            pane: "anchor".to_string(),
            ..Default::default()
        })
    }

    fn split(
        &self,
        _anchor: &lyra::pane::PaneHandle,
        options: &lyra::pane::SplitOptions,
    ) -> anyhow::Result<lyra::pane::PaneHandle> {
        *self.env.lock().unwrap() = options.env.clone();
        Ok(lyra::pane::PaneHandle {
            backend: "recording".to_string(),
            pane: "worker".to_string(),
            ..Default::default()
        })
    }

    fn start_agent(
        &self,
        _pane: &lyra::pane::PaneHandle,
        options: &lyra::pane::StartAgentOptions,
    ) -> anyhow::Result<()> {
        self.started.lock().unwrap().push(options.kind.clone());
        Ok(())
    }

    fn send_text(
        &self,
        _pane: &lyra::pane::PaneHandle,
        _text: &str,
        _submit: bool,
    ) -> anyhow::Result<()> {
        let config_path = self
            .env
            .lock()
            .unwrap()
            .get("LYRA_WORKER_CONFIG")
            .cloned()
            .unwrap();
        let worker_dir = Path::new(&config_path).parent().unwrap();
        fs::write(
            worker_dir.join("RESULT.md"),
            "status: done\nsummary: cross-agent result\nverification: fake test\nblockers:\n",
        )
        .unwrap();
        Ok(())
    }

    fn close(&self, _pane: &lyra::pane::PaneHandle) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn non_omp_harness_uses_result_md_fallback() {
    let _guard = lock_env();
    let dir = repo();
    let store = dir.path().join(".lyra/store");
    ensure_store(&store).unwrap();
    let ticket = create_ticket(
        &store,
        NewTicket {
            project_id: "test-project".to_string(),
            subject: "Cross agent".to_string(),
            prompt: "Do cross-agent work.".to_string(),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            parent: None,
            origin: Some("test".to_string()),
            harness: Some("codex".to_string()),
        },
    )
    .unwrap();
    let backend = RecordingBackend::default();
    let started = start_run(StartOptions {
        repo: dir.path().to_path_buf(),
        store: store.clone(),
        project_id: "test-project".to_string(),
        config: LyraConfig {
            max_parallel: 1,
            max_per_provider: HashMap::new(),
            approval_posture: ApprovalPosture::Ask,
            merge: MergeMode::Auto,
        },
        backend: Some(Box::new(backend)),
        max_parallel: None,
        max_seconds: Some(10),
        ticket_id: None,
        base_ref: None,
        dry_run: false,
        run_id: Some("run-cross-agent".to_string()),
        model_policy: None,
        models: Some(Vec::new()),
        usage: Some(Vec::new()),
    })
    .unwrap();
    assert_eq!(started.tickets[&ticket.id].status, RunTicketStatus::Running);
    let state = monitor_run(MonitorOptions {
        run_dir: run_dir_for(dir.path(), "run-cross-agent"),
        backend: Some(Box::new(HeadlessBackend::new())),
        once: false,
        interval_ms: 25,
        timeout_ms: Some(5000),
        max_parallel: None,
    })
    .unwrap();
    assert_eq!(state.tickets[&ticket.id].status, RunTicketStatus::Done);
    assert_eq!(list_tickets(&store).unwrap()[0].status, TicketStatus::Done);
}
