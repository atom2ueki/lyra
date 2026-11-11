import { execFile } from "node:child_process";

export interface CommandResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export type CommandRunner = (
  file: string,
  args: string[],
  opts?: { cwd?: string; env?: NodeJS.ProcessEnv; timeoutMs?: number },
) => Promise<CommandResult>;

export const runCommand: CommandRunner = async (file, args, opts = {}) => {
  const { promise, resolve, reject } = Promise.withResolvers<CommandResult>();
  execFile(
    file,
    args,
    {
      cwd: opts.cwd,
      env: opts.env,
      timeout: opts.timeoutMs ?? 30_000,
      maxBuffer: 4 << 20,
    },
    (error, stdout, stderr) => {
      if (error && typeof (error as NodeJS.ErrnoException).code === "string") {
        reject(error);
        return;
      }
      resolve({
        stdout: String(stdout ?? ""),
        stderr: String(stderr ?? ""),
        exitCode: error ? (error.code as number | undefined) ?? 1 : 0,
      });
    },
  );
  return promise;
};

export function parseJson(stdout: string): unknown {
  const text = stdout.trim();
  if (!text) throw new Error("command returned no JSON");
  return JSON.parse(text);
}

export function resultOf(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const result = (value as Record<string, unknown>).result;
  return typeof result === "object" && result !== null && !Array.isArray(result)
    ? (result as Record<string, unknown>)
    : null;
}
