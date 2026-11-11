export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

export function envPrefix(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${shellQuote(value)}`)
    .join(" ");
}

export function commandLine(bin: string, args: string[], env: Record<string, string> = {}): string {
  const prefix = envPrefix(env);
  return `${prefix ? `${prefix} ` : ""}${shellQuote(bin)} ${args.map(shellQuote).join(" ")}`;
}
