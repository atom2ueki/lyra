export const graphFormats = ["mermaid", "dot", "json"] as const;
export type GraphFormat = (typeof graphFormats)[number];
