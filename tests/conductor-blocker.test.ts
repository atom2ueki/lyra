import { describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { closeRun, monitorRun, startRun } from "../src/conductor";
import type { PaneBackend, PaneHandle } from "../src/pane-backend";
import { createTicket, ensureStore, listTickets } from "../src/store";
import { workerPaths } from "../src/worker-channel";

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
  readonly started: Array<{ name?: string; args: string[] }> = [];
  readonly closed: PaneHandle[] = [];
  #next = 1;
  async detect(): Promise<boolean> {
    return true;
  }
  async anchor(): Promise<PaneHandle> {
    return { backend: this.name, pane: "anchor" };
  }
  async split(): Promise<PaneHandle> {
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
  const repo = await mkdtemp(join(tmpdir(), "lyra-blocker-repo-"));
  execFileSync("git", ["init", "-b", "main"], { cwd: repo });
  execFileSync("git", ["config", "user.name", "Lyra Test"], { cwd: repo });
  execFileSync("git", ["config", "user.email", "lyra@example.invalid"], { cwd: repo });
  await writeFile(join(repo, "README.md"), "# test\n");
  execFileSync("git", ["add", "."], { cwd: repo });
  execFileSync("git", ["commit", "-m", "init"], { cwd: repo });
  return repo;
}

async function setup(withBlocker = true) {
  const home = await mkdtemp(join(tmpdir(), "lyra-blocker-home-"));
  process.env.LYRA_HOME = home;
  const repo = await gitRepo();
  const store = join(home, "store");
  await ensureStore(store);
  const parent = await createTicket(store, { projectId: "p", subject: "Parent", prompt: "parent" });
  const blocker = withBlocker
    ? await createTicket(store, { projectId: "p", subject: "Blocker", prompt: "blocker" })
    : null;
  return { home, repo, store, parent, blocker };
}

function config() {
  return { maxParallel: 1, maxPerProvider: {}, approvalPosture: "ask" as const, merge: "auto" as const };
}

async function writeResult(
  runDir: string,
  ticketId: string,
  claimId: string,
  runId: string,
  body: Record<string, unknown>,
) {
  const paths = workerPaths(runDir, ticketId);
  await writeFile(
    join(paths.result),
    JSON.stringify({
      schemaVersion: 1,
      ticketId,
      runId,
      claimId,
      finishedAt: new Date().toISOString(),
      ...body,
    }) + "\n",
  );
}

describe("conductor blockers", () => {
  test("suspends the parent, runs the blocker, then resumes the parent checkpoint", async () => {
    const { repo, store, parent, blocker } = await setup(true);
    if (!blocker) throw new Error("blocker missing");
    const backend = new FakeBackend();
    const state = await startRun({
      repo,
      store,
      projectId: "p",
      config: config(),
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
    });
    expect(state.tickets[parent.id]?.status).toBe("running");

    const parentClaim = state.tickets[parent.id].claimId;
    const parentWorktree = state.tickets[parent.id].worktree;
    await writeResult(state.runDir, parent.id, parentClaim, state.runId, {
      status: "blocked",
      summary: "needs the blocker",
      blockers: [blocker.id],
    });

    const afterBlock = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(afterBlock.tickets[parent.id]?.status).toBe("blocked");
    expect(afterBlock.tickets[blocker.id]?.status).toBe("running");
    const parentTicket = (await listTickets(store)).find((t) => t.id === parent.id);
    expect(parentTicket?.status).toBe("todo");
    expect(parentTicket?.blockedBy).toContain(blocker.id);
    expect(parentTicket?.suspension?.worktree).toBe(parentWorktree);

    const blockerClaim = afterBlock.tickets[blocker.id].claimId;
    await writeResult(state.runDir, blocker.id, blockerClaim, state.runId, {
      status: "done",
      summary: "blocker done",
      verification: "ok",
    });

    const afterBlocker = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(afterBlocker.tickets[blocker.id]?.status).toBe("done");
    expect(afterBlocker.tickets[parent.id]?.status).toBe("running");
    expect(afterBlocker.tickets[parent.id]?.worktree).toBe(parentWorktree);
    expect(backend.started.map((s) => s.name)).toEqual([
      `lyra-${parent.id.toLowerCase()}`,
      `lyra-${blocker.id.toLowerCase()}`,
      `lyra-${parent.id.toLowerCase()}`,
    ]);

    const resumeClaim = afterBlocker.tickets[parent.id].claimId;
    await writeResult(state.runDir, parent.id, resumeClaim, state.runId, {
      status: "done",
      summary: "parent done",
      verification: "ok",
    });
    const done = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(done.status).toBe("done");
    expect(done.tickets[parent.id]?.status).toBe("done");
    expect((await listTickets(store)).find((t) => t.id === parent.id)?.suspension).toBeNull();

    const closed = await closeRun(state.runDir);
    expect(closed.status).toBe("closed");
    expect(existsSync(parentWorktree)).toBe(false);
  });

  test("needs_input blocks the run and keeps the worktree", async () => {
    const { repo, store, parent } = await setup(false);
    const backend = new FakeBackend();
    const state = await startRun({
      repo,
      store,
      projectId: "p",
      config: config(),
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
    });
    await writeResult(state.runDir, parent.id, state.tickets[parent.id].claimId, state.runId, {
      status: "needs_input",
      summary: "which API should I use?",
    });
    const next = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(next.status).toBe("blocked");
    expect(next.tickets[parent.id]?.status).toBe("blocked");
    expect(backend.started.length).toBe(1);
    const ticket = (await listTickets(store)).find((t) => t.id === parent.id);
    expect(ticket?.suspension?.needsInput).toBe(true);
    expect(existsSync(ticket?.suspension?.worktree ?? "")).toBe(true);
    const closed = await closeRun(state.runDir);
    expect(closed.status).toBe("closed");
    expect(existsSync(ticket?.suspension?.worktree ?? "")).toBe(true);
  });
});
