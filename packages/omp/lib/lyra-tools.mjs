// Agent-facing Lyra tools. These are domain operations, not CLI flags: the adapter owns argument
// construction, bounded waits, cancellation, and the concise result projection.
// Extension execute order is (callId, params, signal, onUpdate, context).

import { LyraCliError } from "./lyra-cli.mjs";

const READ = "read";
const WRITE = "write";
const DEFAULT_WAIT_MS = 30_000;
const MAX_WAIT_MS = 60_000;
const WAIT_SLICE_MS = 5_000;
const MAX_LISTED_TICKETS = 12;

function text(value) {
  return { content: [{ type: "text", text: value }] };
}

function success(summary, details) {
  return {
    ...text(summary),
    details: { ok: true, ...details },
  };
}

function failed(error, action) {
  return {
    ...text(`${action} failed (${error.code}): ${error.message}${error.next ? `\nNext: ${error.next}` : ""}`),
    details: {
      ok: false,
      action,
      code: error.code,
      next: error.next,
      exitCode: error.exitCode,
      stderrTail: error.stderrTail,
    },
    isError: true,
  };
}

function options(params, ctx, signal) {
  return { cwd: params.cwd ?? ctx?.cwd, signal };
}

function shorten(value, limit = 120) {
  const oneLine = String(value ?? "").replace(/\s+/g, " ").trim();
  return oneLine.length > limit ? `${oneLine.slice(0, limit - 1)}…` : oneLine;
}

function ticketLine(ticket) {
  return `${ticket.id} ${shorten(ticket.subject, 72)} [${ticket.status}]`;
}

function summarizeTickets(tickets) {
  const shown = tickets.slice(0, MAX_LISTED_TICKETS).map(ticketLine);
  const suffix = tickets.length > shown.length ? `\n… ${tickets.length - shown.length} more` : "";
  return shown.join("\n") + suffix;
}

function summarizeRun(run) {
  if (!run) return "Run: none";
  const active = run.tickets.filter((ticket) => ["running", "starting"].includes(ticket.status));
  const blocked = run.tickets.filter((ticket) => ticket.needsInput || ticket.status === "blocked");
  const lines = [
    `Run: ${run.runId} [${run.status}] revision ${run.revision}`,
    `Tickets: ${run.tickets.length} total, ${active.length} active, ${blocked.length} blocked/input`,
  ];
  if (blocked.length) {
    for (const ticket of blocked.slice(0, 5)) {
      lines.push(`  ${ticket.id} ${shorten(ticket.summary ?? ticket.phase ?? ticket.status, 120)}`);
    }
  }
  return lines.join("\n");
}

function nextForGraph(graph, run) {
  if (run?.tickets?.some((ticket) => ticket.needsInput)) return "resolve_input";
  if (["running", "starting"].includes(run?.status)) return "wait";
  if (run?.tickets?.some((ticket) => ["running", "starting"].includes(ticket.status))) return "wait";
  if ((graph?.ready ?? []).length > 0) return "start";
  const open = (graph?.todo ?? 0) + (graph?.in_progress ?? 0);
  if (open > 0) return "blocked";
  return "done";
}

function nextFromRun(run) {
  if (!run) return "start";
  if (run.tickets.some((ticket) => ticket.needsInput)) return "resolve_input";
  if (["running", "starting"].includes(run.status)) return "wait";
  if (run.tickets.some((ticket) => ["running", "starting"].includes(ticket.status))) return "wait";
  if (run.status === "done") return "done";
  if (run.status === "blocked") return "blocked";
  if (run.status === "stopped" || run.status === "closed") return run.status;
  return "wait";
}

function waitTimeout(requested) {
  if (!Number.isFinite(requested) || requested <= 0) return DEFAULT_WAIT_MS;
  return Math.min(Math.trunc(requested), MAX_WAIT_MS);
}

function waitArgs(params, timeoutMs) {
  const args = ["conduct", "wait", "--json", "--concise", "--timeout-ms", String(timeoutMs)];
  if (params.runId) args.push("--run", params.runId);
  if (Number.isInteger(params.afterRevision) && params.afterRevision >= 0) {
    args.push("--after-revision", String(params.afterRevision));
  }
  return args;
}

function clientError(error, action) {
  if (error instanceof LyraCliError) {
    if (error.code === "aborted") throw error;
    return failed(error, action);
  }
  return failed(
    new LyraCliError("adapter_error", error instanceof Error ? error.message : String(error), {
      next: "Retry once; if it persists, run lyra doctor.",
    }),
    action,
  );
}

export function registerLyraTools(pi, client) {
  const z = pi.zod;

  pi.registerTool({
    name: "lyra_status",
    label: "Lyra status",
    description:
      "Inspect the project's Lyra ticket graph and the latest or named run. Read-only. Use this first before starting or resuming work; it returns ready/blocked tickets and a next action.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      runId: z.string().optional().describe("Inspect a specific run instead of the latest run."),
    }),
    approval: READ,
    async execute(_id, params, signal, onUpdate, ctx) {
      try {
        const call = options(params, ctx, signal);
        const status = await client.json(["status", "--json", "--concise"], call);
        if (params.runId) {
          status.run = await client.json(
            ["conduct", "run", "--run", params.runId, "--json", "--concise"],
            call,
          );
        }
        const ready = status.graph?.ready ?? [];
        const blocked = status.graph?.blocked ?? [];
        const next = params.runId ? nextForGraph(status.graph, status.run) : status.next;
        const summary = [
          `Project ${status.projectId}: ${status.graph?.total ?? 0} tickets, ${ready.length} ready, ${blocked.length} blocked`,
          ready.length ? `Ready:\n${summarizeTickets(ready)}` : "Ready: none",
          summarizeRun(status.run),
          `Next: ${next}`,
        ].join("\n");
        return success(summary, {
          action: "status",
          schemaVersion: status.schemaVersion,
          projectId: status.projectId,
          graph: status.graph,
          run: status.run,
          next,
        });
      } catch (error) {
        return clientError(error, "lyra_status");
      }
    },
  });

  pi.registerTool({
    name: "lyra_tickets",
    label: "Lyra tickets",
    description:
      "List Lyra tickets, show one ticket's prompt, acceptance, blockers, and result, or update an editable (todo/failed) ticket. list/show are read-only; update is a write that replaces whole lists for blockedBy and labels.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      action: z.enum(["list", "show", "update"]).describe(
        "list = compact ticket list; show = one full ticket; update = edit a todo/failed ticket.",
      ),
      ticketId: z.string().optional().describe("Required when action is show or update."),
      details: z.boolean().optional().describe("Include lastError and attempts in a list."),
      subject: z.string().optional().describe("Update: new subject."),
      prompt: z.string().optional().describe("Update: new prompt."),
      acceptance: z
        .string()
        .nullable()
        .optional()
        .describe("Update: new acceptance text; null clears it."),
      priority: z.number().int().min(0).max(4).optional().describe("Update: new priority."),
      blockedBy: z
        .array(z.string())
        .optional()
        .describe("Update: replaces the whole dependency list; an empty array clears it."),
      labels: z
        .array(z.string())
        .optional()
        .describe("Update: replaces the whole label list; an empty array clears it."),
    }),
    approval: WRITE,
    async execute(_id, params, signal, onUpdate, ctx) {
      if (params.action !== "list" && !params.ticketId) {
        return failed(
          new LyraCliError("invalid_input", `ticketId is required when action is ${params.action}`, {
            next: "Call lyra_tickets list first, then use a returned ticket id.",
          }),
          "lyra_tickets",
        );
      }
      if (params.action === "update") {
        const writable = [params.subject, params.prompt, params.acceptance, params.priority, params.blockedBy, params.labels];
        if (writable.every((field) => field === undefined)) {
          return failed(
            new LyraCliError("invalid_input", "Provide at least one field to update", {
              next: "Pass subject, prompt, acceptance, priority, blockedBy, or labels.",
            }),
            "lyra_tickets",
          );
        }
      }
      try {
        const call = options(params, ctx, signal);
        if (params.action === "update") {
          const args = ["tickets", "update", params.ticketId, "--json"];
          if (params.subject !== undefined) args.push("--subject", params.subject);
          if (params.prompt !== undefined) args.push("--prompt", params.prompt);
          if (typeof params.acceptance === "string") args.push("--acceptance", params.acceptance);
          if (params.acceptance === null) args.push("--clear-acceptance");
          if (params.priority !== undefined) args.push("--priority", String(params.priority));
          if (Array.isArray(params.blockedBy)) {
            if (params.blockedBy.length === 0) {
              args.push("--clear-blocked-by");
            } else {
              for (const blocker of params.blockedBy) args.push("--blocked-by", blocker);
            }
          }
          if (Array.isArray(params.labels)) {
            if (params.labels.length === 0) {
              args.push("--clear-labels");
            } else {
              for (const label of params.labels) args.push("--label", label);
            }
          }
          const ticket = await client.json(args, call);
          const summary = [
            `Updated ${ticket.id} ${ticket.subject} [${ticket.status}]`,
            `Priority: ${ticket.priority}; blocked by: ${ticket.blockedBy?.length ? ticket.blockedBy.join(", ") : "none"}`,
            ticket.acceptance ? `Acceptance: ${shorten(ticket.acceptance, 300)}` : "",
          ]
            .filter(Boolean)
            .join("\n");
          return success(summary, { action: "update", ticket });
        }
        if (params.action === "show") {
          const ticket = await client.json(
            ["tickets", "show", params.ticketId, "--json"],
            call,
          );
          const summary = [
            `${ticket.id} ${ticket.subject} [${ticket.status}]`,
            `Priority: ${ticket.priority}; blocked by: ${ticket.blockedBy?.length ? ticket.blockedBy.join(", ") : "none"}`,
            `Attempts: ${ticket.attempts ?? 0}`,
            ticket.lastError ? `Last error: ${shorten(ticket.lastError, 240)}` : "",
            `Prompt: ${shorten(ticket.prompt, 500)}`,
            ticket.acceptance ? `Acceptance: ${shorten(ticket.acceptance, 300)}` : "",
          ]
            .filter(Boolean)
            .join("\n");
          return success(summary, { action: "show", ticket });
        }
        const args = ["tickets", "list", "--json"];
        if (params.details) args.push("--details");
        const tickets = await client.json(args, call);
        const ready = tickets.filter((ticket) => ticket.ready);
        const summary = `${tickets.length} tickets, ${ready.length} ready\n${summarizeTickets(tickets)}`;
        return success(summary, { action: "list", tickets });
      } catch (error) {
        return clientError(error, "lyra_tickets");
      }
    },
  });

  pi.registerTool({
    name: "lyra_tickets_create",
    label: "Lyra ticket author",
    description:
      "Create a batch of Lyra tickets with a dependency DAG. Write operation. Keys are temporary and map to generated ticket ids; blockedBy accepts batch keys or existing ticket ids. Validate the DAG before starting a run.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      tickets: z
        .array(
          z.object({
            key: z.string().describe("Unique temporary key used by blockedBy in this batch."),
            subject: z.string(),
            prompt: z.string(),
            blockedBy: z.array(z.string()).optional(),
            priority: z.number().int().min(0).max(4).optional(),
            labels: z.array(z.string()).optional(),
            modelClass: z.string().optional(),
            acceptance: z.string().optional(),
            plan: z.string().optional(),
            parent: z.string().optional(),
            harness: z.string().optional(),
          }),
        )
        .describe("Tickets to create in one validated batch."),
    }),
    approval: WRITE,
    async execute(_id, params, signal, onUpdate, ctx) {
      if (!Array.isArray(params.tickets) || params.tickets.length === 0) {
        return failed(
          new LyraCliError("invalid_input", "Provide at least one ticket", {
            next: "Pass one or more tickets with key, subject, and prompt.",
          }),
          "lyra_tickets_create",
        );
      }
      const keys = new Set();
      for (const ticket of params.tickets) {
        if (!ticket.key?.trim()) {
          return failed(
            new LyraCliError("invalid_input", "Every ticket needs a non-empty key", {
              next: "Give each ticket a unique temporary key.",
            }),
            "lyra_tickets_create",
          );
        }
        if (keys.has(ticket.key)) {
          return failed(
            new LyraCliError("invalid_input", `Duplicate ticket key: ${ticket.key}`, {
              next: "Use a unique key for every ticket in the batch.",
            }),
            "lyra_tickets_create",
          );
        }
        keys.add(ticket.key);
      }
      try {
        const output = await client.json(
          ["tickets", "create", "--json", "--file", "-"],
          { ...options(params, ctx, signal), input: JSON.stringify({ tickets: params.tickets }) },
        );
        const mapped = (output.created ?? []).map((item) => `${item.key} → ${item.id}`).join(", ");
        return success(`Created ${output.created?.length ?? 0} tickets: ${mapped}`, {
          action: "create",
          created: output.created ?? [],
        });
      } catch (error) {
        return clientError(error, "lyra_tickets_create");
      }
    },
  });

  pi.registerTool({
    name: "lyra_start",
    label: "Lyra start",
    description:
      "Start a Lyra run for ready tickets. Write operation. Returns a runId immediately; it does not busy-wait. Call lyra_wait next. Set dryRun to inspect selection without launching workers.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      ticketIds: z.array(z.string()).optional().describe("Explicit ready ticket ids; omit for all ready."),
      maxParallel: z.number().int().positive().optional(),
      maxSeconds: z.number().int().positive().optional(),
      base: z.string().optional(),
      backend: z.enum(["herdr", "ghostty", "headless"]).optional(),
      dryRun: z.boolean().optional(),
    }),
    approval: WRITE,
    async execute(_id, params, signal, onUpdate, ctx) {
      const args = ["conduct", "start", "--json", "--concise"];
      for (const ticketId of params.ticketIds ?? []) args.push("--ticket", ticketId);
      if (params.maxParallel) args.push("--max-parallel", String(params.maxParallel));
      if (params.maxSeconds) args.push("--max-seconds", String(params.maxSeconds));
      if (params.base) args.push("--base", params.base);
      if (params.backend) args.push("--backend", params.backend);
      if (params.dryRun) args.push("--dry-run");
      onUpdate?.(text("Starting Lyra run…"));
      try {
        const run = await client.json(args, options(params, ctx, signal));
        const planned = run.plannedTickets ?? [];
        const summary = [
          `Run ${run.runId} [${run.status}] revision ${run.revision}`,
          planned.length ? `Selected: ${planned.join(", ")}` : "Selected: none",
          `Next: ${run.status === "planned" ? "start" : "wait"}`,
        ].join("\n");
        return success(summary, {
          action: "start",
          runId: run.runId,
          revision: run.revision,
          status: run.status,
          plannedTickets: planned,
          tickets: run.tickets,
          next: run.status === "planned" ? "start" : "wait",
        });
      } catch (error) {
        return clientError(error, "lyra_start");
      }
    },
  });

  pi.registerTool({
    name: "lyra_wait",
    label: "Lyra wait",
    description:
      "Wait for bounded progress on a Lyra run. Write operation because it settles worker results. Returns timedOut without stopping the run; call again with the returned revision. Use after lyra_start and after resolving a blocker.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      runId: z.string().optional().describe("Run to observe; defaults to the latest run."),
      afterRevision: z.number().int().min(0).optional().describe("Return only after this revision."),
      timeoutMs: z.number().int().positive().optional().describe("Total wait budget; capped at 60s."),
    }),
    approval: WRITE,
    async execute(_id, params, signal, onUpdate, ctx) {
      const budget = waitTimeout(params.timeoutMs);
      const deadline = Date.now() + budget;
      let last;
      try {
        while (true) {
          const remaining = Math.max(1, deadline - Date.now());
          const slice = Math.min(WAIT_SLICE_MS, remaining);
          onUpdate?.(
            text(`Waiting for Lyra run${params.runId ? ` ${params.runId}` : ""} (revision ${params.afterRevision ?? "current"})…`),
          );
          last = await client.json(waitArgs(params, slice), {
            ...options(params, ctx, signal),
            timeoutMs: Math.min(slice + 1_000, 30_000),
          });
          if (!last.timedOut || Date.now() >= deadline) break;
        }
        const next = nextFromRun(last);
        const needsInput = last.tickets?.find((ticket) => ticket.needsInput);
        const summary = [
          `Run ${last.runId} [${last.status}] revision ${last.revision}${last.timedOut ? " (timed out)" : ""}`,
          summarizeRun(last),
          needsInput
            ? `Needs input: ${needsInput.id} — ${shorten(needsInput.summary ?? "", 240)}`
            : "",
          `Next: ${next}`,
        ]
          .filter(Boolean)
          .join("\n");
        return success(summary, {
          action: "wait",
          runId: last.runId,
          revision: last.revision,
          status: last.status,
          timedOut: last.timedOut,
          tickets: last.tickets,
          next,
        });
      } catch (error) {
        return clientError(error, "lyra_wait");
      }
    },
  });

  pi.registerTool({
    name: "lyra_control",
    label: "Lyra control",
    description:
      "Stop or close a specific Lyra run. Write operation. stop releases running workers; close removes clean run worktrees. Require an explicit runId so the wrong run is never touched.",
    parameters: z.object({
      cwd: z.string().optional().describe("Project directory; defaults to the session cwd."),
      runId: z.string().describe("The run to stop or close."),
      action: z.enum(["stop", "close"]).describe("stop releases workers; close removes clean worktrees."),
    }),
    approval: WRITE,
    async execute(_id, params, signal, onUpdate, ctx) {
      try {
        const run = await client.json(
          ["conduct", params.action, "--run", params.runId, "--json", "--concise"],
          options(params, ctx, signal),
        );
        return success(`Run ${run.runId} [${run.status}]`, {
          action: params.action,
          runId: run.runId,
          revision: run.revision,
          status: run.status,
          tickets: run.tickets,
        });
      } catch (error) {
        return clientError(error, "lyra_control");
      }
    },
  });
}
