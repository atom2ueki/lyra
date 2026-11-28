use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::agent::status_for;
use crate::conductor::{
    RunState, RunStatus, StartOptions, approve_ticket, close_run, latest_run_dir, list_run_ids,
    load_state, monitor_once, reject_ticket, start_run,
};
use crate::project::current_project;
use crate::project_config::ProjectConfig;
use crate::runner;
use crate::schema::{ready_tickets, ticket_map, validate_graph};
use crate::store::{NewTicket, create_ticket, ensure_store, list_tickets, read_ticket};
use crate::worker::WorkerPaths;

const INDEX: &str = include_str!("web/index.html");

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

#[derive(Clone)]
struct BoardContext {
    root: PathBuf,
    project_id: String,
    store: PathBuf,
}

pub struct ServeOptions {
    pub host: String,
    pub port: u16,
    pub open: bool,
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn response(body: String, status: u16, content_type: &str) -> HttpResponse {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
}

fn json_value(value: Value, status: u16) -> HttpResponse {
    response(
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string()),
        status,
        "application/json; charset=utf-8",
    )
}

fn error_response(message: &str, status: u16) -> HttpResponse {
    json_value(json!({"ok": false, "error": {"message": message}}), status)
}

fn read_body(request: &mut Request) -> Result<String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .context("read request body")?;
    Ok(body)
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(value.replace("%20", " "));
        }
    }
    None
}

fn run_for(root: &Path, requested: Option<&str>) -> Result<Option<RunState>> {
    let Some(run_dir) = requested
        .map(|run_id| crate::conductor::run_dir_for(root, run_id))
        .or(latest_run_dir(root)?)
    else {
        return Ok(None);
    };
    if !run_dir.join("state.json").exists() {
        return Ok(None);
    }
    Ok(Some(load_state(&run_dir)?))
}

fn board_state(context: &BoardContext) -> Result<Value> {
    let tickets = list_tickets(&context.store)?;
    let mut errors = validate_graph(&tickets);
    errors.extend(crate::acceptance::validate_acceptance_paths(
        &tickets,
        &context.root,
    ));
    let ready_ids: Vec<&str> = ready_tickets(&tickets)
        .into_iter()
        .map(|ticket| ticket.id.as_str())
        .collect();
    let by_id = ticket_map(&tickets);
    let blocked: Vec<&str> = tickets
        .iter()
        .filter(|ticket| {
            ticket.status == crate::schema::TicketStatus::Todo
                && ticket.blocked_by.iter().any(|id| {
                    by_id.get(id.as_str()).map(|t| t.status)
                        != Some(crate::schema::TicketStatus::Done)
                })
        })
        .map(|ticket| ticket.id.as_str())
        .collect();
    let run = run_for(&context.root, None)?;
    let run_dir = run.as_ref().map(|state| state.run_dir.clone());
    let items: Vec<Value> = tickets
        .iter()
        .map(|ticket| {
            let run_ticket = run.as_ref().and_then(|state| state.tickets.get(&ticket.id));
            json!({
                "id": ticket.id,
                "subject": ticket.subject,
                "prompt": ticket.prompt,
                "status": status_for(ticket),
                "priority": ticket.priority,
                "labels": ticket.labels,
                "blockedBy": ticket.blocked_by,
                "ready": ready_ids.contains(&ticket.id.as_str()),
                "attempts": ticket.attempts,
                "lastError": ticket.last_error,
                "acceptance": ticket.acceptance,
                "run": run_ticket,
            })
        })
        .collect();
    let mut counts = std::collections::HashMap::new();
    for ticket in &tickets {
        let key = match ticket.status {
            crate::schema::TicketStatus::Todo => "todo",
            crate::schema::TicketStatus::InProgress => "in_progress",
            crate::schema::TicketStatus::Done => "done",
            crate::schema::TicketStatus::Failed => "failed",
            crate::schema::TicketStatus::Closed => "closed",
        };
        *counts.entry(key).or_insert(0u64) += 1;
    }
    let counts = json!({
        "todo": counts.get("todo").copied().unwrap_or(0),
        "in_progress": counts.get("in_progress").copied().unwrap_or(0),
        "done": counts.get("done").copied().unwrap_or(0),
        "failed": counts.get("failed").copied().unwrap_or(0),
        "closed": counts.get("closed").copied().unwrap_or(0),
    });
    Ok(json!({
        "project": {
            "id": context.project_id,
            "root": context.root,
            "store": context.store,
        },
        "run": run,
        "runDir": run_dir,
        "tickets": items,
        "counts": counts,
        "ready": ready_ids,
        "blocked": blocked,
        "errors": errors,
        "runs": list_run_ids(&context.root).unwrap_or_default().into_iter().rev().take(8).collect::<Vec<_>>(),
    }))
}

fn ticket_detail(context: &BoardContext, ticket_id: &str, run_id: Option<&str>) -> Result<Value> {
    let ticket = read_ticket(&context.store, ticket_id)?
        .ok_or_else(|| anyhow::anyhow!("unknown ticket: {ticket_id}"))?;
    let run = run_for(&context.root, run_id)?;
    let run_ticket = run.as_ref().and_then(|state| state.tickets.get(ticket_id));
    let run_dir = run.as_ref().map(|state| PathBuf::from(&state.run_dir));
    let mut detail = json!({
        "ticket": ticket,
        "run": run_ticket,
        "runId": run.as_ref().map(|state| state.run_id.clone()),
    });
    if let Some(run_dir) = run_dir {
        let paths = WorkerPaths::new(&run_dir, ticket_id);
        let read = |path: &Path| fs::read_to_string(path).ok();
        let artifact =
            read(&paths.artifact).and_then(|text| serde_json::from_str::<Value>(&text).ok());
        detail["artifacts"] = json!({
            "plan": read(&paths.plan),
            "result": read(&paths.result_md),
            "diff": read(&paths.diff),
            "log": runner::tail(&paths.log, 200),
            "feedback": read(&paths.feedback),
            "artifact": artifact,
        });
    }
    Ok(detail)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTicketBody {
    subject: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    priority: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartBody {
    #[serde(default)]
    max_parallel: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateBody {
    ticket: String,
    #[serde(default = "default_merge")]
    merge: bool,
    #[serde(default)]
    note: Option<String>,
}

fn default_merge() -> bool {
    true
}

fn handle_api(
    context: &BoardContext,
    lock: &Arc<Mutex<()>>,
    method: &Method,
    url: &str,
    request: &mut Request,
) -> HttpResponse {
    if method == &Method::Get && url.starts_with("/api/state") {
        return match board_state(context) {
            Ok(value) => json_value(value, 200),
            Err(error) => error_response(&format!("{error:#}"), 500),
        };
    }
    if method == &Method::Get && url.starts_with("/api/ticket") {
        let ticket_id = query_param(url, "ticket").unwrap_or_default();
        if ticket_id.is_empty() {
            return error_response("missing ticket", 400);
        }
        let run_id = query_param(url, "run");
        return match ticket_detail(context, &ticket_id, run_id.as_deref()) {
            Ok(value) => json_value(value, 200),
            Err(error) => error_response(&format!("{error:#}"), 404),
        };
    }
    if method == &Method::Post && url == "/api/tickets" {
        let body = match read_body(request) {
            Ok(body) => body,
            Err(error) => return error_response(&format!("{error:#}"), 400),
        };
        let input: CreateTicketBody = match serde_json::from_str(&body) {
            Ok(input) => input,
            Err(error) => return error_response(&format!("invalid ticket: {error}"), 400),
        };
        if input.subject.trim().is_empty() {
            return error_response("subject is required", 400);
        }
        let _guard = lock.lock().unwrap();
        if let Err(error) = ensure_store(&context.store) {
            return error_response(&format!("{error:#}"), 500);
        }
        let ticket = create_ticket(
            &context.store,
            NewTicket {
                project_id: context.project_id.clone(),
                subject: input.subject,
                prompt: input.prompt.unwrap_or_default(),
                blocked_by: input.blocked_by,
                priority: input.priority.unwrap_or(crate::schema::PRIORITY_DEFAULT),
                labels: Vec::new(),
                model_class: None,
                acceptance: None,
                plan: None,
                parent: None,
                origin: Some("board".to_string()),
                harness: None,
            },
        );
        return match ticket {
            Ok(ticket) => json_value(json!({"ok": true, "ticket": ticket}), 200),
            Err(error) => error_response(&format!("{error:#}"), 400),
        };
    }
    if method == &Method::Post && url == "/api/start" {
        let body = read_body(request).unwrap_or_else(|_| "{}".to_string());
        let input: StartBody =
            serde_json::from_str(&body).unwrap_or(StartBody { max_parallel: None });
        let config = match ProjectConfig::load(&context.root) {
            Ok(config) => config,
            Err(error) => return error_response(&format!("{error:#}"), 400),
        };
        let _guard = lock.lock().unwrap();
        return match start_run(StartOptions {
            repo: context.root.clone(),
            store: context.store.clone(),
            project_id: context.project_id.clone(),
            config,
            max_parallel: input.max_parallel,
            max_seconds: None,
            ticket_ids: Vec::new(),
            base_ref: None,
            dry_run: false,
            run_id: None,
        }) {
            Ok(state) => json_value(json!({"ok": true, "run": state}), 200),
            Err(error) => error_response(&format!("{error:#}"), 400),
        };
    }
    if method == &Method::Post && (url == "/api/approve" || url == "/api/reject") {
        let body = match read_body(request) {
            Ok(body) => body,
            Err(error) => return error_response(&format!("{error:#}"), 400),
        };
        let input: GateBody = match serde_json::from_str(&body) {
            Ok(input) => input,
            Err(error) => return error_response(&format!("invalid gate: {error}"), 400),
        };
        let Some(run_dir) = latest_run_dir(&context.root).ok().flatten() else {
            return error_response("no runs found", 404);
        };
        let _guard = lock.lock().unwrap();
        let result = if url == "/api/approve" {
            approve_ticket(&run_dir, &input.ticket, input.merge)
        } else {
            reject_ticket(
                &run_dir,
                &input.ticket,
                input.note.as_deref().unwrap_or("changes requested"),
            )
        };
        return match result {
            Ok(state) => json_value(json!({"ok": true, "run": state}), 200),
            Err(error) => error_response(&format!("{error:#}"), 400),
        };
    }
    if method == &Method::Post && url == "/api/close" {
        let Some(run_dir) = latest_run_dir(&context.root).ok().flatten() else {
            return error_response("no runs found", 404);
        };
        let _guard = lock.lock().unwrap();
        return match close_run(&run_dir) {
            Ok(state) => json_value(json!({"ok": true, "run": state}), 200),
            Err(error) => error_response(&format!("{error:#}"), 400),
        };
    }
    error_response("not found", 404)
}

fn handle_request(
    context: &BoardContext,
    lock: &Arc<Mutex<()>>,
    request: &mut Request,
) -> HttpResponse {
    let method = request.method().clone();
    let url = request.url().to_string();
    if url == "/" || url.starts_with("/?") {
        return response(INDEX.to_string(), 200, "text/html; charset=utf-8");
    }
    if url == "/favicon.ico" {
        return response(String::new(), 204, "text/plain");
    }
    if url.starts_with("/api/") {
        return handle_api(context, lock, &method, &url, request);
    }
    error_response("not found", 404)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let _ = url;
}

pub fn serve(options: ServeOptions) -> Result<()> {
    let (root, project_id, store) = current_project()?;
    ensure_store(&store)?;
    let address = format!("{}:{}", options.host, options.port);
    let server = Server::http(&address).map_err(|error| anyhow::anyhow!("{error}"))?;
    let url = format!("http://{}:{}", options.host, options.port);
    println!("Lyra board: {url}");
    println!("Project:    {}", root.display());
    println!("Store:      {}", store.display());
    if options.open {
        open_browser(&url);
    }
    let context = BoardContext {
        root: root.clone(),
        project_id,
        store,
    };
    let lock = Arc::new(Mutex::new(()));
    let monitor_lock = lock.clone();
    let monitor_root = root.clone();
    thread::spawn(move || {
        loop {
            if let Ok(Some(run_dir)) = latest_run_dir(&monitor_root) {
                if let Ok(state) = load_state(&run_dir) {
                    if state.status == RunStatus::Running {
                        let _guard = monitor_lock.lock().unwrap();
                        let _ = monitor_once(&run_dir, state.max_parallel);
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
    for mut request in server.incoming_requests() {
        let response = handle_request(&context, &lock, &mut request);
        let _ = request.respond(response);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_params_are_read() {
        assert_eq!(
            query_param("/api/ticket?ticket=Alpha&run=run-1", "ticket").as_deref(),
            Some("Alpha")
        );
        assert_eq!(
            query_param("/api/ticket?ticket=Alpha&run=run-1", "run").as_deref(),
            Some("run-1")
        );
        assert_eq!(query_param("/api/state", "ticket"), None);
    }

    #[test]
    fn board_state_lists_ready_tickets() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        ensure_store(&store).unwrap();
        crate::store::create_ticket(
            &store,
            NewTicket {
                project_id: "p".to_string(),
                subject: "Alpha".to_string(),
                prompt: "do it".to_string(),
                blocked_by: Vec::new(),
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
        .unwrap();
        let context = BoardContext {
            root: dir.path().to_path_buf(),
            project_id: "p".to_string(),
            store,
        };
        let value = board_state(&context).unwrap();
        assert_eq!(value["tickets"].as_array().unwrap().len(), 1);
        assert_eq!(value["ready"].as_array().unwrap().len(), 1);
        assert_eq!(value["counts"]["todo"], 1);
    }

    #[test]
    fn missing_ticket_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        ensure_store(&store).unwrap();
        let context = BoardContext {
            root: dir.path().to_path_buf(),
            project_id: "p".to_string(),
            store,
        };
        assert!(ticket_detail(&context, "Missing", None).is_err());
    }
}
