# Phase 1 — CLI agent contract and ticket authoring (data/CLI)

**Goal:** The CLI exposes a concise, typed view of the graph and a batch ticket-authoring path, so an agent can understand and create a DAG without parsing human output or shelling out once per ticket.

**Depends on:** nothing.
**Ships behind:** no user-visible flag; all additions are new JSON projections/subcommands. Existing human output and full JSON remain available.

## Background (verified)

- `lyra status --json` currently returns only counts, ready ids, blocked ids, and graph errors; it has no ticket subject/prompt. `rust/src/cli.rs:342-386`.
- `Ticket` already carries everything the agent needs — id, subject, prompt, status, `blocked_by`, priority, labels, model class, acceptance, plan, parent, origin, harness, attempts, last_error, result. `rust/src/schema.rs:96-162`.
- `create_ticket` allocates the next id and writes one ticket; there is no batch API or temporary key map. `rust/src/store.rs:120-133`.
- `lyra add` accepts one free-text string and produces one ticket with no dependencies. `rust/src/cli.rs:69-75,774-798`.
- `RunState` serialization includes `modelPolicy`, `modelCatalog`, and `usage`, which makes run JSON unsuitable as a default agent response. `rust/src/conductor.rs:630-653`, `rust/src/cli.rs:263-303`.
- `run_dir_for`/`latest_run_dir` already resolve project-scoped runs, so a status projection can include the latest run without new storage. `rust/src/conductor.rs:142-166`.
- Product decisions: the CLI remains the source of truth, and the first agent path is ticket authoring plus inspection (overview §5).

## Tasks

### 1.1 — Agent-facing projections (data, CLI)

- [ ] Add `rust/src/agent.rs` with serializable DTOs:
  - `AgentStatus { schema_version, project_id, graph, run, next }`
  - `GraphSummary { total, todo, in_progress, done, failed, ready, blocked, errors }`
  - `TicketSummary { id, subject, status, priority, blocked_by, ready }`
  - `BlockedTicket { id, subject, waiting_on }`
  - `RunSummary { run_id, status, backend, base_ref, created_at, updated_at, tickets }`
  - `RunTicketSummary { id, status, model, phase, summary, needs_input, blockers }`
- [ ] Add `--concise` to `lyra status`, `lyra conduct start`, `lyra conduct monitor`, and `lyra conduct report`; when `--json --concise` is present, serialize the DTO instead of the full raw state.
- [ ] `status --json --concise` must include ticket subjects and readiness, the latest run summary when present, and a computed `next` value:
  - `start` when ready tickets exist and no run is active
  - `wait` when a run has active tickets
  - `resolve_input` when any run ticket has `needs_input`
  - `done` when no open tickets remain
  - `blocked` when open tickets remain but none are ready
- [ ] Ensure concise run/status JSON contains **no** `modelPolicy`, `modelCatalog`, or `usage` fields; full JSON remains the default when `--concise` is absent.
- [ ] Add `lyra tickets list --json` and `lyra tickets show <id> --json`.
  - `list` uses `TicketSummary` plus a `details` flag for `last_error`/`attempts`.
  - `show` returns the full `Ticket` JSON, including prompt, acceptance, plan, execution, suspension, and result.
- [ ] Add a stable agent error envelope for the new `tickets` subcommands and `--concise` paths:
  ```json
  {"ok":false,"error":{"code":"unknown_ticket","message":"...","next":"..."}}
  ```
  Preserve non-zero exit status; do not change existing human error output.

### 1.2 — Batch ticket creation (data, CLI)

- [ ] Add `lyra tickets create --file <path|->` (also accept `--json` for output).
  - Input:
    ```json
    {"tickets":[
      {"key":"core","subject":"Core","prompt":"...","blockedBy":[]},
      {"key":"ui","subject":"UI","prompt":"...","blockedBy":["core"],"priority":2,
       "labels":["ui"],"modelClass":"implementation","acceptance":"...","plan":"...","harness":"omp"}
    ]}
    ```
  - `key` is required and unique within the batch; `subject` and `prompt` are required; `blockedBy` references keys in the same batch or existing ticket ids.
  - Validate all keys/references and cycles before writing anything.
  - Create all tickets with generated ids, then rewrite `blocked_by` with the mapped ids in a second pass; never write a ticket with an unresolved temporary key.
  - Output `{ "created": [{"key":"core","id":"T-001"}, ...] }`.
- [ ] Keep `lyra add` for humans; implement it by delegating to the same single-ticket creation path so both produce identical defaults.
- [ ] Add `#[serde(deny_unknown_fields)]` to the batch input types and reject duplicate keys with an actionable error.
- [ ] Ensure batch creation is atomic at the logical level: validation errors create zero tickets; a later I/O failure reports exactly which tickets were written.

### 1.3 — Contract tests (test)

- [ ] Add `rust/tests/agent_contract.rs` with a temp git repo/store:
  - creates a 4-ticket DAG via `tickets create --file -`
  - asserts key → id mapping and `blockedBy` references
  - asserts cycle/unknown-key batches create zero tickets
  - asserts `status --json --concise` includes subjects and the expected ready/blocked split
  - asserts `conduct start --dry-run --json --concise` has no catalog/policy/usage fields
  - asserts `tickets show` includes prompt/acceptance
- [ ] Keep existing `status --json` compatibility tests; add a full-output assertion that the default still includes the old counts.
- [ ] Add unit tests for the `next` computation with active, blocked, needs-input, and done states.

## Acceptance

- `cd rust && cargo test --test agent_contract` passes.
- `cd rust && cargo test` passes.
- `cd rust && cargo clippy --all-targets -- -D warnings` passes.
- In a temp repo, `lyra tickets create --file - --json` creates the four-ticket DAG, and `lyra status --json --concise` reports the expected ready/blocked ids and subjects.
- The same command with a cycle or unknown blocker exits non-zero, creates zero ticket files, and returns a JSON error envelope.
- `lyra conduct start --dry-run --json --concise` output contains no `modelPolicy`, `modelCatalog`, or `usage` keys.
