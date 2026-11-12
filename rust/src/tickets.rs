use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::schema::{PRIORITY_DEFAULT, PRIORITY_MAX, PRIORITY_MIN};
use crate::store::{NewTicket, create_ticket, list_tickets, read_ticket, write_ticket};

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
