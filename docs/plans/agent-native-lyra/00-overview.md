# Agent-native Lyra — typed tools over a concise CLI contract

**Status:** PLAN approved 2026-11-13. Phase 1 and Phase 2 complete; Phase 3 next.

This plan supersedes the assumption that the OMP adapter should expose the CLI surface to the model.
It keeps `lyra` as the source of truth; it changes the interface the agent sees.

---

## 1. The vision

A lead OMP agent should be able to say “run this plan” without knowing Lyra's command-line flags.
It inspects the task graph, creates or adjusts tickets, starts a run, waits for bounded progress,
surfaces blockers and human-input questions, and closes the run — all through a few typed tools.

Workers keep the existing contract: they are launched by the conductor, load the Lyra worker
extension, and report through claim-scoped files. They do not call the Lyra CLI or edit the ticket
store.

The end state is not a second orchestrator. It is a **deep agent adapter**: a small tool interface
hiding CLI invocation, JSON projection, long-running polling, cancellation, and next-action policy.

## 2. Current-state reality (evidence-based)

- The OMP adapter is a generic CLI wrapper: one tool takes a command enum plus `args: string[]`.
  It does not expose ticket authoring or a typed run lifecycle.
  `packages/omp/extension/lyra.mjs:47-68`.
- The wrapper ignores the tool `signal` and `onUpdate`, buffers all output, and returns only
  `details: { code }`; cancellation does not terminate the child process.
  `packages/omp/extension/lyra.mjs:9-27,57-68`.
- `lyra status --json` returns counts and ready/blocked ids, but not ticket subjects, prompts, or
  acceptance. An agent cannot explain or choose tickets from it.
  `rust/src/cli.rs:342-386`.
- `lyra conduct start --json` and `report --json` serialize the entire `RunState`, including
  `modelPolicy`, `modelCatalog`, and `usage`; observed in a dry-run JSON dump on 2026-11-13.
  `rust/src/conductor.rs:630-653`, `rust/src/cli.rs:263-303`.
- Ticket creation is one free-text ticket at a time. There is no batch API, no temporary key map,
  and no cross-ticket `blockedBy` authoring.
  `rust/src/cli.rs:69-75,774-798`, `rust/src/schema.rs:96-162`.
- `lyra conduct start` accepts at most one explicit `--ticket`; ready tickets are auto-selected.
  `rust/src/cli.rs:122-139`, `rust/src/conductor.rs:592-611`.
- `lyra conduct monitor` is a blocking loop; its timeout branch sets the run status to `stopped`.
  `rust/src/conductor.rs:944-1003`.
- The Lyra skill is a command reference, not an execution workflow; it does not define when to
  create tickets, how to handle a blocker, or what to do on `needs_input`.
  `packages/omp/skills/lyra/SKILL.md:11-35`.
- OMP custom tools already support the contract this plan needs: `execute(toolCallId, params,
  signal, onUpdate, ctx)`, structured `details`, per-tool approval tiers, and cancellation.
  `omp://custom-tools.md`, `omp://approval-mode.md`.
- OMP's native `task` tool demonstrates the target shape: spawn returns immediately, progress uses
  `onUpdate`, results are structured in `details`, cancellation propagates through `signal`, and
  callers use `wait`/`proc://` for long-running work. `omp://tools/task.md`.
- Prior Lyra research already fixed the architectural boundary: the CLI owns state because an
  in-process extension dies with the OMP session (risk R3 in `~/.agents/plans/lyra-research.md`).
- Anthropic's tool-design research warns against wrapping existing software endpoints directly;
  it recommends a few high-impact workflow tools, namespacing, high-signal/token-efficient
  responses, actionable errors, and evaluation with real tasks.
  <https://www.anthropic.com/research/writing-tools-for-agents>.

**Root cause:** the plugin is a shallow pass-through of a deterministic CLI. The interface the
agent learns is the CLI surface; the adapter hides no workflow logic, so the model must supply
command syntax, parse prose, recover from raw errors, and manage blocking calls.

## 3. Target pattern

```text
Lead agent
  └─ lyra_status / lyra_tickets / lyra_tickets_create / lyra_start / lyra_wait / lyra_control
       └─ typed JSON contract (concise by default)
            └─ lyra CLI (source of truth)
                 ├─ ticket store + run state
                 └─ worker panes + claim-scoped worker extension
```

The adapter is a deep module: a small domain interface hides CLI parsing, JSON projection,
polling, timeouts, cancellation, and next-action selection. It does not own tickets, state, or
worktrees.

Read tools use `approval: "read"`; write tools use `approval: "write"`. Tool names are namespaced
under `lyra_`. Results separate a short model-facing `content` summary from deterministic
`details` for the UI and follow-up calls.

## 4. Sequence (do in order — each links to its own doc)

| Phase | Doc | Gist | Depends on |
|---|---|---|---|
| **1** | `01-cli-agent-contract.md` | Concise agent projections, ticket list/show/batch-create, stable errors | — |
| **2** | `02-run-wait-revision.md` | Revisioned non-destructive wait, multi-ticket start, cancellation-safe run control | 1 |
| **3** | `03-omp-agent-tools.md` | Replace the generic CLI wrapper with six typed OMP tools | 1, 2 |
| **4** | `04-skill-eval-dogfood.md` | Workflow skill, agent evaluation, real dogfood and plugin release | 3 |

The CLI contract ships first because it is the layer that owns truth; the tools consume it. The
skill and eval come last so they describe an interface that actually exists.

## 5. Decisions (CONFIRMED by user 2026-11-13)

1. **Ticket authoring uses a batch tool.** ✅ `lyra_tickets_create` accepts temporary keys and
   `blockedBy` key references; `run.yaml` remains an import/export projection, not the primary
   agent path. See phase 1 §1.3.
2. **Six domain tools, not one action tool.** ✅ `lyra_status`, `lyra_tickets`,
   `lyra_tickets_create`, `lyra_start`, `lyra_wait`, `lyra_control`; read and write tools carry
   different approval tiers. See phase 3 §3.2.
3. **Start is non-blocking; waiting is bounded.** ✅ `lyra_start` returns a run id; `lyra_wait`
   returns after a bounded interval or a state change, and a timeout never marks the run stopped.
   See phase 2 §2.2.
4. **Autonomy stays explicit.** ✅ The agent starts runs only when the user asked to execute;
   `needs_input` always goes back to the user and is never auto-answered. See phase 4 §4.2.
5. **OMP remains a thin adapter; the CLI owns state.** ✅ No scheduler/state in the plugin, no
   worker CLI access. See phase 3 §3.1.
6. **Worker contract is unchanged.** ✅ Workers continue to use `lyra_progress`, `lyra_discover`,
   and `lyra_finish`; the agent-facing tools are for the lead session only. See phase 3 §3.5.

## 6. Non-goals / guardrails

- No MCP server in this plan. OMP is the only agent surface; a portable MCP adapter is a follow-up.
- No new scheduler, no board UI, no native terminal.
- No worker access to `lyra` CLI, ticket store, or another worker's worktree.
- No auto-answering of `needs_input`; no auto-start from a plan without an explicit user request.
- No removal of the `/lyra` slash command in this plan; it remains a human/debug escape hatch.
- No database migration. All changes are additive JSON/CLI/tool surfaces.
- Each phase must leave `cargo test` and the plugin tests green on the default branch.
