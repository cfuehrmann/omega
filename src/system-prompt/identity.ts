/**
 * System prompt — Part 1: Identity prefix.
 *
 * The Claude Code identity string must appear at the very start of the system
 * prompt when using Claude Max OAuth tokens (sk-ant-oat-…). Omitting it causes
 * authentication errors. It is NOT prepended on the API-key path.
 */

/** The exact identity string required by Claude Max OAuth. */
export const CLAUDE_CODE_IDENTITY =
  "You are Claude Code, Anthropic's official CLI for Claude.";

/**
 * Return the identity prefix for the given auth mode.
 * Returns the Claude Code string for OAuth; empty string for API-key auth
 * (caller should skip the empty string rather than prepending a blank line).
 */
export function identityPrefix(authMode: "oauth" | "api-key"): string {
  return authMode === "oauth" ? CLAUDE_CODE_IDENTITY : "";
}
