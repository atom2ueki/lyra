import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { join } from "node:path";
import { lyraHome } from "./config";

export function resolveProjectRoot(cwd = process.cwd()): string {
  try {
    return execFileSync("git", ["rev-parse", "--show-toplevel"], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    try {
      return realpathSync(cwd);
    } catch {
      return cwd;
    }
  }
}

export function projectIdFor(root: string): string {
  let canonical = root;
  try {
    canonical = realpathSync(root);
  } catch {
    // The path may not exist yet; hash the resolved string as given.
  }
  return createHash("sha256").update(canonical).digest("hex").slice(0, 12);
}

export function storeDirFor(root: string): string {
  return join(lyraHome(), "stores", projectIdFor(root));
}

export function runsDirFor(root: string): string {
  return join(lyraHome(), "runs", projectIdFor(root));
}
