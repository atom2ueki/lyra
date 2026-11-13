// Lyra worker extension. Loaded into every worker with `-e` and a per-attempt config file.
// The conductor owns the ticket store; this extension only reports progress/discovery/result
// through claim-scoped files.
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const PHASES = ['investigating', 'planning', 'implementing', 'verifying', 'reviewing', 'waiting_ci', 'merging', 'deploying'];

function readConfig(file) {
  const config = JSON.parse(readFileSync(file, 'utf8'));
  if (config.schemaVersion !== 1) throw new Error(`unsupported worker config schema: ${config.schemaVersion}`);
  return config;
}

function writeJson(file, value) {
  const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
  writeFileSync(temporary, JSON.stringify(value, null, 2) + '\n', { mode: 0o600 });
  renameSync(temporary, file);
}

function git(cwd, args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
}

async function sleep(ms) {
  if (typeof Bun !== 'undefined' && typeof Bun.sleep === 'function') {
    await Bun.sleep(ms);
    return;
  }
  await new Promise((resolve) => setTimeout(resolve, ms));
}

export default function lyraWorkerExtension(pi) {
  const configFile = process.env.LYRA_WORKER_CONFIG;
  if (!configFile) return;
  const config = readConfig(configFile);
  for (const dir of [config.channel.requests, config.channel.responses]) {
    mkdirSync(dir, { recursive: true, mode: 0o700 });
  }

  pi.setLabel(`lyra ${config.ticketId}·${config.claimId.slice(0, 6)}`);
  let context;
  let ownerId;
  let timer;
  let sawWork = false;
  let settled = false;
  let phase = null;
  let detail = null;
  let finished = false;
  let stopping = false;
  const discoveries = new Map();

  function owns(ctx) {
    if (ctx.agent?.kind !== 'main') return false;
    const id = ctx.sessionManager.getSessionId();
    if (ownerId && ownerId !== id) return false;
    if (!context) {
      ownerId = id;
      context = ctx;
      publish();
      timer = ctx.setInterval(() => publish({ heartbeat: true }), 30_000);
    }
    return true;
  }

  function publish(options = {}) {
    if (!context) return;
    const state = stopping ? 'stopping' : sawWork && settled ? 'idle' : sawWork ? 'working' : 'starting';
    writeJson(config.channel.state, {
      runId: config.runId,
      claimId: config.claimId,
      ticketId: config.ticketId,
      pid: process.pid,
      sessionId: context.sessionManager.getSessionId(),
      sessionFile: context.sessionManager.getSessionFile(),
      state,
      phase,
      detail,
      finished,
      updatedAt: new Date().toISOString(),
    });
    if (options.heartbeat) return;
  }

  pi.on('session_start', (_event, ctx) => {
    owns(ctx);
  });
  for (const event of ['before_agent_start', 'agent_start']) {
    pi.on(event, (_event, ctx) => {
      if (!owns(ctx)) return;
      sawWork = true;
      settled = false;
      publish();
    });
  }
  pi.on('session_stop', (_event, ctx) => {
    if (!owns(ctx)) return;
    settled = true;
    publish();
  });
  pi.on('input', (event, ctx) => {
    if (!owns(ctx)) return;
    if (/^\s*\/lyra\s+(?:conduct|board|run)\b/i.test(event.text ?? '')) {
      ctx.ui?.notify?.('A worker cannot run the Lyra conductor or board; use lyra_discover for new tickets.', 'error');
      return { handled: true };
    }
    if (!/^\s*\/(?:exit|quit)\s*$/i.test(event.text ?? '')) settled = false;
  });
  pi.on('before_subagent_spawn', (event) => {
    const role = typeof event.modelRole === 'string' ? event.modelRole.replace(/^@/, '') : '';
    const model = role ? config.modelRoles?.[role] : undefined;
    return model ? { model } : undefined;
  });
  pi.on('session_before_switch', (_event, ctx) => (owns(ctx) ? { cancel: true } : undefined));
  pi.on('session_shutdown', (_event, ctx) => {
    if (!owns(ctx)) return;
    stopping = true;
    if (timer) ctx.clearTimer(timer);
    publish();
  });

  const z = pi.zod;

  pi.registerTool({
    name: 'lyra_progress',
    label: 'Report phase',
    description: `Report what you are doing now. Phases: ${PHASES.join(', ')}. Call it when the phase changes; it never finishes the ticket.`,
    parameters: z.object({
      phase: z.enum(PHASES).describe('The phase you are in now.'),
      detail: z.string().describe('One short line of concrete detail.'),
    }),
    async execute(_id, params, signal, _onUpdate, ctx) {
      const error = (text) => ({ content: [{ type: 'text', text }], isError: true });
      if (!owns(ctx)) return error('Only the assigned main worker session can report this ticket.');
      if (signal?.aborted) return error('The phase report was cancelled.');
      if (!params.detail?.trim()) return error('Report a short, concrete detail line.');
      phase = params.phase;
      detail = params.detail.trim().replace(/\s+/g, ' ');
      publish();
      return {
        content: [{ type: 'text', text: `Recorded phase ${phase}: ${detail}` }],
        details: { phase, detail },
      };
    },
  });

  pi.registerTool({
    name: 'lyra_discover',
    label: 'Discover a ticket',
    description: 'Ask the conductor for a real ticket id: a "blocker" when this ticket cannot finish without other work, or a "followup" for necessary but non-blocking work. Pass existingId to adopt an existing ticket.',
    parameters: z.object({
      kind: z.enum(['blocker', 'followup']).describe('blocker = required to finish; followup = necessary but non-blocking.'),
      reason: z.string().describe('Why this ticket is necessary.'),
      subject: z.string().optional().describe('One-line subject for a new ticket.'),
      prompt: z.string().optional().describe('Full prompt for the new ticket.'),
      existingId: z.string().optional().describe('Existing ticket id such as QuietHarbor.'),
    }),
    async execute(_id, params, signal, _onUpdate, ctx) {
      const error = (text) => ({ content: [{ type: 'text', text }], isError: true });
      if (!owns(ctx)) return error('Only the assigned main worker session can discover tickets.');
      if (!params.reason?.trim()) return error('Explain why this ticket is necessary.');
      if (!params.existingId && !params.subject?.trim()) return error('Give a subject or pass existingId.');
      const requestId = randomUUID();
      const payload = {
        requestId,
        kind: params.kind,
        reason: params.reason.trim(),
        ...(params.subject?.trim() ? { subject: params.subject.trim() } : {}),
        ...(params.prompt?.trim() ? { prompt: params.prompt.trim() } : {}),
        ...(params.existingId ? { existingId: params.existingId.trim() } : {}),
      };
      writeJson(join(config.channel.requests, `${requestId}.json`), payload);
      const responseFile = join(config.channel.responses, `${requestId}.json`);
      const deadline = Date.now() + 120_000;
      while (Date.now() < deadline) {
        if (signal?.aborted) return error('The discovery request was cancelled.');
        if (existsSync(responseFile)) {
          const response = JSON.parse(readFileSync(responseFile, 'utf8'));
          if (typeof response.ticketId === 'string' && response.ticketId) {
            discoveries.set(response.ticketId, { kind: params.kind, reason: payload.reason });
            return {
              content: [{ type: 'text', text: `${response.ticketId} (${params.kind}) is a real ticket now.` }],
              details: { ticketId: response.ticketId, kind: params.kind },
            };
          }
          return error(`The conductor response has no ticketId: ${JSON.stringify(response)}`);
        }
        await sleep(100);
      }
      return error('The discovery request timed out; retry or finish with a blocker.');
    },
  });

  pi.registerTool({
    name: 'lyra_finish',
    label: 'Finish ticket',
    description: 'Report the ticket outcome. The conductor owns the store; never edit ticket files. A done result requires a clean committed worktree whose HEAD descends from the base.',
    parameters: z.object({
      status: z.enum(['done', 'blocked', 'failed', 'needs_input']).describe('done = complete; blocked = needs other tickets; needs_input = a human must answer; failed = could not complete.'),
      summary: z.string().describe('What changed or was learned, and why this is the outcome.'),
      verification: z.string().optional().describe('The checks actually run and what they printed.'),
      evidence: z.array(z.object({
        command: z.string(),
        exitCode: z.number().int(),
        output: z.string(),
      })).optional().describe('Structured evidence from the checks actually run.'),
      blockers: z.array(z.string()).optional().describe('Ticket ids from lyra_discover that stop this ticket.'),
    }),
    approval: 'write',
    async execute(_id, params, signal, _onUpdate, ctx) {
      const error = (text) => ({ content: [{ type: 'text', text }], isError: true });
      if (!owns(ctx)) return error('Only the assigned main worker session can finish this ticket.');
      if (signal?.aborted) return error('The finish was cancelled.');
      if (!params.summary?.trim()) return error('Describe the outcome in a non-empty summary.');
      if (finished) return error('An outcome for this attempt was already recorded; finish your response instead.');
      if (params.status === 'done') {
        if (!params.verification?.trim()) return error('A done ticket needs verification text.');
        try {
          const dirty = git(config.worktree, ['status', '--porcelain']).trim();
          if (dirty) throw new Error(`Commit the work in ${config.worktree} first:\n${dirty}`);
          git(config.worktree, ['merge-base', '--is-ancestor', config.baseHead, 'HEAD']);
        } catch (failure) {
          return error(failure.message);
        }
      }
      if (params.status === 'blocked') {
        if (!params.blockers?.length) return error('A blocked ticket needs blocker ids discovered in this attempt.');
        for (const id of params.blockers) {
          if (discoveries.get(id)?.kind !== 'blocker') {
            return error(`${id} was not discovered by lyra_discover({kind:'blocker'}) in this attempt.`);
          }
        }
      }
      if (params.status === 'needs_input' && !params.summary?.trim()) {
        return error('A needs_input ticket needs a summary of the question for the human.');
      }
      const result = {
        schemaVersion: 1,
        ticketId: config.ticketId,
        runId: config.runId,
        claimId: config.claimId,
        status: params.status,
        summary: params.summary.trim(),
        verification: params.verification?.trim(),
        evidence: params.evidence ?? [],
        blockers: params.blockers ?? [],
        finishedAt: new Date().toISOString(),
      };
      writeJson(config.channel.result, result);
      finished = true;
      publish();
      return {
        content: [{ type: 'text', text: `Recorded ${params.status} for ${config.ticketId}; the conductor will settle it.` }],
        details: { status: params.status },
      };
    },
  });
}
