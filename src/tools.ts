import { readFile, writeFile, readdir, stat } from "fs/promises";
import { join, relative } from "path";
import { spawn } from "child_process";
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Web search (DuckDuckGo) + URL fetch helpers
// ---------------------------------------------------------------------------

const FETCH_MAX_CHARS = 8_000;

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
  const q = encodeURIComponent(input.query.trim());

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
    // Pull out result snippets: <a class="result__snippet">…</a>
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

  if (text.length > FETCH_MAX_CHARS) {
    return text.slice(0, FETCH_MAX_CHARS) + `\n\n[Truncated at ${FETCH_MAX_CHARS} chars. Full page is ${text.length} chars.]`;
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
      "overwrites if it does. Creates parent directories as needed.",
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
      "Search the web using DuckDuckGo. Returns titles, URLs, and snippets for the top results. " +
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
      "Content is truncated at 8000 characters. Use this to read documentation, " +
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

export async function executeTool(
  name: string,
  input: any
): Promise<ToolResult> {
  const startTime = performance.now();
  try {
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
      default:
        return {
          output: `Unknown tool: ${name}`,
          isError: true,
          durationMs: performance.now() - startTime,
        };
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
    default:
      return `${name}: ${JSON.stringify(input)}`;
  }
}
