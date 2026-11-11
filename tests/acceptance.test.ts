import { describe, expect, test } from "bun:test";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { validateAcceptancePaths } from "../src/acceptance";
import { normalizeTicket, type Ticket } from "../src/schema";

const created = "2026-01-01T00:00:00.000Z";

function ticket(id: string, acceptance?: string): Ticket {
  return normalizeTicket({
    schemaVersion: 1,
    id,
    projectId: "proj-test",
    subject: id,
    prompt: id,
    status: "todo",
    blockedBy: [],
    priority: 2,
    labels: [],
    acceptance,
    attempts: 0,
    lastError: null,
    result: null,
    execution: null,
    discoveries: [],
    suspension: null,
    checkpoint: null,
    createdAt: created,
    updatedAt: created,
    startedAt: null,
    finishedAt: null,
  });
}

describe("validateAcceptancePaths", () => {
  test("accepts an existing doc path and ignores prose", async () => {
    const root = await mkdtemp(join(tmpdir(), "lyra-accept-"));
    await mkdir(join(root, "docs"), { recursive: true });
    await writeFile(join(root, "docs", "a.md"), "# A\n");
    expect(validateAcceptancePaths([ticket("T-001", "docs/a.md#acceptance")], root)).toEqual([]);
    expect(validateAcceptancePaths([ticket("T-002", "all checks pass")], root)).toEqual([]);
  });

  test("rejects a missing doc path", async () => {
    const root = await mkdtemp(join(tmpdir(), "lyra-accept-"));
    expect(validateAcceptancePaths([ticket("T-001", "docs/missing.md")], root)).toEqual([
      "T-001.acceptance: file not found: docs/missing.md",
    ]);
  });
});
