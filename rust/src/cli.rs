use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::acceptance::validate_acceptance_paths;
use crate::agent::{RunSummary, TicketSummary, WaitSummary, build_status};
use crate::conductor::{
    MonitorOptions, RunState, StartOptions, WaitOptions, WaitOutcome, approve_ticket, close_run,
    latest_run_dir, load_state, monitor_run, reject_ticket, run_dir_for, start_run, stop_run,
    wait_run,
};
use crate::graph::{to_dot, to_json, to_mermaid};
use crate::project::{current_project, project_id_for, store_dir_for};
use crate::project_config::ProjectConfig;
use crate::runner;
use crate::schema::{
    PRIORITY_MAX, PRIORITY_MIN, Ticket, TicketStatus, ready_tickets, ticket_map, validate_graph,
};
use crate::store::{NewTicket, create_ticket, ensure_store, list_tickets, read_ticket};
use crate::tickets::{BatchInput, TicketUpdate, create_batch, update_ticket};
use crate::worker::WorkerPaths;

#[derive(Parser)]
#[command(
    name = "lyra",
    version,
    about = "Lyra — local task conductor for coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create the local store and the project's `lyra.toml`.
    Init,
    /// Print the project graph and ready tickets.
    Status {
        #[arg(long)]
        store: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Print the ticket graph as Mermaid, DOT, or JSON.
    Graph {
        #[arg(long, value_enum, default_value_t = GraphFormat::Mermaid)]
        format: GraphFormat,
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Add one ticket from a one-line idea.
    Add {
        idea: String,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long = "blocked-by")]
        blocked_by: Vec<String>,
        #[arg(long, value_parser = clap::value_parser!(i32).range(PRIORITY_MIN as i64..=PRIORITY_MAX as i64))]
        priority: Option<i32>,
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// List, show, create, or update tickets.
    Tickets {
        #[command(subcommand)]
        command: TicketsCommand,
    },
    /// Start a run of ready tickets.
    Start {
        #[arg(long = "ticket", action = clap::ArgAction::Append)]
        ticket: Vec<String>,
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
        #[arg(long = "dry-run")]
        dry_run: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Advance a run: settle workers, retry, launch ready tickets.
    Monitor {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        once: bool,
        #[arg(long = "interval-ms", default_value_t = 1000)]
        interval_ms: u64,
        #[arg(long = "timeout-ms")]
        timeout_ms: Option<u64>,
        #[arg(long = "max-parallel")]
        max_parallel: Option<u32>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Wait for a run revision change or settlement without stopping it.
    Wait {
        #[arg(long)]
        run: Option<String>,
        #[arg(long = "after-revision")]
        after_revision: Option<u64>,
        #[arg(long = "timeout-ms", default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(long = "interval-ms", default_value_t = 500)]
        interval_ms: u64,
        #[arg(long = "max-parallel")]
        max_parallel: Option<u32>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Stop running workers and release their claims.
    Stop {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Approve a plan gate or a review gate.
    Approve {
        ticket: String,
        #[arg(long)]
        run: Option<String>,
        /// Mark done without merging the branch into the base branch.
        #[arg(long = "no-merge")]
        no_merge: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Reject a gate with a note; the worker re-runs that phase with feedback.
    Reject {
        ticket: String,
        #[arg(long)]
        note: String,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Close a run and remove clean worktrees.
    Close {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Print a run summary.
    Report {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        concise: bool,
    },
    /// Print a worker's log.
    Logs {
        ticket: String,
        #[arg(long)]
        run: Option<String>,
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    /// Serve the local workspace board.
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 4321)]
        port: u16,
        #[arg(long)]
        open: bool,
    },
    /// Upgrade the Lyra binary.
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
        harness: Option<String>,
        #[arg(long)]
        json: bool,
    },
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

fn resolve_run_dir(root: &Path, run: Option<String>) -> Result<PathBuf> {
    match run {
        Some(run_id) => Ok(run_dir_for(root, &run_id)),
        None => latest_run_dir(root)?.ok_or_else(|| anyhow::anyhow!("no runs found")),
    }
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
        .map(|ticket| ticket.id.as_str())
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
                let phase = ticket.phase.as_deref().unwrap_or("-");
                println!(
                    "  {:<10} {:<8} attempt {:<2} {:<32} {}",
                    id,
                    format!("{:?}", ticket.status),
                    ticket.attempt,
                    ticket.branch,
                    phase
                );
            }
        }
    }
    Ok(())
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
                object.insert("timedOut".to_string(), json!(outcome.timed_out));
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    } else {
        print_run(&outcome.state, false, false)?;
        println!("Timed out: {}", outcome.timed_out);
    }
    Ok(())
}

fn print_logs(root: &Path, ticket_id: &str, run: Option<String>, lines: usize) -> Result<()> {
    let run_dir = resolve_run_dir(root, run)?;
    let state = load_state(&run_dir)?;
    if !state.tickets.contains_key(ticket_id) {
        anyhow::bail!("ticket {ticket_id} is not part of run {}", state.run_id);
    }
    let paths = WorkerPaths::new(&run_dir, ticket_id);
    let text = runner::tail(&paths.log, lines);
    if text.trim().is_empty() {
        if let Ok(state_text) = std::fs::read_to_string(&paths.state) {
            println!("{state_text}");
        } else {
            println!("(no output for {ticket_id})");
        }
    } else {
        println!("{text}");
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
        Some(Command::Init) => {
            let context = resolve_store(None, None)?;
            ensure_store(&context.store)?;
            let path = crate::project_config::ensure_template(&context.root)?;
            println!("Project:  {}", context.root.display());
            println!("Project ID: {}", context.project_id);
            println!("Store:    {}", context.store.display());
            println!("Config:   {}", path.display());
            Ok(())
        }
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
            Ok(())
        }
        Some(Command::Graph { format, store }) => {
            let context = resolve_store(store, None)?;
            let tickets = list_tickets(&context.store)?;
            let output = match format {
                GraphFormat::Mermaid => to_mermaid(&tickets),
                GraphFormat::Dot => to_dot(&tickets),
                GraphFormat::Json => serde_json::to_string_pretty(&to_json(&tickets))?,
            };
            println!("{output}");
            Ok(())
        }
        Some(Command::Add {
            idea,
            prompt,
            blocked_by,
            priority,
            store,
        }) => {
            let context = resolve_store(store, None)?;
            ensure_store(&context.store)?;
            let ticket = create_ticket(
                &context.store,
                NewTicket {
                    project_id: context.project_id,
                    subject: idea.clone(),
                    prompt: prompt.unwrap_or_else(|| idea.clone()),
                    blocked_by,
                    priority: priority.unwrap_or(crate::schema::PRIORITY_DEFAULT),
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
            Ok(())
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
                Ok(())
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
                    println!("\nPrompt:\n{}", ticket.prompt);
                    if let Some(acceptance) = &ticket.acceptance {
                        println!("\nAcceptance:\n{acceptance}");
                    }
                }
                Ok(())
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
                Ok(())
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
                harness,
                json,
            } => {
                let context = resolve_store(store, None)?;
                let ticket = update_ticket(
                    &context.store,
                    &id,
                    TicketUpdate {
                        subject,
                        prompt,
                        acceptance,
                        clear_acceptance,
                        priority,
                        blocked_by,
                        clear_blocked_by,
                        labels,
                        clear_labels,
                        harness,
                    },
                )?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&ticket)?);
                } else {
                    println!("Updated {}  {}", ticket.id, ticket.subject);
                }
                Ok(())
            }
        },
        Some(Command::Start {
            ticket,
            tickets,
            max_parallel,
            max_seconds,
            base,
            run_id,
            dry_run,
            json,
            concise,
        }) => {
            let context = resolve_store(None, None)?;
            let config = ProjectConfig::load(&context.root)?;
            let mut ticket_ids = ticket;
            ticket_ids.extend(tickets);
            let state = start_run(StartOptions {
                repo: context.root,
                store: context.store,
                project_id: context.project_id,
                config,
                max_parallel,
                max_seconds,
                ticket_ids,
                base_ref: base,
                dry_run,
                run_id,
            })?;
            print_run(&state, json, concise)
        }
        Some(Command::Monitor {
            run,
            once,
            interval_ms,
            timeout_ms,
            max_parallel,
            json,
            concise,
        }) => {
            let context = resolve_store(None, None)?;
            let state = monitor_run(MonitorOptions {
                run_dir: resolve_run_dir(&context.root, run)?,
                once,
                interval_ms,
                timeout_ms,
                max_parallel,
            })?;
            print_run(&state, json, concise)
        }
        Some(Command::Wait {
            run,
            after_revision,
            timeout_ms,
            interval_ms,
            max_parallel,
            json,
            concise,
        }) => {
            let context = resolve_store(None, None)?;
            let outcome = wait_run(WaitOptions {
                run_dir: resolve_run_dir(&context.root, run)?,
                after_revision,
                timeout_ms,
                interval_ms,
                max_parallel,
            })?;
            print_wait(&outcome, json, concise)
        }
        Some(Command::Stop { run, json, concise }) => {
            let context = resolve_store(None, None)?;
            let state = stop_run(&resolve_run_dir(&context.root, run)?)?;
            print_run(&state, json, concise)
        }
        Some(Command::Approve {
            ticket,
            run,
            no_merge,
            json,
            concise,
        }) => {
            let context = resolve_store(None, None)?;
            let state = approve_ticket(&resolve_run_dir(&context.root, run)?, &ticket, !no_merge)?;
            print_run(&state, json, concise)
        }
        Some(Command::Reject {
            ticket,
            note,
            run,
            json,
            concise,
        }) => {
            let context = resolve_store(None, None)?;
            let state = reject_ticket(&resolve_run_dir(&context.root, run)?, &ticket, &note)?;
            print_run(&state, json, concise)
        }
        Some(Command::Close { run, json, concise }) => {
            let context = resolve_store(None, None)?;
            let state = close_run(&resolve_run_dir(&context.root, run)?)?;
            print_run(&state, json, concise)
        }
        Some(Command::Report { run, json, concise }) => {
            let context = resolve_store(None, None)?;
            let state = load_state(&resolve_run_dir(&context.root, run)?)?;
            print_run(&state, json, concise)
        }
        Some(Command::Logs { ticket, run, lines }) => {
            let context = resolve_store(None, None)?;
            print_logs(&context.root, &ticket, run, lines)
        }
        Some(Command::Serve { host, port, open }) => {
            crate::server::serve(crate::server::ServeOptions { host, port, open })
        }
        Some(Command::Upgrade {
            check,
            force,
            version,
            repo,
        }) => crate::upgrade::run(crate::upgrade::UpgradeOptions {
            check,
            force,
            version,
            repo,
        }),
    }
}
