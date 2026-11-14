use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::acceptance::validate_acceptance_paths;
use crate::agent::{RunSummary, TicketSummary, WaitSummary, build_status};
use crate::conductor::{
    MonitorOptions, RunState, StartOptions, WaitOptions, WaitOutcome, close_run, latest_run_dir,
    load_state, monitor_run, run_dir_for, start_run, stop_run, wait_run,
};
use crate::config::{ensure_config, load_config, lyra_home, set_pane_backend};
use crate::graph::{to_dot, to_json, to_mermaid};
use crate::import::import_omp_kanban;
use crate::models::{
    ensure_model_policy, load_model_catalog, load_policy, load_stats, load_usage,
    model_policy_path, select_model,
};
use crate::pane::{
    PaneBackend, ghostty::GhosttyBackend, headless::HeadlessBackend, herdr::HerdrBackend,
    resolve_backend, select_backend,
};
use crate::project::{current_project, project_id_for, store_dir_for};
use crate::schema::{
    PRIORITY_MAX, PRIORITY_MIN, Ticket, TicketStatus, ready_tickets, ticket_map, validate_graph,
};
use crate::store::{NewTicket, create_ticket, ensure_store, list_tickets, read_ticket};
use crate::tickets::{BatchInput, TicketUpdate, create_batch, update_ticket};
use crate::worker::ensure_worker_extension;

#[derive(Parser)]
#[command(name = "lyra", version, about = "Lyra — supervised task conductor")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Init,
    Setup,
    Import {
        kind: String,
        source: PathBuf,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long = "project-id")]
        project_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Conduct {
        #[command(subcommand)]
        command: ConductCommand,
    },
    Status {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Graph {
        #[arg(long, value_enum, default_value_t = GraphFormat::Mermaid)]
        format: GraphFormat,
        #[arg(long)]
        store: Option<PathBuf>,
    },
    Models {
        #[arg(long)]
        json: bool,
    },
    Add {
        idea: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long = "project-id")]
        project_id: Option<String>,
    },
    Tickets {
        #[command(subcommand)]
        command: TicketsCommand,
    },
    Close {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Logs {
        ticket: String,
        #[arg(long)]
        run: Option<String>,
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    Report {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Upgrade {
        #[arg(long)]
        check: bool,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        repo: Option<String>,
    },
    Doctor,
}

#[derive(Subcommand)]
enum ConductCommand {
    Status {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Run {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Start {
        /// Explicit ready ticket id; repeat the flag for multiple tickets.
        #[arg(long = "ticket", action = clap::ArgAction::Append)]
        ticket: Vec<String>,
        /// Comma-separated explicit ready ticket ids.
        #[arg(long = "tickets", value_delimiter = ',')]
        tickets: Vec<String>,
        #[arg(long = "max-parallel", alias = "max")]
        max_parallel: Option<u32>,
        #[arg(long = "max-seconds")]
        max_seconds: Option<u64>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long, value_enum)]
        backend: Option<PaneBackendKind>,
        #[arg(long = "dry-run")]
        dry_run: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Monitor {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        once: bool,
        #[arg(long = "interval-ms", default_value_t = 1000)]
        interval_ms: u64,
        /// Interactive monitor timeout marks the run stopped when it expires.
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
        #[arg(long = "max-parallel")]
        max_parallel: Option<u32>,
        #[arg(long, value_enum)]
        backend: Option<PaneBackendKind>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Wait {
        #[arg(long)]
        run: Option<String>,
        /// Return immediately if the run revision is greater than this; defaults to the current revision.
        #[arg(long = "after-revision")]
        after_revision: Option<u64>,
        /// Bounded wait; expiry returns timed_out without changing the run status.
        #[arg(long = "timeout-ms", default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(long = "interval-ms", default_value_t = 500)]
        interval_ms: u64,
        #[arg(long = "max-parallel")]
        max_parallel: Option<u32>,
        #[arg(long, value_enum)]
        backend: Option<PaneBackendKind>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Report {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Resume {
        #[arg(long)]
        run: Option<String>,
        #[arg(long = "interval-ms", default_value_t = 1000)]
        interval_ms: u64,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
        #[arg(long = "max-parallel")]
        max_parallel: Option<u32>,
        #[arg(long, value_enum)]
        backend: Option<PaneBackendKind>,
        #[arg(long)]
        json: bool,
    },
    Stop {
        #[arg(long)]
        run: Option<String>,
        #[arg(long, value_enum)]
        backend: Option<PaneBackendKind>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    Close {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum PaneBackendKind {
    Herdr,
    Ghostty,
    Headless,
}

fn pane_backend(kind: Option<PaneBackendKind>) -> Result<Box<dyn PaneBackend>> {
    match kind {
        Some(PaneBackendKind::Herdr) => Ok(Box::new(HerdrBackend::new())),
        Some(PaneBackendKind::Ghostty) => Ok(Box::new(GhosttyBackend::new())),
        Some(PaneBackendKind::Headless) => Ok(Box::new(HeadlessBackend::new())),
        None => select_backend(),
    }
}

fn resolve_run_dir(root: &Path, run: Option<String>) -> Result<PathBuf> {
    match run {
        Some(run_id) => Ok(run_dir_for(root, &run_id)),
        None => latest_run_dir(root)?.ok_or_else(|| anyhow::anyhow!("no runs found")),
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn print_logs(root: &Path, ticket_id: &str, run: Option<String>, lines: usize) -> Result<()> {
    let run_dir = resolve_run_dir(root, run)?;
    let state = load_state(&run_dir)?;
    let ticket = state
        .tickets
        .get(ticket_id)
        .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} is not part of run {}", state.run_id))?;
    let worker_dir = run_dir
        .join("workers")
        .join(crate::worktree::safe_segment(ticket_id));
    let logs = worker_dir.join("logs");
    if let Ok(entries) = std::fs::read_dir(&logs) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("log"))
            .collect();
        files.sort();
        let mut printed = false;
        for file in files {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.trim().is_empty()
            {
                println!("== {} ==", file.display());
                println!("{}", tail(&text, lines));
                printed = true;
            }
        }
        if printed {
            return Ok(());
        }
    }
    if let Ok(text) = std::fs::read_to_string(worker_dir.join("state.json")) {
        println!("{}", text);
    } else if let Some(pane) = &ticket.pane {
        println!("pane: {:?}", pane);
    } else {
        println!("(no output for {ticket_id})");
    }
    Ok(())
}

fn print_run(state: &RunState, json_output: bool, concise: bool) -> Result<()> {
    if json_output {
        if concise {
            println!(
                "{}",
                serde_json::to_string_pretty(&RunSummary::from_state(state))?
            );
        } else {
            println!("{}", serde_json::to_string_pretty(state)?);
        }
    } else {
        println!("Run:      {}", state.run_id);
        println!("Project:  {}", state.project_id);
        println!("Status:   {:?}", state.status);
        println!("Backend:  {}", state.backend);
        println!("Base:     {}", state.base_ref);
        println!("Dir:      {}", state.run_dir);
        println!("Parallel: {}", state.max_parallel);
        if state.tickets.is_empty() {
            println!("Tickets:  (none launched)");
        } else {
            println!("Tickets:");
            let mut ids: Vec<_> = state.tickets.keys().collect();
            ids.sort();
            for id in ids {
                let ticket = &state.tickets[id];
                let model = ticket.model.as_deref().unwrap_or("-");
                let phase = ticket.phase.as_deref().unwrap_or("-");
                println!(
                    "  {:<10} {:<8} {:<32} {:<24} {}",
                    id,
                    format!("{:?}", ticket.status),
                    ticket.branch,
                    model,
                    phase
                );
            }
        }
    }
    Ok(())
}

#[derive(Subcommand)]
enum TicketsCommand {
    List {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        details: bool,
    },
    Show {
        id: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Create {
        #[arg(long, default_value = "-")]
        file: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long = "project-id")]
        project_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Update {
        id: String,
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long, conflicts_with = "clear_acceptance")]
        acceptance: Option<String>,
        #[arg(long = "clear-acceptance")]
        clear_acceptance: bool,
        #[arg(long, value_parser = clap::value_parser!(i32).range(PRIORITY_MIN as i64..=PRIORITY_MAX as i64))]
        priority: Option<i32>,
        #[arg(long = "blocked-by")]
        blocked_by: Vec<String>,
        #[arg(long = "clear-blocked-by", conflicts_with = "blocked_by")]
        clear_blocked_by: bool,
        #[arg(long = "label")]
        labels: Vec<String>,
        #[arg(long = "clear-labels", conflicts_with = "labels")]
        clear_labels: bool,
        #[arg(long)]
        json: bool,
    },
}

fn print_wait(outcome: &WaitOutcome, json_output: bool, concise: bool) -> Result<()> {
    if json_output {
        let run = RunSummary::from_state(&outcome.state);
        if concise {
            println!(
                "{}",
                serde_json::to_string_pretty(&WaitSummary {
                    run,
                    timed_out: outcome.timed_out,
                })?
            );
        } else {
            let mut value = serde_json::to_value(&outcome.state)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("timedOut".to_string(), serde_json::json!(outcome.timed_out));
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    } else {
        print_run(&outcome.state, false, false)?;
        println!("Timed out: {}", outcome.timed_out);
    }
    Ok(())
}

#[derive(Clone, Copy, ValueEnum)]
enum GraphFormat {
    Mermaid,
    Dot,
    Json,
}

struct StoreContext {
    root: PathBuf,
    project_id: String,
    store: PathBuf,
}

fn resolve_store(store: Option<PathBuf>, project_id: Option<String>) -> Result<StoreContext> {
    let (root, inferred_id, inferred_store) = current_project()?;
    let project_id = project_id.unwrap_or(inferred_id);
    let store = store.unwrap_or_else(|| {
        if project_id == project_id_for(&root) {
            inferred_store
        } else {
            store_dir_for(&root)
        }
    });
    Ok(StoreContext {
        root,
        project_id,
        store,
    })
}

fn counts(tickets: &[Ticket]) -> HashMap<&'static str, usize> {
    let mut counts = HashMap::new();
    for ticket in tickets {
        let key = match ticket.status {
            TicketStatus::Todo => "todo",
            TicketStatus::InProgress => "in_progress",
            TicketStatus::Done => "done",
            TicketStatus::Failed => "failed",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

fn print_status(
    store: &Path,
    project_id: &str,
    root: &Path,
    tickets: &[Ticket],
    json_output: bool,
    concise: bool,
) -> Result<i32> {
    let mut errors = validate_graph(tickets);
    errors.extend(validate_acceptance_paths(tickets, root));
    let by_id = ticket_map(tickets);
    let ready: Vec<&str> = ready_tickets(tickets)
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    let blocked: Vec<&str> = tickets
        .iter()
        .filter(|ticket| {
            ticket.status == TicketStatus::Todo
                && ticket
                    .blocked_by
                    .iter()
                    .any(|id| by_id.get(id.as_str()).map(|t| t.status) != Some(TicketStatus::Done))
        })
        .map(|ticket| ticket.id.as_str())
        .collect();
    let effective_project = tickets
        .first()
        .map(|ticket| ticket.project_id.as_str())
        .unwrap_or(project_id);
    let count_map = counts(tickets);
    if json_output && concise {
        let run = latest_run_dir(root)
            .ok()
            .flatten()
            .and_then(|dir| load_state(&dir).ok());
        let payload = build_status(effective_project, tickets, errors.clone(), run.as_ref());
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if json_output {
        let payload = json!({
            "store": store.display().to_string(),
            "projectId": effective_project,
            "counts": {
                "todo": count_map.get("todo").copied().unwrap_or(0),
                "in_progress": count_map.get("in_progress").copied().unwrap_or(0),
                "done": count_map.get("done").copied().unwrap_or(0),
                "failed": count_map.get("failed").copied().unwrap_or(0),
            },
            "ready": ready,
            "blocked": blocked,
            "errors": errors,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("Store:    {}", store.display());
        println!("Project:  {effective_project}");
        println!(
            "Tickets:  {} (todo {}, in_progress {}, done {}, failed {})",
            tickets.len(),
            count_map.get("todo").copied().unwrap_or(0),
            count_map.get("in_progress").copied().unwrap_or(0),
            count_map.get("done").copied().unwrap_or(0),
            count_map.get("failed").copied().unwrap_or(0),
        );
        println!(
            "Ready:    {}",
            if ready.is_empty() {
                "(none)".to_string()
            } else {
                ready.join(", ")
            }
        );
        println!(
            "Blocked:  {}",
            if blocked.is_empty() {
                "(none)".to_string()
            } else {
                blocked.join(", ")
            }
        );
        if errors.is_empty() {
            println!("Graph:    valid");
        } else {
            println!("Errors:");
            for error in &errors {
                println!("  - {error}");
            }
        }
    }
    Ok(if errors.is_empty() { 0 } else { 1 })
}

fn command_version(bin: &str, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("run {bin}"))?;
    if !output.status.success() {
        anyhow::bail!("{bin} {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_doctor() -> Result<i32> {
    let mut failures = Vec::new();
    let mut check = |name: &str, result: Result<String>| match result {
        Ok(detail) => println!("PASS  {name}  {detail}"),
        Err(error) => {
            failures.push(name.to_string());
            println!("FAIL  {name}  {error}");
        }
    };

    let omp_bin = std::env::var("LYRA_OMP_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "omp".to_string());
    check("omp", command_version(&omp_bin, &["--version"]));
    check("git", command_version("git", &["--version"]));
    check("gh", command_version("gh", &["--version"]));
    check(
        "config",
        load_config().map(|config| {
            format!(
                "max_parallel={} approval={:?} merge={:?} pane_backend={}",
                config.max_parallel,
                config.approval_posture,
                config.merge,
                config.pane_backend.as_deref().unwrap_or("<unset>")
            )
        }),
    );
    check(
        "model policy",
        load_policy().map(|policy| {
            format!(
                "{} classes, {} roles",
                policy.classes.len(),
                policy.roles.len()
            )
        }),
    );
    check(
        "worker extension",
        ensure_worker_extension().map(|path| path.display().to_string()),
    );
    match resolve_backend() {
        Ok(choice) => {
            match choice.name.as_str() {
                "herdr" => {
                    check("herdr", command_version("herdr", &["--version"]));
                }
                "ghostty" => check(
                    "ghostty",
                    Ok("macOS Ghostty surface control via AppleScript".to_string()),
                ),
                _ => {}
            }
            check(
                "pane backend",
                Ok(format!("{} ({})", choice.name, choice.source)),
            );
        }
        Err(error) => check("pane backend", Err(error)),
    }
    check(
        "tty",
        Ok(if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            "interactive".to_string()
        } else {
            "not a TTY (headless backend available)".to_string()
        }),
    );
    check(
        "ticket store",
        current_project().and_then(|(_, _, store)| {
            if store.exists() {
                Ok(store.display().to_string())
            } else {
                anyhow::bail!("missing {}; run lyra init", store.display())
            }
        }),
    );
    if failures.is_empty() {
        println!("\nAll checks passed");
        Ok(0)
    } else {
        println!("\n{} check(s) failed", failures.len());
        Ok(1)
    }
}

fn run_setup() -> Result<i32> {
    let config_path = ensure_config()?;
    let policy_path = ensure_model_policy()?;
    let (root, project_id, store) = current_project()?;
    ensure_store(&store)?;
    let extension = ensure_worker_extension()?;

    let omp_bin = std::env::var("LYRA_OMP_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "omp".to_string());
    let omp = command_version(&omp_bin, &["--version"]).map_err(|error| {
        anyhow::anyhow!("lyra needs the omp agent CLI on PATH (or LYRA_OMP_BIN): {error:#}")
    })?;
    let git = command_version("git", &["--version"])?;

    let choice = resolve_backend()?;
    let mut persisted = false;
    if load_config()?.pane_backend.is_none() && std::env::var("LYRA_PANE_BACKEND").is_err() {
        set_pane_backend(&choice.name)?;
        persisted = true;
    }

    println!("Config:          {}", config_path.display());
    println!("Model policy:    {}", policy_path.display());
    println!("Worker extension: {}", extension.display());
    println!("Project:         {project_id} ({})", root.display());
    println!("Store:           {}", store.display());
    println!("omp:             {omp}");
    println!("git:             {git}");
    println!(
        "Pane backend:    {} ({}){}",
        choice.name,
        choice.source,
        if persisted {
            " — saved to config"
        } else {
            ""
        }
    );
    if choice.name == "ghostty" {
        println!(
            "note: the ghostty backend drives Ghostty via AppleScript (macOS only) and needs the calling window focused for anchoring"
        );
    }
    if choice.name == "headless" {
        println!(
            "note: the headless backend runs workers without visible panes; run setup inside herdr or Ghostty for visible workers"
        );
    }
    let gh = command_version("gh", &["--version"])
        .unwrap_or_else(|_| "not found (optional)".to_string());
    println!("gh:              {gh}");
    println!(
        "\nSetup complete. Run `lyra doctor` for full checks, then `lyra add` + `lyra conduct start`."
    );
    Ok(0)
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            return Ok(());
        }
        Some(Command::Init) => {
            let config = ensure_config()?;
            let policy = ensure_model_policy()?;
            let (root, project_id, store) = current_project()?;
            ensure_store(&store)?;
            let loaded = load_config()?;
            println!("Config:   {}", config.display());
            println!("Policy:   {}", policy.display());
            println!("Home:     {}", lyra_home().display());
            println!("Project:  {}", root.display());
            println!("Project:  {project_id}");
            println!("Store:    {}", store.display());
            println!(
                "Policy:   max_parallel={} approval={:?} merge={:?}",
                loaded.max_parallel, loaded.approval_posture, loaded.merge
            );
        }
        Some(Command::Setup) => {
            std::process::exit(run_setup()?);
        }
        Some(Command::Import {
            kind,
            source,
            store,
            project_id,
            json,
        }) => {
            if kind != "omp-kanban" {
                anyhow::bail!("unknown import kind: {kind}");
            }
            let context = resolve_store(store, project_id)?;
            let report = import_omp_kanban(&source, &context.store, &context.project_id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Imported: {} ticket(s) from {}",
                    report.imported, report.source
                );
                println!("Target:   {}", report.target);
                println!("Project:  {}", report.project_id);
                println!(
                    "Attachments copied: {}",
                    if report.attachments_copied {
                        "yes"
                    } else {
                        "no"
                    }
                );
                println!(
                    "Events copied:      {}",
                    if report.events_copied { "yes" } else { "no" }
                );
                for error in &report.errors {
                    eprintln!("ERROR {}: {}", error.file, error.message);
                }
                for error in &report.validation_errors {
                    eprintln!("GRAPH {error}");
                }
            }
            if !report.errors.is_empty() || !report.validation_errors.is_empty() {
                std::process::exit(1);
            }
        }
        Some(Command::Conduct { command }) => match command {
            ConductCommand::Status {
                store,
                json,
                concise,
            } => {
                let context = resolve_store(store, None)?;
                let tickets = list_tickets(&context.store)?;
                let code = print_status(
                    &context.store,
                    &context.project_id,
                    &context.root,
                    &tickets,
                    json,
                    concise,
                )?;
                if code != 0 {
                    std::process::exit(code);
                }
            }
            ConductCommand::Run { run, json, concise } => {
                let context = resolve_store(None, None)?;
                let state = load_state(&resolve_run_dir(&context.root, run)?)?;
                print_run(&state, json, concise)?;
            }
            ConductCommand::Start {
                ticket,
                tickets,
                max_parallel,
                max_seconds,
                base,
                run_id,
                backend,
                dry_run,
                json,
                concise,
            } => {
                let context = resolve_store(None, None)?;
                let mut ticket_ids = ticket;
                ticket_ids.extend(tickets);
                let state = start_run(StartOptions {
                    repo: context.root.clone(),
                    store: context.store.clone(),
                    project_id: context.project_id.clone(),
                    config: load_config()?,
                    backend: Some(pane_backend(backend)?),
                    max_parallel,
                    max_seconds,
                    ticket_ids,
                    base_ref: base,
                    dry_run,
                    run_id,
                    model_policy: None,
                    models: None,
                    usage: None,
                })?;
                print_run(&state, json, concise)?;
            }
            ConductCommand::Monitor {
                run,
                once,
                interval_ms,
                timeout_ms,
                max_parallel,
                backend,
                json,
                concise,
            } => {
                let context = resolve_store(None, None)?;
                let state = monitor_run(MonitorOptions {
                    run_dir: resolve_run_dir(&context.root, run)?,
                    backend: Some(pane_backend(backend)?),
                    once,
                    interval_ms,
                    timeout_ms,
                    max_parallel,
                })?;
                print_run(&state, json, concise)?;
            }
            ConductCommand::Wait {
                run,
                after_revision,
                timeout_ms,
                interval_ms,
                max_parallel,
                backend,
                json,
                concise,
            } => {
                let context = resolve_store(None, None)?;
                let outcome = wait_run(WaitOptions {
                    run_dir: resolve_run_dir(&context.root, run)?,
                    backend: Some(pane_backend(backend)?),
                    after_revision,
                    timeout_ms,
                    interval_ms,
                    max_parallel,
                })?;
                print_wait(&outcome, json, concise)?;
            }
            ConductCommand::Report { run, json, concise } => {
                let context = resolve_store(None, None)?;
                let state = load_state(&resolve_run_dir(&context.root, run)?)?;
                print_run(&state, json, concise)?;
            }
            ConductCommand::Resume {
                run,
                interval_ms,
                timeout_ms,
                max_parallel,
                backend,
                json,
            } => {
                let context = resolve_store(None, None)?;
                let state = monitor_run(MonitorOptions {
                    run_dir: resolve_run_dir(&context.root, run)?,
                    backend: Some(pane_backend(backend)?),
                    once: false,
                    interval_ms,
                    timeout_ms,
                    max_parallel,
                })?;
                print_run(&state, json, false)?;
            }
            ConductCommand::Stop {
                run,
                backend,
                json,
                concise,
            } => {
                let context = resolve_store(None, None)?;
                let state = stop_run(
                    &resolve_run_dir(&context.root, run)?,
                    Some(pane_backend(backend)?),
                )?;
                print_run(&state, json, concise)?;
            }
            ConductCommand::Close { run, json, concise } => {
                let context = resolve_store(None, None)?;
                let state = close_run(&resolve_run_dir(&context.root, run)?)?;
                print_run(&state, json, concise)?;
            }
        },
        Some(Command::Status {
            store,
            json,
            concise,
        }) => {
            let context = resolve_store(store, None)?;
            let tickets = list_tickets(&context.store)?;
            let code = print_status(
                &context.store,
                &context.project_id,
                &context.root,
                &tickets,
                json,
                concise,
            )?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Some(Command::Graph { format, store }) => {
            let context = resolve_store(store, None)?;
            let tickets = list_tickets(&context.store)?;
            let errors = validate_graph(&tickets);
            if !errors.is_empty() {
                for error in errors {
                    eprintln!("GRAPH {error}");
                }
                std::process::exit(1);
            }
            match format {
                GraphFormat::Mermaid => print!("{}", to_mermaid(&tickets)),
                GraphFormat::Dot => print!("{}", to_dot(&tickets)),
                GraphFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&to_json(&tickets))?)
                }
            }
        }
        Some(Command::Models { json }) => {
            let policy = load_policy()?;
            let models = load_model_catalog().unwrap_or_default();
            let usage = load_usage().unwrap_or_default();
            let stats = load_stats().unwrap_or_default();
            let mut selections = HashMap::new();
            for name in policy.classes.keys() {
                let selected = select_model(
                    &policy,
                    &models,
                    &usage,
                    Some(name),
                    None,
                    &HashMap::new(),
                    &HashMap::new(),
                )
                .map(|choice| choice.selector);
                selections.insert(name.clone(), selected);
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "policy": policy,
                        "selections": selections,
                        "modelCount": models.len(),
                        "stats": stats,
                    }))?
                );
            } else {
                println!("Policy:   {}", model_policy_path().display());
                println!("Models:   {} available", models.len());
                println!("Stats:    {} model observations", stats.len());
                println!("Classes:");
                let mut classes: Vec<_> = selections.iter().collect();
                classes.sort_by_key(|(name, _)| *name);
                for (name, selected) in classes {
                    println!(
                        "  {:<16} {}",
                        name,
                        selected.as_deref().unwrap_or("(none available)")
                    );
                }
                println!("Roles:");
                let mut roles: Vec<_> = policy.roles.iter().collect();
                roles.sort_by_key(|(name, _)| *name);
                for (name, candidates) in roles {
                    println!(
                        "  {:<16} {}",
                        name,
                        candidates.first().map(String::as_str).unwrap_or("(unset)")
                    );
                }
            }
        }
        Some(Command::Add {
            idea,
            store,
            project_id,
        }) => {
            let context = resolve_store(store, project_id)?;
            ensure_store(&context.store)?;
            let ticket = create_ticket(
                &context.store,
                NewTicket {
                    project_id: context.project_id,
                    subject: idea.clone(),
                    prompt: idea,
                    blocked_by: Vec::new(),
                    priority: crate::schema::PRIORITY_DEFAULT,
                    labels: Vec::new(),
                    model_class: None,
                    acceptance: None,
                    plan: None,
                    parent: None,
                    origin: Some("inbox".to_string()),
                    harness: None,
                },
            )?;
            println!("{}  {}", ticket.id, ticket.subject);
        }
        Some(Command::Tickets { command }) => match command {
            TicketsCommand::List {
                store,
                json,
                details,
            } => {
                let context = resolve_store(store, None)?;
                let tickets = list_tickets(&context.store)?;
                let ready_ids: Vec<&str> = ready_tickets(&tickets)
                    .into_iter()
                    .map(|ticket| ticket.id.as_str())
                    .collect();
                let items: Vec<TicketSummary> = tickets
                    .iter()
                    .map(|ticket| {
                        TicketSummary::from_ticket(ticket, ready_ids.contains(&ticket.id.as_str()))
                    })
                    .collect();
                if json {
                    println!("{}", serde_json::to_string_pretty(&items)?);
                } else {
                    for item in &items {
                        println!(
                            "{}  {:<8}  {:<3}  {}{}",
                            item.id,
                            item.status,
                            item.priority,
                            item.subject,
                            if item.ready { "  [ready]" } else { "" }
                        );
                        if details {
                            if !item.blocked_by.is_empty() {
                                println!("    blocked by: {}", item.blocked_by.join(", "));
                            }
                            if let Some(error) = &item.last_error {
                                println!("    last error: {error}");
                            }
                            println!("    attempts: {}", item.attempts);
                        }
                    }
                }
            }
            TicketsCommand::Show { id, store, json } => {
                let context = resolve_store(store, None)?;
                let ticket = read_ticket(&context.store, &id)?
                    .ok_or_else(|| anyhow::anyhow!("unknown ticket: {id}"))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&ticket)?);
                } else {
                    println!("ID:          {}", ticket.id);
                    println!("Subject:     {}", ticket.subject);
                    println!("Status:      {:?}", ticket.status);
                    println!("Priority:    {}", ticket.priority);
                    println!(
                        "Blocked by:  {}",
                        if ticket.blocked_by.is_empty() {
                            "(none)".to_string()
                        } else {
                            ticket.blocked_by.join(", ")
                        }
                    );
                    println!("Attempts:    {}", ticket.attempts);
                    if let Some(error) = &ticket.last_error {
                        println!("Last error:  {error}");
                    }
                    println!(
                        "
Prompt:
{}",
                        ticket.prompt
                    );
                    if let Some(acceptance) = &ticket.acceptance {
                        println!(
                            "
Acceptance:
{acceptance}"
                        );
                    }
                    if let Some(plan) = &ticket.plan {
                        println!(
                            "
Plan:
{plan}"
                        );
                    }
                }
            }
            TicketsCommand::Create {
                file,
                store,
                project_id,
                json,
            } => {
                let context = resolve_store(store, project_id)?;
                let mut text = String::new();
                if file == "-" {
                    std::io::stdin().read_to_string(&mut text)?;
                } else {
                    text =
                        std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?;
                }
                let input: BatchInput =
                    serde_json::from_str(&text).with_context(|| "parse batch ticket JSON")?;
                let output = create_batch(&context.store, &context.project_id, input)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    for item in &output.created {
                        println!("{}  {}", item.id, item.key);
                    }
                }
            }
            TicketsCommand::Update {
                id,
                store,
                subject,
                prompt,
                acceptance,
                clear_acceptance,
                priority,
                blocked_by,
                clear_blocked_by,
                labels,
                clear_labels,
                json,
            } => {
                let context = resolve_store(store, None)?;
                let update = TicketUpdate {
                    subject,
                    prompt,
                    acceptance,
                    clear_acceptance,
                    priority,
                    blocked_by,
                    clear_blocked_by,
                    labels,
                    clear_labels,
                };
                let ticket = update_ticket(&context.store, &id, update)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&ticket)?);
                } else {
                    println!("Updated {}  {}", ticket.id, ticket.subject);
                }
            }
        },
        Some(Command::Close { run, json, concise }) => {
            let context = resolve_store(None, None)?;
            let state = close_run(&resolve_run_dir(&context.root, run)?)?;
            print_run(&state, json, concise)?;
        }
        Some(Command::Logs { ticket, run, lines }) => {
            let context = resolve_store(None, None)?;
            print_logs(&context.root, &ticket, run, lines)?;
        }
        Some(Command::Report { run, json, concise }) => {
            let context = resolve_store(None, None)?;
            let state = load_state(&resolve_run_dir(&context.root, run)?)?;
            print_run(&state, json, concise)?;
        }
        Some(Command::Upgrade {
            check,
            force,
            version,
            repo,
        }) => {
            crate::upgrade::run(crate::upgrade::UpgradeOptions {
                check,
                force,
                version,
                repo,
            })?;
        }
        Some(Command::Doctor) => {
            let code = run_doctor()?;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}
