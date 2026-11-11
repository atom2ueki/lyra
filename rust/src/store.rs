use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::schema::{
    Ticket, TicketExecution, TicketStatus, TicketSuspension, now_iso, ready_tickets, ticket_map,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Message(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn tasks_dir(store: &Path) -> PathBuf {
    store.join("tasks")
}

pub fn events_path(store: &Path) -> PathBuf {
    store.join("events.jsonl")
}

pub fn ensure_store(store: &Path) -> Result<()> {
    fs::create_dir_all(tasks_dir(store))
        .with_context(|| format!("create {}", tasks_dir(store).display()))?;
    fs::create_dir_all(store.join("attachments"))
        .with_context(|| format!("create {}", store.join("attachments").display()))?;
    Ok(())
}

pub fn list_tickets(store: &Path) -> Result<Vec<Ticket>> {
    let dir = tasks_dir(store);
    let entries = fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?;
    let mut tickets = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let ticket: Ticket =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        tickets.push(ticket);
    }
    tickets.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tickets)
}

fn valid_ticket_id(id: &str) -> bool {
    !id.is_empty() && id != "." && id != ".." && !id.contains('/') && !id.contains('\\')
}

pub fn read_ticket(store: &Path, id: &str) -> Result<Option<Ticket>> {
    if !valid_ticket_id(id) {
        anyhow::bail!("invalid ticket id: {id}");
    }
    let path = tasks_dir(store).join(format!("{id}.json"));
    match fs::read_to_string(&path) {
        Ok(text) => {
            let ticket =
                serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
            Ok(Some(ticket))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub fn write_ticket(store: &Path, ticket: &Ticket) -> Result<PathBuf> {
    if !valid_ticket_id(&ticket.id) {
        anyhow::bail!("invalid ticket id: {}", ticket.id);
    }
    ensure_store(store)?;
    let path = tasks_dir(store).join(format!("{}.json", ticket.id));
    let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let text = serde_json::to_string_pretty(ticket)? + "\n";
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("write {}", temporary.display()))?;
    file.sync_all().ok();
    fs::rename(&temporary, &path)
        .with_context(|| format!("rename {} -> {}", temporary.display(), path.display()))?;
    Ok(path)
}

pub fn next_ticket_id(store: &Path) -> Result<String> {
    let tickets = list_tickets(store).unwrap_or_default();
    let mut max = 0u64;
    for ticket in tickets {
        if let Some(raw) = ticket.id.strip_prefix("T-")
            && let Ok(value) = raw.parse::<u64>()
        {
            max = max.max(value);
        }
    }
    Ok(format!("T-{:03}", max + 1))
}

pub struct NewTicket {
    pub project_id: String,
    pub subject: String,
    pub prompt: String,
    pub blocked_by: Vec<String>,
    pub priority: i32,
    pub labels: Vec<String>,
    pub model_class: Option<String>,
    pub acceptance: Option<String>,
    pub plan: Option<String>,
    pub parent: Option<String>,
    pub origin: Option<String>,
    pub harness: Option<String>,
}

pub fn create_ticket(store: &Path, input: NewTicket) -> Result<Ticket> {
    let id = next_ticket_id(store)?;
    let mut ticket = Ticket::new(id, input.project_id, input.subject, input.prompt);
    ticket.blocked_by = input.blocked_by;
    ticket.priority = input.priority;
    ticket.labels = input.labels;
    ticket.model_class = input.model_class;
    ticket.acceptance = input.acceptance;
    ticket.plan = input.plan;
    ticket.parent = input.parent;
    ticket.origin = input.origin;
    ticket.harness = input.harness;
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({"type": "ticket.created", "ticketId": ticket.id}),
    )?;
    Ok(ticket)
}

pub fn append_event(store: &Path, event: Value) -> Result<Value> {
    ensure_store(store)?;
    let path = events_path(store);
    let seq = next_seq(&path)?;
    let mut record = serde_json::Map::new();
    record.insert("seq".to_string(), json!(seq));
    record.insert("ts".to_string(), json!(now_iso()));
    if let Value::Object(map) = event {
        for (key, value) in map {
            record.insert(key, value);
        }
    }
    let record = Value::Object(record);
    let text = serde_json::to_string(&record)? + "\n";
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("append {}", path.display()))?;
    Ok(record)
}

fn next_seq(path: &Path) -> Result<u64> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(1),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    for line in text.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(seq) = value.get("seq").and_then(Value::as_u64)
        {
            return Ok(seq + 1);
        }
    }
    Ok(text.lines().filter(|line| !line.trim().is_empty()).count() as u64 + 1)
}

pub fn claim_ticket(
    store: &Path,
    id: &str,
    run_id: &str,
    claim_id: &str,
) -> Result<Option<Ticket>> {
    let Some(mut ticket) = read_ticket(store, id)? else {
        return Ok(None);
    };
    if ticket.status != TicketStatus::Todo || ticket.execution.is_some() {
        return Ok(None);
    }
    let all = list_tickets(store)?;
    let by_id = ticket_map(&all);
    if !ticket
        .blocked_by
        .iter()
        .all(|blocker| by_id.get(blocker.as_str()).map(|t| t.status) == Some(TicketStatus::Done))
    {
        return Ok(None);
    }
    let now = now_iso();
    ticket.status = TicketStatus::InProgress;
    ticket.attempts += 1;
    ticket.execution = Some(TicketExecution {
        run_id: run_id.to_string(),
        claim_id: claim_id.to_string(),
        claimed_at: now.clone(),
        phase: Some("starting".to_string()),
        detail: None,
        pending_blockers: Vec::new(),
        updated_at: None,
    });
    ticket.started_at = Some(now.clone());
    ticket.updated_at = now;
    ticket.last_error = None;
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({"type": "ticket.claimed", "ticketId": id, "runId": run_id, "claimId": claim_id}),
    )?;
    Ok(Some(ticket))
}

pub fn update_execution(
    store: &Path,
    id: &str,
    run_id: &str,
    claim_id: &str,
    phase: Option<String>,
    detail: Option<String>,
) -> Result<Option<Ticket>> {
    let Some(mut ticket) = read_ticket(store, id)? else {
        return Ok(None);
    };
    let Some(execution) = ticket.execution.as_mut() else {
        return Ok(None);
    };
    if execution.run_id != run_id || execution.claim_id != claim_id {
        return Ok(None);
    }
    execution.phase = phase;
    execution.detail = detail;
    execution.updated_at = Some(now_iso());
    ticket.updated_at = now_iso();
    write_ticket(store, &ticket)?;
    Ok(Some(ticket))
}

pub fn release_ticket(
    store: &Path,
    id: &str,
    run_id: &str,
    claim_id: &str,
    last_error: &str,
) -> Result<Option<Ticket>> {
    let Some(mut ticket) = read_ticket(store, id)? else {
        return Ok(None);
    };
    let Some(execution) = &ticket.execution else {
        return Ok(None);
    };
    if execution.run_id != run_id || execution.claim_id != claim_id {
        return Ok(None);
    }
    ticket.status = TicketStatus::Todo;
    ticket.execution = None;
    ticket.last_error = Some(last_error.to_string());
    ticket.updated_at = now_iso();
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({"type": "ticket.released", "ticketId": id, "runId": run_id, "claimId": claim_id}),
    )?;
    Ok(Some(ticket))
}

pub fn finish_ticket(
    store: &Path,
    id: &str,
    run_id: &str,
    claim_id: &str,
    status: TicketStatus,
    result: Value,
    last_error: Option<String>,
) -> Result<Option<Ticket>> {
    let Some(mut ticket) = read_ticket(store, id)? else {
        return Ok(None);
    };
    let Some(execution) = &ticket.execution else {
        return Ok(None);
    };
    if execution.run_id != run_id || execution.claim_id != claim_id {
        return Ok(None);
    }
    let now = now_iso();
    ticket.status = status;
    ticket.result = Some(result);
    ticket.last_error = last_error;
    ticket.execution = None;
    ticket.suspension = None;
    ticket.finished_at = Some(now.clone());
    ticket.updated_at = now;
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({"type": "ticket.finished", "ticketId": id, "status": status, "runId": run_id, "claimId": claim_id}),
    )?;
    Ok(Some(ticket))
}

pub struct SuspendArgs {
    pub blockers: Vec<String>,
    pub reason: String,
    pub needs_input: bool,
    pub worktree: String,
    pub branch: String,
    pub base_head: String,
}

pub fn suspend_ticket(
    store: &Path,
    id: &str,
    run_id: &str,
    claim_id: &str,
    args: SuspendArgs,
) -> Result<Option<Ticket>> {
    let Some(mut ticket) = read_ticket(store, id)? else {
        return Ok(None);
    };
    let Some(execution) = &ticket.execution else {
        return Ok(None);
    };
    if execution.run_id != run_id || execution.claim_id != claim_id {
        return Ok(None);
    }
    let all = list_tickets(store)?;
    let by_id = ticket_map(&all);
    for blocker in &args.blockers {
        if blocker == id {
            return Err(StoreError::Message(format!("{id}: a ticket cannot block itself")).into());
        }
        if !by_id.contains_key(blocker.as_str()) {
            return Err(StoreError::Message(format!("{id}: unknown blocker {blocker}")).into());
        }
    }
    let now = now_iso();
    for blocker in &args.blockers {
        if !ticket.blocked_by.contains(blocker) {
            ticket.blocked_by.push(blocker.clone());
        }
    }
    ticket.status = TicketStatus::Todo;
    ticket.execution = None;
    ticket.last_error = Some(if args.needs_input {
        "needs_input".to_string()
    } else {
        format!("blocked: {}", args.reason)
    });
    ticket.suspension = Some(TicketSuspension {
        run_id: run_id.to_string(),
        claim_id: claim_id.to_string(),
        reason: args.reason.clone(),
        needs_input: args.needs_input,
        blocked_by: args.blockers.clone(),
        worktree: args.worktree.clone(),
        branch: args.branch.clone(),
        base_head: args.base_head.clone(),
        checkpoint: crate::schema::TicketCheckpoint {
            head: args.base_head.clone(),
            worktree: args.worktree,
            branch: args.branch,
        },
        suspended_at: now.clone(),
    });
    ticket.updated_at = now;
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({
            "type": if args.needs_input { "ticket.needs_input" } else { "ticket.blocked" },
            "ticketId": id,
            "blockers": args.blockers,
            "runId": run_id,
            "claimId": claim_id
        }),
    )?;
    Ok(Some(ticket))
}

pub fn ready_snapshot(store: &Path) -> Result<Vec<Ticket>> {
    Ok(ready_tickets(&list_tickets(store)?)
        .into_iter()
        .cloned()
        .collect())
}
