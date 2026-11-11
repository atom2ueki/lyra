import { randomUUID } from "node:crypto";
import { mkdir, open, readFile, readdir, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  normalizeTicket,
  nowIso,
  type Ticket,
  type TicketStatus,
} from "./schema";
import { isRecord } from "./type-guards";

export class StoreError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "StoreError";
  }
}

export function tasksDir(store: string): string {
  return join(store, "tasks");
}

export function eventsPath(store: string): string {
  return join(store, "events.jsonl");
}

export async function ensureStore(store: string): Promise<void> {
  await mkdir(tasksDir(store), { recursive: true });
  await mkdir(join(store, "attachments"), { recursive: true });
}

export async function listTickets(store: string): Promise<Ticket[]> {
  let names: string[];
  try {
    names = await readdir(tasksDir(store));
  } catch (error) {
    throw new StoreError(`cannot read ticket store ${store}: ${(error as Error).message}`);
  }
  const tickets: Ticket[] = [];
  for (const name of names.filter((n) => n.endsWith(".json")).sort()) {
    const path = join(tasksDir(store), name);
    let raw: unknown;
    try {
      raw = JSON.parse(await readFile(path, "utf8"));
    } catch (error) {
      throw new StoreError(`${path}: ${(error as Error).message}`);
    }
    tickets.push(normalizeTicket(raw));
  }
  return tickets;
}

export async function readTicket(store: string, id: string): Promise<Ticket | null> {
  const path = join(tasksDir(store), `${id}.json`);
  try {
    return normalizeTicket(JSON.parse(await readFile(path, "utf8")));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw new StoreError(`${path}: ${(error as Error).message}`);
  }
}

export async function nextTicketId(store: string): Promise<string> {
  const tickets = await listTickets(store).catch(() => []);
  let max = 0;
  for (const ticket of tickets) {
    const match = /^T-(\d+)$/.exec(ticket.id);
    if (match) max = Math.max(max, Number(match[1]));
  }
  return `T-${String(max + 1).padStart(3, "0")}`;
}

export async function createTicket(
  store: string,
  input: {
    projectId: string;
    subject: string;
    prompt: string;
    blockedBy?: string[];
    priority?: number;
    labels?: string[];
    modelClass?: string;
    acceptance?: string;
    plan?: string;
    parent?: string;
    origin?: string;
    harness?: string;
  },
): Promise<Ticket> {
  const now = nowIso();
  const ticket = normalizeTicket(
    {
      schemaVersion: 1,
      id: await nextTicketId(store),
      projectId: input.projectId,
      subject: input.subject,
      prompt: input.prompt,
      status: "todo",
      blockedBy: input.blockedBy ?? [],
      priority: input.priority ?? 2,
      labels: input.labels ?? [],
      modelClass: input.modelClass,
      acceptance: input.acceptance,
      plan: input.plan,
      parent: input.parent,
      origin: input.origin,
      harness: input.harness,
      attempts: 0,
      lastError: null,
      result: null,
      execution: null,
      discoveries: [],
      suspension: null,
      checkpoint: null,
      createdAt: now,
      updatedAt: now,
      startedAt: null,
      finishedAt: null,
    },
    { now },
  );
  await writeTicket(store, ticket);
  await appendEvent(store, { type: "ticket.created", ticketId: ticket.id });
  return ticket;
}

export async function claimTicket(
  store: string,
  id: string,
  runId: string,
  claimId: string,
): Promise<Ticket | null> {
  const ticket = await readTicket(store, id);
  if (!ticket || ticket.status !== "todo" || ticket.execution) return null;
  const all = await listTickets(store);
  const byId = new Map(all.map((t) => [t.id, t]));
  if (!ticket.blockedBy.every((blocker) => byId.get(blocker)?.status === "done")) return null;
  const now = nowIso();
  const claimed: Ticket = {
    ...ticket,
    status: "in_progress",
    attempts: ticket.attempts + 1,
    execution: { runId, claimId, claimedAt: now, phase: "starting", detail: null },
    startedAt: now,
    updatedAt: now,
    lastError: null,
  };
  await writeTicket(store, claimed);
  await appendEvent(store, { type: "ticket.claimed", ticketId: id, runId, claimId });
  return claimed;
}

export async function updateExecution(
  store: string,
  id: string,
  owner: { runId: string; claimId: string },
  patch: { phase?: string | null; detail?: string | null },
): Promise<Ticket | null> {
  const ticket = await readTicket(store, id);
  if (!ticket?.execution) return null;
  if (ticket.execution.runId !== owner.runId || ticket.execution.claimId !== owner.claimId) {
    return null;
  }
  const updated: Ticket = {
    ...ticket,
    execution: { ...ticket.execution, ...patch, updatedAt: nowIso() },
    updatedAt: nowIso(),
  };
  await writeTicket(store, updated);
  return updated;
}

export async function suspendTicket(
  store: string,
  id: string,
  owner: { runId: string; claimId: string },
  args: {
    blockers: string[];
    reason: string;
    needsInput: boolean;
    worktree: string;
    branch: string;
    baseHead: string;
  },
): Promise<Ticket | null> {
  const ticket = await readTicket(store, id);
  if (!ticket?.execution) return null;
  if (ticket.execution.runId !== owner.runId || ticket.execution.claimId !== owner.claimId) {
    return null;
  }
  const all = await listTickets(store);
  const byId = new Map(all.map((t) => [t.id, t]));
  for (const blocker of args.blockers) {
    if (blocker === id) throw new StoreError(`${id}: a ticket cannot block itself`);
    if (!byId.has(blocker)) throw new StoreError(`${id}: unknown blocker ${blocker}`);
  }
  const blockedBy = [...new Set([...ticket.blockedBy, ...args.blockers])];
  const now = nowIso();
  const suspended: Ticket = {
    ...ticket,
    status: "todo",
    blockedBy,
    execution: null,
    lastError: args.needsInput ? "needs_input" : `blocked: ${args.reason}`,
    suspension: {
      runId: owner.runId,
      claimId: owner.claimId,
      reason: args.reason,
      needsInput: args.needsInput,
      blockedBy: args.blockers,
      worktree: args.worktree,
      branch: args.branch,
      baseHead: args.baseHead,
      checkpoint: {
        head: args.baseHead,
        worktree: args.worktree,
        branch: args.branch,
      },
      suspendedAt: now,
    },
    updatedAt: now,
  };
  await writeTicket(store, suspended);
  await appendEvent(store, {
    type: args.needsInput ? "ticket.needs_input" : "ticket.blocked",
    ticketId: id,
    blockers: args.blockers,
    runId: owner.runId,
    claimId: owner.claimId,
  });
  return suspended;
}

export async function releaseTicket(
  store: string,
  id: string,
  owner: { runId: string; claimId: string },
  lastError: string,
): Promise<Ticket | null> {
  const ticket = await readTicket(store, id);
  if (!ticket?.execution) return null;
  if (ticket.execution.runId !== owner.runId || ticket.execution.claimId !== owner.claimId) {
    return null;
  }
  const released: Ticket = {
    ...ticket,
    status: "todo",
    execution: null,
    lastError,
    updatedAt: nowIso(),
  };
  await writeTicket(store, released);
  await appendEvent(store, { type: "ticket.released", ticketId: id, runId: owner.runId, claimId: owner.claimId });
  return released;
}

export async function finishTicket(
  store: string,
  id: string,
  owner: { runId: string; claimId: string },
  outcome: { status: Exclude<TicketStatus, "todo" | "in_progress">; result: unknown; lastError?: string | null },
): Promise<Ticket | null> {
  const ticket = await readTicket(store, id);
  if (!ticket?.execution) return null;
  if (ticket.execution.runId !== owner.runId || ticket.execution.claimId !== owner.claimId) {
    return null;
  }
  const now = nowIso();
  const finished: Ticket = {
    ...ticket,
    status: outcome.status,
    result: outcome.result,
    lastError: outcome.lastError ?? null,
    execution: null,
    suspension: null,
    finishedAt: now,
    updatedAt: now,
  };
  await writeTicket(store, finished);
  await appendEvent(store, {
    type: "ticket.finished",
    ticketId: id,
    status: outcome.status,
    runId: owner.runId,
    claimId: owner.claimId,
  });
  return finished;
}

export async function writeTicket(store: string, ticket: Ticket): Promise<string> {
  await ensureStore(store);
  const path = join(tasksDir(store), `${ticket.id}.json`);
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  await writeFile(temporary, JSON.stringify(ticket, null, 2) + "\n", { mode: 0o600 });
  await rename(temporary, path);
  return path;
}

export async function appendEvent(
  store: string,
  event: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  await ensureStore(store);
  const path = eventsPath(store);
  const seq = await nextSeq(path);
  const record = { seq, ts: new Date().toISOString(), ...event };
  const handle = await open(path, "a", 0o600);
  try {
    await handle.write(JSON.stringify(record) + "\n");
  } finally {
    await handle.close();
  }
  return record;
}

async function nextSeq(path: string): Promise<number> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch {
    return 1;
  }
  const lines = text.trim().split("\n").filter(Boolean);
  for (let i = lines.length - 1; i >= 0; i--) {
    try {
      const parsed = JSON.parse(lines[i]);
      if (isRecord(parsed) && typeof parsed.seq === "number") return parsed.seq + 1;
    } catch {
      // Ignore a torn tail; the next append still gets a monotonic number.
    }
  }
  return lines.length + 1;
}
