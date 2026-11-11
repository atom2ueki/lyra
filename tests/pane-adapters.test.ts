import { describe, expect, test } from "bun:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CommandResult, CommandRunner } from "../src/pane/exec";
import { GhosttyBackend } from "../src/pane/ghostty";
import { HeadlessBackend } from "../src/pane/headless";

const ok = (stdout: string): CommandResult => ({ stdout, stderr: "", exitCode: 0 });

describe("GhosttyBackend", () => {
  test("anchors, splits, and submits text through osascript argv", async () => {
    const calls: Array<{ file: string; args: string[] }> = [];
    const run: CommandRunner = async (file, args) => {
      calls.push({ file, args });
      const script = args[1] ?? "";
      if (script.includes("frontmost")) return ok("g1");
      if (script.includes("split anchorTerm")) return ok("g2");
      return ok("1");
    };
    const backend = new GhosttyBackend({ env: { GHOSTTY_TERMINAL_ID: "g1" }, run });
    const anchor = await backend.anchor();
    expect(anchor.pane).toBe("g1");
    const pane = await backend.split(anchor, {
      direction: "right",
      cwd: "/tmp/wt",
      command: "omp --help",
    });
    expect(pane.pane).toBe("g2");
    expect(calls[0]?.args).toContain("g1");
    expect(calls[1]?.args).toContain("omp --help");
    await backend.sendText(pane, "hello", { submit: true });
    expect(calls[2]?.args.at(-2)).toBe("hello");
    expect(calls[2]?.args.at(-1)).toBe("1");
  });
});

describe("HeadlessBackend", () => {
  test("spawns a command, reads its log, and waits for output", async () => {
    const dir = await mkdtemp(join(tmpdir(), "lyra-headless-"));
    const backend = new HeadlessBackend({ env: {} });
    const anchor = await backend.anchor();
    const pane = await backend.split(anchor, {
      direction: "right",
      cwd: dir,
      command: "sh -c 'echo headless-ok'",
      logDir: dir,
    });
    const result = await backend.wait(pane, { match: "headless-ok", timeoutMs: 5_000 });
    expect(result.matched).toBe(true);
    expect(await backend.read(pane)).toContain("headless-ok");
    await backend.close(pane);
  });
});
