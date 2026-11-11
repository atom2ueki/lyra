use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::acceptance::validate_acceptance_paths;
use crate::config::{ensure_config, load_config, lyra_home};
use crate::graph::{to_dot, to_json, to_mermaid};
use crate::import::import_omp_kanban;
use crate::models::{
    ensure_model_policy, load_model_catalog, load_policy, load_usage, model_policy_path,
    select_model,
};
use crate::project::{current_project, project_id_for, store_dir_for};
use crate::schema::{ready_tickets, ticket_map, validate_graph, Ticket, TicketStatus};
use crate::store::{create_ticket, ensure_store, list_tickets, NewTicket};

#[derive(Parser)]
#[command(name = "lyra", version, about = "Lyra — supervised task conductor")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Init,
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
    Doctor,
}

#[derive(Subcommand)]
enum ConductCommand {
    Status {
        #[arg(long)]
        store: Option<PathBuf>,
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

fn print_status(store: &Path, project_id: &str, root: &Path, tickets: &[Ticket], json_output: bool) -> Result<i32> {
    let mut errors = validate_graph(tickets);
    errors.extend(validate_acceptance_paths(tickets, root));
    let by_id = ticket_map(tickets);
    let ready: Vec<&str> = ready_tickets(tickets).iter().map(|t| t.id.as_str()).collect();
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
    if json_output {
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
            if ready.is_empty() { "(none)".to_string() } else { ready.join(", ") }
        );
        println!(
            "Blocked:  {}",
            if blocked.is_empty() { "(none)".to_string() } else { blocked.join(", ") }
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
    check("config", load_config().map(|config| {
        format!(
            "max_parallel={} approval={:?} merge={:?}",
            config.max_parallel, config.approval_posture, config.merge
        )
    }));
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
    check("pane backend", Ok(if std::env::var("HERDR_ENV").ok().as_deref() == Some("1") {
        "herdr".to_string()
    } else if std::env::var("TERM_PROGRAM").ok().as_deref() == Some("ghostty") {
        "ghostty".to_string()
    } else {
        "headless".to_string()
    }));
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
                println!("Imported: {} ticket(s) from {}", report.imported, report.source);
                println!("Target:   {}", report.target);
                println!("Project:  {}", report.project_id);
                println!(
                    "Attachments copied: {}",
                    if report.attachments_copied { "yes" } else { "no" }
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
            ConductCommand::Status { store, json } => {
                let context = resolve_store(store, None)?;
                let tickets = list_tickets(&context.store)?;
                let code = print_status(
                    &context.store,
                    &context.project_id,
                    &context.root,
                    &tickets,
                    json,
                )?;
                if code != 0 {
                    std::process::exit(code);
                }
            }
        },
        Some(Command::Status { store, json }) => {
            let context = resolve_store(store, None)?;
            let tickets = list_tickets(&context.store)?;
            let code = print_status(
                &context.store,
                &context.project_id,
                &context.root,
                &tickets,
                json,
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
                GraphFormat::Json => println!("{}", serde_json::to_string_pretty(&to_json(&tickets))?),
            }
        }
        Some(Command::Models { json }) => {
            let policy = load_policy()?;
            let models = load_model_catalog().unwrap_or_default();
            let usage = load_usage().unwrap_or_default();
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
                    }))?
                );
            } else {
                println!("Policy:   {}", model_policy_path().display());
                println!("Models:   {} available", models.len());
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
        Some(Command::Doctor) => {
            let code = run_doctor()?;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}
