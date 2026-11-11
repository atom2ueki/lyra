# Lyra OMP adapter

Thin OMP surface for the standalone Lyra conductor:

- `/lyra <command>` — run `lyra` CLI commands from an OMP session.
- `lyra_conduct` — model-callable status/start/monitor/stop/close tool.

The adapter does not own the task graph; it shells out to the `lyra` binary. Install the core CLI
first (`bun link` in the repository root), or set `LYRA_BIN` to an absolute command.

```sh
# from the Lyra repository root
bun link
omp plugin link ./packages/omp
# restart omp or /reload-plugins
```

The adapter bundles a `lyra` skill and a `commands/lyra.md` prompt stub. The delivery skills
(`lyra-plan`, `lyra-work`, `lyra-review`, `lyra-merge`, `lyra-close`, `lyra-ship`) remain the
user-level skill suite; this package only exposes the conductor entry point.
