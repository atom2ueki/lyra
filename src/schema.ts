/**
 * Lyra ticket schema v1. This is intentionally smaller than the omp-kanban/telos donors; their
 * fields arrive through import adapters and unknown donor fields are preserved under `extra`.
 */
import { isRecord } from "./type-guards";

export const SCHEMA_VERSION = 1 as const;

export const TICKET_STATUSES = ["todo", "in_progress", "done", "failed"] as const;
export type TicketStatus = (typeof TICKET_STATUSES)[number];

export const PRIORITY_MIN = 0;
export const PRIORITY_MAX = 4;
export const PRIORITY_DEFAULT = 2;

export interface TicketExecution {
  runId: string;
  claimId: string;
  claimedAt: string;
  phase?: string | null;
  detail?: string | null;
  pendingBlockers?: string[];
  updatedAt?: string;
}

export interface TicketDiscovery {
  kind: "blocker" | "followup";
  prompt: string;
  id?: string;
  requestId?: string;
  createdAt?: string;
}

export interface TicketSuspension {
  runId: string;
  claimId: string;
  reason: string;
  needsInput: boolean;
  blockedBy: string[];
  worktree: string;
  branch: string;
  baseHead: string;
  checkpoint: { head: string; worktree: string; branch: string };
  suspendedAt: string;
}

export interface TicketAttachment {
  id?: string;
  filename: string;
  storedAs: string;
  contentType?: string;
  bytes?: number;
  sha256?: string;
  createdAt?: string;
}

export interface Ticket {
  schemaVersion: typeof SCHEMA_VERSION;
  id: string;
  projectId: string;
  subject: string;
  prompt: string;
  status: TicketStatus;
  blockedBy: string[];
  priority: number;
  labels: string[];
  modelClass?: string;
  acceptance?: string;
  plan?: string;
  branch?: string;
  budgetUsd?: number;
  source?: string;
  externalId?: string;
  externalUrl?: string;
  parent?: string;
  origin?: string;
  requires?: string[];
  actor?: string;
  harness?: string;
  model?: string;
  attachments?: TicketAttachment[];
  attempts: number;
  lastError: string | null;
  result: unknown | null;
  execution: TicketExecution | null;
  discoveries: TicketDiscovery[];
  suspension: TicketSuspension | null;
  checkpoint: unknown | null;
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  /** Unknown donor fields preserved by an import adapter. */
  extra?: Record<string, unknown>;
}

export class TicketValidationError extends Error {
  readonly errors: string[];
  constructor(errors: string[]) {
    super(`invalid Lyra ticket data:\n- ${errors.join("\n- ")}`);
    this.name = "TicketValidationError";
    this.errors = errors;
  }
}

export function nowIso(): string {
  return new Date().toISOString();
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === "string");
}

function asString(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function asNullableString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function asTimestamp(value: unknown, fallback: string): string {
  if (typeof value === "string" && !Number.isNaN(Date.parse(value))) return value;
  return fallback;
}

function asPriority(value: unknown): number {
  return typeof value === "number" && Number.isInteger(value) ? value : PRIORITY_DEFAULT;
}

function normalizeExecution(value: unknown): TicketExecution | null {
  if (!isRecord(value)) return null;
  const runId = asString(value.runId);
  const claimId = asString(value.claimId);
  if (!runId || !claimId) return null;
  return {
    runId,
    claimId,
    claimedAt: asTimestamp(value.claimedAt, nowIso()),
    phase: typeof value.phase === "string" ? value.phase : null,
    detail: typeof value.detail === "string" ? value.detail : null,
    pendingBlockers: isStringArray(value.pendingBlockers) ? value.pendingBlockers : [],
    updatedAt: typeof value.updatedAt === "string" ? value.updatedAt : undefined,
  };
}

function normalizeDiscoveries(value: unknown): TicketDiscovery[] {
  if (!Array.isArray(value)) return [];
  return value.filter(isRecord).map((d) => ({
    kind: d.kind === "followup" ? "followup" : "blocker",
    prompt: asString(d.prompt),
    id: typeof d.id === "string" ? d.id : undefined,
    requestId: typeof d.requestId === "string" ? d.requestId : undefined,
    createdAt: typeof d.createdAt === "string" ? d.createdAt : undefined,
  }));
}

function normalizeSuspension(value: unknown): TicketSuspension | null {
  if (!isRecord(value)) return null;
  if (
    typeof value.runId !== "string" ||
    typeof value.claimId !== "string" ||
    typeof value.worktree !== "string" ||
    typeof value.branch !== "string" ||
    typeof value.baseHead !== "string"
  ) {
    return null;
  }
  const checkpoint = isRecord(value.checkpoint) ? value.checkpoint : {};
  return {
    runId: value.runId,
    claimId: value.claimId,
    reason: typeof value.reason === "string" ? value.reason : "blocked",
    needsInput: value.needsInput === true,
    blockedBy: isStringArray(value.blockedBy) ? value.blockedBy : [],
    worktree: value.worktree,
    branch: value.branch,
    baseHead: value.baseHead,
    checkpoint: {
      head: typeof checkpoint.head === "string" ? checkpoint.head : "",
      worktree: typeof checkpoint.worktree === "string" ? checkpoint.worktree : value.worktree,
      branch: typeof checkpoint.branch === "string" ? checkpoint.branch : value.branch,
    },
    suspendedAt: typeof value.suspendedAt === "string" ? value.suspendedAt : nowIso(),
  };
}

export function normalizeTicket(
  raw: unknown,
  defaults: { projectId?: string; now?: string } = {},
): Ticket {
  if (!isRecord(raw)) throw new TicketValidationError(["ticket must be a JSON object"]);
  const now = defaults.now ?? nowIso();
  const status = TICKET_STATUSES.includes(raw.status as TicketStatus)
    ? (raw.status as TicketStatus)
    : "todo";
  const priority = asPriority(raw.priority);
  const ticket: Ticket = {
    schemaVersion: SCHEMA_VERSION,
    id: asString(raw.id),
    projectId: asString(raw.projectId, defaults.projectId ?? ""),
    subject: asString(raw.subject),
    prompt: asString(raw.prompt),
    status,
    blockedBy: isStringArray(raw.blockedBy) ? [...new Set(raw.blockedBy)] : [],
    priority: Math.min(PRIORITY_MAX, Math.max(PRIORITY_MIN, priority)),
    labels: isStringArray(raw.labels) ? [...new Set(raw.labels)] : [],
    modelClass: typeof raw.modelClass === "string" ? raw.modelClass : undefined,
    acceptance: typeof raw.acceptance === "string" ? raw.acceptance : undefined,
    plan: typeof raw.plan === "string" ? raw.plan : undefined,
    branch: typeof raw.branch === "string" ? raw.branch : undefined,
    budgetUsd: typeof raw.budgetUsd === "number" ? raw.budgetUsd : undefined,
    source: typeof raw.source === "string" ? raw.source : undefined,
    externalId: typeof raw.externalId === "string" ? raw.externalId : undefined,
    externalUrl: typeof raw.externalUrl === "string" ? raw.externalUrl : undefined,
    parent: typeof raw.parent === "string" ? raw.parent : undefined,
    origin: typeof raw.origin === "string" ? raw.origin : undefined,
    requires: isStringArray(raw.requires) ? [...new Set(raw.requires)] : undefined,
    actor: typeof raw.actor === "string" ? raw.actor : undefined,
    harness: typeof raw.harness === "string" ? raw.harness : undefined,
    model: typeof raw.model === "string" ? raw.model : undefined,
    attachments: Array.isArray(raw.attachments)
      ? raw.attachments.filter(isRecord).map((a) => ({
          id: typeof a.id === "string" ? a.id : undefined,
          filename: asString(a.filename),
          storedAs: asString(a.storedAs),
          contentType: typeof a.contentType === "string" ? a.contentType : undefined,
          bytes: typeof a.bytes === "number" ? a.bytes : undefined,
          sha256: typeof a.sha256 === "string" ? a.sha256 : undefined,
          createdAt: typeof a.createdAt === "string" ? a.createdAt : undefined,
        }))
      : undefined,
    attempts: typeof raw.attempts === "number" && Number.isInteger(raw.attempts) && raw.attempts >= 0
      ? raw.attempts
      : 0,
    lastError: asNullableString(raw.lastError),
    result: raw.result ?? null,
    execution: normalizeExecution(raw.execution),
    discoveries: normalizeDiscoveries(raw.discoveries),
    suspension: normalizeSuspension(raw.suspension),
    checkpoint: raw.checkpoint ?? null,
    createdAt: asTimestamp(raw.createdAt, now),
    updatedAt: asTimestamp(raw.updatedAt, now),
    startedAt: typeof raw.startedAt === "string" ? raw.startedAt : null,
    finishedAt: typeof raw.finishedAt === "string" ? raw.finishedAt : null,
  };
  if (isRecord(raw.extra)) ticket.extra = raw.extra;
  return ticket;
}

export function validateTicket(raw: unknown, path = "ticket"): string[] {
  const errors: string[] = [];
  if (!isRecord(raw)) return [`${path}: must be a JSON object`];
  if (raw.schemaVersion !== SCHEMA_VERSION) {
    errors.push(`${path}.schemaVersion: expected ${SCHEMA_VERSION}`);
  }
  if (typeof raw.id !== "string" || !/^[A-Za-z][A-Za-z0-9_-]*$/.test(raw.id)) {
    errors.push(`${path}.id: must match [A-Za-z][A-Za-z0-9_-]*`);
  }
  if (typeof raw.projectId !== "string" || raw.projectId.length === 0) {
    errors.push(`${path}.projectId: required`);
  }
  if (typeof raw.subject !== "string" || raw.subject.trim().length === 0) {
    errors.push(`${path}.subject: required`);
  }
  if (typeof raw.prompt !== "string" || raw.prompt.length === 0) {
    errors.push(`${path}.prompt: required`);
  }
  if (!TICKET_STATUSES.includes(raw.status as TicketStatus)) {
    errors.push(`${path}.status: expected one of ${TICKET_STATUSES.join(", ")}`);
  }
  if (!isStringArray(raw.blockedBy)) {
    errors.push(`${path}.blockedBy: must be a string array`);
  } else {
    if (new Set(raw.blockedBy).size !== raw.blockedBy.length) {
      errors.push(`${path}.blockedBy: duplicate ids`);
    }
    if (raw.blockedBy.includes(raw.id as string)) {
      errors.push(`${path}.blockedBy: cannot contain itself`);
    }
  }
  if (typeof raw.priority !== "number" || !Number.isInteger(raw.priority) ||
      raw.priority < PRIORITY_MIN || raw.priority > PRIORITY_MAX) {
    errors.push(`${path}.priority: integer ${PRIORITY_MIN}..${PRIORITY_MAX}`);
  }
  if (!isStringArray(raw.labels)) errors.push(`${path}.labels: must be a string array`);
  if (typeof raw.attempts !== "number" || !Number.isInteger(raw.attempts) || raw.attempts < 0) {
    errors.push(`${path}.attempts: non-negative integer`);
  }
  for (const key of ["createdAt", "updatedAt"] as const) {
    if (typeof raw[key] !== "string" || Number.isNaN(Date.parse(raw[key] as string))) {
      errors.push(`${path}.${key}: ISO timestamp required`);
    }
  }
  if (raw.execution !== null && raw.execution !== undefined) {
    if (!isRecord(raw.execution)) {
      errors.push(`${path}.execution: object or null`);
    } else {
      if (typeof raw.execution.runId !== "string" || !raw.execution.runId) {
        errors.push(`${path}.execution.runId: required`);
      }
      if (typeof raw.execution.claimId !== "string" || !raw.execution.claimId) {
        errors.push(`${path}.execution.claimId: required`);
      }
    }
  }
  if (raw.discoveries !== undefined && !Array.isArray(raw.discoveries)) {
    errors.push(`${path}.discoveries: array`);
  }
  return errors;
}

export function validateGraph(rawTickets: unknown[]): string[] {
  const errors: string[] = [];
  const tickets: Ticket[] = [];
  rawTickets.forEach((raw, index) => {
    const id = isRecord(raw) && typeof raw.id === "string" ? raw.id : `tickets[${index}]`;
    const ticketErrors = validateTicket(raw, id);
    errors.push(...ticketErrors);
    if (ticketErrors.length === 0) tickets.push(normalizeTicket(raw));
  });
  if (errors.length > 0) return errors;

  const projectIds = new Set(tickets.map((t) => t.projectId));
  if (projectIds.size > 1) {
    errors.push(`mixed project ids in one store: ${[...projectIds].sort().join(", ")}`);
  }
  const byId = new Map<string, Ticket>();
  for (const ticket of tickets) {
    if (byId.has(ticket.id)) errors.push(`duplicate ticket id: ${ticket.id}`);
    byId.set(ticket.id, ticket);
  }
  for (const ticket of tickets) {
    for (const blocker of ticket.blockedBy) {
      if (!byId.has(blocker)) errors.push(`${ticket.id}.blockedBy: unknown ticket ${blocker}`);
    }
  }

  const visiting = new Set<string>();
  const visited = new Set<string>();
  const stack: string[] = [];
  const visit = (id: string): void => {
    if (visited.has(id)) return;
    if (visiting.has(id)) {
      const at = stack.indexOf(id);
      const cycle = [...stack.slice(at), id].join(" -> ");
      errors.push(`dependency cycle: ${cycle}`);
      return;
    }
    visiting.add(id);
    stack.push(id);
    const ticket = byId.get(id);
    for (const blocker of ticket?.blockedBy ?? []) {
      if (byId.has(blocker)) visit(blocker);
    }
    stack.pop();
    visiting.delete(id);
    visited.add(id);
  };
  for (const id of byId.keys()) visit(id);

  for (const ticket of tickets) {
    if (ticket.status === "done") {
      for (const blocker of ticket.blockedBy) {
        if (byId.get(blocker)?.status !== "done") {
          errors.push(`${ticket.id}: done but blocker ${blocker} is not done`);
        }
      }
    }
  }
  return errors;
}

export function ticketMap(tickets: Ticket[]): Map<string, Ticket> {
  return new Map(tickets.map((t) => [t.id, t]));
}

export function readyTickets(tickets: Ticket[]): Ticket[] {
  const byId = ticketMap(tickets);
  return tickets
    .filter((t) =>
      t.status === "todo" &&
      t.execution === null &&
      t.blockedBy.every((id) => byId.get(id)?.status === "done")
    )
    .sort((a, b) =>
      a.priority - b.priority ||
      a.createdAt.localeCompare(b.createdAt) ||
      a.id.localeCompare(b.id)
    );
}
