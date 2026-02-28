/**
 * Bun test preload — runs before any test module is imported.
 *
 * Wired via bunfig.toml: [test] preload = ["./src/test-setup.ts"]
 *
 * Sets OMEGA_TEST=1 so that production write functions (appendContextMessage,
 * appendEvent, writeDiagnostic) can detect they are running under the
 * test suite and enforce isolation. See plan/backlog.md § "Structural
 * test-pollution prevention" for the full layer-a through layer-e plan.
 */
process.env.OMEGA_TEST = "1";
