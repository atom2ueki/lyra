import { existsSync, realpathSync } from "node:fs";
import { mkdir, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { runCommand } from "./pane/exec";

async function git(repo: string, args: string[]): Promise<string> {
  const result = await runCommand("git", args, { cwd: repo, timeoutMs: 120_000 });
  if (result.exitCode !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr.trim() || result.stdout.trim()}`);
  }
  return result.stdout.trim();
}

export async function detectBaseRef(repo: string): Promise<string> {
  const candidates: string[] = [];
  try {
    const symbolic = await git(repo, ["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]);
    if (symbolic) candidates.push(symbolic);
  } catch {
    // No origin/HEAD; try the conventional names.
  }
  candidates.push("origin/main", "origin/master", "main", "master", "HEAD");
  for (const candidate of candidates) {
    try {
      await git(repo, ["rev-parse", "--verify", `${candidate}^{commit}`]);
      return candidate;
    } catch {
      // Try the next candidate.
    }
  }
  throw new Error(`cannot detect a base ref in ${repo}`);
}

export interface WorktreeInfo {
  dir: string;
  branch: string;
  baseRef: string;
  baseHead: string;
}

function safeSegment(value: string): string {
  return value.replace(/[^A-Za-z0-9_.-]/g, "-");
}

export async function createWorktree(args: {
  repo: string;
  runDir: string;
  runId: string;
  ticketId: string;
  baseRef?: string;
}): Promise<WorktreeInfo> {
  const baseRef = args.baseRef ?? (await detectBaseRef(args.repo));
  const dir = join(args.runDir, "worktrees", safeSegment(args.ticketId));
  const branch = `lyra/${safeSegment(args.runId)}/${safeSegment(args.ticketId)}`;
  if (existsSync(dir)) throw new Error(`worktree already exists: ${dir}`);
  await mkdir(join(args.runDir, "worktrees"), { recursive: true });
  await git(args.repo, ["worktree", "add", "-b", branch, dir, baseRef]);
  const realDir = realpathSync(dir);
  const baseHead = await git(realDir, ["rev-parse", "HEAD"]);
  return { dir: realDir, branch, baseRef, baseHead };
}

export async function resumeWorktree(args: {
  repo: string;
  dir: string;
  branch: string;
  baseHead: string;
}): Promise<WorktreeInfo> {
  if (!existsSync(args.dir)) throw new Error(`checkpoint worktree is gone: ${args.dir}`);
  const realDir = realpathSync(args.dir);
  const list = await git(args.repo, ["worktree", "list", "--porcelain"]);
  const registered = list
    .split("\n\n")
    .some((block) => block.includes(`worktree ${realDir}`) && block.includes(`branch refs/heads/${args.branch}`));
  if (!registered) throw new Error(`checkpoint is not a registered worktree on ${args.branch}: ${args.dir}`);
  const branch = await git(args.dir, ["rev-parse", "--abbrev-ref", "HEAD"]);
  if (branch !== args.branch) throw new Error(`checkpoint is on ${branch}, expected ${args.branch}`);
  try {
    await git(args.dir, ["merge-base", "--is-ancestor", args.baseHead, "HEAD"]);
  } catch {
    throw new Error(`checkpoint HEAD no longer descends from ${args.baseHead}`);
  }
  const head = await git(args.dir, ["rev-parse", "HEAD"]);
  return { dir: args.dir, branch: args.branch, baseRef: args.baseHead, baseHead: head };
}

export async function isWorktreeClean(dir: string): Promise<boolean> {
  if (!existsSync(dir)) return false;
  const status = await git(dir, ["status", "--porcelain"]);
  return status.length === 0;
}

export async function removeWorktree(repo: string, dir: string): Promise<{ removed: boolean; reason?: string }> {
  if (!existsSync(dir)) return { removed: false, reason: "worktree is already gone" };
  if (!(await isWorktreeClean(dir))) return { removed: false, reason: "worktree is dirty" };
  await git(repo, ["worktree", "remove", dir]);
  return { removed: true };
}

export async function closeRunWorktrees(
  repo: string,
  runDir: string,
  keep: string[] = [],
): Promise<Array<{ dir: string; removed: boolean; reason?: string }>> {
  const root = join(runDir, "worktrees");
  if (!existsSync(root)) return [];
  const entries = await readdir(root);
  const kept = new Set(keep);
  const results: Array<{ dir: string; removed: boolean; reason?: string }> = [];
  for (const entry of entries) {
    const dir = join(root, entry);
    const realDir = existsSync(dir) ? realpathSync(dir) : dir;
    if (kept.has(dir) || kept.has(realDir)) {
      results.push({ dir: realDir, removed: false, reason: "kept for a suspended ticket" });
      continue;
    }
    results.push({ dir, ...(await removeWorktree(repo, dir)) });
  }
  if (results.every((result) => result.removed)) {
    await rm(root, { recursive: true, force: true });
  }
  return results;
}
