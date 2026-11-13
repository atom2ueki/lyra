use serde::Serialize;
use serde_json::Value;

use crate::conductor::{RunState, RunTicketStatus};
use crate::schema::{Ticket, TicketStatus, ready_tickets, ticket_map};

pub const AGENT_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub schema_version: u8,
    pub project_id: String,
    pub graph: GraphSummary,
    pub run: Option<RunSummary>,
    pub next: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSummary {
    pub total: usize,
    pub todo: usize,
    pub in_progress: usize,
    pub done: usize,
    pub failed: usize,
    pub ready: Vec<TicketSummary>,
    pub blocked: Vec<BlockedTicket>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub id: String,
    pub subject: String,
    pub status: String,
    pub priority: i32,
    pub blocked_by: Vec<String>,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub attempts: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedTicket {
    pub id: String,
    pub subject: String,
    pub waiting_on: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run_id: String,
    pub status: String,
    pub backend: String,
    pub base_ref: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub planned_tickets: Vec<String>,
    pub tickets: Vec<RunTicketSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSummary {
    #[serde(flatten)]
    pub run: RunSummary,
    pub timed_out: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTicketSummary {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub needs_input: bool,
    pub blockers: Vec<String>,
}

pub fn status_for(ticket: &Ticket) -> &'static str {
    match ticket.status {
        TicketStatus::Todo => "todo",
        TicketStatus::InProgress => "in_progress",
        TicketStatus::Done => "done",
        TicketStatus::Failed => "failed",
    }
}

pub fn run_status(status: RunTicketStatus) -> &'static str {
    status.as_str()
}

impl TicketSummary {
    pub fn from_ticket(ticket: &Ticket, ready: bool) -> Self {
        Self {
            id: ticket.id.clone(),
            subject: ticket.subject.clone(),
            status: status_for(ticket).to_string(),
            priority: ticket.priority,
            blocked_by: ticket.blocked_by.clone(),
            ready,
            last_error: ticket.last_error.clone(),
            attempts: ticket.attempts,
        }
    }
}

impl GraphSummary {
    pub fn build(tickets: &[Ticket], errors: Vec<String>) -> Self {
        let by_id = ticket_map(tickets);
        let ready_ids: Vec<&str> = ready_tickets(tickets)
            .into_iter()
            .map(|ticket| ticket.id.as_str())
            .collect();
        let ready = tickets
            .iter()
            .filter(|ticket| ready_ids.contains(&ticket.id.as_str()))
            .map(|ticket| TicketSummary::from_ticket(ticket, true))
            .collect();
        let blocked = tickets
            .iter()
            .filter(|ticket| {
                ticket.status == TicketStatus::Todo
                    && ticket.blocked_by.iter().any(|id| {
                        by_id.get(id.as_str()).map(|blocker| blocker.status)
                            != Some(TicketStatus::Done)
                    })
            })
            .map(|ticket| BlockedTicket {
                id: ticket.id.clone(),
                subject: ticket.subject.clone(),
                waiting_on: ticket.blocked_by.clone(),
            })
            .collect();
        let mut summary = Self {
            total: tickets.len(),
            todo: 0,
            in_progress: 0,
            done: 0,
            failed: 0,
            ready,
            blocked,
            errors,
        };
        for ticket in tickets {
            match ticket.status {
                TicketStatus::Todo => summary.todo += 1,
                TicketStatus::InProgress => summary.in_progress += 1,
                TicketStatus::Done => summary.done += 1,
                TicketStatus::Failed => summary.failed += 1,
            }
        }
        summary
    }

    pub fn has_open_work(&self) -> bool {
        self.todo > 0 || self.in_progress > 0
    }
}

impl RunSummary {
    pub fn from_state(state: &RunState) -> Self {
        let mut tickets: Vec<RunTicketSummary> = state
            .tickets
            .iter()
            .map(|(id, ticket)| {
                let result = ticket.result.as_ref();
                let status = result
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str);
                RunTicketSummary {
                    id: id.clone(),
                    status: ticket.status.as_str().to_string(),
                    model: ticket.model.clone(),
                    phase: ticket.phase.clone(),
                    summary: result
                        .and_then(|value| value.get("summary"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    needs_input: status == Some("needs_input"),
                    blockers: result
                        .and_then(|value| value.get("blockers"))
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect();
        tickets.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            run_id: state.run_id.clone(),
            status: format!("{:?}", state.status).to_lowercase(),
            backend: state.backend.clone(),
            base_ref: state.base_ref.clone(),
            created_at: state.created_at.clone(),
            updated_at: state.updated_at.clone(),
            revision: state.revision,
            planned_tickets: state.planned_tickets.clone(),
            tickets,
        }
    }

    pub fn has_active(&self) -> bool {
        self.status == "running"
            || self
                .tickets
                .iter()
                .any(|ticket| ticket.status == "running" || ticket.status == "starting")
    }

    pub fn needs_input(&self) -> bool {
        self.tickets.iter().any(|ticket| ticket.needs_input)
    }
}

pub fn next_action(graph: &GraphSummary, run: Option<&RunSummary>) -> &'static str {
    if let Some(run) = run {
        if run.needs_input() {
            return "resolve_input";
        }
        if run.has_active() {
            return "wait";
        }
    }
    if !graph.ready.is_empty() {
        return "start";
    }
    if !graph.has_open_work() {
        return "done";
    }
    if !graph.blocked.is_empty() {
        return "blocked";
    }
    "wait"
}

pub fn build_status(
    project_id: &str,
    tickets: &[Ticket],
    errors: Vec<String>,
    run: Option<&RunState>,
) -> AgentStatus {
    let graph = GraphSummary::build(tickets, errors);
    let run_summary = run.map(RunSummary::from_state);
    let next = next_action(&graph, run_summary.as_ref()).to_string();
    AgentStatus {
        schema_version: AGENT_SCHEMA_VERSION,
        project_id: project_id.to_string(),
        graph,
        run: run_summary,
        next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(ready: bool, blocked: bool, open: bool) -> GraphSummary {
        GraphSummary {
            total: usize::from(open || ready || blocked),
            todo: usize::from(open || ready || blocked),
            in_progress: 0,
            done: usize::from(!open && !ready && !blocked),
            failed: 0,
            ready: if ready {
                vec![TicketSummary {
                    id: "T-001".to_string(),
                    subject: "Ready".to_string(),
                    status: "todo".to_string(),
                    priority: 2,
                    blocked_by: Vec::new(),
                    ready: true,
                    last_error: None,
                    attempts: 0,
                }]
            } else {
                Vec::new()
            },
            blocked: if blocked {
                vec![BlockedTicket {
                    id: "T-002".to_string(),
                    subject: "Blocked".to_string(),
                    waiting_on: vec!["T-001".to_string()],
                }]
            } else {
                Vec::new()
            },
            errors: Vec::new(),
        }
    }

    fn run(status: &str, needs_input: bool) -> RunSummary {
        RunSummary {
            run_id: "run".to_string(),
            status: status.to_string(),
            backend: "headless".to_string(),
            base_ref: "main".to_string(),
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
            revision: 0,
            planned_tickets: Vec::new(),
            tickets: vec![RunTicketSummary {
                id: "T-001".to_string(),
                status: status.to_string(),
                model: None,
                phase: None,
                summary: None,
                needs_input,
                blockers: Vec::new(),
            }],
        }
    }

    #[test]
    fn next_action_prioritizes_input_then_activity() {
        assert_eq!(
            next_action(&graph(false, false, true), Some(&run("running", false))),
            "wait"
        );
        assert_eq!(
            next_action(&graph(false, false, true), Some(&run("running", true))),
            "resolve_input"
        );
        assert_eq!(next_action(&graph(true, false, true), None), "start");
        assert_eq!(next_action(&graph(false, true, true), None), "blocked");
        assert_eq!(next_action(&graph(false, false, false), None), "done");
    }
}
