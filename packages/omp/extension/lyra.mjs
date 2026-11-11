// OMP adapter for Lyra. The conductor lives in the standalone `lyra` CLI; this module only
// exposes /lyra and lyra_conduct inside an OMP session.
import { spawn } from "node:child_process";

function lyraBin() {
  return process.env.LYRA_BIN?.trim() || "lyra";
}

function run(args, cwd, ctx) {
  const { promise, resolve } = Promise.withResolvers();
  const child = spawn(lyraBin(), args, {
    cwd: cwd ?? ctx?.cwd ?? process.cwd(),
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout?.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr?.on("data", (chunk) => {
    stderr += chunk;
  });
  child.on("error", (error) => resolve({ code: 1, stdout, stderr: error.message }));
  child.on("close", (code) => resolve({ code: code ?? 1, stdout, stderr }));
  return promise;
}

export default function lyraOmpExtension(pi) {
  pi.setLabel("Lyra");

  pi.registerCommand("lyra", {
    description: "Run the Lyra task conductor (status/start/monitor/resume/stop/report/close/graph/models/doctor)",
    handler: async (args, ctx) => {
      const argv = String(args ?? "").trim().split(/\s+/).filter(Boolean);
      if (argv.length === 0) {
        ctx.ui?.notify?.("usage: /lyra status|start|monitor|resume|stop|report|close|graph|models|doctor|upgrade", "info");
        return;
      }
      const result = await run(argv, ctx?.cwd, ctx);
      const output = (result.stdout + result.stderr).trim();
      ctx.ui?.notify?.(output || `lyra ${argv.join(" ")} exited ${result.code}`, result.code === 0 ? "info" : "error");
    },
  });

  const z = pi.zod;
  pi.registerTool({
    name: "lyra_conduct",
    label: "Lyra conductor",
    description: "Read Lyra status or run its conductor commands. The standalone lyra CLI owns the task graph; this tool does not edit tickets directly.",
    parameters: z.object({
      command: z.enum(["status", "start", "monitor", "resume", "stop", "report", "close", "graph", "models", "doctor", "upgrade"]),
      args: z.array(z.string()).optional().describe("Extra CLI arguments, e.g. [\"--max\",\"2\"]"),
      cwd: z.string().optional().describe("Project directory (defaults to the session cwd)"),
    }),
    approval: "write",
    async execute(_id, params, _signal, _onUpdate, ctx) {
      const conductorCommands = { status: true, start: true, monitor: true, resume: true, report: true, stop: true, close: true };
      const argv = conductorCommands[params.command]
        ? ["conduct", params.command, ...(params.args ?? [])]
        : [params.command, ...(params.args ?? [])];
      const result = await run(argv, params.cwd ?? ctx?.cwd, ctx);
      const text = (result.stdout + result.stderr).trim() || `lyra exited ${result.code}`;
      return {
        content: [{ type: "text", text }],
        details: { code: result.code },
        isError: result.code !== 0,
      };
    },
  });
}
