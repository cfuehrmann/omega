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

export const EditFileSchema = z.object({
  path:     z.string().describe("Path to the file (absolute or relative to cwd)"),
  old_text: z.string().describe("Exact text to find (must match exactly, must appear once)"),
  new_text: z.string().describe("Text to replace old_text with"),
});

export const ListFilesSchema = z.object({
  path:      z.string().describe("Directory path (absolute or relative to cwd)"),
  recursive: z.boolean().optional().describe("List recursively (optional, default false)"),
});

export const WebSearchSchema = z.object({
  query: z.string().describe("The search query"),
});

export const FetchUrlSchema = z.object({
  url:    z.string().describe("The URL to fetch (must be http or https)"),
  offset: z.number().optional().describe("Character offset to start reading from (optional, default 0). Use the value from the previous response footer to page through content longer than 20000 chars."),
});

export const GrepFilesSchema = z.object({
  pattern:       z.string().describe("Regex or literal string to search for"),
  path:          z.string().describe("Directory (or file) path to search in"),
  file_glob:     z.string().optional().describe("Optional glob to restrict which files are searched (e.g. '*.ts')"),
  context_lines: z.number().optional().describe("Number of context lines to include before and after each match (optional)"),
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

export const WaitProcessSchema = z.object({
  pid:       z.number().describe("Process ID returned by run_background"),
  timeoutMs: z.number().optional().describe("Maximum milliseconds to wait (optional, default 60000)"),
});

export const WaitForOutputSchema = z.object({
  logFile:   z.string().describe("Path to the log file to monitor — the logFile value returned by run_background."),
  timeoutMs: z.number().describe("Maximum milliseconds to wait before giving up and returning whatever the log contains."),
  pattern:   z.string().optional().describe("Return as soon as this string appears anywhere in the log (e.g. 'listening on', 'ready', 'Server started')."),
  minBytes:  z.number().optional().describe("Return as soon as the log reaches this many bytes. Useful when you don't know the ready signal but want to wait for meaningful output."),
});

export const WriteStdinSchema = z.object({
  pid:       z.number().describe("Process ID returned by run_background."),
  text:      z.string().describe("Text to write to the process stdin. Include a newline ('\\n') to submit a line-based prompt."),
  end_stdin: z.boolean().optional().describe("If true, close stdin after writing, signalling EOF to the process. Required for programs that read until end-of-input (e.g. cat). Default false."),
});
