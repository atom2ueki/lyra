import { homedir } from "node:os";
import { join } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { isApprovalPosture, type ApprovalPosture } from "./approval";

export const DEFAULT_CONFIG = `# Lyra user config
max_parallel = 2
approval_posture = "ask"
merge = "auto"

[max_per_provider]
astralform = 2
`;

export interface LyraConfig {
  maxParallel: number;
  maxPerProvider: Record<string, number>;
  approvalPosture: ApprovalPosture;
  merge: "auto" | "manual";
}

export function lyraHome(): string {
  return process.env.LYRA_HOME?.trim() || join(homedir(), ".lyra");
}

export function configPath(): string {
  return join(lyraHome(), "config.toml");
}

export async function ensureConfig(): Promise<string> {
  const path = configPath();
  await mkdir(lyraHome(), { recursive: true });
  try {
    await readFile(path, "utf8");
  } catch {
    await writeFile(path, DEFAULT_CONFIG, { mode: 0o600 });
  }
  return path;
}

export async function loadConfig(): Promise<LyraConfig> {
  let text: string;
  try {
    text = await readFile(configPath(), "utf8");
  } catch {
    text = DEFAULT_CONFIG;
  }
  const parsed = Bun.TOML.parse(text) as Record<string, unknown>;
  const maxParallel = typeof parsed.max_parallel === "number" ? parsed.max_parallel : 2;
  if (!Number.isInteger(maxParallel) || maxParallel < 1) {
    throw new Error(`${configPath()}: max_parallel must be a positive integer`);
  }
  const approval = parsed.approval_posture ?? "ask";
  if (!isApprovalPosture(approval)) {
    throw new Error(`${configPath()}: approval_posture must be safe|ask|unsafe`);
  }
  const merge = parsed.merge === "manual" ? "manual" : "auto";
  const providers = parsed.max_per_provider;
  const maxPerProvider: Record<string, number> = {};
  if (providers !== undefined) {
    if (typeof providers !== "object" || providers === null || Array.isArray(providers)) {
      throw new Error(`${configPath()}: max_per_provider must be a table`);
    }
    for (const [key, value] of Object.entries(providers)) {
      if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
        throw new Error(`${configPath()}: max_per_provider.${key} must be a non-negative integer`);
      }
      maxPerProvider[key] = value;
    }
  }
  return { maxParallel, maxPerProvider, approvalPosture: approval, merge };
}
