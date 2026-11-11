import { copyFile, cp, readdir, readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import { normalizeTicket, validateGraph, type Ticket } from "../schema";
import { appendEvent, ensureStore, writeTicket } from "../store";
import { isRecord } from "../type-guards";

const MAPPED_FIELDS: Record<string, true> = {
  id: true,
  subject: true,
  prompt: true,
  status: true,
  blockedBy: true,
  priority: true,
  labels: true,
  source: true,
  externalId: true,
  externalUrl: true,
  attachments: true,
  attempts: true,
  lastError: true,
  result: true,
  execution: true,
  discoveries: true,
  suspension: true,
  checkpoint: true,
  createdAt: true,
  updatedAt: true,
  startedAt: true,
  finishedAt: true,
  parent: true,
  origin: true,
  requires: true,
  actor: true,
  harness: true,
  model: true,
};

export interface ImportError {
  file: string;
  message: string;
}

export interface ImportReport {
  source: string;
  target: string;
  projectId: string;
  imported: number;
  errors: ImportError[];
  validationErrors: string[];
  attachmentsCopied: boolean;
  eventsCopied: boolean;
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

export async function resolveOmpKanbanRoot(source: string): Promise<string> {
  for (const candidate of [source, join(source, ".omp", "kanban")]) {
    if (await exists(join(candidate, "tasks"))) return candidate;
  }
  throw new Error(`not an omp-kanban store (no tasks/): ${source}`);
}

export function mapOmpKanbanTicket(raw: unknown, projectId: string): Ticket {
  if (!isRecord(raw)) throw new Error("ticket must be a JSON object");
  if (typeof raw.id !== "string" || raw.id.length === 0) throw new Error("ticket id is required");
  const extra = Object.fromEntries(
    Object.entries(raw).filter(([key]) => !MAPPED_FIELDS[key]),
  );
  const ticket = normalizeTicket(
    {
      schemaVersion: 1,
      id: raw.id,
      projectId,
      subject: raw.subject,
      prompt: raw.prompt,
      status: raw.status,
      blockedBy: raw.blockedBy,
      priority: raw.priority,
      labels: raw.labels,
      source: raw.source,
      externalId: raw.externalId,
      externalUrl: raw.externalUrl,
      attachments: raw.attachments,
      attempts: raw.attempts,
      lastError: raw.lastError,
      result: raw.result,
      execution: raw.execution,
      discoveries: raw.discoveries,
      suspension: raw.suspension,
      checkpoint: raw.checkpoint,
      createdAt: raw.createdAt,
      updatedAt: raw.updatedAt,
      startedAt: raw.startedAt,
      finishedAt: raw.finishedAt,
      parent: raw.parent,
      origin: raw.origin,
      requires: raw.requires,
      actor: raw.actor,
      harness: raw.harness,
      model: raw.model,
    },
    { projectId },
  );
  if (Object.keys(extra).length > 0) ticket.extra = { ompKanban: extra };
  return ticket;
}

export async function importOmpKanban(
  source: string,
  target: string,
  projectId: string,
): Promise<ImportReport> {
  const root = await resolveOmpKanbanRoot(source);
  await ensureStore(target);
  const sourceTasks = join(root, "tasks");
  const names = (await readdir(sourceTasks)).filter((n) => n.endsWith(".json")).sort();
  const errors: ImportError[] = [];
  const tickets: Ticket[] = [];

  for (const name of names) {
    const file = join(sourceTasks, name);
    try {
      const raw = JSON.parse(await readFile(file, "utf8"));
      const ticket = mapOmpKanbanTicket(raw, projectId);
      await writeTicket(target, ticket);
      tickets.push(ticket);
    } catch (error) {
      errors.push({ file, message: (error as Error).message });
    }
  }

  let attachmentsCopied = false;
  const sourceAttachments = join(root, "attachments");
  if (await exists(sourceAttachments)) {
    await cp(sourceAttachments, join(target, "attachments"), {
      recursive: true,
      force: false,
      errorOnExist: false,
    });
    attachmentsCopied = true;
  }

  let eventsCopied = false;
  const sourceEvents = join(root, "events.jsonl");
  if (await exists(sourceEvents)) {
    await copyFile(sourceEvents, join(target, "events.imported.jsonl"));
    eventsCopied = true;
  }

  const validationErrors = validateGraph(tickets);
  await appendEvent(target, {
    type: "import.omp-kanban",
    source: root,
    projectId,
    imported: tickets.length,
    errors: errors.length,
  });

  return {
    source: root,
    target,
    projectId,
    imported: tickets.length,
    errors,
    validationErrors,
    attachmentsCopied,
    eventsCopied,
  };
}
