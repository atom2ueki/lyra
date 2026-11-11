import { describe, expect, test } from "bun:test";
import { HerdrBackend } from "../src/pane/herdr";
import type { CommandResult, CommandRunner } from "../src/pane/exec";

function runner(responses: Array<CommandResult | ((args: string[]) => CommandResult)>) {
  const calls: Array<{ file: string; args: string[] }> = [];
  const run: CommandRunner = async (file, args) => {
    calls.push({ file, args });
    const next = responses.shift();
    if (!next) throw new Error("unexpected command");
    return typeof next === "function" ? next(args) : next;
  };
  return { run, calls };
}

const ok = (stdout: string): CommandResult => ({ stdout, stderr: "", exitCode: 0 });

describe("HerdrBackend", () => {
  test("uses the inherited pane and starts an agent with args after --", async () => {
    const { run, calls } = runner([
      ok(JSON.stringify({ id: "x", result: { pane: { pane_id: "w1:p2", tab_id: "w1:t1" } } })),
      ok(JSON.stringify({ id: "y", result: {} })),
    ]);
    const backend = new HerdrBackend({ env: { HERDR_PANE_ID: "w1:p1" }, run });
    const anchor = await backend.anchor();
    expect(anchor.pane).toBe("w1:p1");
    const pane = await backend.split(anchor, { direction: "right", cwd: "/tmp/wt" });
    expect(pane.pane).toBe("w1:p2");
    await backend.startAgent(pane, { kind: "omp", name: "worker", args: ["--model", "x"] });
    expect(calls[0]?.args).toEqual([
      "pane", "split", "--pane", "w1:p1", "--direction", "right", "--cwd", "/tmp/wt", "--no-focus",
    ]);
    expect(calls[1]?.args).toEqual([
      "agent", "start", "worker", "--kind", "omp", "--pane", "w1:p2", "--", "--model", "x",
    ]);
  });

  test("maps agent list entries to pane handles", async () => {
    const { run } = runner([
      ok(JSON.stringify({
        result: {
          agents: [{
            pane_id: "w1:p2",
            tab_id: "w1:t1",
            agent: "omp",
            agent_status: "working",
            cwd: "/tmp/wt",
            agent_session: { value: "/sessions/x.jsonl" },
          }],
        },
      })),
    ]);
    const backend = new HerdrBackend({ env: {}, run });
    const panes = await backend.list();
    expect(panes[0]).toMatchObject({
      backend: "herdr",
      pane: "w1:p2",
      agent: "omp",
      status: "working",
      sessionFile: "/sessions/x.jsonl",
    });
  });
});
