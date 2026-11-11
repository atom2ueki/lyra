import { describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { importOmpKanban } from "../src/import/omp-kanban";
import { listTickets } from "../src/store";

const created = "2026-01-01T00:00:00.000Z";

async function makeSource(): Promise<string> {
  const source = await mkdtemp(join(tmpdir(), "lyra-omp-source-"));
  await mkdir(join(source, "tasks"), { recursive: true });
  await mkdir(join(source, "attachments", "T-001"), { recursive: true });
  await writeFile(join(source, "attachments", "T-001", "shot.txt"), "image-bytes");
  await writeFile(join(source, "events.jsonl"), JSON.stringify({ seq: 1, event: "task.created" }) + "\n");
  await writeFile(
    join(source, "tasks", "T-001.json"),
    JSON.stringify({
      id: "T-001",
      subject: "First",
      prompt: "Do the first thing",
      status: "done",
      blockedBy: [],
      labels: ["test"],
      priority: 1,
      attempts: 1,
      lastError: null,
      result: "ok",
      held: true,
      legacyField: { keep: true },
      createdAt: created,
      updatedAt: created,
      startedAt: created,
      finishedAt: created,
    }) + "\n",
  );
  await writeFile(
    join(source, "tasks", "T-002.json"),
    JSON.stringify({
      id: "T-002",
      subject: "Second",
      prompt: "Do the second thing",
      status: "todo",
      blockedBy: ["T-001"],
      priority: 2,
      attempts: 0,
      lastError: null,
      result: null,
      execution: {
        runId: "run-1",
        claimId: "claim-1",
        claimedAt: created,
        phase: "implementing",
        pendingBlockers: [],
      },
      createdAt: created,
      updatedAt: created,
      startedAt: null,
      finishedAt: null,
    }) + "\n",
  );
  return source;
}

describe("importOmpKanban", () => {
  test("maps tickets, preserves unknown donor fields, and copies attachments/events", async () => {
    const source = await makeSource();
    const target = await mkdtemp(join(tmpdir(), "lyra-target-"));
    const report = await importOmpKanban(source, target, "proj-test");

    expect(report.imported).toBe(2);
    expect(report.errors).toEqual([]);
    expect(report.validationErrors).toEqual([]);
    expect(report.attachmentsCopied).toBe(true);
    expect(report.eventsCopied).toBe(true);

    const tickets = await listTickets(target);
    const first = tickets.find((t) => t.id === "T-001");
    const second = tickets.find((t) => t.id === "T-002");
    expect(first?.status).toBe("done");
    expect(first?.priority).toBe(1);
    expect(first?.labels).toEqual(["test"]);
    expect(first?.extra).toEqual({ ompKanban: { held: true, legacyField: { keep: true } } });
    expect(second?.blockedBy).toEqual(["T-001"]);
    expect(second?.execution?.claimId).toBe("claim-1");
    expect(second?.execution?.phase).toBe("implementing");

    expect(await readFile(join(target, "attachments", "T-001", "shot.txt"), "utf8")).toBe("image-bytes");
    expect(await readFile(join(target, "events.imported.jsonl"), "utf8")).toContain("task.created");
  });

  test("reports a per-file error without aborting the rest", async () => {
    const source = await makeSource();
    await writeFile(join(source, "tasks", "broken.json"), "{not json");
    const target = await mkdtemp(join(tmpdir(), "lyra-target-"));
    const report = await importOmpKanban(source, target, "proj-test");
    expect(report.imported).toBe(2);
    expect(report.errors.length).toBe(1);
    expect(report.errors[0]?.file.endsWith("broken.json")).toBe(true);
  });
});
