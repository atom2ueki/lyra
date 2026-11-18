#!/usr/bin/env node
// Deterministic integration evaluation for the typed Lyra tools.
// It drives the real `lyra` CLI and a fake OMP worker, but the "agent policy" is a scripted
// sequence of tool calls. The live OMP scenario is separate and documented in README.md.

import { spawn } from "node:child_process";
import { chmod, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { LyraClient } from "../../packages/omp/lib/lyra-cli.mjs";
import { registerLyraTools } from "../../packages/omp/lib/lyra-tools.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../..");

const chain = () => {
  const schema = {
    optional: () => schema,
    describe: () => schema,
    int: () => schema,
    positive: () => schema,
    min: () => schema,
    max: () => schema,
    nullable: () => schema,
  };
  return schema;
};

const fakeZod = {
  string: chain,
  number: chain,
  boolean: chain,
  enum: chain,
  array: chain,
  object: (shape) => {
    const schema = chain();
    schema.shape = shape;
    return schema;
  },
};

function makePi() {
  const tools = new Map();
  const pi = {
    zod: fakeZod,
    setLabel() {},
    registerTool(definition) {
      tools.set(definition.name, definition);
    },
    registerCommand() {},
  };
  registerLyraTools(pi, current.client);
  return tools;
}

const current = { client: undefined };

async function run(args, cwd, env) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(args[0], args.slice(1), {
      cwd,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${args.join(" ")} exited ${code}: ${stderr.trim()}`));
    });
  });
}

async function git(repo, ...args) {
  await run(["git", ...args], repo, process.env);
}

async function fakeOmp(dir) {
  const path = join(dir, "fake-omp.sh");
  const script = await readFile(join(HERE, "fixtures/fake-omp.sh"), "utf8");
  await writeFile(path, script);
  await chmod(path, 0o755);
  return path;
}

async function fixture(mode) {
  const repo = await mkdtemp(join(tmpdir(), "lyra-eval-"));
  const home = join(repo, ".home");
  await writeFile(join(repo, "README.md"), "eval\n");
  await git(repo, "init", "-q");
  await git(repo, "config", "user.email", "eval@example.com");
  await git(repo, "config", "user.name", "Eval");
  await git(repo, "add", "README.md");
  await git(repo, "commit", "-qm", "init");
  const omp = await fakeOmp(repo);
  const env = {
    ...process.env,
    HOME: home,
    LYRA_HOME: home,
    LYRA_PANE_BACKEND: "headless",
    LYRA_OMP_BIN: omp,
    FAKE_MODE: mode,
    FAKE_PARENT: "",
  };
  const client = new LyraClient({ bin: process.env.LYRA_BIN, env });
  current.client = client;
  current.cwd = repo;
  await client.text(["init"], { cwd: repo });
  return { repo, home, omp, env, client };
}

function tool(name, pi) {
  const definition = pi.get(name);
  if (!definition) throw new Error(`missing tool ${name}`);
  return definition;
}

async function callTool(pi, name, params, options = {}) {
  const definition = tool(name, pi);
  const record = { name, params, isError: false };
  if (options.calls) options.calls.push(record);
  try {
    const result = await definition.execute("eval-call", params, options.signal, () => {}, { cwd: options.cwd });
    record.isError = result.isError === true;
    return result;
  } catch (error) {
    record.isError = true;
    throw error;
  }
}

async function waitUntilSettled(pi, runId, calls, budgetMs = 20_000, signal) {
  const deadline = Date.now() + budgetMs;
  let afterRevision;
  let last;
  while (Date.now() < deadline) {
    last = await callTool(pi, "lyra_wait", {
      runId,
      ...(afterRevision === undefined ? {} : { afterRevision }),
      timeoutMs: 5_000,
    }, { cwd: current.cwd, calls, signal });
    if (last.isError) return last;
    afterRevision = last.details.revision;
    if (!last.details.timedOut && !["running", "starting"].includes(last.details.status)) {
      return last;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
  }
  throw new Error(`run ${runId} did not settle within ${budgetMs}ms; last=${JSON.stringify(last?.details)}`);
}

async function countTickets(home) {
  const stores = join(home, "stores");
  try {
    const projects = await readdir(stores);
    let count = 0;
    for (const project of projects) {
      try {
        count += (await readdir(join(stores, project, "tasks"))).filter((name) => name.endsWith(".json")).length;
      } catch {
        // Store not created yet.
      }
    }
    return count;
  } catch {
    return 0;
  }
}

function summarizeResult(result) {
  if (result.isError) return `error:${result.details.code}`;
  return "ok";
}





async function scenarioInvalid() {
  const s = await fixture("done");
  const calls = [];
  try {
    const pi = makePi();
    const result = await callTool(pi, "lyra_tickets_create", {
      tickets: [
        { key: "a", subject: "A", prompt: "A", blockedBy: ["b"], harness: "omp" },
        { key: "b", subject: "B", prompt: "B", blockedBy: ["a"], harness: "omp" },
      ],
    }, { cwd: s.repo, calls });
    assert(result.isError && result.details.code === "invalid_input", "cycle rejected");
    assert((await countTickets(s.home)) === 0, "cycle created no tickets");
    return { name: "E-invalid", status: "pass", calls: calls.length, ...resultCounts(calls) };
  } finally {
    await rm(s.repo, { recursive: true, force: true });
  }
}

async function scenarioUpdate() {
  const s = await fixture("done");
  const calls = [];
  try {
    const pi = makePi();
    const created = await callTool(pi, "lyra_tickets_create", {
      tickets: [
        { key: "one", subject: "One", prompt: "One", blockedBy: [], harness: "omp" },
        { key: "two", subject: "Two", prompt: "Two", blockedBy: [], harness: "omp" },
      ],
    }, { cwd: s.repo, calls });
    assert(!created.isError, "update fixture tickets");
    const one = created.details.created[0].id;
    const two = created.details.created[1].id;

    const updated = await callTool(pi, "lyra_tickets", {
      action: "update",
      ticketId: one,
      prompt: "One revised",
      priority: 4,
      blockedBy: [two],
      labels: ["backend"],
    }, { cwd: s.repo, calls });
    assert(!updated.isError, "ticket update");
    assert(updated.details.ticket.prompt === "One revised", "update rewrote prompt");
    assert(updated.details.ticket.blockedBy[0] === two, "update replaced blockers");
    assert(updated.details.ticket.labels[0] === "backend", "update replaced labels");

    const status = await callTool(pi, "lyra_status", {}, { cwd: s.repo, calls });
    const blockedIds = status.details.graph.blocked.map((ticket) => ticket.id);
    assert(blockedIds.includes(one), `update made ${one} blocked`);
    assert(!status.details.graph.ready.some((ticket) => ticket.id === one), "updated ticket is not ready");

    const shown = await callTool(pi, "lyra_tickets", { action: "show", ticketId: one }, { cwd: s.repo, calls });
    const updatedAt = shown.details.ticket.updatedAt;
    const rejected = await callTool(pi, "lyra_tickets", {
      action: "update",
      ticketId: one,
      blockedBy: [one],
    }, { cwd: s.repo, calls });
    assert(rejected.isError && rejected.details.code === "invalid_input", "self blocker rejected");
    const after = await callTool(pi, "lyra_tickets", { action: "show", ticketId: one }, { cwd: s.repo, calls });
    assert(after.details.ticket.updatedAt === updatedAt, "rejected update wrote nothing");
    assert(after.details.ticket.prompt === "One revised", "rejected update changed no field");
    return { name: "F-update", status: "pass", calls: calls.length, ...resultCounts(calls) };
  } finally {
    await rm(s.repo, { recursive: true, force: true });
  }
}

function resultCounts(calls) {
  return {
    toolCalls: calls.length,
    invalidCalls: calls.filter((call) => call.isError).length,
    directShellCalls: 0,
    directStoreMutations: 0,
  };
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function main() {
  const bin = process.env.LYRA_BIN || join(ROOT, "rust/target/debug/lyra");
  process.env.LYRA_BIN = bin;
  const report = { schemaVersion: 1, startedAt: new Date().toISOString(), scenarios: [] };
  const scenarios = [scenarioInvalid, scenarioUpdate];
  for (const scenario of scenarios) {
    const started = Date.now();
    try {
      const result = await scenario();
      report.scenarios.push({ ...result, durationMs: Date.now() - started });
    } catch (error) {
      report.scenarios.push({
        name: scenario.name,
        status: "fail",
        error: error instanceof Error ? error.message : String(error),
        durationMs: Date.now() - started,
      });
    }
  }
  report.finishedAt = new Date().toISOString();
  report.passed = report.scenarios.every((scenario) => scenario.status === "pass");
  const output = join(HERE, "report.json");
  await writeFile(output, JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify(report, null, 2));
  if (!report.passed) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
