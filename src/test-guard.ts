/**
 * Test-pollution guard (Layer b).
 *
 * When OMEGA_TEST=1 (set by src/test-setup.ts via bunfig.toml preload),
 * assertNotProductionPath() throws if a write function is about to write
 * to the production session path. This turns silent test pollution into a
 * loud, immediate test failure.
 *
 * The only guarded path is `.omega/sessions/` — the production session root.
 * `.omega/test-sessions/` is intentionally allowed: unit tests (via
 * makeTestAgent) and e2e tests both write there by design.
 *
 * In production OMEGA_TEST is never set, so this function is a no-op.
 */

const PRODUCTION_PREFIXES = [
  ".omega/sessions/", ".omega\\sessions\\",
  "diagnosis/", "diagnosis\\",
];

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
