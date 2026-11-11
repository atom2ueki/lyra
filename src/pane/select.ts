import type { PaneBackend } from "../pane-backend";
import { GhosttyBackend } from "./ghostty";
import { HeadlessBackend } from "./headless";
import { HerdrBackend } from "./herdr";

export const PANE_BACKENDS = ["herdr", "ghostty", "headless"] as const;
export type PaneBackendName = (typeof PANE_BACKENDS)[number];

export function isPaneBackendName(value: unknown): value is PaneBackendName {
  return typeof value === "string" && (PANE_BACKENDS as readonly string[]).includes(value);
}

export function selectBackend(options: {
  env?: NodeJS.ProcessEnv;
  stdoutIsTTY?: boolean;
} = {}): PaneBackend {
  const env = options.env ?? process.env;
  const explicit = env.LYRA_PANE_BACKEND?.trim();
  if (explicit) {
    if (!isPaneBackendName(explicit)) throw new Error(`unknown LYRA_PANE_BACKEND: ${explicit}`);
    return makeBackend(explicit, env);
  }
  if (env.HERDR_ENV === "1") return makeBackend("herdr", env);
  if (env.TMUX) throw new Error("tmux backend is not implemented yet; use herdr, Ghostty, or headless");
  if (env.ZELLIJ) throw new Error("Zellij backend is not implemented yet; use herdr, Ghostty, or headless");
  if (env.TERM_PROGRAM === "ghostty" || env.GHOSTTY_TERMINAL_ID) return makeBackend("ghostty", env);
  if (options.stdoutIsTTY === false) return makeBackend("headless", env);
  throw new Error(
    "no pane backend detected; set LYRA_PANE_BACKEND=herdr|ghostty|headless",
  );
}

function makeBackend(name: PaneBackendName, env: NodeJS.ProcessEnv): PaneBackend {
  if (name === "herdr") return new HerdrBackend({ env });
  if (name === "ghostty") return new GhosttyBackend({ env });
  return new HeadlessBackend({ env });
}
