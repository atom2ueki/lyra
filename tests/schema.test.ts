import { describe, expect, test } from "bun:test";
import {
  normalizeTicket,
  readyTickets,
  validateGraph,
  type Ticket,
} from "../src/schema";

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

describe("validateGraph", () => {
  test("accepts a valid DAG", () => {
    const tickets = [
      ticket("T-001", { status: "done" }),
      ticket("T-002", { blockedBy: ["T-001"] }),
    ];
    expect(validateGraph(tickets)).toEqual([]);
  });

  test("rejects an unknown blocker", () => {
    expect(validateGraph([ticket("T-001", { blockedBy: ["T-999"] })])).toContain(
      "T-001.blockedBy: unknown ticket T-999",
    );
  });

  test("rejects a self-dependency", () => {
    expect(validateGraph([ticket("T-001", { blockedBy: ["T-001"] })])).toContain(
      "T-001.blockedBy: cannot contain itself",
    );
  });

  test("rejects a cycle", () => {
    const errors = validateGraph([
      ticket("T-001", { blockedBy: ["T-002"] }),
      ticket("T-002", { blockedBy: ["T-001"] }),
    ]);
    expect(errors.some((e) => e.startsWith("dependency cycle:"))).toBe(true);
  });

  test("rejects mixed project ids in one store", () => {
    const errors = validateGraph([
      ticket("T-001"),
      ticket("T-002", { projectId: "other-project" }),
    ]);
    expect(errors).toContain("mixed project ids in one store: other-project, proj-test");
  });

  test("rejects a done ticket blocked by unfinished work", () => {
    expect(
      validateGraph([
        ticket("T-001", { status: "done" }),
        ticket("T-002", { status: "done", blockedBy: ["T-001"] }),
      ]),
    ).toEqual([]);
    expect(
      validateGraph([
        ticket("T-001", { status: "todo" }),
        ticket("T-002", { status: "done", blockedBy: ["T-001"] }),
      ]),
    ).toContain("T-002: done but blocker T-001 is not done");
  });
});

describe("readyTickets", () => {
  test("returns todo tickets whose blockers are done, sorted by priority", () => {
    const tickets = [
      ticket("T-001", { status: "done" }),
      ticket("T-002", { blockedBy: ["T-001"], priority: 1 }),
      ticket("T-003", { priority: 0 }),
      ticket("T-004", { status: "in_progress" }),
    ];
    expect(readyTickets(tickets).map((t) => t.id)).toEqual(["T-003", "T-002"]);
  });

  test("does not ready a ticket with a claimed execution", () => {
    const tickets = [
      ticket("T-001", {
        execution: {
          runId: "run-1",
          claimId: "claim-1",
          claimedAt: created,
        },
      }),
    ];
    expect(readyTickets(tickets)).toEqual([]);
  });
});
