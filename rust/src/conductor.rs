use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::config::{ApprovalPosture, LyraConfig, MergeMode};
use crate::models::{
    ModelInfo, ModelPolicy, UsageReport, load_model_catalog, load_policy, load_usage, provider_of,
    select_model, subagent_roles, write_role_overlay,
};
use crate::pane::{
    PaneBackend, PaneHandle, SplitDirection, SplitOptions, StartAgentOptions, select_backend,
};
use crate::project::runs_dir_for;
use crate::schema::{Ticket, TicketStatus, now_iso, ready_tickets, validate_graph};
use crate::shell::command_line;
use crate::store::{
    NewTicket, SuspendArgs, claim_ticket, create_ticket, finish_ticket, list_tickets, read_ticket,
    release_ticket, suspend_ticket, update_execution,
};
use crate::worker::{
    Continuation, TaskBriefArgs, WorkerChannel, WorkerConfig, WorkerPaths, WorkerResultStatus,
    ensure_worker_extension, read_result, read_state, render_task_brief, write_config,
    write_response, write_task_brief,
};
use crate::worktree::{
    WorktreeInfo, close_run_worktrees, create_worktree, detect_base_ref, remove_worktree,
    resume_worktree,
};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTicketState {
    pub status: RunTicketStatus,
    pub claim_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneHandle>,
    pub worktree: String,
    pub branch: String,
    pub base_head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
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
    pub max_per_provider: HashMap<String, u32>,
    pub approval: ApprovalPosture,
    pub backend: String,
    pub base_ref: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<ModelPolicy>,
    #[serde(default)]
    pub model_catalog: Vec<ModelInfo>,
    #[serde(default)]
    pub usage: Vec<UsageReport>,
    #[serde(default)]
    pub tickets: HashMap<String, RunTicketState>,
}

pub struct StartOptions {
    pub repo: PathBuf,
    pub store: PathBuf,
    pub project_id: String,
    pub config: LyraConfig,
    pub backend: Option<Box<dyn PaneBackend>>,
    pub max_parallel: Option<u32>,
    pub max_seconds: Option<u64>,
    pub ticket_id: Option<String>,
    pub base_ref: Option<String>,
    pub dry_run: bool,
    pub run_id: Option<String>,
    pub model_policy: Option<ModelPolicy>,
    pub models: Option<Vec<ModelInfo>>,
    pub usage: Option<Vec<UsageReport>>,
}

pub struct MonitorOptions {
    pub run_dir: PathBuf,
    pub backend: Option<Box<dyn PaneBackend>>,
    pub once: bool,
    pub interval_ms: u64,
    pub timeout_ms: Option<u64>,
    pub max_parallel: Option<u32>,
}

fn new_run_id() -> String {
    let stamp = Utc::now().format("%Y%m%dT%H%M%S");
    format!("run-{stamp}-{}", &Uuid::new_v4().simple().to_string()[..6])
}

pub fn run_dir_for(repo: &Path, run_id: &str) -> PathBuf {
    runs_dir_for(repo).join(run_id)
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
    state.updated_at = now_iso();
    let path = PathBuf::from(&state.run_dir).join("state.json");
    let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let text = serde_json::to_string_pretty(state)? + "\n";
    fs::write(&temporary, text).with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("rename {} -> {}", temporary.display(), path.display()))?;
    Ok(())
}

fn approval_args(posture: ApprovalPosture) -> Vec<String> {
    match posture {
        ApprovalPosture::Safe => vec!["--approval-mode".into(), "always-ask".into()],
        ApprovalPosture::Ask => vec!["--approval-mode".into(), "write".into()],
        ApprovalPosture::Unsafe => vec!["--approval-mode".into(), "yolo".into()],
    }
}

fn omp_args(
    config: &WorkerConfig,
    max_seconds: Option<u64>,
    roles_path: Option<&Path>,
    extension: &Path,
) -> Vec<String> {
    let mut args = vec![
        "--session-dir".to_string(),
        config.session_dir.clone(),
        "-e".to_string(),
        extension.display().to_string(),
    ];
    args.extend(approval_args(config.approval));
    if let Some(path) = roles_path {
        args.push("--config".to_string());
        args.push(path.display().to_string());
    }
    if let Some(model) = &config.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(budget) = config.budget_usd {
        args.push("--max-cost".to_string());
        args.push(budget.to_string());
    }
    if let Some(seconds) = max_seconds {
        args.push("--max-time".to_string());
        args.push(format!("{seconds}s"));
    }
    args.push(format!("@{}", config.prompt_file));
    args
}

fn active_providers(state: &RunState) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for ticket in state.tickets.values() {
        if ticket.status != RunTicketStatus::Running {
            continue;
        }
        if let Some(model) = &ticket.model {
            *counts.entry(provider_of(model)).or_insert(0) += 1;
        }
    }
    counts
}

struct LaunchContext<'a> {
    config: &'a LyraConfig,
    max_seconds: Option<u64>,
    base_ref: &'a str,
    model_policy: Option<&'a ModelPolicy>,
    models: &'a [ModelInfo],
    usage: &'a [UsageReport],
}

fn launch_ticket(
    state: &mut RunState,
    ticket: &Ticket,
    backend: &dyn PaneBackend,
    anchor: &PaneHandle,
    context: &LaunchContext,
) -> Result<()> {
    let config = context.config;
    let max_seconds = context.max_seconds;
    let base_ref = context.base_ref;
    let model_policy = context.model_policy;
    let models = context.models;
    let usage = context.usage;
    let current = read_ticket(Path::new(&state.store), &ticket.id)?
        .ok_or_else(|| anyhow::anyhow!("unknown ticket: {}", ticket.id))?;
    let suspension = current.suspension.clone();
    let claim_id = Uuid::new_v4().to_string();
    let claimed = claim_ticket(
        Path::new(&state.store),
        &ticket.id,
        &state.run_id,
        &claim_id,
    )?
    .ok_or_else(|| anyhow::anyhow!("ticket {} is no longer claimable", ticket.id))?;

    let mut pane: Option<PaneHandle> = None;
    let mut worktree: Option<WorktreeInfo> = None;
    let mut launched_model: Option<String> = None;
    let result = (|| -> Result<()> {
        let tree = match &suspension {
            Some(suspension) => resume_worktree(
                Path::new(&state.repo),
                Path::new(&suspension.worktree),
                &suspension.branch,
                &suspension.base_head,
            )?,
            None => create_worktree(
                Path::new(&state.repo),
                Path::new(&state.run_dir),
                &state.run_id,
                &ticket.id,
                Some(base_ref),
            )?,
        };
        let worker_base_head = suspension
            .as_ref()
            .map(|value| value.base_head.clone())
            .unwrap_or_else(|| tree.base_head.clone());
        worktree = Some(tree.clone());
        let paths = WorkerPaths::new(Path::new(&state.run_dir), &ticket.id);
        let choice = if let Some(policy) = model_policy {
            if models.is_empty() {
                None
            } else {
                select_model(
                    policy,
                    models,
                    usage,
                    ticket.model_class.as_deref(),
                    ticket.model.as_deref(),
                    &active_providers(state),
                    &config.max_per_provider,
                )
            }
        } else {
            None
        };
        if model_policy.is_some() && !models.is_empty() && choice.is_none() {
            anyhow::bail!(
                "no model candidate available for class {}",
                ticket.model_class.as_deref().unwrap_or("default")
            );
        }
        let model = choice
            .as_ref()
            .map(|choice| choice.selector.clone())
            .or_else(|| ticket.model.clone());
        launched_model = model.clone();
        let roles_path = match (model_policy, &choice) {
            (Some(policy), Some(choice)) => {
                let path = paths.root.join("roles.yml");
                write_role_overlay(&path, policy, &choice.selector)?;
                Some(path)
            }
            _ => None,
        };
        let continuation = suspension.as_ref().map(|suspension| Continuation {
            head: tree.base_head.clone(),
            base_head: suspension.base_head.clone(),
            reason: suspension.reason.clone(),
        });
        let worker_config = WorkerConfig {
            schema_version: 1,
            run_id: state.run_id.clone(),
            claim_id: claim_id.clone(),
            ticket_id: ticket.id.clone(),
            project_id: state.project_id.clone(),
            harness: ticket.harness.clone().unwrap_or_else(|| "omp".to_string()),
            worktree: tree.dir.display().to_string(),
            branch: tree.branch.clone(),
            base_head: worker_base_head.clone(),
            session_dir: Path::new(&state.run_dir)
                .join("sessions")
                .join(&ticket.id)
                .display()
                .to_string(),
            prompt_file: paths.prompt.display().to_string(),
            approval: config.approval_posture,
            model: model.clone(),
            model_class: ticket.model_class.clone(),
            model_roles: model_policy
                .zip(choice.as_ref())
                .map(|(policy, choice)| subagent_roles(policy, &choice.selector)),
            budget_usd: ticket.budget_usd,
            max_seconds,
            continuation,
            channel: WorkerChannel {
                root: paths.root.display().to_string(),
                requests: paths.requests.display().to_string(),
                responses: paths.responses.display().to_string(),
                state: paths.state.display().to_string(),
                result: paths.result.display().to_string(),
            },
        };
        fs::create_dir_all(&worker_config.session_dir)
            .with_context(|| format!("create {}", worker_config.session_dir))?;
        write_config(&paths, &worker_config)?;
        let brief = render_task_brief(&TaskBriefArgs {
            ticket: &claimed,
            run_id: &state.run_id,
            claim_id: &claim_id,
            worktree: &tree.dir.display().to_string(),
            branch: &tree.branch,
            base_ref,
            base_head: &worker_base_head,
            model: model.as_deref(),
            budget_usd: ticket.budget_usd,
            result_md: &paths.result_md.display().to_string(),
            continuation: worker_config.continuation.clone(),
        });
        write_task_brief(&paths, &brief)?;

        let harness = ticket.harness.clone().unwrap_or_else(|| "omp".to_string());
        let worker_name = format!("lyra-{}", ticket.id.to_lowercase())
            .chars()
            .map(|ch| {
                if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let mut env = HashMap::new();
        env.insert(
            "LYRA_WORKER_CONFIG".to_string(),
            paths.config.display().to_string(),
        );
        if harness == "omp" {
            let extension = ensure_worker_extension()?;
            let args = omp_args(
                &worker_config,
                max_seconds,
                roles_path.as_deref(),
                &extension,
            );
            let capabilities = backend.capabilities();
            if capabilities.start_agent {
                pane = Some(backend.split(
                    anchor,
                    &SplitOptions {
                        direction: SplitDirection::Right,
                        cwd: tree.dir.clone(),
                        command: None,
                        env,
                        log_dir: Some(paths.logs.clone()),
                    },
                )?);
                backend.start_agent(
                    pane.as_ref().unwrap(),
                    &StartAgentOptions {
                        kind: "omp".to_string(),
                        name: Some(worker_name),
                        args,
                    },
                )?;
            } else if capabilities.splits {
                let command = command_line(
                    std::env::var("LYRA_OMP_BIN")
                        .unwrap_or_else(|_| "omp".to_string())
                        .as_str(),
                    &args,
                    &HashMap::from([(
                        "LYRA_WORKER_CONFIG".to_string(),
                        paths.config.display().to_string(),
                    )]),
                );
                pane = Some(backend.split(
                    anchor,
                    &SplitOptions {
                        direction: SplitDirection::Right,
                        cwd: tree.dir.clone(),
                        command: Some(command),
                        env,
                        log_dir: Some(paths.logs.clone()),
                    },
                )?);
            } else {
                anyhow::bail!("backend {} cannot create worker panes", backend.name());
            }
        } else {
            let capabilities = backend.capabilities();
            if !capabilities.start_agent || !capabilities.send_text {
                anyhow::bail!(
                    "backend {} cannot start a visible {} worker",
                    backend.name(),
                    harness
                );
            }
            pane = Some(backend.split(
                anchor,
                &SplitOptions {
                    direction: SplitDirection::Right,
                    cwd: tree.dir.clone(),
                    command: None,
                    env,
                    log_dir: Some(paths.logs.clone()),
                },
            )?);
            backend.start_agent(
                pane.as_ref().unwrap(),
                &StartAgentOptions {
                    kind: harness,
                    name: Some(worker_name),
                    args: Vec::new(),
                },
            )?;
            let prompt = format!(
                "You are the Lyra worker for {}. Read {} and execute it in {}. When finished, write {} with exactly these fields:\nstatus: done|blocked|failed|needs_input\nsummary: <what you did or need>\nverification: <checks you ran>\nblockers: <comma-separated ticket ids, blocked only>\nDo not edit the ticket store or another worktree.",
                ticket.id,
                paths.prompt.display(),
                tree.dir.display(),
                paths.result_md.display()
            );
            backend.send_text(pane.as_ref().unwrap(), &prompt, true)?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            let now = now_iso();
            state.tickets.insert(
                ticket.id.clone(),
                RunTicketState {
                    status: RunTicketStatus::Running,
                    claim_id,
                    pane,
                    worktree: worktree
                        .as_ref()
                        .map(|tree| tree.dir.display().to_string())
                        .unwrap_or_default(),
                    branch: worktree
                        .as_ref()
                        .map(|tree| tree.branch.clone())
                        .unwrap_or_default(),
                    base_head: worktree
                        .as_ref()
                        .map(|tree| tree.base_head.clone())
                        .unwrap_or_default(),
                    model: launched_model.clone(),
                    phase: Some("starting".to_string()),
                    detail: None,
                    started_at: now,
                    finished_at: None,
                    result: None,
                    error: None,
                },
            );
            save_state(state)?;
            Ok(())
        }
        Err(error) => {
            let _ = release_ticket(
                Path::new(&state.store),
                &ticket.id,
                &state.run_id,
                &claim_id,
                &format!("{error:#}"),
            );
            if let Some(pane) = &pane {
                let _ = backend.close(pane);
            }
            if suspension.is_none()
                && let Some(tree) = &worktree
            {
                let _ = remove_worktree(Path::new(&state.repo), &tree.dir);
            }
            let now = now_iso();
            state.tickets.insert(
                ticket.id.clone(),
                RunTicketState {
                    status: RunTicketStatus::Failed,
                    claim_id,
                    pane,
                    worktree: worktree
                        .as_ref()
                        .map(|tree| tree.dir.display().to_string())
                        .unwrap_or_default(),
                    branch: worktree
                        .as_ref()
                        .map(|tree| tree.branch.clone())
                        .unwrap_or_default(),
                    base_head: worktree
                        .as_ref()
                        .map(|tree| tree.base_head.clone())
                        .unwrap_or_default(),
                    model: launched_model.clone(),
                    phase: None,
                    detail: None,
                    started_at: now.clone(),
                    finished_at: Some(now),
                    result: None,
                    error: Some(format!("{error:#}")),
                },
            );
            save_state(state)?;
            Err(error)
        }
    }
}

pub fn start_run(options: StartOptions) -> Result<RunState> {
    let max_parallel = options.max_parallel.unwrap_or(options.config.max_parallel);
    let backend = match options.backend {
        Some(backend) => backend,
        None => select_backend()?,
    };
    let tickets = list_tickets(&options.store)?;
    let graph_errors = validate_graph(&tickets);
    if !graph_errors.is_empty() {
        anyhow::bail!("invalid ticket graph:\n- {}", graph_errors.join("\n- "));
    }
    let mut candidates: Vec<Ticket> = ready_tickets(&tickets).into_iter().cloned().collect();
    if let Some(ticket_id) = &options.ticket_id {
        let named = tickets
            .iter()
            .find(|ticket| ticket.id == *ticket_id)
            .ok_or_else(|| anyhow::anyhow!("unknown ticket: {ticket_id}"))?;
        if !candidates.iter().any(|ticket| ticket.id == named.id) {
            anyhow::bail!(
                "ticket {} is not ready (status {:?}, blockers {})",
                named.id,
                named.status,
                if named.blocked_by.is_empty() {
                    "none".to_string()
                } else {
                    named.blocked_by.join(", ")
                }
            );
        }
        candidates = vec![named.clone()];
    }
    let base_ref = match options.base_ref {
        Some(value) => value,
        None => detect_base_ref(&options.repo)?,
    };
    let run_id = options.run_id.unwrap_or_else(new_run_id);
    let run_dir = run_dir_for(&options.repo, &run_id);
    fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let model_policy = options.model_policy.or_else(|| load_policy().ok());
    let models_supplied = options.models.is_some();
    let model_catalog = match options.models {
        Some(models) => models,
        None => load_model_catalog().unwrap_or_default(),
    };
    let usage = match options.usage {
        Some(usage) => usage,
        None if models_supplied => Vec::new(),
        None => load_usage().unwrap_or_default(),
    };
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
        max_per_provider: options.config.max_per_provider.clone(),
        approval: options.config.approval_posture,
        backend: backend.name().to_string(),
        base_ref: base_ref.clone(),
        created_at: now_iso(),
        updated_at: now_iso(),
        model_policy: model_policy.clone(),
        model_catalog: model_catalog.clone(),
        usage: usage.clone(),
        tickets: HashMap::new(),
    };
    save_state(&mut state)?;
    if options.dry_run {
        return Ok(state);
    }
    let anchor = backend.anchor()?;
    let launch_context = LaunchContext {
        config: &options.config,
        max_seconds: options.max_seconds,
        base_ref: &base_ref,
        model_policy: model_policy.as_ref(),
        models: &model_catalog,
        usage: &usage,
    };
    for ticket in candidates.into_iter().take(max_parallel as usize) {
        if let Err(error) = launch_ticket(
            &mut state,
            &ticket,
            backend.as_ref(),
            &anchor,
            &launch_context,
        ) {
            eprintln!("lyra: launch {} failed: {error:#}", ticket.id);
        }
    }
    if state.tickets.is_empty() {
        state.status = RunStatus::Done;
        save_state(&mut state)?;
    }
    Ok(state)
}

fn blocker_depth(store: &Path, ticket_id: &str) -> Result<usize> {
    let mut depth = 0;
    let mut current = read_ticket(store, ticket_id)?;
    let mut seen = HashSet::new();
    while let Some(ticket) = current {
        if let Some(parent) = ticket.parent {
            if seen.contains(&ticket.id) {
                break;
            }
            seen.insert(ticket.id);
            depth += 1;
            current = read_ticket(store, &parent)?;
        } else {
            break;
        }
    }
    Ok(depth)
}

fn process_requests(state: &RunState, ticket_id: &str) -> Result<()> {
    let paths = WorkerPaths::new(Path::new(&state.run_dir), ticket_id);
    for (request_id, payload) in crate::worker::list_requests(&paths)? {
        if paths.responses.join(format!("{request_id}.json")).exists() {
            continue;
        }
        let result = (|| -> Result<Ticket> {
            if let Some(existing) = payload.get("existingId").and_then(Value::as_str) {
                return read_ticket(Path::new(&state.store), existing)?
                    .ok_or_else(|| anyhow::anyhow!("unknown existing ticket: {existing}"));
            }
            if payload.get("kind").and_then(Value::as_str) == Some("blocker") {
                let parent = read_ticket(Path::new(&state.store), ticket_id)?
                    .ok_or_else(|| anyhow::anyhow!("unknown parent ticket: {ticket_id}"))?;
                if parent.blocked_by.len() >= 10 {
                    anyhow::bail!("{ticket_id} already has 10 blockers");
                }
                if blocker_depth(Path::new(&state.store), ticket_id)? >= 3 {
                    anyhow::bail!("blocker depth limit 3 reached at {ticket_id}");
                }
            }
            create_ticket(
                Path::new(&state.store),
                NewTicket {
                    project_id: state.project_id.clone(),
                    subject: payload
                        .get("subject")
                        .and_then(Value::as_str)
                        .unwrap_or("Discovered work")
                        .to_string(),
                    prompt: payload
                        .get("prompt")
                        .and_then(Value::as_str)
                        .or_else(|| payload.get("reason").and_then(Value::as_str))
                        .unwrap_or("Discovered work")
                        .to_string(),
                    blocked_by: Vec::new(),
                    priority: crate::schema::PRIORITY_DEFAULT,
                    labels: Vec::new(),
                    model_class: Some("architecture".to_string()),
                    acceptance: None,
                    plan: None,
                    parent: Some(ticket_id.to_string()),
                    origin: Some("discovered".to_string()),
                    harness: None,
                },
            )
        })();
        let response = match result {
            Ok(ticket) => json!({"ticketId": ticket.id}),
            Err(error) => json!({"error": format!("{error:#}")}),
        };
        write_response(&paths, &request_id, &response)?;
    }
    Ok(())
}

fn settle_worker(state: &mut RunState, ticket_id: &str, backend: &dyn PaneBackend) -> Result<()> {
    let Some(run_ticket) = state.tickets.get(ticket_id).cloned() else {
        return Ok(());
    };
    if run_ticket.status != RunTicketStatus::Running {
        return Ok(());
    }
    let paths = WorkerPaths::new(Path::new(&state.run_dir), ticket_id);
    let Some(result) = read_result(&paths)? else {
        if let Some(worker_state) = read_state(&paths) {
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
                    ticket.phase = phase.clone();
                    ticket.detail = detail.clone();
                }
                let _ = update_execution(
                    Path::new(&state.store),
                    ticket_id,
                    &run_ticket.claim_id,
                    &run_ticket.claim_id,
                    phase,
                    detail,
                );
                save_state(state)?;
            }
        }
        return Ok(());
    };
    if result.claim_id != run_ticket.claim_id {
        return Ok(());
    }
    let status = match result.status {
        WorkerResultStatus::Done => {
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
        WorkerResultStatus::Failed => {
            finish_ticket(
                Path::new(&state.store),
                ticket_id,
                &state.run_id,
                &run_ticket.claim_id,
                TicketStatus::Failed,
                serde_json::to_value(&result)?,
                result.summary.clone(),
            )?
            .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} claim changed before finish"))?;
            RunTicketStatus::Failed
        }
        WorkerResultStatus::Blocked | WorkerResultStatus::NeedsInput => {
            let blockers = if result.status == WorkerResultStatus::Blocked {
                result.blockers.clone()
            } else {
                Vec::new()
            };
            match suspend_ticket(
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
            ) {
                Ok(Some(_)) => RunTicketStatus::Blocked,
                Ok(None) => {
                    anyhow::bail!("ticket {ticket_id} claim changed before suspend");
                }
                Err(_) => {
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
            }
        }
    };
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
    }
    if let Some(pane) = &run_ticket.pane {
        let _ = backend.close(pane);
    }
    save_state(state)?;
    Ok(())
}

fn launch_ready(state: &mut RunState, backend: &dyn PaneBackend, max_parallel: u32) -> Result<()> {
    let running = state
        .tickets
        .values()
        .filter(|ticket| ticket.status == RunTicketStatus::Running)
        .count() as u32;
    let capacity = max_parallel.saturating_sub(running);
    if capacity == 0 {
        return Ok(());
    }
    let tickets = list_tickets(Path::new(&state.store))?;
    let model_policy = state.model_policy.clone();
    let models = state.model_catalog.clone();
    let usage = state.usage.clone();
    let config = LyraConfig {
        max_parallel,
        max_per_provider: state.max_per_provider.clone(),
        approval_posture: state.approval,
        merge: MergeMode::Auto,
    };
    let ready: Vec<Ticket> = ready_tickets(&tickets)
        .into_iter()
        .filter(|ticket| {
            if ticket.suspension.as_ref().is_some_and(|s| s.needs_input) {
                return false;
            }
            match state.tickets.get(&ticket.id) {
                Some(status) => matches!(
                    status.status,
                    RunTicketStatus::Failed | RunTicketStatus::Blocked
                ),
                None => true,
            }
        })
        .cloned()
        .collect();
    if ready.is_empty() {
        return Ok(());
    }
    let anchor = backend.anchor()?;
    let base_ref = state.base_ref.clone();
    let launch_context = LaunchContext {
        config: &config,
        max_seconds: None,
        base_ref: &base_ref,
        model_policy: model_policy.as_ref(),
        models: &models,
        usage: &usage,
    };
    for ticket in ready.into_iter().take(capacity as usize) {
        if let Err(error) = launch_ticket(state, &ticket, backend, &anchor, &launch_context) {
            eprintln!("lyra: launch {} failed: {error:#}", ticket.id);
        }
    }
    Ok(())
}

pub fn monitor_run(options: MonitorOptions) -> Result<RunState> {
    let backend = match options.backend {
        Some(backend) => backend,
        None => select_backend()?,
    };
    let mut state = load_state(&options.run_dir)?;
    let deadline = options
        .timeout_ms
        .map(|timeout| Instant::now() + Duration::from_millis(timeout));
    let max_parallel = options.max_parallel.unwrap_or(state.max_parallel);
    loop {
        let running: Vec<String> = state
            .tickets
            .iter()
            .filter(|(_, ticket)| ticket.status == RunTicketStatus::Running)
            .map(|(id, _)| id.clone())
            .collect();
        for ticket_id in running {
            process_requests(&state, &ticket_id)?;
            settle_worker(&mut state, &ticket_id, backend.as_ref())?;
        }
        state = load_state(&options.run_dir)?;
        launch_ready(&mut state, backend.as_ref(), max_parallel)?;
        state = load_state(&options.run_dir)?;
        let active = state
            .tickets
            .values()
            .any(|ticket| ticket.status == RunTicketStatus::Running);
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
            let open = tickets.iter().any(|ticket| {
                matches!(ticket.status, TicketStatus::Todo | TicketStatus::InProgress)
            });
            state.status = if open {
                RunStatus::Blocked
            } else {
                RunStatus::Done
            };
            save_state(&mut state)?;
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

pub fn stop_run(run_dir: &Path, backend: Option<Box<dyn PaneBackend>>) -> Result<RunState> {
    let backend = match backend {
        Some(backend) => backend,
        None => select_backend()?,
    };
    let mut state = load_state(run_dir)?;
    for (ticket_id, run_ticket) in state.tickets.clone() {
        if run_ticket.status != RunTicketStatus::Running {
            continue;
        }
        if let Some(pane) = &run_ticket.pane {
            let _ = backend.close(pane);
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
    state.status = RunStatus::Closed;
    save_state(&mut state)?;
    Ok(state)
}
