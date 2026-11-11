import { describe, expect, test } from "bun:test";
import { toDot, toJson, toMermaid } from "../src/graph";
import { normalizeTicket, type Ticket } from "../src/schema";

const created = "2026-01-01T00:00:00.000Z";

function ticket(id: string, overrides: Partial<Ticket> = {}): Ticket {
  return normalizeTicket({
    schemaVersion: 1,
    id,
    projectId: "proj-test",
    subject: `Subject ${id}`,
    prompt: `Do ${id}`,
    status: "todo",
    blockedBy: [],
    priority: 2,
    labels: [],
    attempts: 0,
    lastError: null,
    result: null,
    execution: null,
    discoveries: [],
    suspension: null,
    checkpoint: null,
    createdAt: created,
    updatedAt: created,
    startedAt: null,
    finishedAt: null,
    ...overrides,
  });
}

describe("graph projection", () => {
  const tickets = [ticket("T-001"), ticket("T-002", { blockedBy: ["T-001"] })];

  test("mermaid contains nodes and dependency edge", () => {
    const out = toMermaid(tickets);
    expect(out).toContain("flowchart TD");
    expect(out).toContain('n_T_001["T-001 · Subject T-001"]');
    expect(out).toContain("n_T_001 --> n_T_002");
  });

  test("dot contains the edge", () => {
    const out = toDot(tickets);
    expect(out).toContain("digraph lyra");
    expect(out).toContain("n_T_001 -> n_T_002");
  });

  test("json exposes nodes and edges", () => {
    const out = toJson(tickets);
    expect(out.nodes.map((n) => n.id)).toEqual(["T-001", "T-002"]);
    expect(out.edges).toEqual([{ from: "T-001", to: "T-002" }]);
  });
});
