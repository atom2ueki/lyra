/**
 * PaneBackend is a capability-negotiated protocol, not a herdr wrapper. Exactly one backend owns
 * panes for a run. Ghostty is a valid backend because it has native splits; when herdr runs inside
 * Ghostty, herdr is the pane authority and Ghostty is the host.
 */
export type SplitDirection = "right" | "left" | "down" | "up";
export type ReadSource = "visible" | "recent" | "recent-unwrapped" | "detection";

export interface PaneCapabilities {
  workspaces: boolean;
  tabs: boolean;
  splits: boolean;
  runCommand: boolean;
  startAgent: boolean;
  sendText: boolean;
  sendKeys: boolean;
  readOutput: boolean;
  waitForOutput: boolean;
  agentState: boolean;
  sessionIdentity: boolean;
  focus: boolean;
  resize: boolean;
  close: boolean;
}

export interface PaneHandle {
  backend: string;
  workspace?: string;
  tab?: string;
  pane: string;
  agent?: string;
  sessionFile?: string;
  machine?: string;
}

export interface PaneInfo extends PaneHandle {
  status?: string;
  cwd?: string;
}

export interface WaitResult {
  matched: boolean;
  state?: string;
  output?: string;
  timedOut?: boolean;
}

export interface SplitOptions {
  direction: SplitDirection;
  cwd: string;
  command?: string;
  env?: Record<string, string>;
  logDir?: string;
}

export interface StartAgentOptions {
  kind: string;
  args: string[];
  name?: string;
}

export interface ReadOptions {
  source?: ReadSource;
  lines?: number;
}

export interface WaitOptions {
  state?: string;
  match?: string;
  timeoutMs: number;
}

export interface PaneBackend {
  readonly name: string;
  readonly capabilities: PaneCapabilities;
  detect(): Promise<boolean>;
  anchor(): Promise<PaneHandle>;
  split(anchor: PaneHandle, opts: SplitOptions): Promise<PaneHandle>;
  newTab?(opts: { cwd: string; command?: string }): Promise<PaneHandle>;
  run?(pane: PaneHandle, command: string): Promise<void>;
  startAgent?(pane: PaneHandle, opts: StartAgentOptions): Promise<void>;
  sendText?(pane: PaneHandle, text: string, opts?: { submit?: boolean }): Promise<void>;
  sendKeys?(pane: PaneHandle, keys: string[]): Promise<void>;
  read?(pane: PaneHandle, opts?: ReadOptions): Promise<string>;
  wait?(pane: PaneHandle, opts: WaitOptions): Promise<WaitResult>;
  focus?(pane: PaneHandle): Promise<void>;
  resize?(pane: PaneHandle, ratio: number): Promise<void>;
  close?(pane: PaneHandle): Promise<void>;
  list?(): Promise<PaneInfo[]>;
}
