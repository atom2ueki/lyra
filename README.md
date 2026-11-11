# Lyra

Repository: <https://github.com/atom2ueki/lyra>

Lyra is a supervised task conductor for coding agents. It keeps the task graph in a local store,
schedules ready work into agent panes, and records results as files — not as a chat transcript.

The core scheduler, store, pane adapters, worktree handling, and model routing are **Rust**
(`rust/`). The only JavaScript is the OMP worker extension (`rust/src/lyra-worker.mjs`), which is
embedded into the Rust binary and loaded into each OMP worker with `-e`.

Implemented through **P3 + P5** (P4 RPC mode is optional/deferred):

- Lyra ticket schema v1 + local store + DAG validation
- `omp-kanban` import adapter
- `lyra init` / `import` / `status` / `graph` / `models` / `doctor`
- `PaneBackend`: herdr, Ghostty, headless
- worker extension (`lyra_progress`, `lyra_discover`, `lyra_finish`)
- `lyra conduct start|monitor|resume|stop|report`, `lyra add`, `lyra logs`, `lyra close`
- run-scoped worktrees, claims, state/events
- blocker suspension/checkpoint/resume and `needs_input`
- model policy: classes, roles, provider caps, usage-aware selection, per-worker role overlay
- cross-agent launch (`omp`/`codex`/`claude`) with a `RESULT.md` fallback
- OMP adapter package (`packages/omp`) — a thin `/lyra` + `lyra_conduct` wrapper around the binary

No board UI, no native terminal. Those are non-goals for the current product.

## Install

Release builds are macOS Apple Silicon and Linux (x86_64/arm64) binaries.

```sh
curl -fsSL https://raw.githubusercontent.com/atom2ueki/lyra/main/install.sh | sh
lyra --version
lyra upgrade
```

The installer uses `~/.local/bin` by default and verifies the release `SHA256SUMS` before replacing
the binary. Override with `LYRA_INSTALL_DIR`, pin a tag with `LYRA_VERSION=0.1.1`, or use
`LYRA_GITHUB_TOKEN` for a private fork (`gh` is required for authenticated asset downloads).

From source:

```sh
cd rust
cargo build --release
# binary is rust/target/release/lyra
cargo install --path .
```

`lyra upgrade` downloads the latest release asset for the current platform, verifies its checksum,
and atomically replaces the running binary. Use `lyra upgrade --check` to inspect the available
version, `--force` to reinstall the same version, or `--version 0.1.0` to pin a release.

## Usage

```sh
lyra init
lyra import omp-kanban ~/Documents/atom2ueki/omp-kanban/.omp/kanban
lyra conduct status
lyra models
lyra graph --format mermaid
lyra add "a new ticket"
lyra conduct start --max 2
lyra conduct monitor
lyra conduct resume --run <run-id>
lyra report
lyra logs T-001
lyra close
```

`LYRA_HOME` overrides `~/.lyra`; `LYRA_PANE_BACKEND=herdr|ghostty|headless` overrides detection;
`LYRA_OMP_BIN` overrides the `omp` binary for tests.

## Worker contract

Each worker gets a worktree, a `TASK.md` brief, a claim-scoped file channel, and a generated
`roles.yml` for OMP subagent routing. Workers report through the extension; non-OMP harnesses use
`RESULT.md` with these fields:

```text
status: done|blocked|failed|needs_input
summary: <what changed, or the question>
verification: <checks actually run>
blockers: <ticket ids discovered in this attempt>
```

The conductor owns `state.json`, tickets, claims, and worktrees. A worker never edits the ticket
store or another worktree.

## OMP adapter

```sh
omp plugin link ./packages/omp
```

The adapter shells out to the `lyra` binary; it does not own the task graph.

The design and research docs live in `~/.agents/plans/lyra.md` and
`~/.agents/plans/lyra-research.md`.
