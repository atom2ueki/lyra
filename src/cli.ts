#!/usr/bin/env bun
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { ensureConfig, loadConfig, lyraHome } from "./config";
import { validateAcceptancePaths } from "./acceptance";
import {
  closeRun,
  latestRunDir,
  loadRunState,
  monitorRun,
  runDirFor,
  startRun,
  stopRun,
  type RunState,
} from "./conductor";
import { graphFormats, type GraphFormat } from "./graph-format";
import { toDot, toJson, toMermaid } from "./graph";
import { importOmpKanban } from "./import/omp-kanban";
import {
  ensureModelPolicy,
  loadModelCatalog,
  loadModelPolicy,
  loadUsage,
  selectModel,
} from "./models";
import { runCommand } from "./pane/exec";
import { selectBackend } from "./pane/select";
import { projectIdFor, resolveProjectRoot, storeDirFor } from "./project";
import { readyTickets, ticketMap, validateGraph, type Ticket } from "./schema";
import { createTicket, ensureStore, listTickets, StoreError } from "./store";
import { workerPaths } from "./worker-channel";

const HELP = `lyra — supervised task conductor

Usage:
  lyra init                              create ~/.lyra/config.toml + this project's store
  lyra import omp-kanban <store>         import an omp-kanban store
  lyra conduct status                    validate the task graph (read-only)
  lyra conduct start                     claim ready tickets and launch workers
  lyra conduct monitor                   settle results and start the next ready ticket
  lyra conduct stop                      stop workers in the current run
  lyra conduct report                    print the current run report
  lyra add "<idea>"                      create an inbox ticket
  lyra logs <ticket>                     print a worker's state/result/log files
  lyra close                             remove clean run-scoped worktrees
  lyra status                            alias for "lyra conduct status"
  lyra graph [--format mermaid|dot|json] static task-graph projection
  lyra models                            show model policy and current selections
  lyra doctor                            preflight checks
  lyra --help

Options:
  --store <dir>        use an explicit ticket store
  --project-id <id>    override the project id for import/start
  --run <id>           target a specific run (default: latest)
  --max <n>            override the worker cap
  --max-seconds <n>    worker wall-clock limit
  --ticket <id>        start exactly one ready ticket
  --base <ref>         base ref for run-scoped worktrees
  --dry-run            plan without claiming/launching
  --once               monitor one pass
  --interval <ms>      monitor poll interval
  --timeout <ms>       monitor deadline
  --format <fmt>       graph format: mermaid|dot|json
  --json               machine-readable output
`;

interface Options {
  store?: string;
  projectId?: string;
  json: boolean;
  format?: GraphFormat;
  source?: string;
  run?: string;
  max?: number;
  maxSeconds?: number;
  ticket?: string;
  base?: string;
  dryRun: boolean;
  once: boolean;
  interval?: number;
  timeout?: number;
}

function numberArg(value: string | undefined, name: string): number {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) throw new Error(`${name} needs a positive number`);
  return parsed;
}

function parseOptions(args: string[]): Options {
  const options: Options = { json: false, dryRun: false, once: false };
  const positional: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--json") options.json = true;
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--once") options.once = true;
    else if (arg === "--store") options.store = args[++i];
    else if (arg === "--project-id") options.projectId = args[++i];
    else if (arg === "--format") options.format = args[++i] as GraphFormat;
    else if (arg === "--run") options.run = args[++i];
    else if (arg === "--max") options.max = numberArg(args[++i], "--max");
    else if (arg === "--max-seconds") options.maxSeconds = numberArg(args[++i], "--max-seconds");
    else if (arg === "--ticket") options.ticket = args[++i];
    else if (arg === "--base") options.base = args[++i];
    else if (arg === "--interval") options.interval = numberArg(args[++i], "--interval");
    else if (arg === "--timeout") options.timeout = numberArg(args[++i], "--timeout");
    else if (arg === "--help" || arg === "-h") {
      console.log(HELP);
      process.exit(0);
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown option: ${arg}`);
    } else {
      positional.push(arg);
    }
  }
  options.source = positional[0];
  return options;
}

function resolveStore(options: Options): { store: string; projectId: string; projectRoot: string } {
  const projectRoot = resolveProjectRoot();
  const projectId = options.projectId ?? projectIdFor(projectRoot);
  return {
    store: options.store ?? storeDirFor(projectRoot),
    projectId,
    projectRoot,
  };
}

function counts(tickets: Ticket[]): Record<string, number> {
  const result: Record<string, number> = { todo: 0, in_progress: 0, done: 0, failed: 0 };
  for (const ticket of tickets) result[ticket.status] = (result[ticket.status] ?? 0) + 1;
  return result;
}

function printStatus(
  store: string,
  projectId: string,
  projectRoot: string,
  tickets: Ticket[],
  json: boolean,
): number {
  const errors = [...validateGraph(tickets), ...validateAcceptancePaths(tickets, projectRoot)];
  const byId = ticketMap(tickets);
  const ready = readyTickets(tickets).map((t) => t.id);
  const blocked = tickets
    .filter((t) => t.status === "todo" && t.blockedBy.some((id) => byId.get(id)?.status !== "done"))
    .map((t) => t.id);
  const effectiveProjectId = tickets[0]?.projectId ?? projectId;
  const payload = {
    store,
    projectId: effectiveProjectId,
    counts: counts(tickets),
    ready,
    blocked,
    errors,
  };
  if (json) {
    console.log(JSON.stringify(payload, null, 2));
  } else {
    console.log(`Store:    ${store}`);
    console.log(`Project:  ${effectiveProjectId}`);
    console.log(
      `Tickets:  ${tickets.length} ` +
      `(todo ${payload.counts.todo}, in_progress ${payload.counts.in_progress}, ` +
      `done ${payload.counts.done}, failed ${payload.counts.failed})`,
    );
    console.log(`Ready:    ${ready.length ? ready.join(", ") : "(none)"}`);
    console.log(`Blocked:  ${blocked.length ? blocked.join(", ") : "(none)"}`);
    if (errors.length) {
      console.log("Errors:");
      for (const error of errors) console.log(`  - ${error}`);
    } else {
      console.log("Graph:    valid");
    }
  }
  return errors.length ? 1 : 0;
}

function printRunState(state: RunState, json: boolean): void {
  if (json) {
    console.log(JSON.stringify(state, null, 2));
    return;
  }
  console.log(`Run:      ${state.runId}`);
  console.log(`Status:   ${state.status}`);
  console.log(`Backend:  ${state.backend}`);
  console.log(`Run dir:  ${state.runDir}`);
  console.log(`Base:     ${state.baseRef}`);
  if (Object.keys(state.tickets).length === 0) {
    console.log("Tickets:  (none launched)");
    return;
  }
  console.log("Tickets:");
  for (const [id, ticket] of Object.entries(state.tickets)) {
    const detail = ticket.error ?? ticket.detail ?? "";
    console.log(
      `  ${id}  ${ticket.status.padEnd(8)} ${ticket.phase ?? ""}` +
      `${detail ? `  ${detail}` : ""}`,
    );
  }
}

async function resolveRunDir(options: Options, projectRoot: string): Promise<string> {
  if (options.run) {
    return options.run.includes("/") ? options.run : runDirFor(projectRoot, options.run);
  }
  const latest = await latestRunDir(projectRoot);
  if (!latest) throw new Error("no Lyra run found; run lyra conduct start first");
  return latest;
}

async function runDoctor(): Promise<number> {
  const failures: string[] = [];
  const check = async (name: string, fn: () => Promise<string>): Promise<void> => {
    try {
      const detail = await fn();
      console.log(`PASS  ${name}${detail ? `  ${detail}` : ""}`);
    } catch (error) {
      failures.push(name);
      console.log(`FAIL  ${name}  ${(error as Error).message}`);
    }
  };

  await check("omp", async () => {
    const bin = process.env.LYRA_OMP_BIN?.trim() || "omp";
    const result = await runCommand("which", [bin]);
    if (result.exitCode !== 0) throw new Error(`${bin} not found`);
    const version = await runCommand(bin, ["--version"]);
    if (version.exitCode !== 0) throw new Error(version.stderr.trim() || `${bin} --version failed`);
    return `${version.stdout.trim()} (${bin})`;
  });
  await check("git", async () => {
    const result = await runCommand("git", ["--version"]);
    if (result.exitCode !== 0) throw new Error("git not found");
    return result.stdout.trim();
  });
  await check("gh", async () => {
    const result = await runCommand("gh", ["--version"]);
    if (result.exitCode !== 0) throw new Error("gh not found");
    return result.stdout.split("\n")[0]?.trim();
  });
  await check("pane backend", async () => {
    const backend = selectBackend();
    return `${backend.name} (${Object.entries(backend.capabilities)
      .filter(([, ok]) => ok)
      .map(([key]) => key)
      .join(", ")})`;
  });
  await check("model policy", async () => {
    const policy = await loadModelPolicy();
    return `${Object.keys(policy.classes).length} classes, ${Object.keys(policy.roles).length} roles`;
  });
  await check("worker extension", async () => {
    const path = fileURLToPath(new URL("../extensions/lyra-worker.mjs", import.meta.url));
    if (!existsSync(path)) throw new Error(`missing ${path}`);
    return path;
  });
  await check("config", async () => {
    const config = await loadConfig();
    return `max_parallel=${config.maxParallel} approval=${config.approvalPosture} merge=${config.merge}`;
  });
  await check("ticket store", async () => {
    const target = resolveStore({ json: false, dryRun: false, once: false });
    if (!existsSync(target.store)) throw new Error(`missing ${target.store}; run lyra init`);
    return target.store;
  });
  if (failures.length) {
    console.log(`\n${failures.length} check(s) failed`);
    return 1;
  }
  console.log("\nAll checks passed");
  return 0;
}

async function main(argv: string[]): Promise<void> {
  const [command, ...rest] = argv;
  try {
    if (!command || command === "--help" || command === "-h" || command === "help") {
      console.log(HELP);
      return;
    }
    if (command === "init") {
      const config = await ensureConfig();
      const policy = await ensureModelPolicy();
      const projectRoot = resolveProjectRoot();
      const projectId = projectIdFor(projectRoot);
      const store = storeDirFor(projectRoot);
      await ensureStore(store);
      const loaded = await loadConfig();
      console.log(`Config:   ${config}`);
      console.log(`Policy:   ${policy}`);
      console.log(`Home:     ${lyraHome()}`);
      console.log(`Project:  ${projectRoot}`);
      console.log(`Project:  ${projectId}`);
      console.log(`Store:    ${store}`);
      console.log(
        `Policy:   max_parallel=${loaded.maxParallel} approval=${loaded.approvalPosture} ` +
        `merge=${loaded.merge}`,
      );
      return;
    }
    if (command === "import") {
      const [kind, ...args] = rest;
      if (kind !== "omp-kanban") throw new Error(`unknown import kind: ${kind ?? "(missing)"}`);
      const options = parseOptions(args);
      if (!options.source) throw new Error("usage: lyra import omp-kanban <store>");
      const target = resolveStore(options);
      const report = await importOmpKanban(options.source, target.store, target.projectId);
      if (options.json) {
        console.log(JSON.stringify(report, null, 2));
      } else {
        console.log(`Imported: ${report.imported} ticket(s) from ${report.source}`);
        console.log(`Target:   ${report.target}`);
        console.log(`Project:  ${report.projectId}`);
        console.log(`Attachments copied: ${report.attachmentsCopied ? "yes" : "no"}`);
        console.log(`Events copied:      ${report.eventsCopied ? "yes" : "no"}`);
        for (const error of report.errors) console.error(`ERROR ${error.file}: ${error.message}`);
        for (const error of report.validationErrors) console.error(`GRAPH ${error}`);
      }
      if (report.errors.length || report.validationErrors.length) process.exitCode = 1;
      return;
    }
    if (command === "conduct") {
      const [sub, ...args] = rest;
      const options = parseOptions(args);
      const target = resolveStore(options);
      if (sub === "status") {
        const tickets = await listTickets(target.store);
        process.exitCode = printStatus(
          target.store,
          target.projectId,
          target.projectRoot,
          tickets,
          options.json,
        );
        return;
      }
      if (sub === "start") {
        const config = await loadConfig();
        if (options.dryRun) {
          const tickets = await listTickets(target.store);
          const ready = readyTickets(tickets);
          console.log(`Dry run: ${ready.length} ready ticket(s), cap ${options.max ?? config.maxParallel}`);
          for (const ticket of ready.slice(0, options.max ?? config.maxParallel)) {
            console.log(`  ${ticket.id}  ${ticket.subject}`);
          }
          return;
        }
        const state = await startRun({
          repo: target.projectRoot,
          store: target.store,
          projectId: target.projectId,
          config,
          maxParallel: options.max,
          maxSeconds: options.maxSeconds,
          ticketId: options.ticket,
          baseRef: options.base,
        });
        printRunState(state, options.json);
        return;
      }
      if (sub === "monitor") {
        const runDir = await resolveRunDir(options, target.projectRoot);
        const state = await monitorRun({
          runDir,
          once: options.once,
          intervalMs: options.interval,
          timeoutMs: options.timeout,
          maxParallel: options.max,
        });
        printRunState(state, options.json);
        return;
      }
      if (sub === "stop") {
        const runDir = await resolveRunDir(options, target.projectRoot);
        const state = await stopRun(runDir);
        printRunState(state, options.json);
        return;
      }
      if (sub === "report") {
        const runDir = await resolveRunDir(options, target.projectRoot);
        printRunState(await loadRunState(runDir), options.json);
        return;
      }
      throw new Error(`unknown conduct command: ${sub ?? "(missing)"}`);
    }
    if (command === "status") {
      const options = parseOptions(rest);
      const target = resolveStore(options);
      const tickets = await listTickets(target.store);
      process.exitCode = printStatus(
        target.store,
        target.projectId,
        target.projectRoot,
        tickets,
        options.json,
      );
      return;
    }
    if (command === "models") {
      const options = parseOptions(rest);
      const policy = await loadModelPolicy();
      const models = await loadModelCatalog().catch(() => []);
      const usage = await loadUsage().catch(() => []);
      const selections = Object.fromEntries(
        Object.keys(policy.classes).map((name) => [
          name,
          selectModel({ policy, models, usage, modelClass: name })?.selector ?? null,
        ]),
      );
      if (options.json) {
        console.log(JSON.stringify({ policy, selections, modelCount: models.length }, null, 2));
        return;
      }
      console.log(`Policy:   ${await ensureModelPolicy()}`);
      console.log(`Models:   ${models.length} available`);
      console.log("Classes:");
      for (const [name, selector] of Object.entries(selections)) {
        console.log(`  ${name.padEnd(16)} ${selector ?? "(none available)"}`);
      }
      console.log("Roles:");
      for (const [name, candidates] of Object.entries(policy.roles)) {
        console.log(`  ${name.padEnd(16)} ${candidates[0] ?? "(unset)"}`);
      }
      return;
    }
    if (command === "graph") {
      const options = parseOptions(rest);
      const target = resolveStore(options);
      const tickets = await listTickets(target.store);
      const errors = validateGraph(tickets);
      if (errors.length) {
        for (const error of errors) console.error(`GRAPH ${error}`);
        process.exitCode = 1;
        return;
      }
      const format = options.format ?? "mermaid";
      if (!graphFormats.includes(format)) {
        throw new Error(`unknown graph format: ${format}`);
      }
      if (format === "mermaid") process.stdout.write(toMermaid(tickets));
      else if (format === "dot") process.stdout.write(toDot(tickets));
      else if (format === "json") console.log(JSON.stringify(toJson(tickets), null, 2));
      return;
    }
    if (command === "add") {
      const options = parseOptions(rest);
      if (!options.source) throw new Error('usage: lyra add "<idea>"');
      const target = resolveStore(options);
      await ensureStore(target.store);
      const ticket = await createTicket(target.store, {
        projectId: target.projectId,
        subject: options.source,
        prompt: options.source,
        origin: "inbox",
      });
      console.log(`${ticket.id}  ${ticket.subject}`);
      return;
    }
    if (command === "logs") {
      const options = parseOptions(rest);
      const ticketId = options.source;
      if (!ticketId) throw new Error("usage: lyra logs <ticket>");
      const target = resolveStore(options);
      const runDir = await resolveRunDir(options, target.projectRoot);
      await loadRunState(runDir);
      const paths = workerPaths(runDir, ticketId);
      console.log(`Worker: ${paths.root}`);
      for (const [name, path] of [
        ["state", paths.state],
        ["result", paths.result],
        ["RESULT.md", paths.resultMd],
      ] as const) {
        try {
          console.log(`\n--- ${name} ---\n${await readFile(path, "utf8")}`);
        } catch {
          // The worker may not have produced this file yet.
        }
      }
      if (existsSync(paths.logs)) {
        console.log(`\n--- logs ---\n${(await readdir(paths.logs)).join("\n")}`);
      }
      return;
    }
    if (command === "report") {
      const options = parseOptions(rest);
      const target = resolveStore(options);
      const runDir = await resolveRunDir(options, target.projectRoot);
      printRunState(await loadRunState(runDir), options.json);
      return;
    }
    if (command === "close") {
      const options = parseOptions(rest);
      const target = resolveStore(options);
      const runDir = await resolveRunDir(options, target.projectRoot);
      const state = await closeRun(runDir);
      printRunState(state, options.json);
      return;
    }
    if (command === "doctor") {
      process.exitCode = await runDoctor();
      return;
    }
    throw new Error(`unknown command: ${command}`);
  } catch (error) {
    if (error instanceof StoreError) {
      console.error(error.message);
    } else {
      console.error(`lyra: ${(error as Error).message}`);
    }
    process.exitCode = 1;
  }
}

if (import.meta.main) {
  await main(process.argv.slice(2));
}
