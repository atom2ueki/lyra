import assert from "node:assert/strict";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import lyraOmpExtension from "../extension/lyra.mjs";

const chain = () => {
  const schema = {
    optional: () => schema,
    describe: () => schema,
    int: () => schema,
    positive: () => schema,
    min: () => schema,
    max: () => schema,
    nullable: () => schema,
  };
  return schema;
};

const fakeZod = {
  string: chain,
  number: chain,
  boolean: chain,
  enum: chain,
  array: chain,
  object: (shape) => {
    const schema = chain();
    schema.shape = shape;
    return schema;
  },
};

function fakePi() {
  const tools = new Map();
  const commands = new Map();
  const pi = {
    zod: fakeZod,
    setLabel() {},
    registerTool(definition) {
      tools.set(definition.name, definition);
    },
    registerCommand(name, definition) {
      commands.set(name, definition);
    },
  };
  return { pi, tools, commands };
}

async function mockCli() {
  const dir = await mkdtemp(join(tmpdir(), "lyra-omp-test-"));
  const bin = join(dir, "lyra");
  const log = join(dir, "argv.log");
  const stdin = join(dir, "stdin.log");
  const script = `#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$MOCK_LOG"
if [ "$1" = "--version" ]; then
  if [ "\${MOCK_VERSION:-0.2.0}" = "0.1.0" ]; then printf 'lyra 0.1.0\\n'; else printf 'lyra 0.2.0\\n'; fi
  exit 0
fi
if [ "\${MOCK_MODE:-ok}" = "malformed" ]; then printf 'not-json\\n'; exit 0; fi
if [ "\${MOCK_MODE:-ok}" = "error" ]; then
  printf '%s\\n' '{"ok":false,"error":{"code":"invalid_input","message":"bad graph","next":"inspect tickets"}}'
  exit 2
fi
case "$1 $2" in
  "status --json")
    printf '%s\\n' '{"schemaVersion":1,"projectId":"p","graph":{"total":1,"todo":1,"in_progress":0,"done":0,"failed":0,"ready":[{"id":"T-001","subject":"One","status":"todo","priority":2,"blockedBy":[],"ready":true,"attempts":0}],"blocked":[],"errors":[]},"run":null,"next":"start"}'
    ;;
  "tickets list")
    printf '%s\\n' '[{"id":"T-001","subject":"One","status":"todo","priority":2,"blockedBy":[],"ready":true,"attempts":0}]'
    ;;
  "tickets show")
    printf '%s\\n' '{"id":"T-001","subject":"One","prompt":"Do it","status":"todo","priority":2,"blockedBy":["T-000"],"attempts":1,"lastError":"previous"}'
    ;;
  "tickets create")
    cat > "$MOCK_STDIN"
    printf '%s\\n' '{"created":[{"key":"one","id":"T-001"}]}'
    ;;
  "conduct start")
    printf '%s\\n' '{"schemaVersion":1,"runId":"run-1","status":"running","revision":2,"backend":"headless","baseRef":"main","createdAt":"now","updatedAt":"now","plannedTickets":["T-001"],"tickets":[]}'
    ;;
  "tickets update")
    printf '%s\\n' '{"id":"T-001","subject":"New subject","prompt":"Do it better","status":"todo","priority":4,"blockedBy":["T-002"],"labels":["backend"],"attempts":0}'
    ;;
  "conduct wait")
    if [ "\${MOCK_WAIT_SLEEP:-0}" = "1" ]; then
      printf '%s' "$$" > "$MOCK_PID"
      exec sleep 30
    fi
    printf '%s\\n' '{"runId":"run-1","status":"running","revision":2,"timedOut":true,"tickets":[]}'
    ;;
  "conduct stop"|"conduct close")
    printf '%s\\n' '{"runId":"run-1","status":"stopped","revision":3,"tickets":[]}'
    ;;
  "conduct run")
    printf '%s\\n' '{"runId":"run-1","status":"running","revision":2,"tickets":[]}'
    ;;
  *)
    printf 'unknown mock command: %s\\n' "$*" >&2
    exit 3
    ;;
esac
`;
  await writeFile(bin, script);
  await chmod(bin, 0o755);
  return { dir, bin, log, stdin };
}

async function setup({ version = "0.2.0" } = {}) {
  const mock = await mockCli();
  process.env.LYRA_BIN = mock.bin;
  process.env.MOCK_LOG = mock.log;
  process.env.MOCK_STDIN = mock.stdin;
  process.env.MOCK_MODE = "ok";
  process.env.MOCK_VERSION = version;
  process.env.MOCK_WAIT_SLEEP = "0";
  process.env.MOCK_PID = join(mock.dir, "pid");
  const { pi, tools, commands } = fakePi();
  lyraOmpExtension(pi);
  return { ...mock, pi, tools, commands };
}

async function waitForPidFile(path, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return Number(await readFile(path, "utf8"));
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
  throw new Error(`mock child did not write ${path}`);
}

async function waitForExit(pid, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return false;
}

async function call(tool, params, { cwd, signal, onUpdate } = {}) {
  return tool.execute("call-1", params, signal, onUpdate ?? (() => {}), { cwd });
}

test("registers six typed tools and keeps /lyra as a human command", async () => {
  const s = await setup();
  try {
    assert.deepEqual([...s.tools.keys()].sort(), [
      "lyra_control",
      "lyra_start",
      "lyra_status",
      "lyra_tickets",
      "lyra_tickets_create",
      "lyra_wait",
    ]);
    assert.equal(s.tools.has("lyra_conduct"), false);
    assert.equal(s.commands.has("lyra"), true);
    for (const [name, tool] of s.tools) {
      assert.equal(Object.hasOwn(tool.parameters.shape, "args"), false);
      assert.equal(tool.approval, name === "lyra_status" ? "read" : "write");
    }
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("status returns a concise summary and structured details", async () => {
  const s = await setup();
  try {
    const result = await call(s.tools.get("lyra_status"), {}, { cwd: s.dir });
    assert.equal(result.isError, undefined);
    assert.match(result.content[0].text, /1 ready/);
    assert.equal(result.details.ok, true);
    assert.equal(result.details.next, "start");
    assert.equal(result.details.graph.ready[0].id, "T-001");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("status with an explicit run recomputes next for that run", async () => {
  const s = await setup();
  try {
    const result = await call(s.tools.get("lyra_status"), { runId: "run-1" });
    assert.equal(result.isError, undefined);
    assert.equal(result.details.next, "wait");
    assert.equal(result.details.run.runId, "run-1");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("ticket show reads camelCase ticket fields", async () => {
  const s = await setup();
  try {
    const result = await call(s.tools.get("lyra_tickets"), { action: "show", ticketId: "T-001" });
    assert.equal(result.isError, undefined);
    assert.match(result.content[0].text, /blocked by: T-000/);
    assert.match(result.content[0].text, /Last error: previous/);
    assert.equal(result.details.ticket.blockedBy[0], "T-000");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("ticket update sends exact CLI flags and returns details.ticket", async () => {
  const s = await setup();
  try {
    const result = await call(s.tools.get("lyra_tickets"), {
      action: "update",
      ticketId: "T-001",
      subject: "New subject",
      prompt: "Do it better",
      acceptance: null,
      priority: 4,
      blockedBy: ["T-002"],
      labels: ["backend"],
    });
    assert.equal(result.isError, undefined);
    assert.equal(result.details.action, "update");
    assert.equal(result.details.ticket.subject, "New subject");
    assert.equal(result.details.ticket.blockedBy[0], "T-002");
    assert.match(result.content[0].text, /Updated T-001 New subject \[todo\]/);
    const log = await readFile(s.log, "utf8");
    assert.match(
      log,
      /^tickets update T-001 --json --subject New subject --prompt Do it better --clear-acceptance --priority 4 --blocked-by T-002 --label backend$/m,
    );
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("ticket update validates ticketId and requires a writable field", async () => {
  const s = await setup();
  try {
    const missing = await call(s.tools.get("lyra_tickets"), { action: "update", subject: "x" });
    assert.equal(missing.isError, true);
    assert.equal(missing.details.code, "invalid_input");

    const empty = await call(s.tools.get("lyra_tickets"), { action: "update", ticketId: "T-001" });
    assert.equal(empty.isError, true);
    assert.equal(empty.details.code, "invalid_input");
    assert.equal(await readFile(s.log, "utf8").catch(() => ""), "", "must not reach the CLI");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("ticket creation sends JSON on stdin and maps created ids", async () => {
  const s = await setup();
  try {
    const result = await call(s.tools.get("lyra_tickets_create"), {
      tickets: [{ key: "one", subject: "One", prompt: "Do it" }],
    });
    assert.equal(result.isError, undefined);
    assert.deepEqual(result.details.created, [{ key: "one", id: "T-001" }]);
    const sent = JSON.parse(await readFile(s.stdin, "utf8"));
    assert.equal(sent.tickets[0].key, "one");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("wait returns timedOut without inventing a terminal run", async () => {
  const s = await setup();
  try {
    let updates = 0;
    const result = await call(
      s.tools.get("lyra_wait"),
      { runId: "run-1", timeoutMs: 50 },
      { onUpdate: () => { updates += 1; } },
    );
    assert.equal(result.isError, undefined);
    assert.equal(result.details.timedOut, true);
    assert.equal(result.details.status, "running");
    assert.equal(result.details.next, "wait");
    assert.ok(updates > 0);
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("aborting wait terminates the child process", async () => {
  const s = await setup();
  try {
    await call(s.tools.get("lyra_status"), {});
    process.env.MOCK_WAIT_SLEEP = "1";
    const controller = new AbortController();
    const pending = call(s.tools.get("lyra_wait"), { runId: "run-1", timeoutMs: 5000 }, {
      signal: controller.signal,
    });
    const pid = await waitForPidFile(process.env.MOCK_PID, 1_000);
    assert.ok(pid > 0);
    controller.abort();
    await assert.rejects(pending, (error) => error.code === "aborted");
    const exited = await waitForExit(pid, 1_000);
    assert.equal(exited, true, `mock child ${pid} was not terminated`);
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("CLI errors and malformed output become actionable tool failures", async () => {
  const s = await setup();
  try {
    process.env.MOCK_MODE = "error";
    const cliError = await call(s.tools.get("lyra_status"), {});
    assert.equal(cliError.isError, true);
    assert.equal(cliError.details.code, "invalid_input");
    assert.equal(cliError.details.next, "inspect tickets");

    process.env.MOCK_MODE = "malformed";
    const malformed = await call(s.tools.get("lyra_status"), {});
    assert.equal(malformed.isError, true);
    assert.equal(malformed.details.code, "invalid_output");

    process.env.MOCK_MODE = "ok";
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("rejects a CLI that predates the agent contract", async () => {
  const s = await setup({ version: "0.1.0" });
  try {
    const old = await call(s.tools.get("lyra_status"), {});
    assert.equal(old.isError, true);
    assert.equal(old.details.code, "cli_too_old");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});

test("start and control expose typed lifecycle calls", async () => {
  const s = await setup();
  try {
    const started = await call(s.tools.get("lyra_start"), {
      ticketIds: ["T-001"],
      dryRun: true,
      maxParallel: 2,
    });
    assert.equal(started.details.runId, "run-1");
    assert.equal(started.details.status, "running");
    const log = await readFile(s.log, "utf8");
    assert.match(log, /conduct start --json --concise --ticket T-001 --max-parallel 2 --dry-run/);

    const closed = await call(s.tools.get("lyra_control"), {
      runId: "run-1",
      action: "close",
    });
    assert.equal(closed.details.action, "close");
    assert.equal(closed.details.runId, "run-1");
  } finally {
    await rm(s.dir, { recursive: true, force: true });
  }
});
