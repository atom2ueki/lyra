---
name: lyra
description: "Run the Lyra task conductor from an OMP session: status, start, monitor, resume, report, stop, close, graph, models, doctor, upgrade. Use when the user asks to run or inspect a Lyra run, or to hand work to independent worker sessions. Lyra owns the task graph; do not edit ticket files directly."
---

# Lyra

Lyra is the supervised task conductor. The OMP adapter only exposes the CLI; the `lyra` binary owns
the ticket store, worktrees, workers, and state.

## Commands

```text
/lyra status                 # validate the task graph
/lyra start --max 2          # claim ready tickets and launch workers
/lyra monitor --once         # settle results and start the next ready ticket
/lyra resume                 # resume a blocked run after blockers finish
/lyra report                 # run summary and per-ticket state
/lyra stop                   # stop the current run's workers
/lyra close                  # remove clean run-scoped worktrees
/lyra graph --format mermaid # static task-graph projection
/lyra models                 # model classes and current selections
/lyra doctor                 # preflight
/lyra upgrade                # self-update from the latest release
```

The model-callable tools are `lyra_status`, `lyra_tickets`, `lyra_tickets_create`, `lyra_start`, `lyra_wait`, and `lyra_control`.
`/lyra` is a human/debug escape hatch; do not use it for agent work.

## Rules

- Do not edit tickets, run state, or another worker's worktree directly.
- A worker reports through `lyra_progress`, `lyra_discover`, and `lyra_finish`; non-OMP workers
  write `RESULT.md` in their worker directory.
- `lyra-merge` owns merges; `lyra close` owns worktree cleanup.
- Board UI is a non-goal. Use `status` / `report` / `graph`.
