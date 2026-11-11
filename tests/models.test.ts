import { describe, expect, test } from "bun:test";
import {
  capFor,
  modelKey,
  parseModelPolicy,
  providerExhausted,
  providerOf,
  roleOverlay,
  selectModel,
  subagentRoles,
  type ModelInfo,
  type ModelPolicy,
  type UsageReport,
} from "../src/models";

const policy: ModelPolicy = parseModelPolicy({
  classes: {
    default: ["p/a:high", "p/b:high"],
    search: { candidates: ["p/c:auto"] },
  },
  roles: {
    smol: ["p/c:auto"],
    slow: ["p/b:high"],
  },
  limits: { providers: { p: 2 } },
});

const models: ModelInfo[] = [
  { provider: "p", id: "a", selector: "p/a" },
  { provider: "p", id: "b", selector: "p/b" },
  { provider: "p", id: "c", selector: "p/c" },
];

describe("model policy", () => {
  test("parses arrays and candidates objects", () => {
    expect(policy.classes.default).toEqual(["p/a:high", "p/b:high"]);
    expect(policy.classes.search).toEqual(["p/c:auto"]);
    expect(policy.roles.smol).toEqual(["p/c:auto"]);
    expect(policy.limits.providers.p).toBe(2);
  });

  test("strips effort suffixes and resolves providers", () => {
    expect(modelKey("p/a:max")).toBe("p/a");
    expect(modelKey("qwen3:8b")).toBe("qwen3:8b");
    expect(providerOf("p/a:max")).toBe("p");
  });

  test("selects the first available candidate", () => {
    const choice = selectModel({ policy, models, modelClass: "default" });
    expect(choice?.selector).toBe("p/a:high");
  });

  test("skips a capped provider and uses the next candidate", () => {
    const choice = selectModel({
      policy,
      models,
      modelClass: "default",
      activeByProvider: { p: 2 },
    });
    expect(choice).toBeNull();
    expect(capFor("p", policy, { p: 1 })).toBe(1);
  });

  test("skips an exhausted provider", () => {
    const usage: UsageReport[] = [
      { provider: "p", limits: [{ status: "exhausted", amount: { remainingFraction: 0 } }] },
    ];
    expect(providerExhausted(usage, "p")).toBe(true);
    expect(selectModel({ policy, models, usage, modelClass: "default" })).toBeNull();
  });

  test("explicit ticket model wins but remains a valid selector", () => {
    expect(selectModel({ policy, models, ticketModel: "p/b:max" })?.selector).toBe("p/b:max");
  });

  test("builds role overlays", () => {
    expect(subagentRoles(policy, "p/a:high")).toEqual({
      task: "p/a:high",
      smol: "p/c:auto",
      slow: "p/b:high",
      plan: "p/a:high",
    });
    expect(roleOverlay(policy, "p/a:high").modelRoles.default).toBe("p/a:high");
  });
});
