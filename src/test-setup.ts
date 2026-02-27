/**
 * Bun test preload — runs before any test module is imported.
 *
 * Wired via bunfig.toml: [test] preload = ["./src/test-setup.ts"]
 */

// No global setup needed at this time.
// Test isolation for session files (sessions/, diagnosis/) is handled by the
// Agent constructor heuristic: mock streamProvider → all file paths default to null.
