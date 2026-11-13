use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::schema::{
    PRIORITY_DEFAULT, PRIORITY_MAX, PRIORITY_MIN, Ticket, TicketStatus, now_iso, validate_graph,
};
use crate::store::{
    NewTicket, append_event, create_ticket, list_tickets, read_ticket, write_ticket,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchInput {
    pub tickets: Vec<BatchTicket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchTicket {
    pub key: String,
    pub subject: String,
    pub prompt: String,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub model_class: Option<String>,
    #[serde(default)]
    pub acceptance: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub harness: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTicket {
    pub key: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOutput {
    pub created: Vec<CreatedTicket>,
}

fn default_priority() -> i32 {
    PRIORITY_DEFAULT
}

fn validate_dependency_graph(tickets: &[BatchTicket]) -> Result<()> {
    let keys: HashSet<&str> = tickets.iter().map(|ticket| ticket.key.as_str()).collect();
    if keys.len() != tickets.len() {
        let mut seen = HashSet::new();
        for ticket in tickets {
            if !seen.insert(ticket.key.as_str()) {
                bail!("duplicate ticket key: {}", ticket.key);
            }
        }
    }
    let graph: HashMap<&str, Vec<&str>> = tickets
        .iter()
        .map(|ticket| {
            (
                ticket.key.as_str(),
                ticket.blocked_by.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    fn visit<'a>(
        node: &'a str,
        graph: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> Result<()> {
        if visited.contains(node) {
            return Ok(());
        }
        if !visiting.insert(node) {
            bail!("ticket dependency cycle includes {node}");
        }
        if let Some(dependencies) = graph.get(node) {
            for dependency in dependencies {
                if graph.contains_key(dependency) {
                    visit(dependency, graph, visiting, visited)?;
                }
            }
        }
        visiting.remove(node);
        visited.insert(node);
        Ok(())
    }
    for ticket in tickets {
        visit(ticket.key.as_str(), &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

pub fn create_batch(
    store: &std::path::Path,
    project_id: &str,
    input: BatchInput,
) -> Result<CreateOutput> {
    if input.tickets.is_empty() {
        bail!("tickets must not be empty");
    }
    let existing = list_tickets(store).unwrap_or_default();
    let existing_ids: HashSet<String> = existing.iter().map(|ticket| ticket.id.clone()).collect();
    let keys: HashSet<&str> = input
        .tickets
        .iter()
        .map(|ticket| ticket.key.as_str())
        .collect();
    for ticket in &input.tickets {
        if ticket.key.trim().is_empty() {
            bail!("ticket key must not be empty");
        }
        if ticket.subject.trim().is_empty() {
            bail!("ticket {} has an empty subject", ticket.key);
        }
        if ticket.prompt.trim().is_empty() {
            bail!("ticket {} has an empty prompt", ticket.key);
        }
        if existing_ids.contains(&ticket.key) {
            bail!(
                "ticket key {} conflicts with an existing ticket id",
                ticket.key
            );
        }
        if !(PRIORITY_MIN..=PRIORITY_MAX).contains(&ticket.priority) {
            bail!(
                "ticket {} priority must be between {} and {}",
                ticket.key,
                PRIORITY_MIN,
                PRIORITY_MAX
            );
        }
        for dependency in &ticket.blocked_by {
            if dependency == &ticket.key {
                bail!("ticket {} cannot block itself", ticket.key);
            }
            if !keys.contains(dependency.as_str()) && !existing_ids.contains(dependency) {
                bail!(
                    "ticket {} has unknown dependency {}; use a batch key or an existing ticket id",
                    ticket.key,
                    dependency
                );
            }
        }
        if let Some(parent) = &ticket.parent
            && !keys.contains(parent.as_str())
            && !existing_ids.contains(parent)
        {
            bail!("ticket {} has unknown parent {}", ticket.key, parent);
        }
    }
    validate_dependency_graph(&input.tickets)?;

    let mut mapping: HashMap<String, String> = HashMap::new();
    let mut created = Vec::with_capacity(input.tickets.len());
    for ticket in &input.tickets {
        let new_ticket = match create_ticket(
            store,
            NewTicket {
                project_id: project_id.to_string(),
                subject: ticket.subject.clone(),
                prompt: ticket.prompt.clone(),
                blocked_by: Vec::new(),
                priority: ticket.priority,
                labels: ticket.labels.clone(),
                model_class: ticket.model_class.clone(),
                acceptance: ticket.acceptance.clone(),
                plan: ticket.plan.clone(),
                parent: None,
                origin: Some("agent".to_string()),
                harness: ticket.harness.clone(),
            },
        ) {
            Ok(ticket) => ticket,
            Err(error) => {
                let written = created
                    .iter()
                    .map(|ticket: &CreatedTicket| format!("{}={}", ticket.key, ticket.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(error).with_context(|| {
                    format!(
                        "create ticket {}; already created: {}",
                        ticket.key,
                        if written.is_empty() {
                            "(none)"
                        } else {
                            &written
                        }
                    )
                });
            }
        };
        mapping.insert(ticket.key.clone(), new_ticket.id.clone());
        created.push(CreatedTicket {
            key: ticket.key.clone(),
            id: new_ticket.id,
        });
    }

    for ticket in &input.tickets {
        let id = mapping
            .get(&ticket.key)
            .ok_or_else(|| anyhow::anyhow!("missing created id for {}", ticket.key))?;
        let mut stored = read_ticket(store, id)?
            .ok_or_else(|| anyhow::anyhow!("created ticket {} disappeared", id))?;
        stored.blocked_by = ticket
            .blocked_by
            .iter()
            .map(|dependency| {
                mapping
                    .get(dependency)
                    .cloned()
                    .unwrap_or_else(|| dependency.clone())
            })
            .collect();
        if let Some(parent) = &ticket.parent {
            stored.parent = Some(
                mapping
                    .get(parent)
                    .cloned()
                    .unwrap_or_else(|| parent.clone()),
            );
        }
        if let Err(error) = write_ticket(store, &stored) {
            let written = created
                .iter()
                .map(|ticket| format!("{}={}", ticket.key, ticket.id))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(error).with_context(|| {
                format!(
                    "write dependencies for {}; created tickets: {}",
                    ticket.key, written
                )
            });
        }
    }

    Ok(CreateOutput { created })
}

/// Field changes for `lyra tickets update`. `None`/`false`/empty means "leave unchanged";
/// the clear flags empty the corresponding field. Only `todo` and `failed` tickets accept updates.
#[derive(Debug, Default)]
pub struct TicketUpdate {
    pub subject: Option<String>,
    pub prompt: Option<String>,
    pub acceptance: Option<String>,
    pub clear_acceptance: bool,
    pub priority: Option<i32>,
    pub blocked_by: Vec<String>,
    pub clear_blocked_by: bool,
    pub labels: Vec<String>,
    pub clear_labels: bool,
}

impl TicketUpdate {
    pub fn is_empty(&self) -> bool {
        self.subject.is_none()
            && self.prompt.is_none()
            && self.acceptance.is_none()
            && !self.clear_acceptance
            && self.priority.is_none()
            && self.blocked_by.is_empty()
            && !self.clear_blocked_by
            && self.labels.is_empty()
            && !self.clear_labels
    }
}

fn change(before: &Value, after: &Value) -> Value {
    json!({"from": before, "to": after})
}

/// Mutate one editable ticket. The proposed store is re-validated as a whole graph; on any
/// rejection no file is written.
pub fn update_ticket(store: &Path, id: &str, update: TicketUpdate) -> Result<Ticket> {
    let mut ticket =
        read_ticket(store, id)?.ok_or_else(|| anyhow::anyhow!("unknown ticket: {id}"))?;
    match ticket.status {
        TicketStatus::Todo | TicketStatus::Failed => {}
        TicketStatus::InProgress => {
            bail!("{id} is in_progress and cannot be updated while a worker holds it")
        }
        TicketStatus::Done => bail!("{id} is done and cannot be updated; history is immutable"),
    }
    if ticket.suspension.is_some() {
        bail!("{id} is suspended and cannot be updated; resume it first");
    }
    if update.is_empty() {
        bail!("update for {id} must change at least one field");
    }
    if update.acceptance.is_some() && update.clear_acceptance {
        bail!("acceptance cannot be both set and cleared");
    }
    if !update.blocked_by.is_empty() && update.clear_blocked_by {
        bail!("blocked-by cannot be both set and cleared");
    }
    if !update.labels.is_empty() && update.clear_labels {
        bail!("labels cannot be both set and cleared");
    }

    let before = ticket.clone();
    let mut changes = serde_json::Map::new();
    if let Some(subject) = update.subject {
        ticket.subject = subject;
    }
    if let Some(prompt) = update.prompt {
        ticket.prompt = prompt;
    }
    if let Some(acceptance) = update.acceptance {
        ticket.acceptance = Some(acceptance);
    } else if update.clear_acceptance {
        ticket.acceptance = None;
    }
    if let Some(priority) = update.priority {
        ticket.priority = priority;
    }
    if !update.blocked_by.is_empty() {
        ticket.blocked_by = update.blocked_by.clone();
    } else if update.clear_blocked_by {
        ticket.blocked_by = Vec::new();
    }
    if !update.labels.is_empty() {
        ticket.labels = update.labels.clone();
    } else if update.clear_labels {
        ticket.labels = Vec::new();
    }

    if before.subject != ticket.subject {
        changes.insert(
            "subject".into(),
            change(&json!(before.subject), &json!(ticket.subject)),
        );
    }
    if before.prompt != ticket.prompt {
        changes.insert(
            "prompt".into(),
            change(&json!(before.prompt), &json!(ticket.prompt)),
        );
    }
    if before.acceptance != ticket.acceptance {
        changes.insert(
            "acceptance".into(),
            change(
                &before
                    .acceptance
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
                &ticket
                    .acceptance
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
        );
    }
    if before.priority != ticket.priority {
        changes.insert(
            "priority".into(),
            change(&json!(before.priority), &json!(ticket.priority)),
        );
    }
    if before.blocked_by != ticket.blocked_by {
        changes.insert(
            "blockedBy".into(),
            change(&json!(before.blocked_by), &json!(ticket.blocked_by)),
        );
    }
    if before.labels != ticket.labels {
        changes.insert(
            "labels".into(),
            change(&json!(before.labels), &json!(ticket.labels)),
        );
    }

    let proposed: Vec<Ticket> = list_tickets(store)?
        .into_iter()
        .map(|stored| {
            if stored.id == ticket.id {
                ticket.clone()
            } else {
                stored
            }
        })
        .collect();
    let errors = validate_graph(&proposed);
    if !errors.is_empty() {
        bail!("update rejected: {}", errors.join("; "));
    }

    ticket.updated_at = now_iso();
    write_ticket(store, &ticket)?;
    append_event(
        store,
        json!({"type": "ticket.updated", "ticketId": id, "changes": changes}),
    )?;
    Ok(ticket)
}
