# Lyra OMP adapter

Typed OMP surface for the standalone Lyra conductor.

## Agent tools

The lead agent uses six domain tools; none accepts a raw CLI argument array:

| Tool | Approval | Purpose |
|---|---|---|
| `lyra_status` | read | Project graph + latest/named run summary + next action |
| `lyra_tickets` | write | List/show tickets, or update a `todo`/`failed` ticket (subject, prompt, acceptance, priority, blockedBy, labels) |
| `lyra_tickets_create` | write | Create a validated batch of tickets with dependency keys |
| `lyra_start` | write | Start a run and return a `runId` immediately |
| `lyra_wait` | write | Bounded wait; returns `timedOut` without stopping the run |
| `lyra_control` | write | Stop or close an explicit `runId` |

This is a breaking change from `0.1.x`: the generic `lyra_conduct` tool is gone; use the six tools
above. The adapter shells out to the `lyra` binary and owns no state. Install the core CLI first
(`cd rust && cargo install --path .`), or set `LYRA_BIN` to an absolute command.

```sh
# from the Lyra repository root
cd rust && cargo install --path .
cd .. && omp plugin link ./packages/omp
# restart omp or /reload-plugins
```

`/lyra <command>` is a human/debug escape hatch that runs the CLI directly. Agents should use the
typed tools instead; workers use the separate `lyra-worker.mjs` extension and must not call this CLI.

Run the adapter tests with:

```sh
cd packages/omp && npm test
```
