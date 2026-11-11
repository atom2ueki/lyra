import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { lyraHome } from "./config";
import { runCommand } from "./pane/exec";
import { isRecord } from "./type-guards";

export interface ModelInfo {
  provider: string;
  id: string;
  selector: string;
  contextWindow?: number;
  maxTokens?: number;
  reasoning?: boolean;
  input?: string[];
  cost?: Record<string, number>;
}

export interface UsageReport {
  provider: string;
  status?: string;
  limits?: Array<{
    id?: string;
    status?: string;
    amount?: { remainingFraction?: number; usedFraction?: number };
  }>;
}

export interface ModelStat {
  provider: string;
  model: string;
  totalRequests?: number;
  totalCost?: number;
  avgTtft?: number;
  avgTokensPerSecond?: number;
  errorRate?: number;
}

export interface ModelPolicy {
  classes: Record<string, string[]>;
  roles: Record<string, string[]>;
  limits: { providers: Record<string, number> };
}

export const DEFAULT_MODEL_POLICY = `classes:
  architecture:
    - astralform/deepseek-v4.1-flash:max
    - zai/glm-5.3:high
  implementation:
    - astralform/deepseek-v4.1-flash:high
    - xiaomi-token-plan-cn/mimo-v2.6-pro:high
  review:
    - zai/glm-5.3:high
    - astralform/deepseek-v4.1-flash:max
  search:
    - astralform/ornith-1.5-35b-a3b:auto
    - minimax-code-cn/MiniMax-M3.1-Flash:max
roles:
  smol:
    - astralform/ornith-1.5-35b-a3b:auto
    - minimax-code-cn/MiniMax-M3.1-Flash:max
  slow:
    - zai/glm-5.3:high
    - astralform/deepseek-v4.1-flash:max
  plan:
    - zai/glm-5.3:high
    - xiaomi-token-plan-cn/mimo-v2.6-pro:high
limits:
  providers:
    astralform: 2
`;

export interface ModelChoice {
  selector: string;
  provider: string;
  modelClass: string;
  reason: string;
}

const LEVEL_SUFFIX = /:(off|minimal|low|medium|high|xhigh|max|auto)$/i;

export function modelPolicyPath(): string {
  return join(lyraHome(), "models.policy.yaml");
}

function asCandidates(value: unknown): string[] {
  if (Array.isArray(value)) return value.filter((v): v is string => typeof v === "string");
  if (isRecord(value) && Array.isArray(value.candidates)) {
    return value.candidates.filter((v): v is string => typeof v === "string");
  }
  return [];
}

export function parseModelPolicy(raw: unknown): ModelPolicy {
  if (!isRecord(raw)) throw new Error("model policy must be a mapping");
  const classes: Record<string, string[]> = {};
  if (isRecord(raw.classes)) {
    for (const [name, value] of Object.entries(raw.classes)) {
      const candidates = asCandidates(value);
      if (candidates.length > 0) classes[name] = candidates;
    }
  }
  const roles: Record<string, string[]> = {};
  if (isRecord(raw.roles)) {
    for (const [name, value] of Object.entries(raw.roles)) {
      const candidates = asCandidates(value);
      if (candidates.length > 0) roles[name] = candidates;
    }
  }
  const providers: Record<string, number> = {};
  const limits = isRecord(raw.limits) ? raw.limits : {};
  if (isRecord(limits.providers)) {
    for (const [provider, value] of Object.entries(limits.providers)) {
      if (typeof value === "number" && Number.isInteger(value) && value >= 0) {
        providers[provider] = value;
      }
    }
  }
  return { classes, roles, limits: { providers } };
}

export async function ensureModelPolicy(): Promise<string> {
  const path = modelPolicyPath();
  await mkdir(dirname(path), { recursive: true });
  try {
    await readFile(path, "utf8");
  } catch {
    await writeFile(path, DEFAULT_MODEL_POLICY, { mode: 0o600 });
  }
  return path;
}

export async function loadModelPolicy(): Promise<ModelPolicy> {
  let text: string;
  try {
    text = await readFile(modelPolicyPath(), "utf8");
  } catch {
    text = DEFAULT_MODEL_POLICY;
  }
  return parseModelPolicy(Bun.YAML.parse(text));
}

export function modelKey(selector: string): string {
  return selector.replace(LEVEL_SUFFIX, "");
}

export function providerOf(selector: string): string {
  const key = modelKey(selector);
  const slash = key.indexOf("/");
  return slash === -1 ? key : key.slice(0, slash);
}

export function providerExhausted(usage: UsageReport[], provider: string): boolean {
  return usage
    .filter((report) => report.provider === provider)
    .some((report) =>
      (report.limits ?? []).some((limit) => {
        const status = limit.status ?? report.status;
        return (
          status === "exhausted" ||
          status === "blocked" ||
          status === "limit_reached" ||
          (limit.amount?.remainingFraction !== undefined && limit.amount.remainingFraction <= 0)
        );
      }),
    );
}

export function capFor(
  provider: string,
  policy: ModelPolicy,
  configCaps: Record<string, number>,
): number {
  const configured = configCaps[provider];
  const policyCap = policy.limits.providers[provider];
  const caps = [configured, policyCap].filter((v): v is number => v !== undefined);
  return caps.length ? Math.min(...caps) : Number.POSITIVE_INFINITY;
}

export function selectModel(args: {
  policy: ModelPolicy;
  models: ModelInfo[];
  usage?: UsageReport[];
  stats?: ModelStat[];
  modelClass?: string;
  ticketModel?: string;
  activeByProvider?: Record<string, number>;
  configCaps?: Record<string, number>;
}): ModelChoice | null {
  if (args.ticketModel) {
    return {
      selector: args.ticketModel,
      provider: providerOf(args.ticketModel),
      modelClass: args.modelClass ?? "ticket",
      reason: "explicit ticket model",
    };
  }
  const className = args.modelClass && args.policy.classes[args.modelClass]
    ? args.modelClass
    : args.policy.classes.default
      ? "default"
      : Object.keys(args.policy.classes)[0];
  if (!className) return null;
  const active = args.activeByProvider ?? {};
  const caps = args.configCaps ?? {};
  for (const candidate of args.policy.classes[className] ?? []) {
    const provider = providerOf(candidate);
    const key = modelKey(candidate);
    const known = args.models.some((m) => m.selector === key || `${m.provider}/${m.id}` === key);
    if (!known) continue;
    if ((active[provider] ?? 0) >= capFor(provider, args.policy, caps)) continue;
    if (providerExhausted(args.usage ?? [], provider)) continue;
    return {
      selector: candidate,
      provider,
      modelClass: className,
      reason: `policy class ${className}`,
    };
  }
  return null;
}

export function subagentRoles(policy: ModelPolicy, selected: string): Record<string, string> {
  const first = (name: string): string => policy.roles[name]?.[0] ?? selected;
  return {
    task: selected,
    smol: first("smol"),
    slow: first("slow"),
    plan: first("plan"),
  };
}

export function roleOverlay(
  policy: ModelPolicy,
  selected: string,
): { modelRoles: Record<string, string> } {
  return {
    modelRoles: {
      default: selected,
      ...subagentRoles(policy, selected),
    },
  };
}

export async function writeRoleOverlay(
  path: string,
  policy: ModelPolicy,
  selected: string,
): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, Bun.YAML.stringify(roleOverlay(policy, selected)), { mode: 0o600 });
}

function ompBin(): string {
  return process.env.LYRA_OMP_BIN?.trim() || "omp";
}

async function jsonCommand(args: string[]): Promise<unknown> {
  const result = await runCommand(ompBin(), args, { timeoutMs: 120_000 });
  if (result.exitCode !== 0) {
    throw new Error(`${ompBin()} ${args.join(" ")} failed: ${result.stderr.trim()}`);
  }
  const text = result.stdout.trim();
  const objectStart = text.indexOf("{");
  const arrayStart = text.indexOf("[");
  const start = [objectStart, arrayStart].filter((i) => i >= 0).sort((a, b) => a - b)[0];
  if (start === undefined) throw new Error(`${ompBin()} ${args.join(" ")} returned no JSON`);
  return JSON.parse(text.slice(start));
}

export async function loadModelCatalog(): Promise<ModelInfo[]> {
  const raw = await jsonCommand(["models", "ls", "--json"]);
  const list = Array.isArray(raw)
    ? raw
    : isRecord(raw) && Array.isArray(raw.models)
      ? raw.models
      : [];
  if (list.length === 0) throw new Error("omp models ls returned no models");
  return list.filter(isRecord).map((m) => ({
    provider: String(m.provider ?? ""),
    id: String(m.id ?? ""),
    selector: String(m.selector ?? `${m.provider}/${m.id}`),
    contextWindow: typeof m.contextWindow === "number" ? m.contextWindow : undefined,
    maxTokens: typeof m.maxTokens === "number" ? m.maxTokens : undefined,
    reasoning: typeof m.reasoning === "boolean" ? m.reasoning : undefined,
    input: Array.isArray(m.input) ? m.input.filter((v): v is string => typeof v === "string") : undefined,
    cost: isRecord(m.cost)
      ? Object.fromEntries(
          Object.entries(m.cost).filter(([, v]) => typeof v === "number") as Array<[string, number]>,
        )
      : undefined,
  }));
}

export async function loadUsage(): Promise<UsageReport[]> {
  const raw = await jsonCommand(["usage", "--json"]);
  if (!isRecord(raw) || !Array.isArray(raw.reports)) return [];
  return raw.reports.filter(isRecord).map((report) => ({
    provider: String(report.provider ?? ""),
    status: typeof report.status === "string" ? report.status : undefined,
    limits: Array.isArray(report.limits)
      ? report.limits.filter(isRecord).map((limit) => ({
          id: typeof limit.id === "string" ? limit.id : undefined,
          status: typeof limit.status === "string" ? limit.status : undefined,
          amount: isRecord(limit.amount)
            ? {
                remainingFraction:
                  typeof limit.amount.remainingFraction === "number"
                    ? limit.amount.remainingFraction
                    : undefined,
                usedFraction:
                  typeof limit.amount.usedFraction === "number"
                    ? limit.amount.usedFraction
                    : undefined,
              }
            : undefined,
        }))
      : undefined,
  }));
}

export async function loadStats(): Promise<ModelStat[]> {
  const raw = await jsonCommand(["stats", "--json"]);
  if (!isRecord(raw) || !Array.isArray(raw.byModel)) return [];
  return raw.byModel.filter(isRecord).map((stat) => ({
    provider: String(stat.provider ?? ""),
    model: String(stat.model ?? ""),
    totalRequests: typeof stat.totalRequests === "number" ? stat.totalRequests : undefined,
    totalCost: typeof stat.totalCost === "number" ? stat.totalCost : undefined,
    avgTtft: typeof stat.avgTtft === "number" ? stat.avgTtft : undefined,
    avgTokensPerSecond:
      typeof stat.avgTokensPerSecond === "number" ? stat.avgTokensPerSecond : undefined,
    errorRate: typeof stat.errorRate === "number" ? stat.errorRate : undefined,
  }));
}

export function activeProviders(stateTickets: Record<string, { status: string; model?: string }>): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const ticket of Object.values(stateTickets)) {
    if (ticket.status !== "running" || !ticket.model) continue;
    const provider = providerOf(ticket.model);
    counts[provider] = (counts[provider] ?? 0) + 1;
  }
  return counts;
}
