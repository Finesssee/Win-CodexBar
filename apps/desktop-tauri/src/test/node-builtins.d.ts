// Minimal ambient declarations for the Node builtins used by test files.
// This workspace does not depend on @types/node; declaring only the touched
// signatures keeps tsc honest without pulling the full Node type surface.
// Delete this file if @types/node is ever added (the real declarations
// supersede these).
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}

// Node 20.11+ / vite-node inject import.meta.dirname|filename; those types
// also come from @types/node, so declare the touched member here. Note:
// import.meta.url is NOT reliable under vitest's jsdom transform (it can
// resolve relative URLs against the dev-server origin) — use dirname instead.
interface ImportMeta {
  readonly dirname?: string;
}
