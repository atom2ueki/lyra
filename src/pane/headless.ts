import { spawn, type ChildProcess } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import type {
  PaneBackend,
  PaneCapabilities,
  PaneHandle,
  PaneInfo,
  ReadOptions,
  SplitOptions,
  StartAgentOptions,
  WaitOptions,
  WaitResult,
} from "../pane-backend";

const CAPABILITIES: PaneCapabilities = {
  workspaces: false,
  tabs: false,
  splits: true,
  runCommand: true,
  startAgent: false,
  sendText: false,
  sendKeys: false,
  readOutput: true,
  waitForOutput: true,
  agentState: false,
  sessionIdentity: false,
  focus: false,
  resize: false,
  close: true,
};

interface HeadlessProcess {
  child: ChildProcess;
  logPath: string;
  exitCode: number | null;
}

export class HeadlessBackend implements PaneBackend {
  readonly name = "headless";
  readonly capabilities = CAPABILITIES;
  readonly #env: NodeJS.ProcessEnv;
  readonly #processes = new Map<string, HeadlessProcess>();
  #next = 1;

  constructor(options: { env?: NodeJS.ProcessEnv } = {}) {
    this.#env = options.env ?? process.env;
  }

  async detect(): Promise<boolean> {
    return !process.stdout.isTTY || this.#env.LYRA_PANE_BACKEND === "headless";
  }

  async anchor(): Promise<PaneHandle> {
    return { backend: this.name, pane: "headless" };
  }

  async split(_anchor: PaneHandle, opts: SplitOptions): Promise<PaneHandle> {
    if (!opts.command) throw new Error("headless backend requires a command");
    const pane = `headless-${this.#next++}`;
    const logDir = opts.logDir ?? opts.cwd;
    await mkdir(logDir, { recursive: true });
    const logPath = join(logDir, `${pane}.log`);
    const stream = createWriteStream(logPath, { flags: "a", mode: 0o600 });
    const child = spawn("sh", ["-c", opts.command], {
      cwd: opts.cwd,
      env: { ...this.#env, ...opts.env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    child.stdout?.pipe(stream);
    child.stderr?.pipe(stream);
    const record: HeadlessProcess = { child, logPath, exitCode: null };
    child.on("exit", (code) => {
      record.exitCode = code ?? 1;
      stream.end();
    });
    child.on("error", () => {
      record.exitCode = 1;
      stream.end();
    });
    this.#processes.set(pane, record);
    return { backend: this.name, pane };
  }

  async run(pane: PaneHandle, command: string): Promise<void> {
    throw new Error(`headless run is not available for ${pane.pane}; use split with a command`);
  }

  async read(pane: PaneHandle, _opts: ReadOptions = {}): Promise<string> {
    const record = this.#processes.get(pane.pane);
    if (!record) throw new Error(`headless process not found: ${pane.pane}`);
    return readFile(record.logPath, "utf8");
  }

  async wait(pane: PaneHandle, opts: WaitOptions): Promise<WaitResult> {
    const record = this.#processes.get(pane.pane);
    if (!record) return { matched: false, timedOut: false };
    const deadline = Date.now() + opts.timeoutMs;
    while (Date.now() < deadline) {
      const output = await readFile(record.logPath, "utf8").catch(() => "");
      if (opts.match && output.includes(opts.match)) return { matched: true, output };
      if (!opts.match && record.exitCode !== null) {
        return { matched: true, output, state: `exit:${record.exitCode}` };
      }
      await Bun.sleep(100);
    }
    return { matched: false, timedOut: true };
  }

  async close(pane: PaneHandle): Promise<void> {
    const record = this.#processes.get(pane.pane);
    if (!record) return;
    if (record.exitCode === null) {
      record.child.kill("SIGTERM");
      await Bun.sleep(100);
      if (record.exitCode === null) record.child.kill("SIGKILL");
    }
  }

  async list(): Promise<PaneInfo[]> {
    return [...this.#processes.entries()].map(([pane, record]) => ({
      backend: this.name,
      pane,
      status: record.exitCode === null ? "running" : `exit:${record.exitCode}`,
    }));
  }

  async startAgent(_pane: PaneHandle, _opts: StartAgentOptions): Promise<void> {
    throw new Error("headless backend cannot start agents; split with the omp command instead");
  }
}
