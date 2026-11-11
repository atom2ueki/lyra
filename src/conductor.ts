import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ApprovalPosture } from "./approval";
import { ompApprovalMapping } from "./approval";
import { loadConfig, type LyraConfig } from "./config";
import {
  activeProviders,
  loadModelCatalog,
  loadModelPolicy,
  loadUsage,
  selectModel,
  subagentRoles,
  writeRoleOverlay,
  type ModelInfo,
  type ModelPolicy,
  type UsageReport,
} from "./models";
import type { PaneBackend, PaneHandle, SplitDirection } from "./pane-backend";
import { selectBackend } from "./pane/select";
import { runsDirFor } from "./project";
import { readyTickets, validateGraph, type Ticket } from "./schema";
import { commandLine } from "./shell";
import {
  claimTicket,
  createTicket,
  finishTicket,
  listTickets,
  readTicket,
  releaseTicket,
  suspendTicket,
  updateExecution,
} from "./store";
import {
  closeRunWorktrees,
  createWorktree,
  detectBaseRef,
  removeWorktree,
  resumeWorktree,
  type WorktreeInfo,
} from "./worktree";
import {
  listWorkerRequests,
  readWorkerResult,
  readWorkerState,
  renderTaskBrief,
  workerPaths,
  writeTaskBrief,
  writeWorkerConfig,
  writeWorkerResponse,
} from "./worker-channel";
import type { WorkerConfig } from "./worker";

export interface RunTicketState {
  status: "starting" | "running" | "done" | "blocked" | "failed" | "stopped";
  claimId: string;
  pane?: PaneHandle;
  worktree: string;
  branch: string;
  baseHead: string;
  model?: string;
  phase?: string | null;
  detail?: string | null;
  startedAt: string;
  finishedAt?: string;
  result?: unknown;
  error?: string;
}

export interface RunState {
  schemaVersion: 1;
  runId: string;
  projectId: string;
  repo: string;
  store: string;
  runDir: string;
  status: "planned" | "running" | "done" | "blocked" | "stopped" | "closed";
  maxParallel: number;
  maxPerProvider: Record<string, number>;
  approval: ApprovalPosture;
  backend: string;
  baseRef: string;
  createdAt: string;
  updatedAt: string;
  modelPolicy?: ModelPolicy | null;
  modelCatalog?: ModelInfo[];
  usage?: UsageReport[];
  tickets: Record<string, RunTicketState>;
}

export interface StartOptions {
  repo: string;
  store: string;
  projectId: string;
  config?: LyraConfig;
  backend?: PaneBackend;
  anchor?: PaneHandle;
  maxParallel?: number;
  maxSeconds?: number;
  ticketId?: string;
  baseRef?: string;
  dryRun?: boolean;
  runId?: string;
  modelPolicy?: ModelPolicy;
  models?: ModelInfo[];
  usage?: UsageReport[];
}

export interface MonitorOptions {
  runDir: string;
  backend?: PaneBackend;
  once?: boolean;
  intervalMs?: number;
  timeoutMs?: number;
  maxParallel?: number;
}

function newRunId(): string {
  const stamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..*$/, "");
  return `run-${stamp}-${randomUUID().slice(0, 6)}`;
}

export function runDirFor(repo: string, runId: string): string {
  return join(runsDirFor(repo), runId);
}

export async function listRunIds(repo: string): Promise<string[]> {
  const root = runsDirFor(repo);
  try {
    const entries = await readdir(root, { withFileTypes: true });
    return entries.filter((e) => e.isDirectory()).map((e) => e.name).sort();
  } catch {
    return [];
  }
}

export async function latestRunDir(repo: string): Promise<string | null> {
  const ids = await listRunIds(repo);
  return ids.length ? runDirFor(repo, ids[ids.length - 1]) : null;
}

export async function loadRunState(runDir: string): Promise<RunState> {
  const raw: unknown = JSON.parse(await readFile(join(runDir, "state.json"), "utf8"));
  return raw as RunState;
}

export async function saveRunState(state: RunState): Promise<void> {
  state.updatedAt = new Date().toISOString();
  const path = join(state.runDir, "state.json");
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  await writeFile(temporary, JSON.stringify(state, null, 2) + "\n", { mode: 0o600 });
  await rename(temporary, path);
}

function ompBin(): string {
  return process.env.LYRA_OMP_BIN?.trim() || "omp";
}

function workerExtensionPath(): string {
  return fileURLToPath(new URL("../extensions/lyra-worker.mjs", import.meta.url));
}

function ompArgs(
  config: WorkerConfig,
  ticket: Ticket,
  maxSeconds: number | undefined,
  rolesPath?: string,
): string[] {
  const approval = ompApprovalMapping(config.approval);
  const args = ["--session-dir", config.sessionDir, "-e", workerExtensionPath(), ...approval.ompArgs];
  if (rolesPath) args.push("--config", rolesPath);
  if (config.model) args.push("--model", config.model);
  if (config.budgetUsd !== undefined) args.push("--max-cost", String(config.budgetUsd));
  if (maxSeconds !== undefined) args.push("--max-time", `${maxSeconds}s`);
  args.push(`@${config.promptFile}`);
  return args;
}

async function launchTicket(
  state: RunState,
  ticket: Ticket,
  backend: PaneBackend,
  anchor: PaneHandle,
  config: LyraConfig,
  maxSeconds: number | undefined,
  baseRef: string,
  modelPolicy: ModelPolicy | null,
  modelCatalog: ModelInfo[],
  usage: UsageReport[],
): Promise<void> {
  const current = await readTicket(state.store, ticket.id);
  if (!current) throw new Error(`unknown ticket: ${ticket.id}`);
  const suspension = current.suspension;
  const claimId = randomUUID();
  const claimed = await claimTicket(state.store, ticket.id, state.runId, claimId);
  if (!claimed) throw new Error(`ticket ${ticket.id} is no longer claimable`);
  const owner = { runId: state.runId, claimId };
  let worktree: WorktreeInfo | null = null;
  let pane: PaneHandle | undefined;
  try {
    worktree = suspension
      ? await resumeWorktree({
          repo: state.repo,
          dir: suspension.worktree,
          branch: suspension.branch,
          baseHead: suspension.baseHead,
        })
      : await createWorktree({
          repo: state.repo,
          runDir: state.runDir,
          runId: state.runId,
          ticketId: ticket.id,
          baseRef,
        });
    const workerBaseHead = suspension?.baseHead ?? worktree.baseHead;
    const paths = workerPaths(state.runDir, ticket.id);
    const choice = modelPolicy && modelCatalog.length
      ? selectModel({
          policy: modelPolicy,
          models: modelCatalog,
          usage,
          modelClass: ticket.modelClass,
          ticketModel: ticket.model,
          activeByProvider: activeProviders(state.tickets),
          configCaps: config.maxPerProvider,
        })
      : null;
    if (modelPolicy && modelCatalog.length && !choice) {
      throw new Error(`no model candidate available for class ${ticket.modelClass ?? "default"}`);
    }
    const model = choice?.selector ?? ticket.model;
    const rolesPath = choice && modelPolicy ? join(paths.root, "roles.yml") : undefined;
    if (rolesPath && modelPolicy && choice) {
      await writeRoleOverlay(rolesPath, modelPolicy, choice.selector);
    }
    const harness = ticket.harness ?? "omp";
    const workerConfig: WorkerConfig = {
      schemaVersion: 1,
      runId: state.runId,
      claimId,
      ticketId: ticket.id,
      projectId: state.projectId,
      harness,
      worktree: worktree.dir,
      branch: worktree.branch,
      baseHead: workerBaseHead,
      sessionDir: join(state.runDir, "sessions", ticket.id),
      promptFile: paths.prompt,
      approval: config.approvalPosture,
      model,
      modelClass: ticket.modelClass,
      modelRoles: modelPolicy && choice ? subagentRoles(modelPolicy, choice.selector) : undefined,
      budgetUsd: ticket.budgetUsd,
      maxSeconds,
      continuation: suspension
        ? { head: worktree.baseHead, baseHead: suspension.baseHead, reason: suspension.reason }
        : undefined,
      channel: {
        root: paths.root,
        requests: paths.requests,
        responses: paths.responses,
        state: paths.state,
        result: paths.result,
      },
    };
    await writeWorkerConfig(paths, workerConfig);
    const brief = renderTaskBrief({
      ticket: claimed,
      runId: state.runId,
      claimId,
      worktree: worktree.dir,
      branch: worktree.branch,
      baseRef,
      baseHead: workerBaseHead,
      model,
      budgetUsd: ticket.budgetUsd,
      resumed: suspension
        ? { head: worktree.baseHead, baseHead: suspension.baseHead, reason: suspension.reason }
        : undefined,
    });
    await writeTaskBrief(paths, brief);

    const workerName = `lyra-${ticket.id.toLowerCase()}`.replace(/[^a-z0-9_-]/g, "-");
    if (harness === "omp") {
      const args = ompArgs(workerConfig, claimed, maxSeconds, rolesPath);
      if (backend.capabilities.startAgent && backend.startAgent) {
        pane = await backend.split(anchor, {
          direction: "right" as SplitDirection,
          cwd: worktree.dir,
          env: { LYRA_WORKER_CONFIG: paths.config },
          logDir: paths.logs,
        });
        await backend.startAgent(pane, { kind: "omp", name: workerName, args });
      } else if (backend.capabilities.splits) {
        const command = commandLine(ompBin(), args, { LYRA_WORKER_CONFIG: paths.config });
        pane = await backend.split(anchor, {
          direction: "right" as SplitDirection,
          cwd: worktree.dir,
          command,
          env: { LYRA_WORKER_CONFIG: paths.config },
          logDir: paths.logs,
        });
      } else {
        throw new Error(`backend ${backend.name} cannot create worker panes`);
      }
    } else {
      if (!backend.capabilities.startAgent || !backend.startAgent || !backend.sendText) {
        throw new Error(`backend ${backend.name} cannot start a visible ${harness} worker`);
      }
      pane = await backend.split(anchor, {
        direction: "right" as SplitDirection,
        cwd: worktree.dir,
        env: { LYRA_WORKER_CONFIG: paths.config },
        logDir: paths.logs,
      });
      await backend.startAgent(pane, { kind: harness, name: workerName, args: [] });
      await backend.sendText(
        pane,
        `You are the Lyra worker for ${ticket.id}. Read ${paths.prompt} and execute it in ` +
          `${worktree.dir}. When finished, write ${paths.resultMd} with exactly these fields:\n` +
          `status: done|blocked|failed|needs_input\nsummary: <what you did or need>\n` +
          `verification: <checks you ran>\nblockers: <comma-separated ticket ids, blocked only>\n` +
          `Do not edit the ticket store or another worktree.`,
        { submit: true },
      );
    }

    const now = new Date().toISOString();
    state.tickets[ticket.id] = {
      status: "running",
      claimId,
      pane,
      worktree: worktree.dir,
      branch: worktree.branch,
      baseHead: workerBaseHead,
      model,
      phase: "starting",
      detail: null,
      startedAt: now,
    };
    await saveRunState(state);
  } catch (error) {
    await releaseTicket(state.store, ticket.id, owner, (error as Error).message).catch(() => null);
    if (pane && backend.close) await backend.close(pane).catch(() => null);
    if (worktree && !suspension) await removeWorktree(state.repo, worktree.dir).catch(() => null);
    state.tickets[ticket.id] = {
      status: "failed",
      claimId,
      worktree: worktree?.dir ?? "",
      branch: worktree?.branch ?? "",
      baseHead: worktree?.baseHead ?? "",
      startedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
      error: (error as Error).message,
    };
    await saveRunState(state);
    throw error;
  }
}

export async function startRun(options: StartOptions): Promise<RunState> {
  const config = options.config ?? (await loadConfig());
  const maxParallel = options.maxParallel ?? config.maxParallel;
  const backend = options.backend ?? selectBackend();
  const repo = options.repo;
  const tickets = await listTickets(options.store);
  const graphErrors = validateGraph(tickets);
  if (graphErrors.length) throw new Error(`invalid ticket graph:\n- ${graphErrors.join("\n- ")}`);
  let candidates = readyTickets(tickets);
  if (options.ticketId) {
    const named = tickets.find((t) => t.id === options.ticketId);
    if (!named) throw new Error(`unknown ticket: ${options.ticketId}`);
    if (!candidates.some((t) => t.id === named.id)) {
      throw new Error(`ticket ${named.id} is not ready (status ${named.status}, blockers ${named.blockedBy.join(", ") || "none"})`);
    }
    candidates = [named];
  }
  const baseRef = options.baseRef ?? (await detectBaseRef(repo));
  const runId = options.runId ?? newRunId();
  const runDir = runDirFor(repo, runId);
  await mkdir(runDir, { recursive: true });
  const modelPolicy = options.modelPolicy !== undefined
    ? options.modelPolicy
    : await loadModelPolicy().catch(() => null);
  const modelCatalog = options.models !== undefined
    ? options.models
    : await loadModelCatalog().catch(() => []);
  const usage = options.usage !== undefined
    ? options.usage
    : options.models !== undefined
      ? []
      : await loadUsage().catch(() => []);
  const state: RunState = {
    schemaVersion: 1,
    runId,
    projectId: options.projectId,
    repo,
    store: options.store,
    runDir,
    status: options.dryRun ? "planned" : "running",
    maxParallel,
    maxPerProvider: config.maxPerProvider,
    approval: config.approvalPosture,
    backend: backend.name,
    baseRef,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    modelPolicy,
    modelCatalog,
    usage,
    tickets: {},
  };
  await saveRunState(state);
  if (options.dryRun) return state;

  const anchor = options.anchor ?? (await backend.anchor());
  for (const ticket of candidates.slice(0, maxParallel)) {
    await launchTicket(
      state,
      ticket,
      backend,
      anchor,
      config,
      options.maxSeconds,
      baseRef,
      modelPolicy,
      modelCatalog,
      usage,
    );
  }
  if (Object.keys(state.tickets).length === 0) {
    state.status = "done";
    await saveRunState(state);
  }
  return state;
}

const MAX_BLOCKERS_PER_TICKET = 10;
const MAX_BLOCKER_DEPTH = 3;

async function blockerDepth(store: string, ticketId: string): Promise<number> {
  let depth = 0;
  let current = await readTicket(store, ticketId);
  const seen = new Set<string>();
  while (current?.parent && depth < MAX_BLOCKER_DEPTH + 1) {
    if (seen.has(current.id)) break;
    seen.add(current.id);
    depth++;
    current = await readTicket(store, current.parent);
  }
  return depth;
}

async function processRequests(state: RunState, ticketId: string): Promise<void> {
  const paths = workerPaths(state.runDir, ticketId);
  for (const request of await listWorkerRequests(paths)) {
    const payload = request.payload;
    try {
      let ticket: Ticket | null = null;
      if (typeof payload.existingId === "string" && payload.existingId) {
        ticket = await readTicket(state.store, payload.existingId);
        if (!ticket) throw new Error(`unknown existing ticket: ${payload.existingId}`);
      } else {
        if (payload.kind === "blocker") {
          const parent = await readTicket(state.store, ticketId);
          if (!parent) throw new Error(`unknown parent ticket: ${ticketId}`);
          if (parent.blockedBy.length >= MAX_BLOCKERS_PER_TICKET) {
            throw new Error(`${ticketId} already has ${MAX_BLOCKERS_PER_TICKET} blockers`);
          }
          if ((await blockerDepth(state.store, ticketId)) >= MAX_BLOCKER_DEPTH) {
            throw new Error(`blocker depth limit ${MAX_BLOCKER_DEPTH} reached at ${ticketId}`);
          }
        }
        ticket = await createTicket(state.store, {
          projectId: state.projectId,
          subject: String(payload.subject ?? "Discovered work"),
          prompt: String(payload.prompt ?? payload.reason ?? "Discovered work"),
          parent: ticketId,
          origin: "discovered",
        });
      }
      await writeWorkerResponse(paths, request.id, { ticketId: ticket.id });
    } catch (error) {
      await writeWorkerResponse(paths, request.id, { error: (error as Error).message });
    }
  }
}

async function settleWorker(
  state: RunState,
  ticketId: string,
  backend: PaneBackend,
): Promise<void> {
  const runTicket = state.tickets[ticketId];
  if (!runTicket || runTicket.status !== "running") return;
  const paths = workerPaths(state.runDir, ticketId);
  const owner = { runId: state.runId, claimId: runTicket.claimId };
  const result = await readWorkerResult(paths);
  if (!result || result.claimId !== runTicket.claimId) {
    const workerState = await readWorkerState(paths);
    if (workerState) {
      const phase = typeof workerState.phase === "string" ? workerState.phase : null;
      const detail = typeof workerState.detail === "string" ? workerState.detail : null;
      if (phase !== runTicket.phase || detail !== runTicket.detail) {
        runTicket.phase = phase;
        runTicket.detail = detail;
        await updateExecution(state.store, ticketId, owner, { phase, detail }).catch(() => null);
        await saveRunState(state);
      }
    }
    return;
  }
  if (result.status === "done") {
    await finishTicket(state.store, ticketId, owner, { status: "done", result });
    runTicket.status = "done";
  } else if (result.status === "blocked" || result.status === "needs_input") {
    const blockers = result.status === "blocked" ? result.blockers ?? [] : [];
    const suspended = await suspendTicket(state.store, ticketId, owner, {
      blockers,
      reason: result.summary ?? result.status,
      needsInput: result.status === "needs_input",
      worktree: runTicket.worktree,
      branch: runTicket.branch,
      baseHead: runTicket.baseHead,
    }).catch(async (error) => {
      await finishTicket(state.store, ticketId, owner, {
        status: "failed",
        result,
        lastError: (error as Error).message,
      });
      return null;
    });
    runTicket.status = suspended ? "blocked" : "failed";
  } else {
    await finishTicket(state.store, ticketId, owner, {
      status: "failed",
      result,
      lastError: `${result.status}: ${result.summary ?? ""}`.trim(),
    });
    runTicket.status = "failed";
  }
  runTicket.finishedAt = new Date().toISOString();
  runTicket.result = result;
  if (runTicket.pane && backend.close) {
    await backend.close(runTicket.pane).catch(() => null);
  }
  await saveRunState(state);
}

export async function monitorRun(options: MonitorOptions): Promise<RunState> {
  const backend = options.backend ?? selectBackend();
  const state = await loadRunState(options.runDir);
  const deadline = options.timeoutMs ? Date.now() + options.timeoutMs : null;
  const maxParallel = options.maxParallel ?? state.maxParallel;
  const interval = options.intervalMs ?? 1_000;
  while (true) {
    let next = await loadRunState(state.runDir);
    const running = Object.entries(next.tickets).filter(([, t]) => t.status === "running");
    for (const [ticketId] of running) {
      await processRequests(next, ticketId);
      await settleWorker(next, ticketId, backend);
    }
    next = await loadRunState(state.runDir);
    const stillRunning = Object.values(next.tickets).filter((t) => t.status === "running").length;
    const tickets = await listTickets(next.store);
    const ready = readyTickets(tickets).filter((t) => {
      if (t.suspension?.needsInput) return false;
      const stateTicket = next.tickets[t.id];
      return !stateTicket || stateTicket.status === "failed" || stateTicket.status === "blocked";
    });
    const capacity = Math.max(0, maxParallel - stillRunning);
    if (capacity > 0 && ready.length > 0) {
      const anchor = await backend.anchor();
      const config: LyraConfig = {
        maxParallel,
        maxPerProvider: next.maxPerProvider,
        approvalPosture: next.approval,
        merge: "auto",
      };
      for (const ticket of ready.slice(0, capacity)) {
        await launchTicket(
          next,
          ticket,
          backend,
          anchor,
          config,
          undefined,
          next.baseRef,
          next.modelPolicy ?? null,
          next.modelCatalog ?? [],
          next.usage ?? [],
        ).catch(() => null);
      }
    }
    next = await loadRunState(state.runDir);
    const active = Object.values(next.tickets).some((t) => t.status === "running");
    const after = await listTickets(next.store);
    const remaining = readyTickets(after).filter((t) => !t.suspension?.needsInput);
    if (!active && remaining.length === 0) {
      const open = after.filter((t) => t.status === "todo" || t.status === "in_progress");
      next.status = open.length ? "blocked" : "done";
      await saveRunState(next);
      return next;
    }
    if (deadline !== null && Date.now() >= deadline) {
      next.status = "stopped";
      await saveRunState(next);
      return next;
    }
    if (options.once) return next;
    await Bun.sleep(interval);
  }
}

export async function stopRun(runDir: string, backend?: PaneBackend): Promise<RunState> {
  const state = await loadRunState(runDir);
  const resolved = backend ?? selectBackend();
  for (const [ticketId, runTicket] of Object.entries(state.tickets)) {
    if (runTicket.status !== "running") continue;
    if (runTicket.pane && resolved.close) await resolved.close(runTicket.pane).catch(() => null);
    await releaseTicket(
      state.store,
      ticketId,
      { runId: state.runId, claimId: runTicket.claimId },
      "conductor stopped",
    ).catch(() => null);
    runTicket.status = "stopped";
    runTicket.finishedAt = new Date().toISOString();
  }
  state.status = "stopped";
  await saveRunState(state);
  return state;
}

export async function closeRun(runDir: string): Promise<RunState> {
  const state = await loadRunState(runDir);
  const tickets = await listTickets(state.store);
  const keep = tickets.flatMap((t) => (t.suspension ? [t.suspension.worktree] : []));
  const worktrees = await closeRunWorktrees(state.repo, state.runDir, keep);
  for (const result of worktrees) {
    if (!result.removed) {
      const ticket = Object.entries(state.tickets).find(([, t]) => t.worktree === result.dir)?.[0];
      if (ticket) state.tickets[ticket].error = `worktree kept: ${result.reason ?? "unknown"}`;
    }
  }
  state.status = "closed";
  await saveRunState(state);
  return state;
}

export async function loadLatestRun(repo: string): Promise<RunState | null> {
  const dir = await latestRunDir(repo);
  return dir ? loadRunState(dir) : null;
}

export function runStatePath(runDir: string): string {
  return join(runDir, "state.json");
}

export function runExists(runDir: string): boolean {
  return existsSync(join(runDir, "state.json"));
}
