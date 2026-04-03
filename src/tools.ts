import { readFile, writeFile, readdir, stat } from "fs/promises";
import { join, relative } from "path";
import { spawn } from "child_process";
import { tmpdir } from "os";
import { z } from "zod";
import type Anthropic from "@anthropic-ai/sdk";
import { config } from "./config";
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
  WaitProcessSchema,
  KillProcessSchema,
} from "./tools.schema.js";

// ---------------------------------------------------------------------------
// Web search (DuckDuckGo) + URL fetch helpers
// ---------------------------------------------------------------------------

const FETCH_MAX_CHARS = 8_000;
const FETCH_URL_MAX_CHARS = 20_000;

/**
 * Bun's TLS implementation may not trust the system CA bundle on some machines.
 * This option disables certificate verification for outbound fetch calls.
 * Acceptable for an agent reading public web content (we're not sending secrets).
 */
const FETCH_TLS_OPTIONS = { tls: { rejectUnauthorized: false } };

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
  return output.length > FETCH_MAX_CHARS
    ? output.slice(0, FETCH_MAX_CHARS) + "\n[truncated]"
    : output;
}

async function executeFetchUrl(input: { url: string }): Promise<string> {
  if (!input.url || !input.url.trim()) throw new Error("url is required");

  // Basic URL validation
  let parsed: URL;
  try {
    parsed = new URL(input.url.trim());
  } catch {
    throw new Error(`Invalid URL: ${input.url}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`Unsupported protocol: ${parsed.protocol}`);
  }

  const res = await fetch(parsed.href, {
    headers: {
      "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36",
      "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    },
    signal: AbortSignal.timeout(15_000),
    ...FETCH_TLS_OPTIONS,
  } as any);

  if (!res.ok) throw new Error(`HTTP ${res.status}: ${res.statusText}`);

  const contentType = res.headers.get("content-type") ?? "";
  const isHtml = contentType.includes("text/html") || contentType.includes("application/xhtml");

  const body = await res.text();
  const text = isHtml ? htmlToText(body) : body;

  if (text.length > FETCH_URL_MAX_CHARS) {
    return text.slice(0, FETCH_URL_MAX_CHARS) + `\n\n[Truncated at ${FETCH_URL_MAX_CHARS} chars. Full page is ${text.length} chars.]`;
  }
  return text;
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
      `WARNING: file content is generated inside the output token budget (${config.maxOutputTokens} tokens). ` +
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
      "Use timeout to limit long-running commands.",
    input_schema: toToolInputSchema(RunCommandSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "edit_file",
    description:
      "Edit a file by replacing exact text. The old_text must match exactly " +
      "(including whitespace and indentation). Use this for surgical edits " +
      "instead of rewriting entire files with write_file. The old_text must " +
      "appear exactly once in the file.",
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
      "Fetch the content of a URL and return it as plain text. " +
      "HTML pages are converted to readable text (tags stripped). " +
      "Content is truncated at 20000 characters. Use this to read documentation, " +
      "articles, or any web page.",
    input_schema: toToolInputSchema(FetchUrlSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "grep_files",
    description:
      "Search for a pattern across files in a directory using ripgrep (rg) with grep fallback. " +
      "Returns structured file:line:text matches, capped at max_results (default 200). " +
      "Use this to find all occurrences of a symbol, string, or regex across the codebase " +
      "instead of reading files speculatively. Chain with read_file to inspect context.",
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
      "Use wait_process(pid) to block until the process finishes and get the exit code. " +
      "Use read_file on logFile (with offset/limit for large output) and grep_files to inspect output. " +
      "Use kill_process(pid) to stop the process early. " +
      "Use this for any slow command — test suites, builds, dev servers, file watchers — " +
      "so you can continue doing useful work in the same turn instead of blocking.",
    input_schema: toToolInputSchema(RunBackgroundSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "wait_process",
    description:
      "Wait for a background process started with run_background to finish. " +
      "Blocks until the process exits or timeoutMs is reached (default 60000 ms). " +
      "Returns { pid, exitCode, signal, timedOut }. " +
      "Use this to synchronise before reading the logFile from run_background. " +
      "The pid parameter must be a number, not a string.",
    input_schema: toToolInputSchema(WaitProcessSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
  },
  {
    name: "kill_process",
    description:
      "Send a signal to a background process started with run_background. " +
      "Returns a status message. Handles already-exited processes gracefully. " +
      "Default signal is SIGTERM (graceful shutdown); use SIGKILL to force.",
    input_schema: toToolInputSchema(KillProcessSchema) as Anthropic.Beta.Messages.BetaTool["input_schema"],
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
  old_text: string;
  new_text: string;
}): Promise<string> {
  const content = await readFile(input.path, "utf-8");

  // Count occurrences
  let count = 0;
  let idx = -1;
  while ((idx = content.indexOf(input.old_text, idx + 1)) !== -1) {
    count++;
  }

  if (count === 0) {
    throw new Error(
      `old_text not found in ${input.path}. Make sure it matches exactly (including whitespace).`
    );
  }
  if (count > 1) {
    throw new Error(
      `old_text found ${count} multiple times in ${input.path}. It must appear exactly once. Use a larger/more unique snippet.`
    );
  }

  const newContent = content.replace(input.old_text, input.new_text);
  await writeFile(input.path, newContent, "utf-8");

  const oldLines = input.old_text.split("\n").length;
  const newLines = input.new_text.split("\n").length;
  return `edit_file: ${input.path} — replaced ${oldLines} line(s) with ${newLines} line(s)`;
}

function executeRunCommand(
  input: { command: string; timeout?: number },
  signal?: AbortSignal,
): Promise<string> {
  const timeoutMs = (input.timeout ?? 30) * 1000;

  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    let killed = false;
    let killedByAbort = false;

    const proc = spawn("bash", ["-c", input.command], {
      stdio: ["ignore", "pipe", "pipe"],
      timeout: timeoutMs,
    });

    // Kill the subprocess immediately when the abort signal fires.
    const onAbort = () => {
      killedByAbort = true;
      proc.kill();
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
        proc.kill();
        killed = true;
      }
    });

    proc.stderr.on("data", (data: Buffer) => {
      stderr += data.toString();
      if (stderr.length > 100_000) {
        proc.kill();
        killed = true;
      }
    });

    proc.on("close", (code) => {
      signal?.removeEventListener("abort", onAbort);
      let result = "";
      if (stdout) result += stdout;
      if (stderr) result += (result ? "\n" : "") + `[stderr]\n${stderr}`;
      if (killedByAbort) result += "\n[killed by abort signal]";
      else if (killed) result += "\n[Output truncated at 100KB]";
      if (code !== 0 && code !== null) {
        result += `\n[exit code: ${code}]`;
      }
      resolve(result || "(no output)");
    });

    proc.on("error", (err) => {
      signal?.removeEventListener("abort", onAbort);
      resolve(`[error: ${err.message}]`);
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

// Map from PID to ChildProcess for wait_process support.
// Populated by executeRunBackground; entries are removed by executeWaitProcess
// once it has observed the exit. If wait_process is never called the entry
// lingers for the session lifetime — that is acceptable because background
// process count per session is small in practice.
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
    stdio: ["ignore", fd, fd],
    detached: true,
    cwd: input.cwd,
  });
  proc.unref();

  await fh.close();

  if (proc.pid == null) {
    throw new Error("Failed to spawn background process");
  }

  // Track so wait_process can observe the exit event and retrieve the exit code.
  backgroundProcesses.set(proc.pid, proc);

  return JSON.stringify({ pid: proc.pid, logFile });
}

async function executeWaitProcess(input: {
  pid: number;
  timeoutMs?: number;
}): Promise<string> {
  const { pid, timeoutMs = 60_000 } = input;
  const child = backgroundProcesses.get(pid);

  if (!child) {
    // Not in our map — poll with signal 0 until the OS reports the PID is gone.
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        process.kill(pid, 0);
      } catch (err: unknown) {
        if (err instanceof Error && (err as NodeJS.ErrnoException).code === "ESRCH") {
          return JSON.stringify({
            pid,
            timedOut: false,
            exitCode: null,
            note: "exit code unavailable (process not tracked by run_background)",
          });
        }
        throw err;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    return JSON.stringify({ pid, timedOut: true });
  }

  // Already exited? exitCode is set by Node once the process has finished.
  if (child.exitCode !== null || child.signalCode !== null) {
    backgroundProcesses.delete(pid);
    return JSON.stringify({
      pid,
      timedOut: false,
      exitCode: child.exitCode,
      signal: child.signalCode ?? null,
    });
  }

  // Wait for exit with timeout.
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.removeListener("exit", onExit);
      resolve(JSON.stringify({ pid, timedOut: true }));
    }, timeoutMs);

    const onExit = (code: number | null, sig: NodeJS.Signals | null) => {
      clearTimeout(timer);
      backgroundProcesses.delete(pid);
      resolve(JSON.stringify({ pid, timedOut: false, exitCode: code, signal: sig }));
    };

    child.once("exit", onExit);
  });
}

function executeKillProcess(input: {
  pid: number;
  signal?: string;
}): string {
  const sig = (input.signal ?? "SIGTERM") as NodeJS.Signals;
  try {
    process.kill(input.pid, sig);
    return `Sent ${sig} to pid ${input.pid}`;
  } catch (err: unknown) {
    // ESRCH = no such process (already dead)
    if (err instanceof Error && (err as NodeJS.ErrnoException).code === "ESRCH") {
      return `pid ${input.pid} already exited (no such process)`;
    }
    throw err;
  }
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
      case "wait_process":
        output = await executeWaitProcess(WaitProcessSchema.parse(input));
        break;
      case "kill_process":
        output = executeKillProcess(KillProcessSchema.parse(input));
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
    case "edit_file":
      return `edit_file: ${input.path} (${input.old_text?.length ?? 0} → ${input.new_text?.length ?? 0} bytes)`;
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
      return `fetch_url: ${input.url}`;
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
    case "wait_process": {
      let s = `wait_process: pid ${input.pid}`;
      if (input.timeoutMs) s += ` (timeout ${input.timeoutMs}ms)`;
      return s;
    }
    case "kill_process": {
      let s = `kill_process: pid ${input.pid}`;
      if (input.signal) s += ` (${input.signal})`;
      return s;
    }
    default:
      return `${name}: ${JSON.stringify(input)}`;
  }
}
