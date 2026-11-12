# Phase 2 — Revisioned wait and non-destructive run control (data/CLI)

**Goal:** An agent can start a run, poll it with a bounded `wait`, recover from a timeout, and cancel cleanly — without a timeout changing the run's lifecycle state.

**Depends on:** 1.
**Ships behind:** no user-visible flag; new `wait`/revision fields are additive. Existing `monitor` behavior remains for human use.

## Background (verified)

- `monitor_run` is the only settling loop; it calls `process_requests`, `settle_worker`, and `launch_ready`, then saves state. `rust/src/conductor.rs:944-1003`.
- Its `timeout_ms` branch sets `state.status = RunStatus::Stopped` and returns. That is a lifecycle mutation, not a poll timeout. `rust/src/conductor.rs:994-997`.
- `save_state` only updates `updated_at`; there is no monotonic revision an agent can pass back. `rust/src/conductor.rs:182-196`.
- `RunState` has `created_at`/`updated_at` but no revision field. `rust/src/conductor.rs:79-110`.
- `start_run` accepts one `ticket_id` and otherwise auto-selects ready tickets. `rust/src/conductor.rs:592-611`.
- The OMP adapter currently spawns the CLI without a signal, so aborting a tool call cannot stop a monitor. `packages/omp/extension/lyra.mjs:9-27`.
- OMP custom tools pass an `AbortSignal` to `execute`; cancellation must propagate to the child and the run must remain recoverable. `omp://custom-tools.md`.

## Tasks

### 2.1 — Run revision (data)

- [ ] Add `revision: u64` to `RunState` with `#[serde(default)]` so old state files load as revision 0.
- [ ] Increment `revision` exactly once per successful `save_state` write, after `updated_at` is set.
- [ ] Do not increment on load or on a failed write.
- [ ] Include `revision` in full and concise run/status projections.
- [ ] Add a test that three saves produce revisions 1, 2, 3 and that loading an old JSON without `revision` yields 0.

### 2.2 — Non-destructive wait command (CLI)

- [ ] Add `lyra conduct wait --run <id> [--after-revision N] [--timeout-ms 30000] [--interval-ms 500] [--json] [--concise]`.
- [ ] Behavior:
  - Load the run and return immediately if `revision > after_revision`, or if the run is terminal/blocked and has no active tickets.
  - Otherwise perform one non-destructive monitor pass, save state, and repeat until the revision changes, a terminal/blocked state is reached, or the timeout expires.
  - On timeout, return the current state with `timed_out: true`; **never** change `RunStatus` to `Stopped`.
  - If no `--run` is provided, use the latest run.
- [ ] Refactor the monitor loop into a reusable one-pass helper (e.g. `monitor_once`) used by `wait` and `monitor`, so settling/launch behavior cannot drift between them.
- [ ] Keep human `monitor --timeout-ms` semantics unchanged (it may still stop the run for an interactive operator); document the difference in `--help`.
- [ ] Include `timed_out`, `revision`, and the concise run summary in `wait --json`.

### 2.3 — Multi-ticket start and control (CLI)

- [ ] Change `conduct start --ticket` to accept repeated values (`Vec<String>`); add `--tickets <id,id>` as a convenience.
- [ ] Resolve the requested ids against ready tickets; fail with a structured error naming the not-ready/unknown ids and their blockers.
- [ ] Preserve the current auto-select behavior when no explicit ids are supplied.
- [ ] Ensure `stop` and `close` return concise JSON with the run id and per-ticket status; `stop` remains the explicit lifecycle action.

### 2.4 — Cancellation contract (test/CLI boundary)

- [ ] Document the contract: the OMP tool passes `signal` to the CLI child; on abort the adapter sends `SIGTERM`, waits briefly, then `SIGKILL`; the run remains resumable because the command is only a poll/pass.
- [ ] Add a CLI-level test with a fake worker that remains running: `wait --timeout-ms 100` returns `timed_out: true` and the persisted run status is still `running`.
- [ ] Add a test that a second `wait --after-revision <old>` returns immediately without another monitor pass.
- [ ] Add a fake slow-worker test: after the worker writes a result, the next wait settles it and increments the revision.

## Acceptance

- `cd rust && cargo test wait_revision` passes.
- `cd rust && cargo test --test conductor` passes with the existing DAG/suspension tests.
- `lyra conduct wait --run <id> --timeout-ms 100 --json` on an active fake-worker run returns `timed_out:true` and `lyra report --run <id>` still reports `running`.
- `lyra conduct start --ticket T-002 --ticket T-004 --dry-run --json` reports the selected ready tickets; a not-ready id fails with a structured error naming its blockers.
- The OMP adapter abort test (implemented in phase 3) proves the child process exits on signal.
