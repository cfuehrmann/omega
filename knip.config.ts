import type { KnipConfig } from "knip";

// Phase 3.7: knip is configured via the sibling `knip.json` (which
// `bunx knip` auto-discovers ahead of this file). The TS-side surface
// is just `e2e/` Playwright specs now; this file is kept for
// historical / local-extension purposes and mirrors `knip.json`.
const config: KnipConfig = {
  entry: ["e2e/**/*.spec.ts"],
  project: ["e2e/**/*.ts"],
};

export default config;
