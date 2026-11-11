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
import { parseJson, resultOf, runCommand, type CommandRunner } from "./exec";

const CAPABILITIES: PaneCapabilities = {
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

interface HerdrOptions {
  bin?: string;
  env?: NodeJS.ProcessEnv;
  run?: CommandRunner;
}

function asHandle(pane: Record<string, unknown>): PaneHandle {
  const paneId = String(pane.pane_id ?? pane.id ?? "");
  if (!paneId) throw new Error("herdr returned a pane without pane_id");
  const tabId = typeof pane.tab_id === "string" ? pane.tab_id : undefined;
  const workspace = paneId.includes(":") ? paneId.split(":")[0] : undefined;
  const session = pane.agent_session;
  const sessionFile =
    typeof session === "object" && session !== null && typeof (session as Record<string, unknown>).value === "string"
      ? String((session as Record<string, unknown>).value)
      : undefined;
  return {
    backend: "herdr",
    workspace,
    tab: tabId,
    pane: paneId,
    agent: typeof pane.agent === "string" ? pane.agent : undefined,
    sessionFile,
  };
}

export class HerdrBackend implements PaneBackend {
  readonly name = "herdr";
  readonly capabilities = CAPABILITIES;
  readonly #bin: string;
  readonly #env: NodeJS.ProcessEnv;
  readonly #run: CommandRunner;

  constructor(options: HerdrOptions = {}) {
    this.#bin = options.bin ?? "herdr";
    this.#env = options.env ?? process.env;
    this.#run = options.run ?? runCommand;
  }

  async detect(): Promise<boolean> {
    return this.#env.HERDR_ENV === "1";
  }

  async anchor(): Promise<PaneHandle> {
    const inherited = this.#env.HERDR_PANE_ID?.trim();
    if (inherited) return { backend: this.name, pane: inherited };
    const result = await this.#run(this.#bin, ["pane", "current", "--current"], {
      env: this.#env,
    });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr pane current failed");
    const payload = resultOf(parseJson(result.stdout));
    const pane = payload?.pane;
    if (typeof pane !== "object" || pane === null) throw new Error("herdr pane current returned no pane");
    return asHandle(pane as Record<string, unknown>);
  }

  async split(anchor: PaneHandle, opts: SplitOptions): Promise<PaneHandle> {
    const args = [
      "pane",
      "split",
      "--pane",
      anchor.pane,
      "--direction",
      opts.direction,
      "--cwd",
      opts.cwd,
      "--no-focus",
    ];
    for (const [key, value] of Object.entries(opts.env ?? {})) {
      args.push("--env", `${key}=${value}`);
    }
    if (opts.command) args.push("--", opts.command);
    const result = await this.#run(this.#bin, args, { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr pane split failed");
    const payload = resultOf(parseJson(result.stdout));
    const pane = payload?.pane;
    if (typeof pane !== "object" || pane === null) throw new Error("herdr pane split returned no pane");
    return asHandle(pane as Record<string, unknown>);
  }

  async run(pane: PaneHandle, command: string): Promise<void> {
    const result = await this.#run(this.#bin, ["pane", "run", pane.pane, command], {
      env: this.#env,
    });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr pane run failed");
  }

  async startAgent(pane: PaneHandle, opts: StartAgentOptions): Promise<void> {
    const name = opts.name ?? `worker-${pane.pane}`;
    const args = ["agent", "start", name, "--kind", opts.kind, "--pane", pane.pane, "--", ...opts.args];
    const result = await this.#run(this.#bin, args, { env: this.#env, timeoutMs: 300_000 });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr agent start failed");
  }

  async sendText(pane: PaneHandle, text: string, opts: { submit?: boolean } = {}): Promise<void> {
    if (opts.submit !== false && pane.agent) {
      const result = await this.#run(this.#bin, ["agent", "prompt", pane.agent, text], {
        env: this.#env,
      });
      if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr agent prompt failed");
      return;
    }
    const result = await this.#run(this.#bin, ["pane", "send-text", pane.pane, text], {
      env: this.#env,
    });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr send-text failed");
    if (opts.submit !== false) await this.sendKeys(pane, ["enter"]);
  }

  async sendKeys(pane: PaneHandle, keys: string[]): Promise<void> {
    const target = pane.agent ? ["agent", "send-keys", pane.agent] : ["pane", "send-keys", pane.pane];
    const result = await this.#run(this.#bin, [...target, ...keys], { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr send-keys failed");
  }

  async read(pane: PaneHandle, opts: ReadOptions = {}): Promise<string> {
    const target = pane.agent ? ["agent", "read", pane.agent] : ["pane", "read", pane.pane];
    const args = [...target, "--source", opts.source ?? "recent-unwrapped", "--lines", String(opts.lines ?? 120)];
    const result = await this.#run(this.#bin, args, { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr read failed");
    return result.stdout;
  }

  async wait(pane: PaneHandle, opts: WaitOptions): Promise<WaitResult> {
    if (opts.state) {
      const target = pane.agent ?? pane.pane;
      const result = await this.#run(
        this.#bin,
        ["agent", "wait", target, "--until", opts.state, "--timeout", String(opts.timeoutMs)],
        { env: this.#env, timeoutMs: opts.timeoutMs + 5_000 },
      );
      return {
        matched: result.exitCode === 0,
        state: opts.state,
        timedOut: result.exitCode !== 0,
      };
    }
    if (opts.match) {
      const result = await this.#run(
        this.#bin,
        ["pane", "wait-output", pane.pane, "--match", opts.match, "--timeout", String(opts.timeoutMs)],
        { env: this.#env, timeoutMs: opts.timeoutMs + 5_000 },
      );
      return { matched: result.exitCode === 0, timedOut: result.exitCode !== 0, output: result.stdout };
    }
    throw new Error("herdr wait needs state or match");
  }

  async focus(pane: PaneHandle): Promise<void> {
    if (!pane.agent) return;
    const result = await this.#run(this.#bin, ["agent", "focus", pane.agent], { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr focus failed");
  }

  async close(pane: PaneHandle): Promise<void> {
    const result = await this.#run(this.#bin, ["pane", "close", pane.pane], { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr pane close failed");
  }

  async list(): Promise<PaneInfo[]> {
    const result = await this.#run(this.#bin, ["agent", "list"], { env: this.#env });
    if (result.exitCode !== 0) throw new Error(result.stderr.trim() || "herdr agent list failed");
    const payload = resultOf(parseJson(result.stdout));
    const agents = payload?.agents;
    if (!Array.isArray(agents)) return [];
    return agents
      .filter((a): a is Record<string, unknown> => typeof a === "object" && a !== null)
      .map((a) => ({ ...asHandle(a), status: typeof a.agent_status === "string" ? a.agent_status : undefined, cwd: typeof a.cwd === "string" ? a.cwd : undefined }));
  }
}
