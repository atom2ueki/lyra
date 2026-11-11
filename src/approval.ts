/**
 * Lyra does not implement a second tool-approval engine. It selects a launch posture and maps it to
 * the harness. OMP mapping is here; Codex/Claude adapters map the same posture to their own flags.
 */
export const APPROVAL_POSTURES = ["safe", "ask", "unsafe"] as const;
export type ApprovalPosture = (typeof APPROVAL_POSTURES)[number];

export function isApprovalPosture(value: unknown): value is ApprovalPosture {
  return typeof value === "string" && (APPROVAL_POSTURES as readonly string[]).includes(value);
}

export interface ApprovalMapping {
  posture: ApprovalPosture;
  ompArgs: string[];
  note: string;
}

export function ompApprovalMapping(posture: ApprovalPosture): ApprovalMapping {
  switch (posture) {
    case "safe":
      return {
        posture,
        ompArgs: ["--approval-mode", "always-ask"],
        note: "read auto; write/exec prompt; add tools.restrictedFolders for sensitive paths",
      };
    case "ask":
      return {
        posture,
        ompArgs: ["--approval-mode", "write"],
        note: "read/write auto; exec prompts; the visible-worker default",
      };
    case "unsafe":
      return {
        posture,
        ompArgs: ["--approval-mode", "yolo"],
        note: "all auto; only inside a sandbox or with an explicit per-task opt-in",
      };
  }
}

export function isUnsafePosture(posture: ApprovalPosture): boolean {
  return posture === "unsafe";
}
