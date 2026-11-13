// Typed subprocess transport for the standalone `lyra` CLI.
// The OMP adapter never reads or writes Lyra state directly; every operation crosses this seam.

import { spawn } from "node:child_process";

const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_TIMEOUT_MS = 120_000;
const MAX_OUTPUT_BYTES = 1_000_000;
const KILL_GRACE_MS = 500;
const MIN_VERSION = [0, 2, 0];

export class LyraCliError extends Error {
  constructor(code, message, options = {}) {
    super(message);
    this.name = "LyraCliError";
    this.code = code;
    this.next = options.next;
    this.exitCode = options.exitCode;
    this.stderrTail = options.stderrTail;
    this.cause = options.cause;
  }
}

function tail(value, limit = 500) {
  return value.length > limit ? value.slice(-limit) : value;
}

function lastLine(value) {
  const lines = value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  return lines.at(-1) ?? "";
}

function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function versionParts(text) {
  const match = /\b(\d+)\.(\d+)\.(\d+)\b/.exec(text);
  return match ? match.slice(1).map(Number) : undefined;
}

function isOlder(actual, minimum) {
  for (let index = 0; index < Math.max(actual.length, minimum.length); index += 1) {
    const left = actual[index] ?? 0;
    const right = minimum[index] ?? 0;
    if (left !== right) return left < right;
  }
  return false;
}

function clampedTimeout(value) {
  if (!Number.isFinite(value) || value <= 0) return DEFAULT_TIMEOUT_MS;
  return Math.min(Math.max(Math.trunc(value), 1), MAX_TIMEOUT_MS);
}

export class LyraClient {
  #bin;
  #env;
  #compat;

  constructor({ bin = process.env.LYRA_BIN?.trim() || "lyra", env = process.env } = {}) {
    this.#bin = bin;
    this.#env = env;
  }

  get bin() {
    return this.#bin;
  }

  async text(args, options = {}) {
    const result = await this.#run(args, options);
    return result.stdout.trim();
  }

  async json(args, options = {}) {
    await this.ensureCompatible(options);
    const result = await this.#run(args, options);
    const value = parseJson(result.stdout);
    if (value === undefined) {
      throw new LyraCliError(
        "invalid_output",
        `lyra ${args.join(" ")} did not return JSON`,
        {
          next: "Upgrade Lyra or inspect stderr for a CLI error.",
          exitCode: result.code,
          stderrTail: tail(result.stderr),
        },
      );
    }
    return value;
  }

  async ensureCompatible(options = {}) {
    if (!this.#compat) {
      this.#compat = this.#checkCompatibility(options).catch((error) => {
        this.#compat = undefined;
        throw error;
      });
    }
    return this.#compat;
  }

  async #checkCompatibility(options) {
    const result = await this.#run(["--version"], {
      cwd: options.cwd,
      signal: options.signal,
      timeoutMs: 5_000,
    });
    const actual = versionParts(result.stdout);
    if (actual && isOlder(actual, MIN_VERSION)) {
      throw new LyraCliError(
        "cli_too_old",
        `lyra ${actual.join(".")} does not provide the agent CLI contract`,
        {
          next: "Run `lyra upgrade` or set LYRA_BIN to a newer binary.",
          exitCode: 1,
        },
      );
    }
    return result.stdout.trim();
  }

  async #run(args, { cwd, input, signal, timeoutMs } = {}) {
    if (signal?.aborted) {
      throw new LyraCliError("aborted", "Lyra command was cancelled", {
        next: "The run remains recoverable; call lyra_status after reconnecting.",
      });
    }
    const timeout = clampedTimeout(timeoutMs);
    const child = spawn(this.#bin, args, {
      cwd,
      env: this.#env,
      stdio: ["pipe", "pipe", "pipe"],
    });

    return new Promise((resolve, reject) => {
      let stdout = "";
      let stderr = "";
      let truncated = false;
      let settled = false;
      let abortReason;
      let killTimer;
      let timeoutTimer;

      const cleanup = () => {
        clearTimeout(timeoutTimer);
        clearTimeout(killTimer);
        signal?.removeEventListener("abort", onAbort);
      };
      const finish = (error, result) => {
        if (settled) return;
        settled = true;
        cleanup();
        if (error) reject(error);
        else resolve(result);
      };
      const collect = (chunk, append) => {
        if (append.length >= MAX_OUTPUT_BYTES) {
          truncated = true;
          return append;
        }
        const remaining = MAX_OUTPUT_BYTES - append.length;
        const text = chunk.toString("utf8");
        if (text.length > remaining) truncated = true;
        return append + text.slice(0, remaining);
      };
      const terminate = (reason) => {
        if (settled || abortReason) return;
        abortReason = reason;
        child.kill("SIGTERM");
        killTimer = setTimeout(() => {
          if (!settled) child.kill("SIGKILL");
        }, KILL_GRACE_MS);
        killTimer.unref?.();
      };
      const onAbort = () => terminate("aborted");

      child.stdin?.on("error", () => {
        // A failed write means the child already exited; the close/error path owns the result.
      });
      child.stdout?.on("data", (chunk) => {
        stdout = collect(chunk, stdout);
      });
      child.stderr?.on("data", (chunk) => {
        stderr = collect(chunk, stderr);
      });
      child.on("error", (cause) => {
        if (cause.code === "ENOENT") {
          finish(
            new LyraCliError(
              "missing_binary",
              `Cannot find the Lyra binary (${this.#bin})`,
              {
                next: "Install with `curl -fsSL https://raw.githubusercontent.com/atom2ueki/lyra/main/install.sh | sh` or set LYRA_BIN.",
                cause,
              },
            ),
          );
          return;
        }
        finish(
          new LyraCliError("spawn_failed", `Failed to start lyra: ${cause.message}`, {
            next: "Check LYRA_BIN and the current working directory.",
            cause,
          }),
        );
      });
      child.on("close", (code) => {
        const exitCode = code ?? 1;
        if (abortReason === "timeout") {
          finish(
            new LyraCliError("timeout", `lyra ${args.join(" ")} timed out`, {
              next: "Retry with a longer timeout or inspect the run with lyra_status.",
              exitCode,
              stderrTail: tail(stderr),
            }),
          );
          return;
        }
        if (abortReason === "aborted") {
          finish(
            new LyraCliError("aborted", "Lyra command was cancelled", {
              next: "The run remains recoverable; call lyra_status after reconnecting.",
              exitCode,
              stderrTail: tail(stderr),
            }),
          );
          return;
        }
        if (truncated) {
          finish(
            new LyraCliError("output_too_large", "Lyra output exceeded the adapter limit", {
              next: "Use a narrower status or ticket query.",
              exitCode,
              stderrTail: tail(stderr),
            }),
          );
          return;
        }
        if (exitCode !== 0) {
          const envelope = parseJson(stdout);
          const error = envelope?.error;
          finish(
            new LyraCliError(
              typeof error?.code === "string" ? error.code : "cli_error",
              typeof error?.message === "string" && error.message
                ? error.message
                : lastLine(stderr) || `lyra exited ${exitCode}`,
              {
                next: typeof error?.next === "string" ? error.next : undefined,
                exitCode,
                stderrTail: tail(stderr),
              },
            ),
          );
          return;
        }
        finish(undefined, { code: exitCode, stdout, stderr });
      });

      timeoutTimer = setTimeout(() => terminate("timeout"), timeout);
      timeoutTimer.unref?.();
      signal?.addEventListener("abort", onAbort, { once: true });
      if (signal?.aborted) terminate("aborted");
      if (input === undefined) {
        child.stdin?.end();
      } else {
        child.stdin?.end(input);
      }
    });
  }
}
