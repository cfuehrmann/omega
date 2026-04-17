import { readFile, writeFile, readdir, stat, mkdir } from "fs/promises";
import { createHash } from "crypto";
import { join, relative } from "path";
import { spawn } from "child_process";
import { tmpdir } from "os";
import { z } from "zod";
import type Anthropic from "@anthropic-ai/sdk";
import {
  toToolInputSchema,
  ReadFileSchema,
  WriteFileSchema,
  RunCommandSchema,
  EditFileSchema,
  ListFilesSchema,
  WebSearchSchema,
  FetchUrlSchema,
  GrepFilesSchema,
  FindFilesSchema,
  RunBackgroundSchema,
  WaitForOutputSchema,
  WriteStdinSchema,
} from "./tools.schema.js";

// ---------------------------------------------------------------------------
// Web search (DuckDuckGo) + URL fetch helpers
// ---------------------------------------------------------------------------

/**
 * Maximum characters returned by a single web_search call.
 * 10 results × ~200–500 chars each typically yields ≈ 2–5 k chars;
 * 8 000 provides comfortable headroom without flooding the context window.
 */
const WEB_SEARCH_MAX_CHARS = 8_000;

/**
 * Characters returned per fetch_url window. Balances readability (a full
 * section of a doc page) against context size. The agent can paginate with
 * the offset parameter when more content is needed.
 */
const FETCH_URL_MAX_CHARS = 20_000;

/**
 * HTTP timeout for Brave Search API requests. 10 s is generous for a JSON
 * search endpoint; failing fast avoids blocking the agent turn on a slow
 * external service.
 */
const WEB_SEARCH_TIMEOUT_MS = 10_000;

/**
 * HTTP timeout for fetch_url requests. Slightly longer than the search
 * timeout to accommodate slow or large pages (heavy documentation sites,
 * GitHub blobs, etc.).
 */
const FETCH_URL_TIMEOUT_MS = 15_000;

/**
 * Maximum lines returned by read_file before truncation. 2 000 lines covers
 * the vast majority of source files; larger files should be read in sections
 * via offset/limit.
 */
const READ_FILE_MAX_LINES = 2_000;

/**
 * Maximum bytes returned by read_file before truncation. Byte-level fallback
 * for files where line count is low but content is dense (e.g. minified JS).
 * 50 KB comfortably fits within the context window as a single read.
 */
const READ_FILE_MAX_BYTES = 50_000;

/**
 * Maximum directory entries returned by list_files (flat or recursive).
 * 1 000 covers even large monorepos at a single level; prevents accidental
 * full-tree dumps of enormous repositories.
 */
const LIST_FILES_MAX_ENTRIES = 1_000;

/**
 * Default run_command timeout in seconds. 120 s covers the vast majority of
 * build commands, test suites, and CLI tools. The caller can pass a higher
 * value for known long-running commands (e.g. a slow integration test suite).
 */
const RUN_COMMAND_DEFAULT_TIMEOUT_S = 120;

/**
 * Per-stream stdout/stderr cap for run_command before killing the process.
 * 100 KB is ample for typical tool output while preventing runaway commands
 * from filling the context window. Matched by MAX_TOOL_OUTPUT_CHARS.
 */
const RUN_COMMAND_OUTPUT_CAP_BYTES = 100_000;

/**
 * Polling interval for wait_for_output. 200 ms gives prompt detection of
 * log output without busy-polling. Fine-grained enough for dev-server
 * readiness checks while adding at most one poll-cycle of extra latency.
 */
const WAIT_FOR_OUTPUT_POLL_MS = 200;

/**
 * Universal ceiling on the characters returned by any single tool call.
 * Prevents unexpectedly large outputs (e.g. an offset-less read_file on a
 * 1 MB file) from consuming the full context window. Matched to
 * RUN_COMMAND_OUTPUT_CAP_BYTES so the live cap and the final catch-all cap
 * are consistent.
 */
const MAX_TOOL_OUTPUT_CHARS = 100_000;

/**
 * Bun's TLS implementation may not trust the system CA bundle on some machines.
 * This option disables certificate verification for outbound fetch calls.
 * Acceptable for an agent reading public web content (we're not sending secrets).
 */
const FETCH_TLS_OPTIONS = { tls: { rejectUnauthorized: false } };

/**
 * Maximum characters returned from a postprocess command before truncation.
 * Small enough to keep the tool result context-friendly; the full downloaded
 * content is always available in the cache file for follow-up queries.
 */
const FETCH_URL_POSTPROCESS_MAX_CHARS = 8_000;

/**
 * Session-scoped web cache directory. Created lazily on first fetch_url call.
 * Keyed on process.pid so it is unique per Omega session and automatically
 * abandoned when the process exits (no stale-content risk).
 */
let webCacheDirPath: string | null = null;

async function getWebCacheDir(): Promise<string> {
  if (webCacheDirPath) return webCacheDirPath;
  const dir = join(tmpdir(), `omega-webcache-${process.pid}`);
  await mkdir(dir, { recursive: true });
  webCacheDirPath = dir;
  return webCacheDirPath;
}

/**
 * Strip HTML tags and collapse whitespace, returning plain text.
 * Good enough for readability without a full DOM parser.
 */
function htmlToText(html: string): string {
  // Remove <script> and <style> blocks entirely
  let text = html
    .replace(/<script[\s\S]*?<\/script>/gi, " ")
    .replace(/<style[\s\S]*?<\/style>/gi, " ");
  // Replace block-level tags with newlines
  text = text.replace(/<\/?(p|div|br|li|h[1-6]|tr|td|th|blockquote)[^>]*>/gi, "\n");
  // Strip remaining tags
  text = text.replace(/<[^>]+>/g, "");
  // Decode common HTML entities
  text = text
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, " ");
  // Collapse whitespace
  text = text.replace(/[ \t]+/g, " ").replace(/\n{3,}/g, "\n\n").trim();
  return text;
}

async function executeWebSearch(input: { query: string }): Promise<string> {
  if (!input.query || !input.query.trim()) {
    throw new Error("query is required");
  }
  const braveKey = process.env.BRAVE_SEARCH_API_KEY;
  if (!braveKey) {
    throw new Error(
      "BRAVE_SEARCH_API_KEY is not set. Web search requires a Brave Search API key.",
    );
  }

  const q = encodeURIComponent(input.query.trim());
  const url = `https://api.search.brave.com/res/v1/web/search?q=${q}&count=10&text_decorations=0&search_lang=en`;
  const res = await fetch(url, {
    headers: {
      "Accept": "application/json",
      "Accept-Encoding": "gzip",
      "X-Subscription-Token": braveKey,
    },
    signal: AbortSignal.timeout(10_000),
    ...FETCH_TLS_OPTIONS,
  } as any);
  if (!res.ok) throw new Error(`Brave Search API error: ${res.status}`);

  const data = await res.json() as any;
  const webResults: Array<{ title: string; url: string; description?: string }> =
    data?.web?.results ?? [];

  if (webResults.length === 0) return "No results found.";

  const lines: string[] = ["Results:"];
  for (const r of webResults) {
    lines.push(`• ${r.url}`);
    lines.push(`  ${r.title}`);
    if (r.description) lines.push(`  ${r.description}`);
  }
  const output = lines.join("\n");
  return output.length > WEB_SEARCH_MAX_CHARS
    ? output.slice(0, WEB_SEARCH_MAX_CHARS) + "\n[truncated]"
    : output;
}

async function executeFetchUrl(input: { url: string; postprocess: string }): Promise<string> {
  if (!input.url?.trim()) throw new Error("url is required");
  if (!input.postprocess?.trim()) throw new Error("postprocess is required");

  // Validate URL
  let parsed: URL;
  try {
    parsed = new URL(input.url.trim());
  } catch {
    throw new Error(`Invalid URL: ${input.url}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`Unsupported protocol: ${parsed.protocol}`);
  }

  // Content-addressed cache: SHA-256 of the normalized URL.
  // Same URL → same file; re-download is skipped within the session.
  const urlHash = createHash("sha256").update(parsed.href).digest("hex");
  const cacheDir = await getWebCacheDir();
  const cacheFile = join(cacheDir, `${urlHash}.txt`);

  // Download only if not already cached this session
  const fileExists = await stat(cacheFile).then(() => true).catch(() => false);
  let charCount: number;
  if (!fileExists) {
    const res = await fetch(parsed.href, {
      headers: {
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36",
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
      },
      signal: AbortSignal.timeout(FETCH_URL_TIMEOUT_MS),
      ...FETCH_TLS_OPTIONS,
    } as any);
    if (!res.ok) throw new Error(`HTTP ${res.status}: ${res.statusText}`);

    const contentType = res.headers.get("content-type") ?? "";
    const isHtml = contentType.includes("text/html") || contentType.includes("application/xhtml");
    const body = await res.text();
    const text = isHtml ? htmlToText(body) : body;
    await writeFile(cacheFile, text, "utf-8");
    charCount = text.length;
  } else {
    const content = await readFile(cacheFile, "utf-8");
    charCount = content.length;
  }

  // Run postprocess command with the cached file piped to stdin.
  // Single-quoting the path is safe: it contains only hex chars + separators.
  const wrappedCmd = `${input.postprocess.trim()} < '${cacheFile}'`;
  const postResult = await new Promise<{ stdout: string; stderr: string; code: number | null }>(
    (resolve) => {
      let stdout = "";
      let stderr = "";
      const proc = spawn("bash", ["-c", wrappedCmd], {
        stdio: ["ignore", "pipe", "pipe"],
      });
      proc.stdout.on("data", (d: Buffer) => { stdout += d.toString(); });
      proc.stderr.on("data", (d: Buffer) => { stderr += d.toString(); });
      proc.on("close", (code) => resolve({ stdout, stderr, code }));
      proc.on("error", (err) => resolve({ stdout: "", stderr: err.message, code: -1 }));
    }
  );

  // code 0 = success; code 1 = grep/rg "no matches" — not an error
  const ppIsError = postResult.code !== 0 && postResult.code !== 1;
  let ppOut = ppIsError
    ? (postResult.stderr.trim() || `[exit code ${postResult.code}]`)
    : postResult.stdout;

  let truncated = false;
  if (ppOut.length > FETCH_URL_POSTPROCESS_MAX_CHARS) {
    ppOut = ppOut.slice(0, FETCH_URL_POSTPROCESS_MAX_CHARS);
    truncated = true;
  }

  let result = `Cached: ${cacheFile} (${charCount} chars)\n`;
  result += `\n--- postprocess: ${input.postprocess.trim()} ---\n`;
  if (ppIsError) {
    result += `[error] ${ppOut}`;
  } else if (!ppOut.trim()) {
    result += "(no output)";
  } else {
    result += ppOut.trimEnd();
    if (truncated) {
      result += `\n[postprocess output truncated at ${FETCH_URL_POSTPROCESS_MAX_CHARS} chars — use read_file or grep_files on the cached file for more]`;
    }
  }
  result += "\n--- end ---";

  return result;
}

// Tool definitions for the Anthropic API
export const toolDefinitions: Anthropic.Beta.Messages.BetaTool[] = [
  {
    name: "read_file",
    description:
      "Read the contents of a file. Returns the file content as text. " +
      "For large files, use offset and limit to read a specific line range.",
    input_schema: toToolInputSchema(ReadFileSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "write_file",
    description:
      "Write content to a file. Creates the file if it doesn't exist, " +
      "overwrites if it does. Creates parent directories as needed. " +
      "WARNING: file content is generated inside the output token budget. " +
      "Files longer than ~500 lines or ~20 000 characters risk being cut off mid-write. " +
      "For large new files write a skeleton first, then extend with edit_file. " +
      "For large existing files always prefer edit_file over a full rewrite.",
    input_schema: toToolInputSchema(WriteFileSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "run_command",
    description:
      "Execute a shell command and return its stdout, stderr, and exit code. " +
      "The command runs in the current working directory. " +
      "The default timeout is 120 s — pass a higher value for very slow commands.",
    input_schema: toToolInputSchema(RunCommandSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "edit_file",
    description:
      "Edit a file by replacing exact text. The old_text must match exactly " +
      "(including whitespace and indentation). Use this for surgical edits " +
      "instead of rewriting entire files with write_file. Each old_text must " +
      "appear exactly once in the file. For multiple edits to the same file, " +
      "pass a `replacements` array — this is faster and avoids round-trips. " +
      "Always pass ALL changes to a file in a single call.",
    input_schema: toToolInputSchema(EditFileSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "list_files",
    description:
      "List files and directories. Returns names with '/' suffix for directories. " +
      "Use recursive to list the full tree (up to 1000 entries).",
    input_schema: toToolInputSchema(ListFilesSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "web_search",
    description:
      "Search the web using Brave Search. Returns titles, URLs, and snippets for the top results. " +
      "Use this to look up documentation, current information, or anything not in local files.",
    input_schema: toToolInputSchema(WebSearchSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "fetch_url",
    description:
      "Download a URL to a session-local cache file (content-addressed by URL hash) and " +
      "immediately run a shell postprocessing command on the full downloaded text. " +
      "HTML is converted to readable text before caching. " +
      "The tool result contains the cache file path and postprocess output (≤ 8 000 chars). " +
      "For any further queries on the same content, use read_file or grep_files on the cache path. " +
      "postprocess is required and receives the full content on stdin. " +
      "Prefer grep or awk when you know what to look for, head -N as the catch-all. " +
      "Never use cat — head -N gives the same result on short pages and stays bounded on long ones.",
    input_schema: toToolInputSchema(FetchUrlSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "grep_files",
    description:
      "Search for a pattern across files in a directory using ripgrep (rg) with grep fallback. " +
      "Returns structured file:line:text matches, capped at max_results (default 200). " +
      "Use this to find all occurrences of a symbol, string, or regex across the codebase " +
      "instead of reading files speculatively. Chain with read_file to inspect context. " +
      "By default includes 2 context lines around each match; pass 0 for bare matches.",
    input_schema: toToolInputSchema(GrepFilesSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "find_files",
    description:
      "Find files and directories by name/glob pattern using fd (with find fallback). " +
      "Returns a list of matching paths, capped at max_results (default 200). " +
      "Use this to locate files when you know the name or extension but not the exact path. " +
      "Ignores hidden files and .gitignore'd paths by default (set hidden=true to include them). " +
      "Chain with read_file or grep_files to inspect contents.",
    input_schema: toToolInputSchema(FindFilesSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "run_background",
    description:
      "Start a long-running process in the background and return immediately. " +
      "stdout and stderr are redirected to a temporary log file. " +
      "Returns { pid, logFile }. " +
      "Use read_file on logFile (with offset/limit for large output) and grep_files to inspect output. " +
      "Use run_command(\"kill <pid>\") to stop the process early. " +
      "Reserve this for processes that must stay alive indefinitely " +
      "(dev servers, file watchers, interactive processes that need write_stdin). " +
      "For finite commands (builds, test suites, commits), prefer run_command with a sufficient timeout.",
    input_schema: toToolInputSchema(RunBackgroundSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "wait_for_output",
    description:
      "Poll a background-process log file until a condition is met, then return the log contents. " +
      "Returns when the FIRST of these occurs: (1) pattern appears in the log, " +
      "(2) log reaches minBytes in size, or (3) timeoutMs elapses. " +
      "If neither pattern nor minBytes is given, returns as soon as any output appears. " +
      "Returns { output, matched, minBytesReached, timedOut }. " +
      "Use this after run_background instead of sleep + tail to wait for a server or process to become ready.",
    input_schema: toToolInputSchema(WaitForOutputSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "write_stdin",
    description:
      "Write text to the stdin of a background process started with run_background. " +
      "Use this to answer interactive prompts (e.g. y/n confirmations, passwords, menu choices). " +
      "Include a newline ('\\n') at the end of text to submit a line-based prompt. " +
      "Set end_stdin=true to close stdin after writing, signalling EOF to the process " +
      "(required for programs like cat that read until end of input). " +
      "Returns an error if the pid is not a tracked background process or stdin is already closed.",
    input_schema: toToolInputSchema(WriteStdinSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
];

// Tool execution

export interface ToolResult {
  output: string;
  isError: boolean;
  durationMs: number;
}

async function executeReadFile(input: {
  path: string;
  offset?: number;
  limit?: number;
}): Promise<string> {
  const content = await readFile(input.path, "utf-8");
  const lines = content.split("\n");

  if (input.offset || input.limit) {
    const start = (input.offset ?? 1) - 1;
    const end = input.limit ? start + input.limit : lines.length;
    const slice = lines.slice(start, end);
    const totalLines = lines.length;
    let result = slice.join("\n");
    if (end < totalLines) {
      result += `\n\n[${totalLines - end} more lines. Use offset=${end + 1} to continue.]`;
    }
    return result;
  }

  // Truncate very large files
  const MAX_LINES = 2000;
  const MAX_BYTES = 50_000;
  if (lines.length > MAX_LINES) {
    return (
      lines.slice(0, MAX_LINES).join("\n") +
      `\n\n[Truncated. ${lines.length - MAX_LINES} more lines. Use offset/limit to read more.]`
    );
  }
  if (content.length > MAX_BYTES) {
    return (
      content.slice(0, MAX_BYTES) +
      `\n\n[Truncated at ${MAX_BYTES} bytes. Use offset/limit to read more.]`
    );
  }
  return content;
}

async function executeWriteFile(input: {
  path: string;
  content: string;
}): Promise<string> {
  const { mkdir } = await import("fs/promises");
  const { dirname } = await import("path");
  await mkdir(dirname(input.path), { recursive: true });
  await writeFile(input.path, input.content, "utf-8");
  const lines = input.content.split("\n").length;
  return `Wrote ${input.content.length} bytes (${lines} lines) to ${input.path}`;
}

async function executeEditFile(input: {
  path: string;
  replacements: { old_text: string; new_text: string }[];
}): Promise<string> {
  const { replacements } = input;
  if (!replacements || replacements.length === 0) {
    throw new Error("edit_file requires a non-empty replacements array.");
  }

  let content = await readFile(input.path, "utf-8");
  const summaries: string[] = [];

  for (let i = 0; i < replacements.length; i++) {
    const { old_text, new_text } = replacements[i]!;

    // Count occurrences
    let count = 0;
    let idx = -1;
    while ((idx = content.indexOf(old_text, idx + 1)) !== -1) {
      count++;
    }

    const label = replacements.length > 1 ? ` (replacement ${i + 1}/${replacements.length})` : "";
    if (count === 0) {
      throw new Error(
        `old_text not found in ${input.path}${label}. Make sure it matches exactly (including whitespace).`
      );
    }
    if (count > 1) {
      throw new Error(
        `old_text found ${count} times in ${input.path}${label}. It must appear exactly once. Use a larger/more unique snippet.`
      );
    }

    content = content.replace(old_text, new_text);
    const oldLines = old_text.split("\n").length;
    const newLines = new_text.split("\n").length;
    summaries.push(`replaced ${oldLines} line(s) with ${newLines} line(s)`);
  }

  await writeFile(input.path, content, "utf-8");

  if (summaries.length === 1) {
    return `edit_file: ${input.path} — ${summaries[0]}`;
  }
  return `edit_file: ${input.path} — ${summaries.length} replacements applied:\n${summaries.map((s, i) => `  ${i + 1}. ${s}`).join("\n")}`;
}

function executeRunCommand(
  input: { command: string; timeout?: number },
  signal?: AbortSignal,
): Promise<string> {
  const timeoutMs = (input.timeout ?? 120) * 1000;
  const timeoutS = input.timeout ?? 120;

  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    let killed = false;
    let killedByAbort = false;
    let killedByTimeout = false;
    // Settled flag: once true, the Promise has been resolved and subsequent
    // proc.on("close") / error callbacks are no-ops. This is necessary because
    // orphaned child processes (e.g. bun test worker threads) can keep the pipe
    // FDs alive long after bash has been killed — causing proc.on("close") to
    // fire far beyond the requested timeout. We resolve immediately on timeout
    // or abort rather than waiting for all pipe writers to exit.
    let settled = false;

    const settle = (result: string): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timeoutHandle);
      signal?.removeEventListener("abort", onAbort);
      resolve(result);
    };

    const buildResult = (suffix: string): string => {
      let result = "";
      if (stdout) result += stdout;
      if (stderr) result += (result ? "\n" : "") + `[stderr]\n${stderr}`;
      result += suffix;
      return result || "(no output)";
    };

    const proc = spawn("bash", ["-c", input.command], {
      stdio: ["ignore", "pipe", "pipe"],
      // detached: true makes bash a new process-group leader (PGID = PID).
      // Combined with killGroup() below, this ensures SIGKILL reaches bash
      // AND all its child processes (e.g. bun test worker threads), preventing
      // orphan leaks when the timeout or abort fires.
      detached: true,
    });

    // Kill bash and its entire process group.
    const killGroup = () => {
      try {
        process.kill(-proc.pid!, "SIGKILL");
      } catch {
        // Process may have already exited — ignore ESRCH.
      }
    };

    // Manual timeout: kill the process group and resolve immediately without
    // waiting for proc.on("close"). Orphaned children (e.g. spawned by bun
    // test) keep the pipe FDs open after bash exits, which can delay close by
    // many minutes.
    const timeoutHandle = setTimeout(() => {
      killedByTimeout = true;
      killGroup();
      settle(buildResult(`\n[killed: timeout after ${timeoutS}s]`));
    }, timeoutMs);

    // Kill the process group immediately when the abort signal fires, and
    // resolve without waiting for close (same orphan-pipe concern as timeout).
    const onAbort = () => {
      killedByAbort = true;
      killGroup();
      settle(buildResult("\n[killed by abort signal]"));
    };
    if (signal?.aborted) {
      onAbort();
    } else {
      signal?.addEventListener("abort", onAbort, { once: true });
    }

    proc.stdout.on("data", (data: Buffer) => {
      stdout += data.toString();
      // Cap output size
      if (stdout.length > 100_000) {
        killGroup();
        killed = true;
      }
    });

    proc.stderr.on("data", (data: Buffer) => {
      stderr += data.toString();
      if (stderr.length > 100_000) {
        killGroup();
        killed = true;
      }
    });

    proc.on("close", (code) => {
      let suffix = "";
      if (killedByAbort) suffix = "\n[killed by abort signal]";
      else if (killedByTimeout) suffix = `\n[killed: timeout after ${timeoutS}s]`;
      else if (killed) suffix = "\n[Output truncated at 100KB]";
      else if (code !== 0 && code !== null) suffix = `\n[exit code: ${code}]`;
      settle(buildResult(suffix));
    });

    proc.on("error", (err) => {
      settle(`[error: ${err.message}]`);
    });
  });
}

async function executeListFiles(input: {
  path: string;
  recursive?: boolean;
}): Promise<string> {
  const results: string[] = [];
  const MAX_ENTRIES = 1000;

  async function walk(dir: string, depth: number) {
    if (results.length >= MAX_ENTRIES) return;
    const entries = await readdir(dir, { withFileTypes: true });
    // Sort: directories first, then alphabetical
    entries.sort((a, b) => {
      if (a.isDirectory() && !b.isDirectory()) return -1;
      if (!a.isDirectory() && b.isDirectory()) return 1;
      return a.name.localeCompare(b.name);
    });
    for (const entry of entries) {
      if (results.length >= MAX_ENTRIES) break;
      // Skip hidden dirs at top level when not recursive, skip node_modules always
      if (entry.name === "node_modules") continue;
      if (entry.name.startsWith(".") && depth === 0 && !input.recursive) continue;
      const rel = relative(input.path, join(dir, entry.name));
      if (entry.isDirectory()) {
        results.push(rel + "/");
        if (input.recursive && !entry.name.startsWith(".git") && entry.name !== "node_modules") {
          await walk(join(dir, entry.name), depth + 1);
        }
      } else {
        results.push(rel);
      }
    }
  }

  await walk(input.path, 0);
  let output = results.join("\n");
  if (results.length >= MAX_ENTRIES) {
    output += `\n\n[Truncated at ${MAX_ENTRIES} entries]`;
  }
  return output;
}

async function executeGrepFiles(input: {
  pattern: string;
  path: string;
  file_glob?: string;
  context_lines?: number;
  case_sensitive?: boolean;
  max_results?: number;
}): Promise<string> {
  const maxResults = input.max_results ?? 200;
  const caseSensitive = input.case_sensitive ?? false;

  // Try ripgrep first, fall back to grep
  const hasRg = await new Promise<boolean>((resolve) => {
    const p = spawn("which", ["rg"], { stdio: "ignore" });
    p.on("close", (code) => resolve(code === 0));
    p.on("error", () => resolve(false));
  });

  let args: string[];
  if (hasRg) {
    args = [
      "rg",
      "--line-number",
      "--with-filename",
      "--no-heading",
      ...(caseSensitive ? ["--case-sensitive"] : ["--ignore-case"]),
      ...(input.file_glob ? ["--glob", input.file_glob] : []),
      ...(input.context_lines ? ["--context", String(input.context_lines)] : []),
      "--", // end of flags — prevents patterns like --color from being parsed as flags
      input.pattern,
      input.path,
    ];
  } else {
    args = [
      "grep",
      "-rn",
      ...(caseSensitive ? [] : ["-i"]),
      ...(input.file_glob ? [`--include=${input.file_glob}`] : []),
      ...(input.context_lines ? [`-C${input.context_lines}`] : []),
      "--", // end of flags
      input.pattern,
      input.path,
    ];
  }

  const output = await new Promise<{ stdout: string; stderr: string; code: number | null }>(
    (resolve) => {
      let stdout = "";
      let stderr = "";
      const [cmd, ...cmdArgs] = args as [string, ...string[]];
      const proc = spawn(cmd, cmdArgs, { stdio: ["ignore", "pipe", "pipe"] });
      proc.stdout.on("data", (d: Buffer) => { stdout += d.toString(); });
      proc.stderr.on("data", (d: Buffer) => { stderr += d.toString(); });
      proc.on("close", (code) => resolve({ stdout, stderr, code }));
      proc.on("error", (err) => resolve({ stdout: "", stderr: err.message, code: -1 }));
    }
  );

  // Exit code 1 from grep/rg means "no matches" — not an error
  if (output.code !== 0 && output.code !== 1) {
    const msg = output.stderr.trim() || `Search failed (exit ${output.code})`;
    throw new Error(msg);
  }

  const raw = output.stdout.trim();
  if (!raw) return "No matches found.";

  const lines = raw.split("\n");
  if (lines.length <= maxResults) {
    return lines.join("\n");
  }

  const truncated = lines.slice(0, maxResults).join("\n");
  return `${truncated}\n\n[truncated: showing ${maxResults} of ${lines.length} matches]`;
}

async function executeFindFiles(input: {
  pattern: string;
  path: string;
  type?: string;
  hidden?: boolean;
  max_results?: number;
}): Promise<string> {
  const maxResults = input.max_results ?? 200;
  const includeHidden = input.hidden ?? false;

  // Try fd first, fall back to find
  const hasFd = await new Promise<boolean>((resolve) => {
    const p = spawn("which", ["fd"], { stdio: "ignore" });
    p.on("close", (code) => resolve(code === 0));
    p.on("error", () => resolve(false));
  });

  let args: string[];
  if (hasFd) {
    args = [
      "fd",
      "--glob",
      input.pattern,
      input.path,
      ...(input.type ? ["--type", input.type] : []),
      ...(includeHidden ? ["--hidden", "--no-ignore"] : []),
    ];
  } else {
    // find fallback
    const typeFlag = input.type === "f" ? "f" : input.type === "d" ? "d" : input.type === "l" ? "l" : null;
    args = [
      "find",
      input.path,
      ...(typeFlag ? ["-type", typeFlag] : []),
      "-name",
      input.pattern,
      ...(includeHidden ? [] : ["!", "-name", ".*"]),
    ];
  }

  const result = await new Promise<{ stdout: string; stderr: string; code: number | null }>(
    (resolve) => {
      let stdout = "";
      let stderr = "";
      const [cmd, ...cmdArgs] = args as [string, ...string[]];
      const proc = spawn(cmd, cmdArgs, { stdio: ["ignore", "pipe", "pipe"] });
      proc.stdout.on("data", (d: Buffer) => { stdout += d.toString(); });
      proc.stderr.on("data", (d: Buffer) => { stderr += d.toString(); });
      proc.on("close", (code) => resolve({ stdout, stderr, code }));
      proc.on("error", (err) => resolve({ stdout: "", stderr: err.message, code: -1 }));
    }
  );

  if (result.code !== 0 && result.code !== 1) {
    const msg = result.stderr.trim() || `find_files failed (exit ${result.code})`;
    throw new Error(msg);
  }

  const raw = result.stdout.trim();
  if (!raw) return "No files found.";

  const lines = raw.split("\n").filter(Boolean);
  if (lines.length <= maxResults) {
    return lines.join("\n");
  }

  const truncated = lines.slice(0, maxResults).join("\n");
  return `${truncated}\n\n[truncated: showing ${maxResults} of ${lines.length} results]`;
}

// Map from PID to ChildProcess, used by write_stdin to find the process's
// stdin.  Populated by executeRunBackground; entries linger for the session
// lifetime — acceptable because background process count per session is small.
const backgroundProcesses = new Map<number, ReturnType<typeof spawn>>();

async function executeRunBackground(input: {
  command: string;
  cwd?: string;
}): Promise<string> {
  const logFile = join(tmpdir(), `omega-bg-${Date.now()}-${Math.random().toString(36).slice(2)}.log`);

  // Open the log file for writing
  const { open } = await import("fs/promises");
  const fh = await open(logFile, "w");
  const fd = fh.fd;

  const proc = spawn("bash", ["-c", input.command], {
    stdio: ["pipe", fd, fd],
    detached: true,
    cwd: input.cwd,
  });
  proc.unref();

  await fh.close();

  if (proc.pid == null) {
    throw new Error("Failed to spawn background process");
  }

  // Track so write_stdin can find the process's stdin.
  backgroundProcesses.set(proc.pid, proc);

  return JSON.stringify({ pid: proc.pid, logFile });
}

async function executeWaitForOutput(input: {
  logFile: string;
  timeoutMs: number;
  pattern?: string;
  minBytes?: number;
}): Promise<string> {
  const { logFile, timeoutMs, pattern, minBytes } = input;
  const deadline = Date.now() + timeoutMs;
  const POLL_MS = 200;

  // Determine which conditions are active.
  // If neither pattern nor minBytes is given, trigger on any output (minBytes = 1).
  const hasPattern  = pattern  !== undefined;
  const hasMinBytes = minBytes !== undefined;
  const effectiveMinBytes = hasMinBytes ? minBytes! : (hasPattern ? null : 1);

  const read = async (): Promise<string> => {
    try { return await readFile(logFile, "utf-8"); }
    catch { return ""; }
  };

  while (Date.now() < deadline) {
    const content = await read();

    if (hasPattern && content.includes(pattern!)) {
      return JSON.stringify({ output: content, matched: true,  minBytesReached: false, timedOut: false });
    }
    if (effectiveMinBytes !== null && content.length >= effectiveMinBytes) {
      return JSON.stringify({ output: content, matched: false, minBytesReached: true,  timedOut: false });
    }

    await new Promise((r) => setTimeout(r, WAIT_FOR_OUTPUT_POLL_MS));
  }

  // Timed out — return whatever is in the log
  const content = await read();
  return JSON.stringify({ output: content, matched: false, minBytesReached: false, timedOut: true });
}

async function executeWriteStdin(input: {
  pid: number;
  text: string;
  end_stdin?: boolean;
}): Promise<string> {
  const { pid, text, end_stdin = false } = input;
  const child = backgroundProcesses.get(pid);

  if (!child) {
    throw new Error(
      `No tracked process with pid ${pid}. Only processes started with run_background can receive stdin.`,
    );
  }

  const stdin = child.stdin;
  if (!stdin) {
    throw new Error(`Process ${pid} has no writable stdin.`);
  }

  if (stdin.writableEnded || stdin.destroyed) {
    throw new Error(`stdin for pid ${pid} is already closed.`);
  }

  await new Promise<void>((resolve, reject) => {
    stdin.write(text, (err: Error | null | undefined) => {
      if (err) reject(err);
      else resolve();
    });
  });

  if (end_stdin) {
    stdin.end();
    return `Wrote ${text.length} chars to stdin of pid ${pid} and closed stdin (EOF)`;
  }

  return `Wrote ${text.length} chars to stdin of pid ${pid}`;
}

export async function executeTool(
  name: string,
  input: any,
  signal?: AbortSignal,
): Promise<ToolResult> {
  const startTime = performance.now();
  try {
    let output: string;
    switch (name) {
      case "read_file":
        output = await executeReadFile(ReadFileSchema.parse(input));
        break;
      case "write_file":
        output = await executeWriteFile(WriteFileSchema.parse(input));
        break;
      case "edit_file":
        output = await executeEditFile(EditFileSchema.parse(input));
        break;
      case "run_command":
        output = await executeRunCommand(RunCommandSchema.parse(input), signal);
        break;
      case "list_files":
        output = await executeListFiles(ListFilesSchema.parse(input));
        break;
      case "web_search":
        output = await executeWebSearch(WebSearchSchema.parse(input));
        break;
      case "fetch_url":
        output = await executeFetchUrl(FetchUrlSchema.parse(input));
        break;
      case "grep_files":
        output = await executeGrepFiles(GrepFilesSchema.parse(input));
        break;
      case "find_files":
        output = await executeFindFiles(FindFilesSchema.parse(input));
        break;
      case "run_background":
        output = await executeRunBackground(RunBackgroundSchema.parse(input));
        break;
      case "wait_for_output":
        output = await executeWaitForOutput(WaitForOutputSchema.parse(input));
        break;
      case "write_stdin":
        output = await executeWriteStdin(WriteStdinSchema.parse(input));
        break;
      default:
        return {
          output: `Unknown tool: ${name}`,
          isError: true,
          durationMs: Math.round(performance.now() - startTime),
        };
    }
    const MAX_TOOL_OUTPUT_CHARS = 100_000;
    if (output.length > MAX_TOOL_OUTPUT_CHARS) {
      output = output.slice(0, MAX_TOOL_OUTPUT_CHARS) +
        `\n\n[truncated: tool output was ${output.length} chars; showing first ${MAX_TOOL_OUTPUT_CHARS}. Use offset/limit or a more specific query to see other parts.]`;
    }
    return {
      output,
      isError: false,
      durationMs: Math.round(performance.now() - startTime),
    };
  } catch (err: unknown) {
    const msg = err instanceof z.ZodError
      ? z.prettifyError(err)
      : err instanceof Error ? err.message : String(err);
    return {
      output: `Error: ${msg}`,
      isError: true,
      durationMs: Math.round(performance.now() - startTime),
    };
  }
}

// Format a tool call for display
export function formatToolCall(name: string, input: any): string {
  switch (name) {
    case "read_file": {
      let s = `read_file: ${input.path}`;
      if (input.offset) s += ` (from line ${input.offset})`;
      if (input.limit) s += ` (${input.limit} lines)`;
      return s;
    }
    case "write_file":
      return `write_file: ${input.path} (${input.content?.length ?? 0} bytes)`;
    case "edit_file": {
      const reps = input.replacements as { old_text: string; new_text: string }[] | undefined;
      const count = reps?.length ?? 0;
      return `edit_file: ${input.path} (${count} replacement${count === 1 ? "" : "s"})`;
    }
    case "run_command":
      return `run_command: ${input.command}`;
    case "list_files": {
      let s = `list_files: ${input.path}`;
      if (input.recursive) s += " (recursive)";
      return s;
    }
    case "web_search":
      return `web_search: ${input.query}`;
    case "fetch_url":
      return `fetch_url: ${input.url} | ${input.postprocess ?? ""}`;
    case "grep_files": {
      let s = `grep_files: ${input.pattern} in ${input.path}`;
      if (input.file_glob) s += ` [${input.file_glob}]`;
      return s;
    }
    case "find_files": {
      let s = `find_files: ${input.pattern} in ${input.path}`;
      if (input.type) s += ` [type=${input.type}]`;
      return s;
    }
    case "run_background":
      return `run_background: ${input.command}`;
    case "wait_for_output": {
      let s = `wait_for_output: ${input.logFile} (timeout ${input.timeoutMs}ms)`;
      if (input.pattern)  s += ` pattern="${input.pattern}"`;
      if (input.minBytes) s += ` minBytes=${input.minBytes}`;
      return s;
    }
    case "write_stdin": {
      let s = `write_stdin: pid ${input.pid} (${input.text?.length ?? 0} chars)`;
      if (input.end_stdin) s += " [close stdin]";
      return s;
    }
    default:
      return `${name}: ${JSON.stringify(input)}`;
  }
}
