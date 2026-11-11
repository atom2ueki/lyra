use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const SCHEMA_VERSION: u8 = 1;
pub const PRIORITY_MIN: i32 = 0;
pub const PRIORITY_MAX: i32 = 4;
pub const PRIORITY_DEFAULT: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Todo,
    InProgress,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketExecution {
    pub run_id: String,
    pub claim_id: String,
    pub claimed_at: String,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub pending_blockers: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketDiscovery {
    pub kind: String,
    pub prompt: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketAttachment {
    #[serde(default)]
    pub id: Option<String>,
    pub filename: String,
    pub stored_as: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketSuspension {
    pub run_id: String,
    pub claim_id: String,
    pub reason: String,
    #[serde(default)]
    pub needs_input: bool,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    pub worktree: String,
    pub branch: String,
    pub base_head: String,
    pub checkpoint: TicketCheckpoint,
    pub suspended_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TicketCheckpoint {
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub worktree: String,
    #[serde(default)]
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    pub id: String,
    pub project_id: String,
    pub subject: String,
    pub prompt: String,
    pub status: TicketStatus,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<TicketAttachment>>,
    #[serde(default)]
    pub attempts: u64,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub execution: Option<TicketExecution>,
    #[serde(default)]
    pub discoveries: Vec<TicketDiscovery>,
    #[serde(default)]
    pub suspension: Option<TicketSuspension>,
    #[serde(default)]
    pub checkpoint: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

fn default_schema_version() -> u8 {
    SCHEMA_VERSION
}

fn default_priority() -> i32 {
    PRIORITY_DEFAULT
}

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl Ticket {
    pub fn new(id: String, project_id: String, subject: String, prompt: String) -> Self {
        let now = now_iso();
        Self {
            schema_version: SCHEMA_VERSION,
            id,
            project_id,
            subject,
            prompt,
            status: TicketStatus::Todo,
            blocked_by: Vec::new(),
            priority: PRIORITY_DEFAULT,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            branch: None,
            budget_usd: None,
            source: None,
            external_id: None,
            external_url: None,
            parent: None,
            origin: None,
            requires: None,
            actor: None,
            harness: None,
            model: None,
            attachments: None,
            attempts: 0,
            last_error: None,
            result: None,
            execution: None,
            discoveries: Vec::new(),
            suspension: None,
            checkpoint: None,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            finished_at: None,
            extra: None,
        }
    }
}

fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_iso_timestamp(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
}

pub fn validate_ticket(ticket: &Ticket) -> Vec<String> {
    let mut errors = Vec::new();
    if ticket.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "{}.schemaVersion: expected {}",
            ticket.id, SCHEMA_VERSION
        ));
    }
    if !is_valid_id(&ticket.id) {
        errors.push(format!(
            "{}.id: must match [A-Za-z][A-Za-z0-9_-]*",
            ticket.id
        ));
    }
    if ticket.project_id.is_empty() {
        errors.push(format!("{}.projectId: required", ticket.id));
    }
    if ticket.subject.trim().is_empty() {
        errors.push(format!("{}.subject: required", ticket.id));
    }
    if ticket.prompt.is_empty() {
        errors.push(format!("{}.prompt: required", ticket.id));
    }
    let mut blocked = HashSet::new();
    for blocker in &ticket.blocked_by {
        if blocker == &ticket.id {
            errors.push(format!("{}.blockedBy: cannot contain itself", ticket.id));
        }
        if !blocked.insert(blocker) {
            errors.push(format!("{}.blockedBy: duplicate ids", ticket.id));
        }
    }
    if !(PRIORITY_MIN..=PRIORITY_MAX).contains(&ticket.priority) {
        errors.push(format!(
            "{}.priority: integer {}..{}",
            ticket.id, PRIORITY_MIN, PRIORITY_MAX
        ));
    }
    if !is_iso_timestamp(&ticket.created_at) {
        errors.push(format!("{}.createdAt: ISO timestamp required", ticket.id));
    }
    if !is_iso_timestamp(&ticket.updated_at) {
        errors.push(format!("{}.updatedAt: ISO timestamp required", ticket.id));
    }
    if let Some(execution) = &ticket.execution {
        if execution.run_id.is_empty() {
            errors.push(format!("{}.execution.runId: required", ticket.id));
        }
        if execution.claim_id.is_empty() {
            errors.push(format!("{}.execution.claimId: required", ticket.id));
        }
    }
    errors
}

pub fn validate_graph(tickets: &[Ticket]) -> Vec<String> {
    let mut errors = Vec::new();
    for ticket in tickets {
        errors.extend(validate_ticket(ticket));
    }

    let project_ids: HashSet<&str> = tickets.iter().map(|t| t.project_id.as_str()).collect();
    if project_ids.len() > 1 {
        let mut ids: Vec<&str> = project_ids.into_iter().collect();
        ids.sort_unstable();
        errors.push(format!("mixed project ids in one store: {}", ids.join(", ")));
    }

    let by_id: HashMap<&str, &Ticket> = tickets.iter().map(|t| (t.id.as_str(), t)).collect();
    for ticket in tickets {
        for blocker in &ticket.blocked_by {
            if !by_id.contains_key(blocker.as_str()) {
                errors.push(format!("{}.blockedBy: unknown ticket {}", ticket.id, blocker));
            }
        }
    }

    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack: Vec<&str> = Vec::new();
    for ticket in tickets {
        visit(
            ticket.id.as_str(),
            &by_id,
            &mut visiting,
            &mut visited,
            &mut stack,
            &mut errors,
        );
    }

    for ticket in tickets {
        if ticket.status != TicketStatus::Done {
            continue;
        }
        for blocker in &ticket.blocked_by {
            if by_id.get(blocker.as_str()).map(|t| t.status) != Some(TicketStatus::Done) {
                errors.push(format!(
                    "{}: done but blocker {} is not done",
                    ticket.id, blocker
                ));
            }
        }
    }
    errors
}

fn visit<'a>(
    id: &'a str,
    by_id: &HashMap<&'a str, &'a Ticket>,
    visiting: &mut HashSet<&'a str>,
    visited: &mut HashSet<&'a str>,
    stack: &mut Vec<&'a str>,
    errors: &mut Vec<String>,
) {
    if visited.contains(id) {
        return;
    }
    if !visiting.insert(id) {
        let start = stack.iter().position(|entry| *entry == id).unwrap_or(0);
        let mut cycle: Vec<&str> = stack[start..].to_vec();
        cycle.push(id);
        errors.push(format!("dependency cycle: {}", cycle.join(" -> ")));
        return;
    }
    stack.push(id);
    if let Some(ticket) = by_id.get(id) {
        for blocker in &ticket.blocked_by {
            if by_id.contains_key(blocker.as_str()) {
                visit(
                    blocker.as_str(),
                    by_id,
                    visiting,
                    visited,
                    stack,
                    errors,
                );
            }
        }
    }
    stack.pop();
    visiting.remove(id);
    visited.insert(id);
}

pub fn ticket_map(tickets: &[Ticket]) -> HashMap<&str, &Ticket> {
    tickets.iter().map(|t| (t.id.as_str(), t)).collect()
}

pub fn ready_tickets(tickets: &[Ticket]) -> Vec<&Ticket> {
    let by_id = ticket_map(tickets);
    let mut ready: Vec<&Ticket> = tickets
        .iter()
        .filter(|t| {
            t.status == TicketStatus::Todo
                && t.execution.is_none()
                && t.blocked_by
                    .iter()
                    .all(|id| by_id.get(id.as_str()).map(|b| b.status) == Some(TicketStatus::Done))
        })
        .collect();
    ready.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    ready
}
