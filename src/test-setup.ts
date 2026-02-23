/**
 * Bun test preload — runs before any test module is imported.
 *
 * Sets OMEGA_LOG_FILE=/dev/null so that logger.ts (loaded transitively by
 * agent.ts and others) does NOT write to or rotate the production omega.log.
 *
 * Wired via bunfig.toml: [test] preload = ["./src/test-setup.ts"]
 */
process.env.OMEGA_LOG_FILE = "/dev/null";
