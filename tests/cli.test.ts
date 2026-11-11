import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const created = "2026-01-01T00:00:00.000Z";
const cli = join(import.meta.dir, "..", "src", "cli.ts");

async function makeSource(): Promise<string> {
  const source = await mkdtemp(join(tmpdir(), "lyra-cli-source-"));
  await mkdir(join(source, "tasks"), { recursive: true });
  await writeFile(
    join(source, "tasks", "T-001.json"),
    JSON.stringify({
      id: "T-001",
      subject: "CLI ticket",
      prompt: "Do the CLI thing",
      status: "todo",
      blockedBy: [],
      priority: 2,
      attempts: 0,
      lastError: null,
      result: null,
      createdAt: created,
      updatedAt: created,
      startedAt: null,
      finishedAt: null,
    }) + "\n",
  );
  return source;
}

function run(args: string[], env: Record<string, string>) {
  return Bun.spawnSync({
    cmd: [process.execPath, "run", cli, ...args],
    env: { ...process.env, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
}

describe("lyra CLI", () => {
  test("imports a store and reports status as JSON", async () => {
    const source = await makeSource();
    const home = await mkdtemp(join(tmpdir(), "lyra-home-"));
    const store = join(home, "store");
    const env = { LYRA_HOME: home };

    const imported = run(
      ["import", "omp-kanban", source, "--store", store, "--project-id", "proj-test", "--json"],
      env,
    );
    expect(imported.exitCode).toBe(0);
    const report = JSON.parse(imported.stdout.toString());
    expect(report.imported).toBe(1);

    const status = run(["conduct", "status", "--store", store, "--json"], env);
    expect(status.exitCode).toBe(0);
    const payload = JSON.parse(status.stdout.toString());
    expect(payload.counts.todo).toBe(1);
    expect(payload.ready).toEqual(["T-001"]);
    expect(payload.errors).toEqual([]);

    const graph = run(["graph", "--store", store, "--format", "mermaid"], env);
    expect(graph.exitCode).toBe(0);
    expect(graph.stdout.toString()).toContain("flowchart TD");
    expect(graph.stdout.toString()).toContain("T-001");
  });
});
