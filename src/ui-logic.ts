/**
 * Pure UI logic — no React, no Ink, fully unit-testable.
 *
 * Extracted from ui.tsx so shortcut guards and other logic can be tested
 * without needing ink-testing-library.
 */

export interface ShortcutContext {
  /** Current value of the text input prompt. */
  inputValue: string;
  /** Whether the agent is currently streaming a response. */
  isStreaming: boolean;
  /** Whether a tool confirmation is pending. */
  hasPendingTool: boolean;
  /** Whether the agent is initialised and ready. */
  isReady: boolean;
  /** Whether the session resume prompt has been resolved. */
  resumeDone: boolean;
}

/**
 * Returns true if the given single-character shortcut key should be handled
 * as a global keyboard shortcut rather than passed to the text input.
 *
 * Shortcuts only fire when:
 *   - The key is a known shortcut (currently: i, q)
 *   - The prompt is empty (user is not mid-typing)
 *   - The agent is idle (not streaming, no pending tool)
 *   - The UI is fully ready and past any resume prompt
 */
export function shouldHandleShortcut(key: string, ctx: ShortcutContext): boolean {
  if (key !== "i" && key !== "q") return false;
  if (ctx.inputValue.length > 0) return false;
  if (ctx.isStreaming) return false;
  if (ctx.hasPendingTool) return false;
  if (!ctx.isReady) return false;
  if (!ctx.resumeDone) return false;
  return true;
}
