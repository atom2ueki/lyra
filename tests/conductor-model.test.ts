import { describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { monitorRun, startRun } from "../src/conductor";
import { parseModelPolicy, type ModelInfo } from "../src/models";
import type { PaneBackend, PaneHandle } from "../src/pane-backend";
import { createTicket, ensureStore } from "../src/store";
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
  readonly started: Array<{ name?: string; kind: string; args: string[] }> = [];
  readonly sent: string[] = [];
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
  async startAgent(_pane: PaneHandle, opts: { name?: string; kind: string; args: string[] }): Promise<void> {
    this.started.push({ name: opts.name, kind: opts.kind, args: opts.args });
  }
  async sendText(_pane: PaneHandle, text: string): Promise<void> {
    this.sent.push(text);
  }
  async close(): Promise<void> {}
}

async function repo(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "lyra-model-repo-"));
  execFileSync("git", ["init", "-b", "main"], { cwd: dir });
  execFileSync("git", ["config", "user.name", "Lyra Test"], { cwd: dir });
  execFileSync("git", ["config", "user.email", "lyra@example.invalid"], { cwd: dir });
  await writeFile(join(dir, "README.md"), "# test\n");
  execFileSync("git", ["add", "."], { cwd: dir });
  execFileSync("git", ["commit", "-m", "init"], { cwd: dir });
  return dir;
}

describe("conductor model policy", () => {
  test("selects the class model and passes a role overlay", async () => {
    const home = await mkdtemp(join(tmpdir(), "lyra-model-home-"));
    process.env.LYRA_HOME = home;
    const dir = await repo();
    const store = join(home, "store");
    await ensureStore(store);
    const ticket = await createTicket(store, {
      projectId: "p",
      subject: "Model task",
      prompt: "do it",
      modelClass: "implementation",
    });
    const policy = parseModelPolicy({
      classes: { implementation: ["p/a:high"] },
      roles: { smol: ["p/a:auto"] },
      limits: { providers: { p: 2 } },
    });
    const models: ModelInfo[] = [{ provider: "p", id: "a", selector: "p/a" }];
    const backend = new FakeBackend();
    const state = await startRun({
      repo: dir,
      store,
      projectId: "p",
      config: { maxParallel: 1, maxPerProvider: { p: 1 }, approvalPosture: "ask", merge: "auto" },
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
      modelPolicy: policy,
      models,
      usage: [],
    });
    expect(state.tickets[ticket.id]?.model).toBe("p/a:high");
    expect(backend.started[0]?.args).toContain("--model");
    expect(backend.started[0]?.args).toContain("p/a:high");
    const configIndex = backend.started[0]?.args.indexOf("--config") ?? -1;
    expect(configIndex).toBeGreaterThanOrEqual(0);
    const rolesPath = backend.started[0]?.args[configIndex + 1];
    expect(rolesPath).toBe(join(workerPaths(state.runDir, ticket.id).root, "roles.yml"));
    expect(existsSync(rolesPath ?? "")).toBe(true);
  });

  test("launches a codex worker with a RESULT.md contract", async () => {
    const home = await mkdtemp(join(tmpdir(), "lyra-cross-home-"));
    process.env.LYRA_HOME = home;
    const dir = await repo();
    const store = join(home, "store");
    await ensureStore(store);
    const ticket = await createTicket(store, {
      projectId: "p",
      subject: "Cross agent",
      prompt: "do it",
      harness: "codex",
    });
    const backend = new FakeBackend();
    const state = await startRun({
      repo: dir,
      store,
      projectId: "p",
      config: { maxParallel: 1, maxPerProvider: {}, approvalPosture: "ask", merge: "auto" },
      backend,
      maxParallel: 1,
      baseRef: "HEAD",
      models: [],
      usage: [],
    });
    expect(backend.started[0]?.kind).toBe("codex");
    expect(backend.sent[0]).toContain("RESULT.md");
    const paths = workerPaths(state.runDir, ticket.id);
    await writeFile(
      paths.resultMd,
      "status: done\nsummary: cross agent done\nverification: written\n",
    );
    const next = await monitorRun({ runDir: state.runDir, backend, once: true, maxParallel: 1 });
    expect(next.tickets[ticket.id]?.status).toBe("done");
  });
});
