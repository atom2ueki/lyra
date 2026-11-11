import { describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { closeRun, monitorRun, startRun, stopRun } from "../src/conductor";
import type { PaneBackend, PaneHandle } from "../src/pane-backend";
import { createTicket, ensureStore, listTickets } from "../src/store";
import { workerPaths } from "../src/worker-channel";

const projectId = "proj-test";

class FakeBackend implements PaneBackend {
  readonly name = "fake";
  readonly capabilities = {
    workspaces: true,
    tabs: true,
    splits: true,
    runCommand: true,
    startAgent: true,
    sendText: true,
    sendKeys: true,
    readOutput: true,
    waitForOutput: true,
    agentState: true,
    sessionIdentity: true,
    focus: true,
    resize: false,
    close: true,
  };
  readonly splits: Array<{ cwd: string; env?: Record<string, string> }> = [];
  readonly started: Array<{ name?: string; args: string[] }> = [];
  readonly closed: PaneHandle[] = [];
  #next = 1;

  async detect(): Promise<boolean> {
    return true;
  }
  async anchor(): Promise<PaneHandle> {
    return { backend: this.name, pane: "anchor" };
  }
  async split(_anchor: PaneHandle, opts: { cwd: string; env?: Record<string, string> }): Promise<PaneHandle> {
    this.splits.push({ cwd: opts.cwd, env: opts.env });
    return { backend: this.name, pane: `pane-${this.#next++}` };
  }
  async startAgent(_pane: PaneHandle, opts: { name?: string; args: string[] }): Promise<void> {
    this.started.push({ name: opts.name, args: opts.args });
  }
  async close(pane: PaneHandle): Promise<void> {
    this.closed.push(pane);
  }
}

async function gitRepo(): Promise<string> {
  const repo = await mkdtemp(join(tmpdir(), "lyra-repo-"));
  execFileSync("git", ["init", "-b", "main"], { cwd: repo });
  execFileSync("git", ["config", "user.name", "Lyra Test"], { cwd: repo });
  execFileSync("git", ["config", "user.email", "lyra@example.invalid"], { cwd: repo });
  await writeFile(join(repo, "README.md"), "# test\n");
  execFileSync("git", ["add", "."], { cwd: repo });
  execFileSync("git", ["commit", "-m", "init"], { cwd: repo });
  return repo;
}

async function setup(blocked = false) {
  const home = await mkdtemp(join(tmpdir(), "lyra-home-"));
  process.env.LYRA_HOME = home;
  const repo = await gitRepo();
  const store = join(home, "store");
  await ensureStore(store);
  const first = await createTicket(store, {
    projectId,
    subject: "First",
    prompt: "Do first",
  });
  if (blocked) {
    await createTicket(store, {
      projectId,
      subject: "Second",
      prompt: "Do second",
      blockedBy: [first.id],
    });
  }
  return { home, repo, store, first };
}

function config(maxParallel = 1) {
  return {
    maxParallel,
    maxPerProvider: {},
    approvalPosture: "ask" as const,
    merge: "auto" as const,
  };
}

describe("conductor", () => {
  test("starts a ready ticket, settles its result, and starts the next", async () => {
    const { repo, store, first } = await setup(true);
    const backend = new FakeBackend();
    const state = await startRun({
      repo,
      store,
      projectId,
      config: config(),
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
    });

    expect(state.tickets[first.id]?.status).toBe("running");
    expect(backend.started.length).toBe(1);
    expect(backend.started[0]?.name).toBe(`lyra-${first.id.toLowerCase()}`);
    expect(existsSync(state.tickets[first.id].worktree)).toBe(true);

    const paths = workerPaths(state.runDir, first.id);
    const claimId = state.tickets[first.id].claimId;
    await writeFile(
      join(paths.result),
      JSON.stringify({
        schemaVersion: 1,
        ticketId: first.id,
        runId: state.runId,
        claimId,
        status: "done",
        summary: "done",
        verification: "bun test",
        finishedAt: new Date().toISOString(),
      }) + "\n",
    );

    const next = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(next.tickets[first.id]?.status).toBe("done");
    expect(next.tickets["T-002"]?.status).toBe("running");
    expect((await listTickets(store)).find((t) => t.id === first.id)?.status).toBe("done");
    expect(backend.closed.length).toBe(1);
  });

  test("stopping a run releases the claim and closes owned panes", async () => {
    const { repo, store, first } = await setup(false);
    const backend = new FakeBackend();
    const state = await startRun({
      repo,
      store,
      projectId,
      config: config(),
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
    });
    const stopped = await stopRun(state.runDir, backend);
    expect(stopped.status).toBe("stopped");
    expect(stopped.tickets[first.id]?.status).toBe("stopped");
    expect((await listTickets(store)).find((t) => t.id === first.id)?.status).toBe("todo");
    expect(backend.closed.length).toBe(1);
  });

  test("close removes clean run-scoped worktrees", async () => {
    const { repo, store, first } = await setup(false);
    const backend = new FakeBackend();
    const state = await startRun({
      repo,
      store,
      projectId,
      config: config(),
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
    });
    const worktree = state.tickets[first.id].worktree;
    const closed = await closeRun(state.runDir);
    expect(closed.status).toBe("closed");
    expect(existsSync(worktree)).toBe(false);
  });
});
