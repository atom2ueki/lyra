import { existsSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { Ticket } from "./schema";

/**
 * `acceptance` is either a Markdown/document path (optionally with an #anchor) or prose. Path-like
 * values must resolve to an existing file under the project root; prose is not filesystem-checked.
 */
export function validateAcceptancePaths(tickets: Ticket[], projectRoot: string): string[] {
  const errors: string[] = [];
  for (const ticket of tickets) {
    const acceptance = ticket.acceptance?.trim();
    if (!acceptance) continue;
    const value = acceptance.replace(/#.*$/, "");
    const pathLike = /^(\.{0,2}\/|\/|docs\/)/.test(value) || value.endsWith(".md");
    if (!pathLike) continue;
    const target = isAbsolute(value) ? value : join(projectRoot, value);
    if (!existsSync(target)) {
      errors.push(`${ticket.id}.acceptance: file not found: ${value}`);
    }
  }
  return errors;
}
