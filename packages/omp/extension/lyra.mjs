// OMP adapter for Lyra. The conductor lives in the standalone `lyra` CLI; this module exposes
// /lyra to humans and the typed Lyra tools to the lead agent. Workers use the separate worker
// extension and never call this CLI.

import { LyraClient } from "../lib/lyra-cli.mjs";
import { registerLyraTools } from "../lib/lyra-tools.mjs";

const USAGE =
  "/lyra status | start --max 2 | monitor | resume | wait | report | stop | close | graph | models | doctor | upgrade";

export default function lyraOmpExtension(pi) {
  pi.setLabel("Lyra");

  const client = new LyraClient({
    bin: process.env.LYRA_BIN?.trim() || "lyra",
  });

  registerLyraTools(pi, client);

  pi.registerCommand("lyra", {
    description: "Run the Lyra conductor CLI directly (human/debug escape hatch; agents use lyra_* tools)",
    handler: async (args, ctx) => {
      const argv = String(args ?? "")
        .trim()
        .split(/\s+/)
        .filter(Boolean);
      if (argv.length === 0) {
        ctx.ui?.notify?.(USAGE, "info");
        return;
      }
      try {
        const output = await client.text(argv, { cwd: ctx?.cwd });
        ctx.ui?.notify?.(output || `lyra ${argv.join(" ")} completed`, "info");
      } catch (error) {
        const message = `${error.code ? `${error.code}: ` : ""}${error.message}`;
        ctx.ui?.notify?.(message, "error");
      }
    },
  });
}
