/**
 * System prompt — identity prefix.
 *
 * Previously this prepended "You are Claude Code, Anthropic's official CLI for Claude."
 * when using OAuth auth. Empirical testing (2025) showed that prefix has no effect on
 * authentication — only the HTTP headers (anthropic-beta: claude-code-20250219,oauth-2025-04-20
 * and x-app: cli) gate OAuth access. The prefix was cargo-cult and has been removed.
 *
 * File retained as a tombstone so git history explains why it's gone.
 */
