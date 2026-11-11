import type {
  PaneBackend,
  PaneCapabilities,
  PaneHandle,
  ReadOptions,
  SplitOptions,
  StartAgentOptions,
  WaitOptions,
  WaitResult,
} from "../pane-backend";
import { runCommand, type CommandRunner } from "./exec";

const CAPABILITIES: PaneCapabilities = {
  workspaces: false,
  tabs: false,
  splits: true,
  runCommand: true,
  startAgent: false,
  sendText: true,
  sendKeys: true,
  readOutput: false,
  waitForOutput: false,
  agentState: false,
  sessionIdentity: false,
  focus: true,
  resize: false,
  close: true,
};

const ANCHOR = `
on run argv
  set requestedId to item 1 of argv
  tell application "Ghostty"
    if requestedId is not "" then
      repeat with t in terminals
        if (id of contents of t) is requestedId then return id of contents of t
      end repeat
      error "ghostty terminal not found: " & requestedId
    end if
    if frontmost is false then error "Ghostty is not frontmost; focus the calling window and retry"
    return id of (focused terminal of selected tab of front window)
  end tell
end run
`;

const SPLIT = `
on run argv
  set anchorId to item 1 of argv
  set paneCwd to item 2 of argv
  set paneCommand to item 3 of argv
  set paneDirection to item 4 of argv
  tell application "Ghostty"
    set anchorTerm to missing value
    repeat with t in terminals
      if (id of contents of t) is anchorId then
        set anchorTerm to contents of t
        exit repeat
      end if
    end repeat
    if anchorTerm is missing value then error "ghostty anchor terminal not found: " & anchorId
    set cfg to new surface configuration
    set initial working directory of cfg to paneCwd
    if paneCommand is not "" then set command of cfg to paneCommand
    set wait after command of cfg to false
    if paneDirection is "right" then
      set newTerm to split anchorTerm direction right with configuration cfg
    else if paneDirection is "left" then
      set newTerm to split anchorTerm direction left with configuration cfg
    else if paneDirection is "down" then
      set newTerm to split anchorTerm direction down with configuration cfg
    else if paneDirection is "up" then
      set newTerm to split anchorTerm direction up with configuration cfg
    else
      error "unsupported split direction: " & paneDirection
    end if
    return id of newTerm
  end tell
end run
`;

const INPUT = `
on run argv
  set targetId to item 1 of argv
  set inputText to item 2 of argv
  set submitIt to item 3 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        set targetTerm to contents of t
        input text inputText to targetTerm
        if submitIt is "1" then send key "enter" to targetTerm
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
`;

const KEY = `
on run argv
  set targetId to item 1 of argv
  set keyName to item 2 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        send key keyName to (contents of t)
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
`;

const CLOSE = `
on run argv
  set targetId to item 1 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        close (contents of t)
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
`;

const FOCUS = `
on run argv
  set targetId to item 1 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        focus (contents of t)
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
`;

interface GhosttyOptions {
  env?: NodeJS.ProcessEnv;
  run?: CommandRunner;
  osascriptBin?: string;
}

export class GhosttyBackend implements PaneBackend {
  readonly name = "ghostty";
  readonly capabilities = CAPABILITIES;
  readonly #env: NodeJS.ProcessEnv;
  readonly #run: CommandRunner;
  readonly #osascript: string;

  constructor(options: GhosttyOptions = {}) {
    this.#env = options.env ?? process.env;
    this.#run = options.run ?? runCommand;
    this.#osascript = options.osascriptBin ?? "osascript";
  }

  async detect(): Promise<boolean> {
    return this.#env.TERM_PROGRAM === "ghostty" || Boolean(this.#env.GHOSTTY_TERMINAL_ID);
  }

  async #script(label: string, script: string, argv: string[]): Promise<string> {
    const result = await this.#run(this.#osascript, ["-e", script, "--", ...argv], {
      env: this.#env,
      timeoutMs: 120_000,
    });
    if (result.exitCode !== 0) {
      throw new Error(`ghostty ${label}: ${result.stderr.trim() || result.stdout.trim()}`);
    }
    return result.stdout.trim();
  }

  async anchor(): Promise<PaneHandle> {
    const inherited = this.#env.GHOSTTY_TERMINAL_ID?.trim() ?? "";
    const id = await this.#script("anchor", ANCHOR, [inherited]);
    return { backend: this.name, pane: id };
  }

  async split(anchor: PaneHandle, opts: SplitOptions): Promise<PaneHandle> {
    const id = await this.#script("split", SPLIT, [
      anchor.pane,
      opts.cwd,
      opts.command ?? "",
      opts.direction,
    ]);
    if (!id) throw new Error("ghostty split returned no terminal id");
    return { backend: this.name, pane: id };
  }

  async sendText(pane: PaneHandle, text: string, opts: { submit?: boolean } = {}): Promise<void> {
    const ok = await this.#script("input", INPUT, [pane.pane, text, opts.submit === false ? "0" : "1"]);
    if (ok !== "1") throw new Error("ghostty input failed: pane not found");
  }

  async sendKeys(pane: PaneHandle, keys: string[]): Promise<void> {
    for (const key of keys) {
      const mapped = key === "enter" ? "enter" : key === "esc" ? "escape" : key;
      const ok = await this.#script("key", KEY, [pane.pane, mapped]);
      if (ok !== "1") throw new Error("ghostty key failed: pane not found");
    }
  }

  async focus(pane: PaneHandle): Promise<void> {
    const ok = await this.#script("focus", FOCUS, [pane.pane]);
    if (ok !== "1") throw new Error("ghostty focus failed: pane not found");
  }

  async close(pane: PaneHandle): Promise<void> {
    const ok = await this.#script("close", CLOSE, [pane.pane]);
    if (ok !== "1") throw new Error("ghostty close failed: pane not found");
  }

  async read(_pane: PaneHandle, _opts?: ReadOptions): Promise<string> {
    throw new Error("ghostty backend cannot read pane output; use RESULT.md/session JSONL");
  }

  async wait(_pane: PaneHandle, _opts: WaitOptions): Promise<WaitResult> {
    throw new Error("ghostty backend cannot wait on output; use RESULT.md/session state");
  }

  async startAgent(_pane: PaneHandle, _opts: StartAgentOptions): Promise<void> {
    throw new Error("ghostty backend cannot start agents; split with a command instead");
  }
}
