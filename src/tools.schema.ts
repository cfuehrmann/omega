/**
 * Zod input schemas for every tool Omega exposes.
 *
 * Each schema serves two purposes:
 *   1. Runtime validation — `XxxSchema.parse(input)` in `executeTool` validates
 *      the JSON the LLM sends before the execute function touches it.
 *   2. JSON Schema generation — `toToolInputSchema(XxxSchema)` produces the
 *      `input_schema` object for the Anthropic API tool definition, replacing
 *      hand-written JSON Schema objects and keeping the two in sync.
 *
 * The `toToolInputSchema` helper uses `{ io: "input" }` so Zod does NOT emit
 * `additionalProperties: false` (preserving backward-compatible tool definitions)
 * and then strips Zod's top-level `$schema` key so the output matches the plain
 * `{ type: "object", properties: {...}, required: [...] }` shape the API expects.
 */

import { z } from "zod";

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/**
 * Convert a Zod object schema to the JSON Schema shape required by Anthropic's
 * `input_schema` field.  Descriptions attached via `.describe()` are preserved.
 */
export function toToolInputSchema(schema: z.ZodObject<z.ZodRawShape>): Record<string, unknown> {
  const js = z.toJSONSchema(schema, { io: "input" }) as Record<string, unknown>;
  delete js["$schema"];
  return js;
}

// ---------------------------------------------------------------------------
// Tool input schemas
// ---------------------------------------------------------------------------

export const ReadFileSchema = z.object({
  path:   z.string().describe("Path to the file (absolute or relative to cwd)"),
  offset: z.number().optional().describe("Starting line number (1-indexed, optional)"),
  limit:  z.number().optional().describe("Maximum number of lines to read (optional)"),
});

export const WriteFileSchema = z.object({
  path:    z.string().describe("Path to the file (absolute or relative to cwd)"),
  content: z.string().describe("Content to write to the file"),
});

export const RunCommandSchema = z.object({
  command: z.string().describe("The shell command to execute"),
  timeout: z.number().optional().describe("Timeout in seconds (optional, default 120)"),
});

const ReplacementSchema = z.object({
  old_text: z.string().describe("Exact text to find (must match exactly, must appear once)"),
  new_text: z.string().describe("Text to replace old_text with"),
});

export const EditFileSchema = z.object({
  path:         z.string().describe("Path to the file (absolute or relative to cwd)"),
  replacements: z.array(ReplacementSchema).describe("One or more replacements to apply in order. Each old_text must appear exactly once in the file. Pass all changes to this file together — never call edit_file on the same file twice in a row."),
});

export const ListFilesSchema = z.object({
  path:      z.string().describe("Directory path (absolute or relative to cwd)"),
  recursive: z.boolean().optional().describe("List recursively (optional, default false)"),
});

export const WebSearchSchema = z.object({
  query: z.string().describe("The search query"),
});

export const FetchUrlSchema = z.object({
  url:         z.string().describe("The URL to fetch (must be http or https)"),
  postprocess: z.string().describe(
    "Shell command to run on the downloaded text, received on stdin. " +
    "Examples: grep -n 'pattern', head -80, jq '.', awk '/foo/', python3 -c '...'. " +
    "Required: decide what to extract before fetching."
  ),
});

export const GrepFilesSchema = z.object({
  pattern:       z.string().describe("Regex or literal string to search for"),
  path:          z.string().describe("Directory (or file) path to search in"),
  file_glob:     z.string().optional().describe("Optional glob to restrict which files are searched (e.g. '*.ts')"),
  context_lines: z.number().optional().default(2).describe("Number of context lines to include before and after each match (default 2, pass 0 for bare matches)"),
  case_sensitive: z.boolean().optional().describe("If true, match is case-sensitive. Default: false (case-insensitive)"),
  max_results:   z.number().optional().describe("Maximum number of match lines to return (default 200)"),
});

export const FindFilesSchema = z.object({
  pattern:     z.string().describe("Glob or regex pattern to match against file/directory names"),
  path:        z.string().describe("Root directory to search in"),
  type:        z.string().optional().describe("Filter by entry type: 'f' (files), 'd' (directories), 'l' (symlinks). Omit for all."),
  hidden:      z.boolean().optional().describe("Include hidden files and .gitignore'd paths (default false)"),
  max_results: z.number().optional().describe("Maximum number of results to return (default 200)"),
});

export const RunBackgroundSchema = z.object({
  command: z.string().describe("Shell command to run in the background"),
  cwd:     z.string().optional().describe("Working directory for the process (optional, defaults to cwd)"),
});

export const WaitForOutputSchema = z.object({
  logFile:   z.string().describe("Path to the log file to monitor — the logFile value returned by run_background."),
  pid:       z.number().describe("The pid returned by run_background. Used to detect process exit: if the process dies before the pattern matches, wait_for_output returns immediately with processExited=true and the exit code, rather than waiting for the full timeout."),
  timeoutMs: z.number().describe("Maximum milliseconds to wait before giving up and returning whatever the log contains."),
  pattern:   z.string().optional().describe("Return as soon as this pattern matches anywhere in the log. Interpreted as a JavaScript regex, so use '|' for alternation (e.g. 'ready|started|Error'). Simple strings like 'ready' also work as-is."),
  minBytes:  z.number().optional().describe("Return as soon as the log reaches this many bytes. Useful when you don't know the ready signal but want to wait for meaningful output."),
});

export const WriteStdinSchema = z.object({
  pid:       z.number().describe("Process ID returned by run_background."),
  text:      z.string().describe("Text to write to the process stdin. Include a newline ('\\n') to submit a line-based prompt."),
  end_stdin: z.boolean().optional().describe("If true, close stdin after writing, signalling EOF to the process. Required for programs that read until end-of-input (e.g. cat). Default false."),
});

// ---------------------------------------------------------------------------
// Display helper — shared by terminal (tools.ts) and web (App.tsx)
// ---------------------------------------------------------------------------

/**
 * Extract the primary human-readable argument for a tool call.
 *
 * Returns the single most important display value — file path, command,
 * search query, etc. — without extra details like byte counts or flags.
 * Used by both the terminal `formatToolCall` and the web UI to avoid
 * duplicating tool-name knowledge.
 */
export function primaryToolArg(name: string, input: unknown): string {
  if (input == null) return "(none)";
  const inp = input as Record<string, unknown>;
  switch (name) {
    case "read_file":
    case "write_file":
    case "edit_file":
      return String(inp.path ?? "");
    case "list_files":
      return String(inp.path ?? "");
    case "find_files":
      return String(inp.pattern ?? "");
    case "run_command":
    case "run_background":
      return String(inp.command ?? "");
    case "grep_files":
      return `${inp.pattern} @ ${inp.path}`;
    case "fetch_url":
      return String(inp.url ?? "");
    case "web_search":
      return String(inp.query ?? "");
    case "wait_for_output":
      return String(inp.logFile ?? "");
    case "write_stdin":
      return String(inp.text ?? "");
    default:
      return typeof input === "object" ? JSON.stringify(input) : String(input);
  }
}
