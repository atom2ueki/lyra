# Phase 3 — Replace the CLI wrapper with typed OMP tools (adapter)

**Goal:** The lead OMP agent sees six domain tools with typed inputs, concise structured results, per-tool approval, cancellation, and next-action hints; no tool takes an arbitrary CLI argument array.

**Depends on:** 1, 2.
**Ships behind:** new tool names; the existing `lyra_conduct` tool is removed. `/lyra` remains a human/debug slash command.

## Background (verified)

- Current adapter registers `/lyra` and one `lyra_conduct` tool with `command` + `args: string[]`; it buffers output and returns only `{code}`. `packages/omp/extension/lyra.mjs:32-69`.
- The `execute` signature receives `signal` and `onUpdate`, but both are ignored. `packages/omp/extension/lyra.mjs:57`.
- OMP custom tools support structured `details`, `onUpdate`, `AbortSignal`, and per-tool approval tiers (`read`/`write`/`exec`). `omp://custom-tools.md`, `omp://approval-mode.md`.
- OMP's `task` tool is the reference shape: immediate spawn, bounded/background wait, structured details, signal propagation, and actionable errors. `omp://tools/task.md`.
- The plugin is in-process with no isolation; it must never own run state or perform long blocking work beyond a bounded wait. `~/.agents/plans/lyra-research.md` §3.1, R3.

## Tasks

### 3.1 — CLI transport module (adapter)

- [ ] Add `packages/omp/lib/lyra-cli.mjs`:
  - `runJson(args, { cwd, input, signal, timeoutMs })`: spawn `LYRA_BIN`/`lyra`, collect stdout/stderr, validate a JSON object, reject malformed output with a repair hint.
  - `runText(args, ...)` for the human `/lyra` command only.
  - `LyraCliError { code, message, next, stderr, exitCode }`.
  - Parse the Phase 1 error envelope when present; otherwise classify exit codes and include the last meaningful stderr line.
  - Abort handling: on `signal.aborted`, `SIGTERM`, then `SIGKILL` after a short grace period; always resolve/reject once.
  - A hard default timeout per call (e.g. 30s for start/wait, 10s for reads); caller may override but not exceed a cap.
- [ ] Resolve the binary once per session: `LYRA_BIN`, then `lyra` on PATH. On `ENOENT`, return an actionable error (“install with ... or set LYRA_BIN”).
- [ ] Check `lyra --version` lazily on first call; surface a clear “upgrade lyra” error if `--concise` is unavailable.

### 3.2 — Six tools (adapter)

Register these through `pi.registerTool` in `packages/omp/extension/lyra.mjs` (or imported modules):

| Tool | Approval | Input | Calls |
|---|---|---|---|
| `lyra_status` | `read` | `cwd?`, `runId?` | `lyra status --json --concise` (and run summary when `runId`) |
| `lyra_tickets` | `read` | `cwd?`, `action: list\|show`, `ticketId?` | `lyra tickets list/show --json` |
| `lyra_tickets_create` | `write` | `cwd?`, `tickets[]` | `lyra tickets create --json --file -` with JSON on stdin |
| `lyra_start` | `write` | `cwd?`, `ticketIds?`, `maxParallel?`, `maxSeconds?`, `base?`, `backend?`, `dryRun?` | `lyra conduct start --json --concise` |
| `lyra_wait` | `write` | `cwd?`, `runId?`, `afterRevision?`, `timeoutMs?` | `lyra conduct wait --json --concise` |
| `lyra_control` | `write` | `cwd?`, `runId?`, `action: stop\|close` | `lyra conduct stop/close --json --concise` |

- [ ] Tool names are `lyra_`-prefixed and unique; no generic `args` array.
- [ ] Each tool description states when to call it, what it returns, and the approval/ownership boundary. Do not duplicate the full CLI help in every description.
- [ ] `lyra_tickets_create` schema mirrors Phase 1 batch input (`key`, `subject`, `prompt`, `blockedBy`, optional fields); reject duplicate keys client-side with an actionable error.
- [ ] `lyra_start` never blocks; it returns `runId`, started tickets, and `next`.
- [ ] `lyra_wait` is bounded by the CLI and returns `timed_out`; it never calls destructive `monitor --timeout`.
- [ ] `lyra_control` requires an explicit `action` and `runId` when more than one run exists.
- [ ] Remove the old `lyra_conduct` registration. Keep `/lyra` for humans; document that it is not the agent path.

### 3.3 — Result and error contract (adapter)

- [ ] Return `content` as a short Markdown summary (≤ ~12 lines): run id/status, changed tickets, blockers/needs-input, and `next`.
- [ ] Return deterministic `details`:
  ```ts
  { ok: true, action, projectId?, runId?, revision?, status?, tickets?, next?, timedOut? }
  ```
  Preserve the CLI's `schemaVersion`; do not dump the full raw state.
- [ ] On failure return `isError: true`, a concise message, and details with `code`, `next`, `exitCode`, `stderrTail`.
- [ ] `needs_input` results include the exact human question and the affected ticket id; do not synthesize an answer.
- [ ] Use `onUpdate` for wait iterations (“waiting for run ..., revision ...”) and start acknowledgment.
- [ ] Never expose model catalog/policy/usage unless a caller explicitly requests full debug output (not part of the six tools).

### 3.4 — Tests (adapter)

- [ ] Add `packages/omp/test/agent-tools.test.mjs` using `node:test`.
- [ ] Create a mock `lyra` executable in a temp dir that:
  - responds to `--version`
  - emits concise JSON for status/start/wait/control
  - records argv/stdin to a file
  - sleeps for a wait-cancel test
- [ ] Assert:
  - no tool accepts `args`
  - read tools set `approval: "read"`; write tools set `"write"`
  - `lyra_tickets_create` sends JSON on stdin and maps the response
  - `lyra_wait` returns `timed_out` without marking a run stopped
  - aborting `lyra_wait` terminates the mock child
  - malformed JSON/CLI errors become actionable `isError` results
- [ ] Add `scripts.test` to `packages/omp/package.json` and run `node --test`.

### 3.5 — Boundary guardrails (adapter)

- [ ] Worker extension remains unchanged in this phase; lead tools are not available in worker sessions (the worker config/extension path is separate).
- [ ] No tool reads or writes `.lyra` files directly; all state actions go through the CLI.
- [ ] Reject `cwd` paths outside the session's approved root only when the CLI/harness policy requires it; otherwise pass through and let the CLI resolve the project.
- [ ] Document that `/lyra` is a human escape hatch and must not be used by the model when a typed tool exists.

## Acceptance

- `cd packages/omp && npm test` (or `node --test`) passes.
- A mock-CLI tool test proves `lyra_wait` times out with the run still active and that abort kills the child.
- The old generic `lyra_conduct` tool is absent from the registered tool list; `/lyra` still registers.
- All six tools have typed schemas, approval tiers, and `details`; none accepts `args: string[]`.
- A transcript of an agent using the tools shows no direct bash call to `lyra` and no ticket-store edits.
