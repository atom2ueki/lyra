import type { ApprovalPosture } from "./approval";
import type { PaneHandle } from "./pane-backend";

export const WORKER_TOOLS = ["lyra_progress", "lyra_discover", "lyra_finish"] as const;
export type WorkerTool = (typeof WORKER_TOOLS)[number];

export const WORKER_PHASES = [
  "investigating",
  "planning",
  "implementing",
  "verifying",
  "reviewing",
  "waiting_ci",
  "merging",
  "deploying",
] as const;
export type WorkerPhase = (typeof WORKER_PHASES)[number];

export type WorkerHarnessKind = "omp" | "codex" | "claude" | string;

export interface WorkerConfig {
  schemaVersion: 1;
  runId: string;
  claimId: string;
  ticketId: string;
  projectId: string;
  harness: WorkerHarnessKind;
  worktree: string;
  branch: string;
  baseHead: string;
  sessionDir: string;
  promptFile: string;
  approval: ApprovalPosture;
  model?: string;
  modelClass?: string;
  modelRoles?: Record<string, string>;
  budgetUsd?: number;
  maxSeconds?: number;
  continuation?: { head: string; baseHead: string; reason: string };
  channel: {
    root: string;
    requests: string;
    responses: string;
    state: string;
    result: string;
  };
}

export type WorkerResultStatus = "done" | "blocked" | "failed" | "needs_input";

export interface WorkerEvidence {
  command: string;
  exitCode: number;
  output: string;
}

export interface WorkerResult {
  schemaVersion: 1;
  ticketId: string;
  runId: string;
  claimId: string;
  status: WorkerResultStatus;
  summary?: string;
  verification?: string;
  evidence?: WorkerEvidence[];
  blockers?: string[];
  deployment?: unknown;
  finishedAt: string;
}

export interface WorkerLaunch {
  pane: PaneHandle;
  config: WorkerConfig;
  command: string;
  args: string[];
}

export interface WorkerAdapter {
  readonly kind: WorkerHarnessKind;
  buildLaunch(args: {
    name: string;
    config: WorkerConfig;
    modelArgs: string[];
    approvalArgs: string[];
  }): { command: string; args: string[] };
}
