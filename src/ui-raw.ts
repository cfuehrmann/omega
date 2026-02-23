/**
 * Terminal UI entry point — thin re-export shim.
 *
 * The implementation lives in src/terminal/:
 *   input.ts    — key parsing, line editing, displayWidth
 *   renderer.ts — ANSI block renderers
 *   app.ts      — application loop (runApp)
 *
 * This file is kept as the package.json entry point and as the import
 * target for tests that reference "./ui-raw.js", so nothing needs updating
 * when the internals move.
 */

export { parseKeys, displayWidth } from "./terminal/input.js";
export { renderToolStart, renderToolResult, renderApiRequest } from "./terminal/renderer.js";
export { runApp } from "./terminal/app.js";

// Entry point — only run when executed directly, not when imported in tests
if (import.meta.main) {
  const { runApp } = await import("./terminal/app.js");
  runApp().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
