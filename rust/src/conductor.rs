use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::project::runs_dir_for;
use crate::project_config::ProjectConfig;
use crate::runner::{self, WorkerHandle};
use crate::schema::{Ticket, TicketStatus, now_iso, ready_tickets, validate_graph};
use crate::store::{
    SuspendArgs, claim_ticket, finish_ticket, list_tickets, release_ticket, suspend_ticket,
};
use crate::worker::{
    WorkerPaths, WorkerResultStatus, read_result, read_state, render_task_brief, write_task_brief,
};
use crate::worktree::{
    WorktreeInfo, close_run_worktrees, create_worktree, detect_base_ref, resume_worktree,
};

/// Consecutive attempts tolerated for one ticket within a run before it is
/// left failed. Retry is always explicit: the same worktree, the same branch.
pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Planned,
    Running,
    Done,
    Blocked,
    Stopped,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunTicketStatus {
    Starting,
    Running,
    Done,
    Blocked,
    Failed,
    Stopped,
}

impl RunTicketStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTicketState {
    pub status: RunTicketStatus,
    pub claim_id: String,
    /// `process` is the only backend in 0.7. The field stays for status
    /// compatibility with the OMP adapter and older consumers.
    pub model: Option<String>,
    pub worktree: String,
    pub branch: String,
    pub base_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub schema_version: u8,
    pub run_id: String,
    pub project_id: String,
    pub repo: String,
    pub store: String,
    pub run_dir: String,
    pub status: RunStatus,
    pub max_parallel: u32,
    #[serde(default)]
    pub max_seconds: Option<u64>,
    pub base_ref: String,
    /// Always `process` in 0.7; retained for status consumers.
    pub backend: String,
    pub worker_build: Vec<String>,
    #[serde(default)]
    pub worker_plan: Option<Vec<String>>,
    #[serde(default)]
    pub worker_env: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planned_tickets: Vec<String>,
    #[serde(default)]
    pub tickets: HashMap<String, RunTicketState>,
}

pub struct StartOptions {
    pub repo: PathBuf,
    pub store: PathBuf,
    pub project_id: String,
    pub config: ProjectConfig,
    pub max_parallel: Option<u32>,
    pub max_seconds: Option<u64>,
    pub ticket_ids: Vec<String>,
    pub base_ref: Option<String>,
    pub dry_run: bool,
    pub run_id: Option<String>,
}

pub struct MonitorOptions {
    pub run_dir: PathBuf,
    pub once: bool,
    pub interval_ms: u64,
    pub timeout_ms: Option<u64>,
    pub max_parallel: Option<u32>,
}

pub struct WaitOptions {
    pub run_dir: PathBuf,
    pub after_revision: Option<u64>,
    pub timeout_ms: u64,
    pub interval_ms: u64,
    pub max_parallel: Option<u32>,
}

pub struct WaitOutcome {
    pub state: RunState,
    pub timed_out: bool,
}

fn new_run_id() -> String {
    let stamp = Utc::now().format("%Y%m%dT%H%M%S");
    format!("run-{stamp}-{}", &Uuid::new_v4().simple().to_string()[..6])
}

pub fn run_dir_for(repo: &Path, run_id: &str) -> PathBuf {
    runs_dir_for(repo).join(crate::worktree::safe_segment(run_id))
}

pub fn list_run_ids(repo: &Path) -> Result<Vec<String>> {
    let root = runs_dir_for(repo);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        if entry.path().is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            ids.push(name.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

pub fn latest_run_dir(repo: &Path) -> Result<Option<PathBuf>> {
    let ids = list_run_ids(repo)?;
    Ok(ids.last().map(|id| run_dir_for(repo, id)))
}

pub fn load_state(run_dir: &Path) -> Result<RunState> {
    let path = run_dir.join("state.json");
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn save_state(state: &mut RunState) -> Result<()> {
    let mut persisted = state.clone();
    persisted.updated_at = now_iso();
    persisted.revision = state.revision.saturating_add(1);
    let path = PathBuf::from(&persisted.run_dir).join("state.json");
    let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let text = serde_json::to_string_pretty(&persisted)? + "\n";
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("rename {} -> {}", temporary.display(), path.display()))?;
    *state = persisted;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn worktree_is_clean(path: &Path) -> Result<bool> {
    Ok(git(path, &["status", "--porcelain"])?.is_empty())
}

fn worker_env(
    state: &RunState,
    ticket_id: &str,
    subject: &str,
    claim_id: &str,
    worktree: &Path,
    paths: &WorkerPaths,
    attempt: u32,
) -> HashMap<String, String> {
    let mut env = state.worker_env.clone();
    env.insert("LYRA_RUN_ID".to_string(), state.run_id.clone());
    env.insert("LYRA_TICKET_ID".to_string(), ticket_id.to_string());
    env.insert("LYRA_TICKET_SUBJECT".to_string(), subject.to_string());
    env.insert("LYRA_CLAIM_ID".to_string(), claim_id.to_string());
    env.insert("LYRA_WORKTREE".to_string(), worktree.display().to_string());
    env.insert(
        "LYRA_PROMPT_FILE".to_string(),
        paths.prompt.display().to_string(),
    );
    env.insert(
        "LYRA_RESULT_FILE".to_string(),
        paths.result_md.display().to_string(),
    );
    env.insert(
        "LYRA_PLAN_FILE".to_string(),
        paths.plan.display().to_string(),
    );
    env.insert(
        "LYRA_DIFF_FILE".to_string(),
        paths.diff.display().to_string(),
    );
    env.insert("LYRA_ATTEMPT".to_string(), attempt.to_string());
    env
}

fn launch_ticket(state: &mut RunState, ticket: &Ticket) -> Result<()> {
    let claim_id = Uuid::new_v4().to_string();
    let claimed = claim_ticket(
        Path::new(&state.store),
        &ticket.id,
        &state.run_id,
        &claim_id,
    )?
    .ok_or_else(|| anyhow::anyhow!("ticket {} is no longer claimable", ticket.id))?;
    let attempt = state
        .tickets
        .get(&ticket.id)
        .map(|ticket| ticket.attempt.saturating_add(1))
        .unwrap_or(1);

    let previous = state.tickets.get(&ticket.id).cloned();
    let tree: WorktreeInfo = match &claimed.suspension {
        Some(suspension) => resume_worktree(
            Path::new(&state.repo),
            Path::new(&suspension.worktree),
            &suspension.branch,
            &suspension.base_head,
        )?,
        None if previous.as_ref().is_some_and(|previous| {
            !previous.worktree.is_empty()
                && !previous.branch.is_empty()
                && Path::new(&previous.worktree).exists()
        }) =>
        {
            let previous = previous.as_ref().unwrap();
            resume_worktree(
                Path::new(&state.repo),
                Path::new(&previous.worktree),
                &previous.branch,
                &previous.base_head,
            )?
        }
        None => create_worktree(
            Path::new(&state.repo),
            Path::new(&state.run_dir),
            &state.run_id,
            &ticket.id,
            Some(&state.base_ref),
        )?,
    };

    let paths = WorkerPaths::new(Path::new(&state.run_dir), &ticket.id);
    let brief = render_task_brief(&claimed, &tree.dir, &paths.prompt, &paths.result_md);
    write_task_brief(&paths, &brief)?;
    let command = crate::project_config::expand_command(
        &state.worker_build,
        &paths.prompt,
        None,
        &tree.dir,
        &ticket.id,
    );
    let env = worker_env(
        state,
        &ticket.id,
        &claimed.subject,
        &claim_id,
        &tree.dir,
        &paths,
        attempt,
    );
    let log_dir = Path::new(&state.run_dir).join("workers");
    let handle = match runner::spawn(&runner::SpawnOptions {
        ticket_id: ticket.id.clone(),
        command,
        cwd: tree.dir.clone(),
        env,
        log_dir,
    }) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = release_ticket(
                Path::new(&state.store),
                &ticket.id,
                &state.run_id,
                &claim_id,
                &format!("worker spawn failed: {error:#}"),
            );
            return Err(error);
        }
    };

    state.tickets.insert(
        ticket.id.clone(),
        RunTicketState {
            status: RunTicketStatus::Running,
            claim_id,
            model: None,
            worktree: tree.dir.display().to_string(),
            branch: tree.branch,
            base_head: tree.base_head,
            phase: Some("building".to_string()),
            detail: Some("worker running".to_string()),
            started_at: now_iso(),
            finished_at: None,
            attempt,
            worker: Some(handle),
            result: None,
            error: None,
        },
    );
    save_state(state)?;
    Ok(())
}

fn launch_ready(state: &mut RunState) -> Result<()> {
    // Failed tickets are retried explicitly up to MAX_ATTEMPTS. Put them back
    // in the ready pool before asking the graph; the attempt counter in run
    // state is what bounds the retry.
    let retry: Vec<String> = state
        .tickets
        .iter()
        .filter(|(_, ticket)| {
            ticket.status == RunTicketStatus::Failed && ticket.attempt < MAX_ATTEMPTS
        })
        .map(|(id, _)| id.clone())
        .collect();
    for ticket_id in retry {
        let _ = crate::store::requeue_ticket(Path::new(&state.store), &ticket_id);
    }
    let running = state
        .tickets
        .values()
        .filter(|ticket| {
            matches!(
                ticket.status,
                RunTicketStatus::Starting | RunTicketStatus::Running
            )
        })
        .count() as u32;
    let capacity = state.max_parallel.saturating_sub(running);
    if capacity == 0 {
        return Ok(());
    }
    let tickets = list_tickets(Path::new(&state.store))?;
    let ready: Vec<Ticket> = ready_tickets(&tickets)
        .into_iter()
        .filter(|ticket| {
            if ticket
                .suspension
                .as_ref()
                .is_some_and(|suspension| suspension.needs_input)
            {
                return false;
            }
            match state.tickets.get(&ticket.id) {
                Some(run_ticket) => {
                    if run_ticket.status == RunTicketStatus::Done
                        || run_ticket.attempt >= MAX_ATTEMPTS
                    {
                        return false;
                    }
                    matches!(
                        run_ticket.status,
                        RunTicketStatus::Failed | RunTicketStatus::Blocked
                    )
                }
                None => true,
            }
        })
        .cloned()
        .collect();
    for ticket in ready.into_iter().take(capacity as usize) {
        if let Err(error) = launch_ticket(state, &ticket) {
            eprintln!("lyra: launch {} failed: {error:#}", ticket.id);
            state.tickets.insert(
                ticket.id.clone(),
                RunTicketState {
                    status: RunTicketStatus::Failed,
                    claim_id: String::new(),
                    model: None,
                    worktree: String::new(),
                    branch: String::new(),
                    base_head: String::new(),
                    phase: Some("failed".to_string()),
                    detail: Some("launch failed".to_string()),
                    started_at: now_iso(),
                    finished_at: Some(now_iso()),
                    attempt: 1,
                    worker: None,
                    result: None,
                    error: Some(format!("{error:#}")),
                },
            );
            save_state(state)?;
        }
    }
    Ok(())
}

fn settle_worker(state: &mut RunState, ticket_id: &str) -> Result<()> {
    let Some(run_ticket) = state.tickets.get(ticket_id).cloned() else {
        return Ok(());
    };
    if run_ticket.status != RunTicketStatus::Running {
        return Ok(());
    }
    let paths = WorkerPaths::new(Path::new(&state.run_dir), ticket_id);
    let result = match read_result(&paths, ticket_id, &state.run_id, &run_ticket.claim_id)? {
        Some(result) => result,
        None => {
            if let Some(worker) = &run_ticket.worker
                && !runner::is_running(worker.pid)
            {
                let tail = runner::tail(Path::new(&worker.log), 20);
                let message = if tail.is_empty() {
                    "worker exited without RESULT.md".to_string()
                } else {
                    format!("worker exited without RESULT.md\n{tail}")
                };
                let _ = finish_ticket(
                    Path::new(&state.store),
                    ticket_id,
                    &state.run_id,
                    &run_ticket.claim_id,
                    TicketStatus::Failed,
                    json!({"status": "failed", "summary": message}),
                    Some(message.clone()),
                );
                if let Some(ticket) = state.tickets.get_mut(ticket_id) {
                    ticket.status = RunTicketStatus::Failed;
                    ticket.phase = Some("failed".to_string());
                    ticket.detail = Some("worker exited without result".to_string());
                    ticket.finished_at = Some(now_iso());
                    ticket.error = Some(message);
                }
                save_state(state)?;
            } else if let Some(worker_state) = read_state(&paths) {
                let phase = worker_state
                    .get("phase")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let detail = worker_state
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if phase != run_ticket.phase || detail != run_ticket.detail {
                    if let Some(ticket) = state.tickets.get_mut(ticket_id) {
                        ticket.phase = phase;
                        ticket.detail = detail;
                    }
                    save_state(state)?;
                }
            }
            return Ok(());
        }
    };
    if !result.claim_id.is_empty() && result.claim_id != run_ticket.claim_id {
        return Ok(());
    }
    let status = match result.status {
        WorkerResultStatus::Done => {
            if !worktree_is_clean(Path::new(&run_ticket.worktree))? {
                let message = "worker reported done but left uncommitted changes".to_string();
                finish_ticket(
                    Path::new(&state.store),
                    ticket_id,
                    &state.run_id,
                    &run_ticket.claim_id,
                    TicketStatus::Failed,
                    serde_json::to_value(&result)?,
                    Some(message.clone()),
                )?;
                RunTicketStatus::Failed
            } else {
                finish_ticket(
                    Path::new(&state.store),
                    ticket_id,
                    &state.run_id,
                    &run_ticket.claim_id,
                    TicketStatus::Done,
                    serde_json::to_value(&result)?,
                    None,
                )?
                .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} claim changed before finish"))?;
                RunTicketStatus::Done
            }
        }
        WorkerResultStatus::Failed => {
            finish_ticket(
                Path::new(&state.store),
                ticket_id,
                &state.run_id,
                &run_ticket.claim_id,
                TicketStatus::Failed,
                serde_json::to_value(&result)?,
                result.summary.clone(),
            )?;
            RunTicketStatus::Failed
        }
        WorkerResultStatus::Blocked | WorkerResultStatus::NeedsInput => {
            let blockers = if result.status == WorkerResultStatus::Blocked {
                result.blockers.clone()
            } else {
                Vec::new()
            };
            suspend_ticket(
                Path::new(&state.store),
                ticket_id,
                &state.run_id,
                &run_ticket.claim_id,
                SuspendArgs {
                    blockers,
                    reason: result.summary.clone().unwrap_or_default(),
                    needs_input: result.status == WorkerResultStatus::NeedsInput,
                    worktree: run_ticket.worktree.clone(),
                    branch: run_ticket.branch.clone(),
                    base_head: run_ticket.base_head.clone(),
                },
            )?;
            RunTicketStatus::Blocked
        }
    };
    if let Some(worker) = &run_ticket.worker {
        let _ = runner::stop(worker.pid, Duration::from_secs(2));
    }
    if let Some(ticket) = state.tickets.get_mut(ticket_id) {
        ticket.status = status;
        ticket.phase = Some(
            match status {
                RunTicketStatus::Done => "done",
                RunTicketStatus::Blocked => "blocked",
                RunTicketStatus::Failed => "failed",
                _ => "settled",
            }
            .to_string(),
        );
        ticket.finished_at = Some(now_iso());
        ticket.result = Some(serde_json::to_value(&result)?);
        ticket.error = if status == RunTicketStatus::Failed {
            result.summary.clone()
        } else {
            None
        };
    }
    save_state(state)?;
    Ok(())
}

fn deadline_exceeded(ticket: &RunTicketState, max_seconds: Option<u64>) -> bool {
    let Some(max_seconds) = max_seconds else {
        return false;
    };
    let Ok(started) = DateTime::parse_from_rfc3339(&ticket.started_at) else {
        return false;
    };
    let elapsed = Utc::now().signed_duration_since(started.with_timezone(&Utc));
    elapsed.num_seconds() as u64 >= max_seconds
}

fn stop_timed_out(state: &mut RunState, ticket_id: &str) -> Result<()> {
    let Some(run_ticket) = state.tickets.get(ticket_id).cloned() else {
        return Ok(());
    };
    if let Some(worker) = &run_ticket.worker {
        let _ = runner::stop(worker.pid, Duration::from_secs(2));
    }
    let message = format!(
        "worker exceeded max_seconds {}",
        state.max_seconds.unwrap_or_default()
    );
    let _ = finish_ticket(
        Path::new(&state.store),
        ticket_id,
        &state.run_id,
        &run_ticket.claim_id,
        TicketStatus::Failed,
        json!({"status": "failed", "summary": message}),
        Some(message.clone()),
    );
    if let Some(ticket) = state.tickets.get_mut(ticket_id) {
        ticket.status = RunTicketStatus::Failed;
        ticket.phase = Some("failed".to_string());
        ticket.detail = Some("max_seconds exceeded".to_string());
        ticket.finished_at = Some(now_iso());
        ticket.error = Some(message);
    }
    save_state(state)?;
    Ok(())
}

fn reconcile_claims(state: &mut RunState) -> Result<()> {
    let store_tickets = list_tickets(Path::new(&state.store))?;
    let mut changed = false;
    for ticket in &store_tickets {
        if ticket.status != TicketStatus::InProgress {
            continue;
        }
        let Some(execution) = &ticket.execution else {
            continue;
        };
        if execution.run_id != state.run_id || state.tickets.contains_key(&ticket.id) {
            continue;
        }
        eprintln!("lyra: adopting orphaned claim on {}", ticket.id);
        let log_dir = Path::new(&state.run_dir)
            .join("workers")
            .join(crate::worktree::safe_segment(&ticket.id));
        let worker = fs::read_to_string(log_dir.join("worker.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<WorkerHandle>(&text).ok())
            .filter(|handle| runner::is_running(handle.pid));
        state.tickets.insert(
            ticket.id.clone(),
            RunTicketState {
                status: RunTicketStatus::Running,
                claim_id: execution.claim_id.clone(),
                model: None,
                worktree: Path::new(&state.run_dir)
                    .join("worktrees")
                    .join(crate::worktree::safe_segment(&ticket.id))
                    .display()
                    .to_string(),
                branch: String::new(),
                base_head: String::new(),
                phase: Some("adopted".to_string()),
                detail: None,
                started_at: now_iso(),
                finished_at: None,
                attempt: 1,
                worker,
                result: None,
                error: None,
            },
        );
        changed = true;
    }
    if changed {
        save_state(state)?;
    }
    Ok(())
}

fn run_has_active(state: &RunState) -> bool {
    state.tickets.values().any(|ticket| {
        matches!(
            ticket.status,
            RunTicketStatus::Running | RunTicketStatus::Starting
        )
    })
}

fn run_is_settled(state: &RunState) -> bool {
    !run_has_active(state)
        && matches!(
            state.status,
            RunStatus::Planned
                | RunStatus::Done
                | RunStatus::Blocked
                | RunStatus::Stopped
                | RunStatus::Closed
        )
}

pub fn monitor_once(run_dir: &Path, max_parallel: u32) -> Result<RunState> {
    let mut state = load_state(run_dir)?;
    reconcile_claims(&mut state)?;
    let running: Vec<String> = state
        .tickets
        .iter()
        .filter(|(_, ticket)| ticket.status == RunTicketStatus::Running)
        .map(|(id, _)| id.clone())
        .collect();
    for ticket_id in running {
        settle_worker(&mut state, &ticket_id)?;
        if let Some(ticket) = state.tickets.get(&ticket_id)
            && ticket.status == RunTicketStatus::Running
            && deadline_exceeded(ticket, state.max_seconds)
        {
            stop_timed_out(&mut state, &ticket_id)?;
        }
    }
    state = load_state(run_dir)?;
    state.max_parallel = max_parallel;
    launch_ready(&mut state)?;
    state = load_state(run_dir)?;
    let active = run_has_active(&state);
    let tickets = list_tickets(Path::new(&state.store))?;
    let remaining: Vec<&Ticket> = ready_tickets(&tickets)
        .into_iter()
        .filter(|ticket| {
            !ticket
                .suspension
                .as_ref()
                .is_some_and(|suspension| suspension.needs_input)
        })
        .collect();
    if !active && remaining.is_empty() {
        let open = tickets
            .iter()
            .any(|ticket| matches!(ticket.status, TicketStatus::Todo | TicketStatus::InProgress));
        state.status = if open {
            RunStatus::Blocked
        } else {
            RunStatus::Done
        };
        save_state(&mut state)?;
    }
    Ok(state)
}

pub fn start_run(options: StartOptions) -> Result<RunState> {
    let max_parallel = options
        .max_parallel
        .unwrap_or_else(|| options.config.max_parallel());
    let tickets = list_tickets(&options.store)?;
    let graph_errors = validate_graph(&tickets);
    if !graph_errors.is_empty() {
        bail!("invalid ticket graph:\n- {}", graph_errors.join("\n- "));
    }
    let ready: Vec<Ticket> = ready_tickets(&tickets).into_iter().cloned().collect();
    let mut candidates = ready.clone();
    if !options.ticket_ids.is_empty() {
        let ready_ids: HashSet<&str> = ready.iter().map(|ticket| ticket.id.as_str()).collect();
        let mut unknown = Vec::new();
        let mut not_ready = Vec::new();
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for ticket_id in &options.ticket_ids {
            if !seen.insert(ticket_id.clone()) {
                continue;
            }
            let Some(ticket) = tickets.iter().find(|ticket| ticket.id == *ticket_id) else {
                unknown.push(ticket_id.clone());
                continue;
            };
            if !ready_ids.contains(ticket.id.as_str()) {
                not_ready.push(format!(
                    "{} (blocked by {})",
                    ticket.id,
                    if ticket.blocked_by.is_empty() {
                        "nothing; status is not ready".to_string()
                    } else {
                        ticket.blocked_by.join(", ")
                    }
                ));
                continue;
            }
            selected.push(ticket.clone());
        }
        if !unknown.is_empty() {
            bail!("unknown tickets: {}", unknown.join(", "));
        }
        if !not_ready.is_empty() {
            bail!("tickets not ready: {}", not_ready.join("; "));
        }
        candidates = selected;
    }
    let base_ref = match options.base_ref {
        Some(value) => value,
        None => options
            .config
            .project
            .base_ref
            .clone()
            .unwrap_or(detect_base_ref(&options.repo)?),
    };
    let run_id = options.run_id.unwrap_or_else(new_run_id);
    let run_dir = run_dir_for(&options.repo, &run_id);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let max_seconds = options.max_seconds.or(options.config.project.max_seconds);
    let mut state = RunState {
        schema_version: 1,
        run_id,
        project_id: options.project_id,
        repo: options.repo.display().to_string(),
        store: options.store.display().to_string(),
        run_dir: run_dir.display().to_string(),
        status: if options.dry_run {
            RunStatus::Planned
        } else {
            RunStatus::Running
        },
        max_parallel,
        max_seconds,
        base_ref: base_ref.clone(),
        backend: "process".to_string(),
        worker_build: options.config.worker.build.clone(),
        worker_plan: options.config.worker.plan.clone(),
        worker_env: options.config.worker.env.clone(),
        created_at: now_iso(),
        updated_at: now_iso(),
        revision: 0,
        planned_tickets: candidates.iter().map(|ticket| ticket.id.clone()).collect(),
        tickets: HashMap::new(),
    };
    save_state(&mut state)?;
    if options.dry_run {
        return Ok(state);
    }
    launch_ready(&mut state)?;
    state = load_state(&run_dir)?;
    if state.tickets.is_empty() {
        state.status = RunStatus::Done;
        save_state(&mut state)?;
    }
    Ok(state)
}

pub fn monitor_run(options: MonitorOptions) -> Result<RunState> {
    let max_parallel = match options.max_parallel {
        Some(value) => value,
        None => load_state(&options.run_dir)?.max_parallel,
    };
    let deadline = options
        .timeout_ms
        .map(|timeout| Instant::now() + Duration::from_millis(timeout));
    loop {
        let mut state = monitor_once(&options.run_dir, max_parallel)?;
        if run_is_settled(&state) {
            return Ok(state);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            state.status = RunStatus::Stopped;
            save_state(&mut state)?;
            return Ok(state);
        }
        if options.once {
            return Ok(state);
        }
        sleep(Duration::from_millis(options.interval_ms));
    }
}

pub fn wait_run(options: WaitOptions) -> Result<WaitOutcome> {
    let deadline = Instant::now() + Duration::from_millis(options.timeout_ms);
    let after_revision = match options.after_revision {
        Some(revision) => revision,
        None => load_state(&options.run_dir)?.revision,
    };
    loop {
        let state = load_state(&options.run_dir)?;
        if state.revision > after_revision || run_is_settled(&state) {
            return Ok(WaitOutcome {
                state,
                timed_out: false,
            });
        }
        let max_parallel = options.max_parallel.unwrap_or(state.max_parallel);
        let state = monitor_once(&options.run_dir, max_parallel)?;
        if state.revision > after_revision || run_is_settled(&state) {
            return Ok(WaitOutcome {
                state,
                timed_out: false,
            });
        }
        if Instant::now() >= deadline {
            return Ok(WaitOutcome {
                state,
                timed_out: true,
            });
        }
        sleep(Duration::from_millis(options.interval_ms));
    }
}

pub fn stop_run(run_dir: &Path) -> Result<RunState> {
    let mut state = load_state(run_dir)?;
    for (ticket_id, run_ticket) in state.tickets.clone() {
        if run_ticket.status != RunTicketStatus::Running {
            continue;
        }
        if let Some(worker) = &run_ticket.worker {
            let _ = runner::stop(worker.pid, Duration::from_secs(2));
        }
        let _ = release_ticket(
            Path::new(&state.store),
            &ticket_id,
            &state.run_id,
            &run_ticket.claim_id,
            "conductor stopped",
        );
        if let Some(ticket) = state.tickets.get_mut(&ticket_id) {
            ticket.status = RunTicketStatus::Stopped;
            ticket.finished_at = Some(now_iso());
        }
    }
    state.status = RunStatus::Stopped;
    release_run_claims(Path::new(&state.store), &state.run_id);
    save_state(&mut state)?;
    Ok(state)
}

pub fn close_run(run_dir: &Path) -> Result<RunState> {
    let mut state = load_state(run_dir)?;
    let tickets = list_tickets(Path::new(&state.store))?;
    let keep: HashSet<PathBuf> = tickets
        .iter()
        .filter_map(|ticket| {
            ticket
                .suspension
                .as_ref()
                .map(|suspension| PathBuf::from(&suspension.worktree))
        })
        .collect();
    let results = close_run_worktrees(Path::new(&state.repo), Path::new(&state.run_dir), &keep)?;
    for (dir, removed, reason) in results {
        if !removed {
            let matching = state
                .tickets
                .iter()
                .find(|(_, ticket)| ticket.worktree == dir.display().to_string())
                .map(|(ticket_id, _)| ticket_id.clone());
            if let Some(ticket_id) = matching
                && let Some(ticket) = state.tickets.get_mut(&ticket_id)
            {
                ticket.error = Some(format!(
                    "worktree kept: {}",
                    reason.unwrap_or_else(|| "unknown".to_string())
                ));
            }
        }
    }
    release_run_claims(Path::new(&state.store), &state.run_id);
    state.status = RunStatus::Closed;
    save_state(&mut state)?;
    Ok(state)
}

fn release_run_claims(store: &Path, run_id: &str) {
    let Ok(tickets) = list_tickets(store) else {
        return;
    };
    for ticket in &tickets {
        let Some(execution) = &ticket.execution else {
            continue;
        };
        if execution.run_id != run_id {
            continue;
        }
        let _ = release_ticket(
            store,
            &ticket.id,
            run_id,
            &execution.claim_id,
            "run stopped or closed; claim released by store reconciliation",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(dir: &Path) -> RunState {
        RunState {
            schema_version: 1,
            run_id: "run-test".to_string(),
            project_id: "project".to_string(),
            repo: "/tmp/repo".to_string(),
            store: "/tmp/store".to_string(),
            run_dir: dir.display().to_string(),
            status: RunStatus::Running,
            max_parallel: 1,
            max_seconds: None,
            base_ref: "main".to_string(),
            backend: "process".to_string(),
            worker_build: vec!["true".to_string()],
            worker_plan: None,
            worker_env: HashMap::new(),
            created_at: now_iso(),
            updated_at: now_iso(),
            revision: 0,
            planned_tickets: Vec::new(),
            tickets: HashMap::new(),
        }
    }

    #[test]
    fn each_successful_save_increments_the_revision_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state(dir.path());
        save_state(&mut state).unwrap();
        assert_eq!(state.revision, 1);
        save_state(&mut state).unwrap();
        assert_eq!(state.revision, 2);
        let loaded = load_state(dir.path()).unwrap();
        assert_eq!(loaded.revision, 2);
    }

    #[test]
    fn stop_run_releases_orphan_claims_known_only_to_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".lyra/store");
        crate::store::ensure_store(&store).unwrap();
        let ticket = crate::store::create_ticket(
            &store,
            crate::store::NewTicket {
                project_id: "p".to_string(),
                subject: "Orphan".to_string(),
                prompt: "noop".to_string(),
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
        let mut state = test_state(dir.path());
        state.store = store.display().to_string();
        crate::store::claim_ticket(&store, &ticket.id, &state.run_id, "claim-orphan")
            .unwrap()
            .unwrap();
        let run_dir = PathBuf::from(&state.run_dir);
        std::fs::create_dir_all(&run_dir).unwrap();
        save_state(&mut state).unwrap();
        stop_run(&run_dir).unwrap();
        let tickets = crate::store::list_tickets(&store).unwrap();
        assert_eq!(tickets[0].status, crate::schema::TicketStatus::Todo);
        assert!(tickets[0].execution.is_none());
    }
}
