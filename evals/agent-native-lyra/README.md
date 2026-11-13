# Agent-native Lyra eval

Deterministic integration evaluation for the six typed OMP tools. It drives the real `lyra` CLI
with a fake OMP worker and a scripted tool policy; the live OMP decision test is documented below.

## Run

```sh
bash evals/agent-native-lyra/run.sh
```

The runner builds `rust/target/debug/lyra` when missing and writes `report.json` next to this file.
Set `LYRA_BIN=/absolute/lyra` to test an installed binary instead.

Scenarios:

- **A — DAG scheduling:** four tickets, two ready and two dependent; `lyra_start` + repeated
  `lyra_wait` reaches `done`.
- **B — blocker:** a parent worker requests a blocker; the conductor creates and runs it; the
  parent resumes to `done`.
- **C — needs_input:** a worker returns an exact question; the tool returns `next:"resolve_input"`
  and the runner does not auto-answer.
- **D — cancellation:** `lyra_wait` is aborted, the CLI child exits, the run remains active, and a
  later wait settles it.
- **E — invalid input:** a cyclic ticket batch is rejected without writing ticket files.

## Report contract

Each scenario records `status`, `toolCalls`, `invalidCalls`, `directShellCalls`,
`directStoreMutations`, and `durationMs`. The deterministic runner uses no shell to call Lyra and
never writes the ticket store itself; scenario E asserts that a rejected batch creates zero ticket
files.

Thresholds for CI: every scenario `pass`; `directShellCalls == 0`; `directStoreMutations == 0`.
Expected invalid calls are allowed only in scenario D (abort) and E (rejected cycle).

## Live OMP smoke

The deterministic runner exercises the tool/CLI/worker contract without spending model tokens.
The live runner starts a real OMP print session with the linked plugin, creates a four-ticket DAG,
starts two workers, waits through the dependent tickets, and closes the run. The recorded run
finished with all four tickets `done` and status `closed`; no `bash` tool event was emitted.

Run the live test with:

```sh
bash evals/agent-native-lyra/run-live.sh
```

It is not a CI dependency because it needs a configured provider and consumes model tokens.
