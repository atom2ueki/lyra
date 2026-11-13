# Real headless dogfood — 2026-11-13

A temporary git repository was initialized with a real `lyra` binary and `approval_posture =
"unsafe"` so an unattended headless OMP worker could write and commit.

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
