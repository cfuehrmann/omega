/**
 * System prompt assembly.
 *
 * Combines all parts into the final string sent as the `system` field on
 * every API call:
 *
 *   [core instructions]        — role, tools, working dir, policies
 *   [append section]           — optional, from .omega/system-prompt-append.md
 *
 * Each part lives in its own file and can be read, tested, and edited
 * independently.
 */

import { corePrompt } from "./core.js";
import { formatAppendSection } from "./append.js";

export interface BuildSystemPromptArgs {
  cwd: string;
  maxOutputTokens: number;
  /** Pre-loaded append content (null = file was absent). */
  appendContent: string | null;
}

/**
 * Assemble the complete system prompt from all parts.
 */
export function buildSystemPrompt({
  cwd,
  maxOutputTokens,
  appendContent,
}: BuildSystemPromptArgs): string {
  const parts: string[] = [];

  parts.push(corePrompt({ cwd, maxOutputTokens }));

  const appendSection = formatAppendSection(appendContent);
  if (appendSection) parts.push(appendSection);

  return parts.join("\n\n");
}
