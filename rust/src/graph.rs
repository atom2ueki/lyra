use crate::schema::Ticket;
use serde_json::{Value, json};

fn node_id(id: &str) -> String {
    let mut out = String::from("n_");
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn label(ticket: &Ticket) -> String {
    let subject = ticket
        .subject
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!("{} · {}", ticket.id, subject).replace('"', "'")
}

pub fn to_mermaid(tickets: &[Ticket]) -> String {
    let mut lines = vec!["flowchart TD".to_string()];
    for ticket in tickets {
        lines.push(format!("  {}[\"{}\"]", node_id(&ticket.id), label(ticket)));
    }
    for ticket in tickets {
        for blocker in &ticket.blocked_by {
            lines.push(format!(
                "  {} --> {}",
                node_id(blocker),
                node_id(&ticket.id)
            ));
        }
    }
    lines.join("\n") + "\n"
}

pub fn to_dot(tickets: &[Ticket]) -> String {
    let mut lines = vec!["digraph lyra {".to_string(), "  rankdir=LR;".to_string()];
    for ticket in tickets {
        lines.push(format!(
            "  {} [label=\"{}\"];",
            node_id(&ticket.id),
            label(ticket)
        ));
    }
    for ticket in tickets {
        for blocker in &ticket.blocked_by {
            lines.push(format!(
                "  {} -> {};",
                node_id(blocker),
                node_id(&ticket.id)
            ));
        }
    }
    lines.push("}".to_string());
    lines.join("\n") + "\n"
}

pub fn to_json(tickets: &[Ticket]) -> Value {
    let nodes: Vec<Value> = tickets
        .iter()
        .map(|ticket| {
            json!({
                "id": ticket.id,
                "subject": ticket.subject,
                "status": ticket.status,
                "priority": ticket.priority,
                "blockedBy": ticket.blocked_by,
            })
        })
        .collect();
    let edges: Vec<Value> = tickets
        .iter()
        .flat_map(|ticket| {
            ticket
                .blocked_by
                .iter()
                .map(move |blocker| json!({"from": blocker, "to": ticket.id}))
        })
        .collect();
    json!({"nodes": nodes, "edges": edges})
}
