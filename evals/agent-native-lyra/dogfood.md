# Real headless dogfood — 2026-11-13

A temporary git repository was initialized with a real `lyra` binary and
`approval_posture = "unsafe"` so an unattended headless OMP worker could write and commit.

Ticket prompt: create `worker-ok.txt`, commit it, then call `lyra_finish` exactly once with
`status:"done"`, verification, and evidence.

Observed:

```
start:  run-real [running]; plannedTickets: [T-001]
wait:   revision 3, T-001 running (model zai/glm-5.3:high)
wait:   revision 5, status done, T-001 done
        summary: "created worker-ok.txt"
report: status done; T-001 done
```

The worker created and committed the file, called the worker extension tool, and the conductor
settled the ticket. `lyra_control(close)` (or `lyra close --concise`) then closed the run.

This was a real headless OMP worker, not the fake fixture used by `run.sh`.

## Live agent DAG — same date

A second temporary repo used the linked OMP plugin and a fake headless worker. A real OMP print
session received only the Lyra tools and created a four-ticket DAG (two ready, two dependent),
started two workers, waited through the settled graph, and closed the run. The final
`lyra_control` result listed `T-001`–`T-004` all `done` with status `closed`; the JSON event stream
contained no `bash` tool call.
