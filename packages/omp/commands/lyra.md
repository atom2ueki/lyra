Run the Lyra conductor in the current project.

1. Run `lyra status` first and report the graph errors, ready tickets, and blocked tickets.
2. If the user asked to execute, run `lyra conduct start --max <cap>` and then
   `lyra conduct monitor` until the run settles.
3. Do not edit tickets or worktrees directly. Do not merge outside `lyra-merge`.
4. Report the run id, status, per-ticket model/phase, and any blocker or `needs_input`.
