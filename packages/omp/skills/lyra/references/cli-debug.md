# Lyra CLI debug reference

Use this only for human/debug commands. Agent work goes through the typed `lyra_*` tools.

## Command map

```sh
lyra status --json --concise       # graph + latest run + next action
lyra tickets list --json           # ticket summaries
lyra tickets show T-001 --json     # prompt, acceptance, blockers, result
lyra tickets create --file - --json # batch DAG from stdin
lyra conduct start --json --concise [--ticket ID]... [--tickets ID,ID]
lyra conduct wait --run RUN --json --concise [--after-revision N] [--timeout-ms 30000]
lyra conduct run --run RUN --json --concise
lyra conduct stop --run RUN --json --concise
lyra conduct close --run RUN --json --concise
lyra conduct monitor --run RUN --timeout-ms N   # interactive lifecycle action
lyra graph --format mermaid|dot|json
lyra models --json
lyra doctor
lyra upgrade [--check] [--force] [--version TAG]
```

## State rules

- `wait` is a non-destructive poll/pass. Its timeout returns `timedOut: true` and leaves the run
  active.
- `monitor --timeout-ms` is an interactive lifecycle action; it may mark the run stopped.
- `close` removes clean run-scoped worktrees and leaves suspended worktrees in place.
- `--ticket` may be repeated; `--tickets` is comma-separated.
- JSON errors use the `{ok:false,error:{code,message,next}}` envelope.

## Ownership

- Lead agent: typed `lyra_*` tools.
- Worker OMP: `lyra_progress`, `lyra_discover`, `lyra_finish` via the worker extension.
- Do not edit `.lyra` state, ticket JSON, or another worker's worktree by hand.
