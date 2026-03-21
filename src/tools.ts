import { readFile, writeFile, readdir, stat } from "fs/promises";
import { join, relative } from "path";
import { spawn } from "child_process";
import { tmpdir } from "os";
import type Anthropic from "@anthropic-ai/sdk";
import { config } from "./config";

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

async function executeBraveSearch(query: string, apiKey: string): Promise<string> {
  const q = encodeURIComponent(query.trim());
  const url = `https://api.search.brave.com/res/v1/web/search?q=${q}&count=10&text_decorations=0&search_lang=en`;
  const res = await fetch(url, {
    headers: {
      "Accept": "application/json",
      "Accept-Encoding": "gzip",
      "X-Subscription-Token": apiKey,
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

async function executeDuckDuckGoSearch(query: string): Promise<string> {
  const q = encodeURIComponent(query.trim());

  // DuckDuckGo Instant Answer API — no API key required
  const apiUrl = `https://api.duckduckgo.com/?q=${q}&format=json&no_html=1&skip_disambig=1`;
  const res = await fetch(apiUrl, {
    headers: { "User-Agent": "omega-agent/1.0 (terminal AI assistant)" },
    signal: AbortSignal.timeout(10_000),
    ...FETCH_TLS_OPTIONS,
  } as any);
  if (!res.ok) throw new Error(`DuckDuckGo API error: ${res.status}`);

  const data = await res.json() as any;
  const lines: string[] = [];

  // Abstract (direct answer)
  if (data.Abstract) {
    lines.push(`${data.Abstract}`);
    if (data.AbstractURL) lines.push(`Source: ${data.AbstractURL}`);
    lines.push("");
  }

  // Answer (e.g. calculations, conversions)
  if (data.Answer) {
    lines.push(`Answer: ${data.Answer}`);
    lines.push("");
  }

  // Related topics
  const topics: Array<{ Text: string; FirstURL: string }> = data.RelatedTopics ?? [];
  const results = topics
    .filter((t) => t.Text && t.FirstURL)
    .slice(0, 8);

  if (results.length > 0) {
    lines.push("Results:");
    for (const r of results) {
      lines.push(`• ${r.FirstURL}`);
      lines.push(`  ${r.Text}`);
    }
    lines.push("");
  }

  // If DDG gave us nothing useful, fall back to an HTML scrape
  if (lines.length === 0) {
    const htmlUrl = `https://html.duckduckgo.com/html/?q=${q}`;
    const htmlRes = await fetch(htmlUrl, {
      headers: {
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36",
      },
      signal: AbortSignal.timeout(10_000),
      ...FETCH_TLS_OPTIONS,
    } as any);
    if (!htmlRes.ok) throw new Error(`DuckDuckGo HTML error: ${htmlRes.status}`);
    const html = await htmlRes.text();
    const snippetRe = /<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>([\s\S]*?)<\/a>/gi;
    const urlRe = /<a[^>]+class="[^"]*result__url[^"]*"[^>]*>([\s\S]*?)<\/a>/gi;
    const snippets: string[] = [];
    let m: RegExpExecArray | null;
    while ((m = snippetRe.exec(html)) !== null && snippets.length < 6) {
      snippets.push(htmlToText(m[1]));
    }
    const urls: string[] = [];
    while ((m = urlRe.exec(html)) !== null && urls.length < 6) {
      urls.push(htmlToText(m[1]).trim());
    }
    if (snippets.length > 0) {
      lines.push("Results:");
      for (let i = 0; i < snippets.length; i++) {
        if (urls[i]) lines.push(`• ${urls[i]}`);
        lines.push(`  ${snippets[i]}`);
      }
    } else {
      lines.push("No results found.");
    }
  }

  const output = lines.join("\n");
  return output.length > FETCH_MAX_CHARS
    ? output.slice(0, FETCH_MAX_CHARS) + "\n[truncated]"
    : output;
}

async function executeWebSearch(input: { query: string }): Promise<string> {
  if (!input.query || !input.query.trim()) {
    throw new Error("query is required");
  }
  const braveKey = process.env.BRAVE_SEARCH_API_KEY;
  if (braveKey) {
    return executeBraveSearch(input.query, braveKey);
  }
  return executeDuckDuckGoSearch(input.query);
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
export const toolDefinitions: Anthropic.Tool[] = [
  {
    name: "read_file",
    description:
      "Read the contents of a file. Returns the file content as text. " +
      "For large files, use offset and limit to read a specific line range.",
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Path to the file (absolute or relative to cwd)",
        },
        offset: {
          type: "number",
          description: "Starting line number (1-indexed, optional)",
        },
        limit: {
          type: "number",
          description: "Maximum number of lines to read (optional)",
        },
      },
      required: ["path"],
    },
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
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Path to the file (absolute or relative to cwd)",
        },
        content: {
          type: "string",
          description: "Content to write to the file",
        },
      },
      required: ["path", "content"],
    },
  },
  {
    name: "run_command",
    description:
      "Execute a shell command and return its stdout, stderr, and exit code. " +
      "The command runs in the current working directory. " +
      "Use timeout to limit long-running commands.",
    input_schema: {
      type: "object" as const,
      properties: {
        command: {
          type: "string",
          description: "The shell command to execute",
        },
        timeout: {
          type: "number",
          description: "Timeout in seconds (optional, default 30)",
        },
      },
      required: ["command"],
    },
  },
  {
    name: "edit_file",
    description:
      "Edit a file by replacing exact text. The old_text must match exactly " +
      "(including whitespace and indentation). Use this for surgical edits " +
      "instead of rewriting entire files with write_file. The old_text must " +
      "appear exactly once in the file.",
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Path to the file (absolute or relative to cwd)",
        },
        old_text: {
          type: "string",
          description: "Exact text to find (must match exactly, must appear once)",
        },
        new_text: {
          type: "string",
          description: "Text to replace old_text with",
        },
      },
      required: ["path", "old_text", "new_text"],
    },
  },
  {
    name: "list_files",
    description:
      "List files and directories. Returns names with '/' suffix for directories. " +
      "Use recursive to list the full tree (up to 1000 entries).",
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Directory path (absolute or relative to cwd)",
        },
        recursive: {
          type: "boolean",
          description: "List recursively (optional, default false)",
        },
      },
      required: ["path"],
    },
  },
  {
    name: "web_search",
    description:
      "Search the web using Brave Search (or DuckDuckGo as fallback). Returns titles, URLs, and snippets for the top results. " +
      "Use this to look up documentation, current information, or anything not in local files.",
    input_schema: {
      type: "object" as const,
      properties: {
        query: {
          type: "string",
          description: "The search query",
        },
      },
      required: ["query"],
    },
  },
  {
    name: "fetch_url",
    description:
      "Fetch the content of a URL and return it as plain text. " +
      "HTML pages are converted to readable text (tags stripped). " +
      "Content is truncated at 20000 characters. Use this to read documentation, " +
      "articles, or any web page.",
    input_schema: {
      type: "object" as const,
      properties: {
        url: {
          type: "string",
          description: "The URL to fetch (must be http or https)",
        },
      },
      required: ["url"],
    },
  },
  {
    name: "grep_files",
    description:
      "Search for a pattern across files in a directory using ripgrep (rg) with grep fallback. " +
      "Returns structured file:line:text matches, capped at max_results (default 200). " +
      "Use this to find all occurrences of a symbol, string, or regex across the codebase " +
      "instead of reading files speculatively. Chain with read_file to inspect context.",
    input_schema: {
      type: "object" as const,
      properties: {
        pattern: {
          type: "string",
          description: "Regex or literal string to search for",
        },
        path: {
          type: "string",
          description: "Directory (or file) path to search in",
        },
        file_glob: {
          type: "string",
          description: "Optional glob to restrict which files are searched (e.g. '*.ts')",
        },
        context_lines: {
          type: "number",
          description: "Number of context lines to include before and after each match (optional)",
        },
        case_sensitive: {
          type: "boolean",
          description: "If true, match is case-sensitive. Default: false (case-insensitive)",
        },
        max_results: {
          type: "number",
          description: "Maximum number of match lines to return (default 200)",
        },
      },
      required: ["pattern", "path"],
    },
  },
  {
    name: "find_files",
    description:
      "Find files and directories by name/glob pattern using fd (with find fallback). " +
      "Returns a list of matching paths, capped at max_results (default 200). " +
      "Use this to locate files when you know the name or extension but not the exact path. " +
      "Ignores hidden files and .gitignore'd paths by default (set hidden=true to include them). " +
      "Chain with read_file or grep_files to inspect contents.",
    input_schema: {
      type: "object" as const,
      properties: {
        pattern: {
          type: "string",
          description: "Glob or regex pattern to match against file/directory names",
        },
        path: {
          type: "string",
          description: "Root directory to search in",
        },
        type: {
          type: "string",
          description: "Filter by entry type: 'f' (files), 'd' (directories), 'l' (symlinks). Omit for all.",
        },
        hidden: {
          type: "boolean",
          description: "Include hidden files and .gitignore'd paths (default false)",
        },
        max_results: {
          type: "number",
          description: "Maximum number of results to return (default 200)",
        },
      },
      required: ["pattern", "path"],
    },
  },
  {
    name: "run_background",
    description:
      "Start a long-running process in the background and return immediately. " +
      "stdout and stderr are redirected to a temporary log file. " +
      "Returns { pid, logFile } — use read_file on logFile to inspect output, " +
      "and kill_process(pid) to stop the process when done. " +
      "Use this for dev servers, file watchers, or any 'start → inspect → stop' workflow.",
    input_schema: {
      type: "object" as const,
      properties: {
        command: {
          type: "string",
          description: "Shell command to run in the background",
        },
        cwd: {
          type: "string",
          description: "Working directory for the process (optional, defaults to cwd)",
        },
      },
      required: ["command"],
    },
  },
  {
    name: "kill_process",
    description:
      "Send a signal to a background process started with run_background. " +
      "Returns a status message. Handles already-exited processes gracefully. " +
      "Default signal is SIGTERM (graceful shutdown); use SIGKILL to force.",
    input_schema: {
      type: "object" as const,
      properties: {
        pid: {
          type: "number",
          description: "Process ID returned by run_background",
        },
        signal: {
          type: "string",
          description: "Signal to send (optional, default SIGTERM). E.g. SIGTERM, SIGKILL, SIGINT.",
        },
      },
      required: ["pid"],
    },
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

function executeRunCommand(input: {
  command: string;
  timeout?: number;
}): Promise<string> {
  const timeoutMs = (input.timeout ?? 30) * 1000;

  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    let killed = false;

    const proc = spawn("bash", ["-c", input.command], {
      stdio: ["ignore", "pipe", "pipe"],
      timeout: timeoutMs,
    });

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
      let result = "";
      if (stdout) result += stdout;
      if (stderr) result += (result ? "\n" : "") + `[stderr]\n${stderr}`;
      if (killed) result += "\n[Output truncated at 100KB]";
      if (code !== 0 && code !== null) {
        result += `\n[exit code: ${code}]`;
      }
      resolve(result || "(no output)");
    });

    proc.on("error", (err) => {
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
      input.pattern,
      input.path,
    ];
  }

  const output = await new Promise<{ stdout: string; stderr: string; code: number | null }>(
    (resolve) => {
      let stdout = "";
      let stderr = "";
      const proc = spawn(args[0], args.slice(1), { stdio: ["ignore", "pipe", "pipe"] });
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
      const proc = spawn(args[0], args.slice(1), { stdio: ["ignore", "pipe", "pipe"] });
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

  return JSON.stringify({ pid: proc.pid, logFile });
}

function executeKillProcess(input: {
  pid: number;
  signal?: string;
}): string {
  const sig = (input.signal ?? "SIGTERM") as NodeJS.Signals;
  try {
    process.kill(input.pid, sig);
    return `Sent ${sig} to pid ${input.pid}`;
  } catch (err: any) {
    // ESRCH = no such process (already dead)
    if (err.code === "ESRCH") {
      return `pid ${input.pid} already exited (no such process)`;
    }
    throw err;
  }
}

function validateToolInput(name: string, input: any): void {
  const tool = toolDefinitions.find((t) => t.name === name);
  if (!tool) return;
  const schema = (tool as any).input_schema;
  if (!schema) return;

  const required: string[] = schema.required ?? [];
  const props: Record<string, any> = schema.properties ?? {};

  for (const key of required) {
    if (input == null || input[key] == null) {
      throw new Error(`Missing required field: ${key}`);
    }
    const expectedType = props[key]?.type;
    if (expectedType && typeof input[key] !== expectedType) {
      throw new Error(`Invalid type for ${key}: expected ${expectedType}`);
    }
  }
}

export async function executeTool(
  name: string,
  input: any
): Promise<ToolResult> {
  const startTime = performance.now();
  try {
    validateToolInput(name, input);

    let output: string;
    switch (name) {
      case "read_file":
        output = await executeReadFile(input);
        break;
      case "write_file":
        output = await executeWriteFile(input);
        break;
      case "edit_file":
        output = await executeEditFile(input);
        break;
      case "run_command":
        output = await executeRunCommand(input);
        break;
      case "list_files":
        output = await executeListFiles(input);
        break;
      case "web_search":
        output = await executeWebSearch(input);
        break;
      case "fetch_url":
        output = await executeFetchUrl(input);
        break;
      case "grep_files":
        output = await executeGrepFiles(input);
        break;
      case "find_files":
        output = await executeFindFiles(input);
        break;
      case "run_background":
        output = await executeRunBackground(input);
        break;
      case "kill_process":
        output = executeKillProcess(input);
        break;
      default:
        return {
          output: `Unknown tool: ${name}`,
          isError: true,
          durationMs: performance.now() - startTime,
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
      durationMs: performance.now() - startTime,
    };
  } catch (err: any) {
    return {
      output: `Error: ${err.message}`,
      isError: true,
      durationMs: performance.now() - startTime,
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
    case "kill_process": {
      let s = `kill_process: pid ${input.pid}`;
      if (input.signal) s += ` (${input.signal})`;
      return s;
    }
    default:
      return `${name}: ${JSON.stringify(input)}`;
  }
}
