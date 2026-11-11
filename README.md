# Lyra

Lyra is a supervised task conductor for coding agents. It keeps the task graph in a local store,
schedules ready work into agent panes, and records results as files — not as a chat transcript.

Implemented through **P3 + P5** (P4 RPC mode is optional/deferred):

- Lyra ticket schema v1 + local store + DAG validation
- `omp-kanban` import adapter
- `lyra init` / `import` / `conduct status` / `status` / `graph` / `models` / `doctor`
- PaneBackend adapters: herdr, Ghostty, headless
- worker extension (`lyra_progress`, `lyra_discover`, `lyra_finish`)
- `lyra conduct start|monitor|stop|report`, `lyra add`, `lyra logs`, `lyra close`
- run-scoped worktrees, claims, state/events
- blocker suspension/checkpoint/resume and `needs_input`
- model policy: classes, roles, provider caps, usage-aware selection, worker role overlay
- cross-agent launch (`omp`/`codex`/`claude`) with a `RESULT.md` fallback
- OMP adapter package `packages/omp` (`/lyra`, `lyra_conduct`, bundled `lyra` skill)

No board UI, no native terminal. Those are non-goals for the current product.

## Usage

```sh
bun install
bun run src/cli.ts --help

# initialize ~/.lyra, config, model policy, and the current project's store
bun run src/cli.ts init

# import an omp-kanban store
bun run src/cli.ts import omp-kanban ~/Documents/atom2ueki/omp-kanban/.omp/kanban

bun run src/cli.ts conduct status
bun run src/cli.ts models
bun run src/cli.ts graph --format mermaid
bun run src/cli.ts add "a new ticket"
bun run src/cli.ts conduct start --max 2
bun run src/cli.ts conduct monitor --once
bun run src/cli.ts logs T-001
bun run src/cli.ts close
```

`LYRA_HOME` overrides `~/.lyra`; `LYRA_PANE_BACKEND=herdr|ghostty|headless` overrides detection;
`LYRA_OMP_BIN` overrides the `omp` binary for tests.

## OMP adapter

```sh
bun link                         # exposes the `lyra` binary
omp plugin link ./packages/omp   # exposes /lyra and lyra_conduct
```

The adapter shells out to the standalone CLI; it does not own the task graph.

The design and research docs live in `~/.agents/plans/lyra.md` and
`~/.agents/plans/lyra-research.md`.
