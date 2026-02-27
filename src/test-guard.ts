/**
 * Test-pollution guard (Layer b).
 *
 * When OMEGA_TEST=1 (set by src/test-setup.ts via bunfig.toml preload),
 * assertNotProductionPath() throws if a write function is about to write
 * to a production path. This turns silent test pollution into a loud,
 * immediate test failure.
 *
 * Production paths are anything under sessions/ or diagnosis/ (relative
 * to the working directory). Explicit temp-dir paths used by tests that
 * legitimately write files (e.g. context-hash.test.ts) are unaffected.
 *
 * In production OMEGA_TEST is never set, so this function is a no-op.
 */

const PRODUCTION_PREFIXES = ["sessions/", "sessions\\", "diagnosis/", "diagnosis\\"];

export function assertNotProductionPath(filePath: string, fnName: string): void {
  if (process.env.OMEGA_TEST !== "1") return;
  const rel = filePath.replace(/\\/g, "/");
  for (const prefix of PRODUCTION_PREFIXES) {
    if (rel.startsWith(prefix.replace(/\\/g, "/")) || rel === prefix.replace(/\\/g, "/").replace(/\/$/, "")) {
      throw new Error(
        `[OMEGA_TEST] ${fnName} attempted to write to production path "${filePath}". ` +
        `Pass an explicit null or a temp-dir path instead. ` +
        `See plan/backlog.md § "Structural test-pollution prevention".`
      );
    }
  }
}
