import type { Ticket } from "./schema";

function nodeId(id: string): string {
  return `n_${id.replace(/[^A-Za-z0-9_]/g, "_")}`;
}

function label(ticket: Ticket): string {
  const subject = ticket.subject.replace(/\s+/g, " ").trim();
  return `${ticket.id} · ${subject}`.replace(/"/g, "'");
}

export function toMermaid(tickets: Ticket[]): string {
  const lines = ["flowchart TD"];
  for (const ticket of tickets) {
    lines.push(`  ${nodeId(ticket.id)}["${label(ticket)}"]`);
  }
  for (const ticket of tickets) {
    for (const blocker of ticket.blockedBy) {
      lines.push(`  ${nodeId(blocker)} --> ${nodeId(ticket.id)}`);
    }
  }
  return lines.join("\n") + "\n";
}

export function toDot(tickets: Ticket[]): string {
  const lines = ["digraph lyra {", "  rankdir=LR;"];
  for (const ticket of tickets) {
    lines.push(`  ${nodeId(ticket.id)} [label="${label(ticket)}"];`);
  }
  for (const ticket of tickets) {
    for (const blocker of ticket.blockedBy) {
      lines.push(`  ${nodeId(blocker)} -> ${nodeId(ticket.id)};`);
    }
  }
  lines.push("}");
  return lines.join("\n") + "\n";
}

export interface GraphJson {
  nodes: Array<{
    id: string;
    subject: string;
    status: string;
    priority: number;
    blockedBy: string[];
  }>;
  edges: Array<{ from: string; to: string }>;
}

export function toJson(tickets: Ticket[]): GraphJson {
  return {
    nodes: tickets.map((t) => ({
      id: t.id,
      subject: t.subject,
      status: t.status,
      priority: t.priority,
      blockedBy: t.blockedBy,
    })),
    edges: tickets.flatMap((t) => t.blockedBy.map((from) => ({ from, to: t.id }))),
  };
}
