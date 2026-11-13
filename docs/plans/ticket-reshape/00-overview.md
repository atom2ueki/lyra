# Ticket reshape — editable tickets for the supervised conductor

**Status:** PROPOSED 2026-11-14. Implementation has not started.

> **Pilot scope:** the first self-hosted run uses one vertical-slice ticket,
> `TicketReshape`, because Lyra does not yet have an integration/merge step for
> dependent worker branches. The four-ticket split below is the follow-up once a
> single worker branch has landed cleanly. Do not start the four-ticket graph on
> the first run.

## Problem

Lyra can create tickets but cannot change them. The lead-agent surface has
`lyra_tickets` (`list` / `show`), `lyra_tickets_create`, and no update operation;
the CLI has `lyra tickets list|show|create`. When a worker discovers a bad
dependency, or the lead gets new information after the graph is created, the only
options are to create replacement tickets or hand-edit `.lyra`, which the worker
contract forbids.

This is a conductor gap: the skill says “create or adjust follow-up work”, but only
create exists.

## Feature

Add one operation, **update an existing ticket**, to the CLI and the OMP adapter.

### CLI

```sh
lyra tickets update <ticket-id> \
  [--subject TEXT] \
  [--prompt TEXT] \
  [--acceptance TEXT | --clear-acceptance] \
  [--priority 0..4] \
  [--blocked-by ID]... [--clear-blocked-by] \
  [--label TEXT]... [--clear-labels] \
  --json
```

- Omitted fields stay unchanged.
- `--blocked-by` replaces the whole dependency list; repeat for multiple ids.
- `--label` replaces the whole label list; repeat for multiple labels.
- `--json` prints the updated ticket (the same shape as `lyra tickets show --json`).
- Existing `T-001` ids and new `AdjectiveNoun` ids are both valid.

### OMP adapter

Extend the existing write tool:

```json
{
  "action": "update",
  "ticketId": "QuietHarbor",
  "prompt": "new prompt",
  "acceptance": "new acceptance",
  "priority": 4,
  "blockedBy": ["SwiftFalcon"],
  "labels": ["backend"]
}
```

The tool returns the updated ticket in `details.ticket` and a short content summary.

## Safety rules

Only `todo` and `failed` tickets are editable. Reject:

- `in_progress` tickets (a worker may be writing the worktree)
- `done` tickets (history is immutable)
- tickets with a `suspension` (a checkpoint is awaiting resume)
- unknown or self-referential blockers
- a dependency cycle across the whole store
- an active run that holds the ticket `in_progress`

Validation reuses the full graph validator after the proposed mutation, not just a
local check. A failed update must leave the store byte-identical.

On success:

- set `updated_at`
- append a `ticket_updated` event with old/new field values (no secret content)
- never change `created_at`, `attempts`, `last_error`, `result`, or `execution`

No schema migration: all fields already exist in schema v1.

## Ticket graph

Four tickets, two at a time:

| Ticket | Work | Blocked by |
|---|---|---|
| `ReshapeCore` | store mutation primitive + full DAG validation + unit tests | — |
| `ReshapeCLI` | `lyra tickets update` clap surface, JSON output, integration tests | `ReshapeCore` |
| `ReshapeAdapter` | `lyra_tickets action=update`, fixture tests, tool error mapping | `ReshapeCore` |
| `ReshapeDocs` | README, `packages/omp/skills/lyra/references/cli-debug.md`, skill wording, deterministic eval | `ReshapeCLI`, `ReshapeAdapter` |

## Acceptance

```sh
lyra tickets update QuietHarbor --prompt "new prompt" --priority 4 --json
lyra tickets show QuietHarbor --json
# prompt and priority changed; status remains ready/todo unless dependencies changed

lyra tickets update QuietHarbor --blocked-by SwiftFalcon --json
lyra status --json
# QuietHarbor is blocked by SwiftFalcon

lyra tickets update QuietHarbor --blocked-by QuietHarbor --json
# exit 1, structured error, no write

lyra tickets update QuietHarbor --blocked-by Missing --json
# exit 1, unknown dependency, no write

lyra tickets update <done-ticket> --prompt "..." --json
# exit 1, ticket is immutable
```

Automated:

- Rust unit/integration tests for success, no-op, clear fields, cycle, unknown
  blocker, self blocker, in-progress refusal, done refusal, and store unchanged on
  error
- Node adapter test that asserts `update` reaches the CLI with the exact flags and
  returns `details.ticket`
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `npm test` in
  `packages/omp`, and `evals/agent-native-lyra/run.sh`
- A live Lyra run must create all four tickets, finish them, and close cleanly

## Guardrails

- Do not touch `~/Documents/atom2ueki/astralform/Astralform` from this work.
- Do not hand-edit `~/.lyra` tickets or state.
- Workers must not call the `lyra` CLI; only the lead session uses typed tools.
- No new scheduler, board UI, database, or schema migration.
- Keep ticket ids readable; do not reintroduce `T-###` for new tickets.

## Handoff

Start a fresh OMP session in this repository, then:

1. Read this file and `lyra_status`.
2. Present the four-ticket DAG to the user; do not create tickets until confirmed.
3. After confirmation, call `lyra_tickets_create` with the four tickets and the
   dependencies above.
4. Start with `maxParallel=2`, wait in bounded increments, and close the run only
   after all four tickets are settled.
