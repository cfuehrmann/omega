/**
 * Raw terminal UI — no library, no live zone.
 *
 * Everything is printed to scrollback as it happens.
 * No cursor movement, no live zone, no line counting.
 * The terminal owns all layout.
 */

import { Agent } from "./agent.js";
import { config } from "./config.js";
import { formatTurnFooter } from "./turn-footer.js";

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

const CSI = "\x1b[";

function sgr(...codes: number[]) {
  return `${CSI}${codes.join(";")}m`;
}

const RESET   = sgr(0);
const BOLD    = sgr(1);
const DIM_ON  = sgr(2);

function styled(s: string, ...codes: number[]): string {
  return sgr(...codes) + s + RESET;
}

function bold   (s: string) { return styled(s, 1); }
function dim    (s: string) { return styled(s, 2); }
function green  (s: string) { return styled(s, 32); }
function cyan   (s: string) { return styled(s, 36); }
function blue   (s: string) { return styled(s, 34); }
function yellow (s: string) { return styled(s, 33); }
function magenta(s: string) { return styled(s, 35); }
function red    (s: string) { return styled(s, 31); }

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

const TIME_WIDTH = 10;
const INDENT  = "  ";
const INDENT2 = "    ";
const INDENT3 = "      ";

function now(): string {
  return new Date().toLocaleTimeString("en-GB");
}


function truncateOutput(text: string, maxLines = 10): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… [${lines.length - maxLines} more lines]`;
}

/** Print lines to stdout, first line gets timestamp, rest get indent. */
function printBlock(time: string, lines: string[]): void {
  for (let i = 0; i < lines.length; i++) {
    const prefix = i === 0
      ? dim(time.padEnd(TIME_WIDTH))
      : " ".repeat(TIME_WIDTH);
    process.stdout.write(prefix + lines[i] + "\n");
  }
}

function println(s: string): void {
  process.stdout.write(s + "\n");
}

// ---------------------------------------------------------------------------
// Block renderers
// ---------------------------------------------------------------------------

function renderUserMessage(content: string): string[] {
  const lines: string[] = [];
  lines.push(bold(green("message")));
  lines.push(green(`${INDENT}role: "user"`));
  lines.push(green(`${INDENT}content:`));
  for (const line of content.split("\n")) {
    lines.push(bold(green(`${INDENT2}${line}`)));
  }
  return lines;
}

function summariseContent(content: any): string {
  if (typeof content === "string") {
    return content.length <= 60 ? `"${content}"` : `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  const parts = (content as any[]).map((b: any) => {
    if (b.type === "text")        return `text: <${b.text.length} chars>`;
    if (b.type === "tool_use")    return `tool_use: "${b.name}"`;
    if (b.type === "tool_result") return `tool_result: <${(b.content as string).length} chars>`;
    return b.type;
  });
  return `[${parts.join(", ")}]`;
}

export function renderApiRequest(
  callNumber: number,
  provider: "anthropic" | "openai",
  url: string,
  request: any,
): string[] {
  const shortUrl = url.replace(/^https?:\/\//, "");
  const lines: string[] = [];
  lines.push(bold(cyan(`${shortUrl}  #${callNumber}`)));
  if (provider === "anthropic") {
    const last = request.messages?.[request.messages.length - 1];
    lines.push(cyan(`${INDENT}model: "${request.model}"`));
    lines.push(cyan(`${INDENT}system: <${request.system.length} chars>`));
    lines.push(cyan(`${INDENT}tools: [${request.tools.map((t: any) => `"${t.name}"`).join(", ")}]`));
    lines.push(cyan(`${INDENT}max_tokens: ${request.max_tokens}`));
    lines.push(cyan(`${INDENT}messages: <${request.messages.length}> …`));
    if (last) {
      lines.push(dim(cyan(`${INDENT2}{ role: "${last.role}", content: ${summariseContent(last.content)} }`)));
    }
  } else {
    const last = request.input?.[request.input.length - 1];
    lines.push(cyan(`${INDENT}model: "${request.model}"`));
    lines.push(cyan(`${INDENT}instructions: <${(request.instructions ?? "").length} chars>`));
    lines.push(cyan(`${INDENT}tools: <${request.tools?.length ?? 0}>`));
    lines.push(cyan(`${INDENT}max_output_tokens: ${request.max_output_tokens}`));
    lines.push(cyan(`${INDENT}input: <${request.input?.length ?? 0}> …`));
    if (last) {
      lines.push(dim(cyan(`${INDENT2}${summariseOpenAiInput(last)}`)));
    }
  }
  return lines;
}

function summariseOpenAiInput(item: any): string {
  if (item.role && typeof item.content === "string") {
    return `{ role: "${item.role}", content: ${item.content.length <= 60 ? `"${item.content}"` : `<${item.content.length} chars>`} }`;
  }
  if (item.type === "function_call") {
    return `{ type: "function_call", name: "${item.name}", arguments: <${(item.arguments ?? "").length} chars> }`;
  }
  if (item.type === "function_call_output") {
    return `{ type: "function_call_output", call_id: "${item.call_id}", output: <${(item.output ?? "").length} chars> }`;
  }
  return `{ ${JSON.stringify(item).slice(0, 80)} }`;
}

function renderApiResponse(
  provider: "anthropic" | "openai",
  url: string,
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: any[],
  raw?: any,
): string[] {
  const shortUrl = url.replace(/^https?:\/\//, "");
  const lines: string[] = [];
  lines.push(bold(blue(`${shortUrl} response`)));
  if (provider === "anthropic") {
    lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
    lines.push(blue(`${INDENT}usage:`));
    lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
    lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
    lines.push(blue(`${INDENT}content:`));
  } else {
    lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
    lines.push(blue(`${INDENT}usage:`));
    lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
    lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
    if (raw?.usage?.cached_input_tokens !== undefined) {
      lines.push(dim(blue(`${INDENT2}cached_input_tokens: ${raw.usage.cached_input_tokens}`)));
    }
    if (raw?.usage?.cache_creation_input_tokens !== undefined) {
      lines.push(dim(blue(`${INDENT2}cache_creation_input_tokens: ${raw.usage.cache_creation_input_tokens}`)));
    }
    lines.push(blue(`${INDENT}content:`));
  }
  for (const block of content) {
    if (block.type === "text") {
      lines.push(blue(`${INDENT2}text:`));
      const preview = block.text.length <= 120 ? block.text : `<${block.text.length} chars>`;
      for (const line of preview.split("\n")) {
        lines.push(dim(blue(`${INDENT3}${line}`)));
      }
    } else if (block.type === "tool_use") {
      lines.push(blue(`${INDENT2}tool_use:`));
      lines.push(dim(blue(`${INDENT3}name: "${block.name}"`)));
      lines.push(dim(blue(`${INDENT3}input: ${JSON.stringify(block.input)}`)));
    } else {
      lines.push(dim(blue(`${INDENT2}${block.type}`)));
    }
  }
  return lines;
}

/** Render the start of a tool call (name + input only). Shown immediately when the call is issued. */
export function renderToolStart(name: string, input: any): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`tool call`)));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  lines.push(yellow(`${INDENT}input: ${JSON.stringify(input)}`));
  return lines;
}

/** Render the result of a tool call. Shown with a fresh timestamp when the tool completes. */
export function renderToolResult(result: { output: string; isError: boolean }): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`tool result`)));
  lines.push(dim(yellow(`${INDENT}is_error: ${result.isError}`)));
  lines.push(dim(yellow(`${INDENT}content:`)));
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push(dim(yellow(`${INDENT2}${line}`)));
  }
  return lines;
}

function renderToolExecution(
  name: string,
  input: any,
  result: { output: string; isError: boolean },
): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow("tool execution")));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  lines.push(yellow(`${INDENT}input: ${JSON.stringify(input)}`));
  lines.push(yellow(`${INDENT}result:`));
  lines.push(dim(yellow(`${INDENT2}is_error: ${result.isError}`)));
  lines.push(dim(yellow(`${INDENT2}content:`)));
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push(dim(yellow(`${INDENT3}${line}`)));
  }
  return lines;
}

function renderToolResultMessage(
  results: Array<{ tool_use_id: string; content: string; is_error: boolean }>,
): string[] {
  const lines: string[] = [];
  lines.push(bold(magenta("message")));
  lines.push(magenta(`${INDENT}role: "user"`));
  lines.push(magenta(`${INDENT}content:`));
  for (const r of results) {
    lines.push(magenta(`${INDENT2}tool_result:`));
    lines.push(dim(magenta(`${INDENT3}tool_use_id: "${r.tool_use_id}"`)));
    lines.push(dim(magenta(`${INDENT3}is_error: ${r.is_error}`)));
    lines.push(dim(magenta(`${INDENT3}content: <${r.content.length} chars>`)));
  }
  return lines;
}

function renderAssistantMessage(text: string, dimText?: string): string[] {
  const lines: string[] = [];
  lines.push(bold("message"));
  lines.push(`${INDENT}role: "assistant"`);
  lines.push(`${INDENT}content:`);
  for (const line of text.split("\n")) {
    lines.push(`${INDENT2}${line}`);
  }
  if (dimText) {
    lines.push(dim(`${INDENT}${dimText}`));
  }
  return lines;
}

function renderStatus(streaming: boolean): string {
  return dim(streaming ? "Esc to interrupt" : "Ctrl+C to quit  /gpt /anthropic /opus");
}

// ---------------------------------------------------------------------------
// Raw stdin
// ---------------------------------------------------------------------------

/** Callbacks for key input parsing. */
export interface KeyCallbacks {
  onSubmit: (line: string) => void;
  onEscape: () => void;
  onExit: () => void;
}

/**
 * Parse a raw terminal chunk and dispatch to callbacks.
 * Maintains input buffer state across calls via a module-level variable
 * (for production use via setupRawInput) or via the returned buffer value.
 *
 * Exported for unit testing without requiring a real TTY.
 *
 * @param chunk     Raw bytes from stdin (UTF-8 string).
 * @param callbacks Handlers for submit, escape, and exit.
 * @param buf       Current input buffer (defaults to shared module buffer).
 * @returns         Updated input buffer after processing.
 */
/**
 * Returns the number of terminal columns a Unicode character occupies.
 * CJK and other wide characters occupy 2 columns; everything else 1.
 * Control characters and zero-width characters return 0.
 */
export function displayWidth(ch: string): number {
  const cp = ch.codePointAt(0);
  if (cp === undefined) return 0;
  // Control characters
  if (cp < 32 || (cp >= 0x7f && cp < 0xa0)) return 0;
  // Wide/fullwidth Unicode ranges (East Asian Width W or F):
  // CJK Unified Ideographs and extensions
  if (cp >= 0x1100 && cp <= 0x115f) return 2;  // Hangul Jamo
  if (cp === 0x2329 || cp === 0x232a) return 2;
  if (cp >= 0x2e80 && cp <= 0x303e) return 2;  // CJK Radicals, Kangxi, etc.
  if (cp >= 0x3041 && cp <= 0x33bf) return 2;  // Hiragana, Katakana, Bopomofo, etc.
  if (cp >= 0x33ff && cp <= 0xfe4f) return 2;  // Many CJK blocks
  if (cp >= 0xfe51 && cp <= 0xfe6f) return 2;  // Small/fullwidth forms
  if (cp >= 0xff01 && cp <= 0xff60) return 2;  // Fullwidth ASCII, half/fullwidth
  if (cp >= 0xffe0 && cp <= 0xffe6) return 2;
  if (cp >= 0x1f004 && cp <= 0x1f9ff) return 2; // Emoji/symbols (broad range)
  if (cp >= 0x20000 && cp <= 0x2fffd) return 2; // CJK Extension B+
  if (cp >= 0x30000 && cp <= 0x3fffd) return 2;
  return 1;
}

/** Persistent bracketed-paste state shared across calls in production. */
const sharedPasteState = { inPaste: false };

export function parseKeys(
  chunk: string,
  callbacks: KeyCallbacks,
  buf: { value: string; columns?: number } = sharedBuffer,
  options: { inputEnabled?: boolean; pasteState?: { inPaste: boolean } } = {},
): string {
  const { onSubmit, onEscape, onExit } = callbacks;
  const inputEnabled = options.inputEnabled ?? true;
  const pasteState = options.pasteState ?? sharedPasteState;

  for (let i = 0; i < chunk.length; i++) {
    const ch = chunk[i];
    const code = chunk.charCodeAt(i);

    if (ch === "\x03") { onExit(); return buf.value; }  // Ctrl+C — stop processing

    if (ch === "\x1b") {                       // Escape or CSI sequence
      // Check for bracketed paste markers: \x1b[200~ (start) and \x1b[201~ (end)
      if (chunk.startsWith("[200~", i + 1)) {
        pasteState.inPaste = true;
        i += 5; // skip past "[200~"
        continue;
      }
      if (chunk.startsWith("[201~", i + 1)) {
        pasteState.inPaste = false;
        i += 5; // skip past "[201~"
        // Echo the full buffer so the user can see what was pasted
        process.stdout.write(buf.value);
        continue;
      }
      if (i + 1 < chunk.length && chunk[i + 1] === "[") {
        // Arrow key or other CSI sequence — skip it
        i += 2;
        while (i < chunk.length && !/[A-Za-z~]/.test(chunk[i])) i++;
      } else {
        if (!pasteState.inPaste) onEscape();
      }
      continue;
    }

    if (!inputEnabled) continue;

    if (ch === "\r" || ch === "\n") {
      if (pasteState.inPaste) {
        // Inside a paste: preserve newlines literally in the buffer
        buf.value += "\n";
      } else {
        // Normal Enter: submit
        const line = buf.value;
        buf.value = "";
        if (buf.columns !== undefined) buf.columns = 0;
        onSubmit(line);
      }
      continue;
    }

    if (code === 127 || code === 8) {          // Backspace
      if (buf.value.length > 0) {
        // Get the last code point (handle surrogate pairs)
        const lastCodePoint = [...buf.value].at(-1) ?? "";
        const w = displayWidth(lastCodePoint) || 1;
        buf.value = buf.value.slice(0, -lastCodePoint.length);
        if (buf.columns !== undefined) buf.columns -= w;
        // Erase: move back w columns, overwrite with spaces, move back again
        process.stdout.write("\b".repeat(w) + " ".repeat(w) + "\b".repeat(w));
      }
      continue;
    }

    // Printable character
    if (code >= 32 || code > 127) {
      const w = displayWidth(ch) || 1;
      buf.value += ch;
      if (buf.columns !== undefined) buf.columns += w;
      if (!pasteState.inPaste) process.stdout.write(ch);  // echo only when not pasting
    }
  }

  return buf.value;
}

/** Shared mutable buffer used by setupRawInput in production. */
const sharedBuffer: { value: string; columns: number } = { value: "", columns: 0 };

function setupRawInput(
  onSubmit: (line: string) => void,
  onEscape: () => void,
  onExit: () => void,
): void {
  if (!process.stdin.setRawMode) {
    console.error("Error: stdin is not a TTY. Run in an interactive terminal.");
    process.exit(1);
  }
  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stdin.setEncoding("utf-8");

  // Enable bracketed paste mode so pasted newlines don't trigger submit
  process.stdout.write("\x1b[?2004h");

  process.stdin.on("data", (chunk: string) => {
    parseKeys(chunk, { onSubmit, onEscape, onExit });
  });
}

// ---------------------------------------------------------------------------
// Input prompt
// ---------------------------------------------------------------------------

function printPrompt(prefix: string): void {
  process.stdout.write(dim(now().padEnd(TIME_WIDTH)) + prefix);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/** Fold session into world state and exit. Used for all clean-shutdown paths. */
async function shutdown(agent: Agent, code: number = 0): Promise<never> {
  // Clear the current prompt line before rendering shutdown events.
  process.stdout.write("\r\x1b[2K");  // CR + erase entire line

  let hadEvents = false;
  for await (const event of agent.foldCurrentSessionIntoWorldState()) {
    if (!hadEvents) {
      // Print the shutdown banner once, before the first event.
      printBlock(now(), [magenta("Ctrl+C — compacting to world file and shutting down")]);
      hadEvents = true;
    }
    switch (event.type) {
      case "api_call_start":
        printBlock(now(), renderApiRequest(
          event.callNumber,
          event.provider,
          event.url,
          event.request,
        ));
        break;

      case "api_response":
        printBlock(now(), renderApiResponse(
          event.provider,
          event.url,
          event.stopReason,
          event.usage,
          event.content,
          event.raw,
        ));
        break;

      case "world_state_saved":
        printBlock(now(), [dim(`✓ world state saved  Written ${event.charCount} chars to ${event.path}`)]);
        break;

      case "error":
        printBlock(now(), [red(`⚠ World state save failed: ${event.error}`)]);
        break;
    }
  }

  // Disable bracketed paste mode before exit so the terminal is clean
  process.stdout.write("\x1b[?2004l");
  process.exit(code);
}

export async function runApp(): Promise<void> {
  const agent = new Agent();

  // Init
  try {
    const mode = await agent.init();
    printBlock(now(), [
      mode === "Claude Max"
        ? `✓ Authenticated: ${mode}`
        : red(`⚠ Auth: ${mode}`),
    ]);
  } catch (err: any) {
    printBlock(now(), [red(`⚠ Auth error: ${err.message}`)]);
  }

  // Load world state (zone 1) into memory for system prompt injection
  await agent.loadWorldState().catch(() => {});

  let abortController: AbortController | null = null;
  let isStreaming = false;
  let shuttingDown = false;

  async function handleSubmit(line: string): Promise<void> {
    println("");  // newline after inline-echoed input

    const trimmed = line.trim();
    if (!trimmed) {
      printBlock(now(), [dim("(empty input — type a message)")]);
      printPrompt(bold(green("❯ ")));
      return;
    }
    if (isStreaming) {
      printPrompt(bold(green("❯ ")));
      return;
    }

    isStreaming = true;
    const confirmTool = async () => true;
    abortController = new AbortController();
    let fullText = "";
    let streamingStarted = false;

    try {
      for await (const event of agent.sendMessage(trimmed, confirmTool, abortController.signal)) {
        switch (event.type) {

          case "user_message":
            printBlock(now(), renderUserMessage(event.content));
            break;

          case "api_call_start":
            printBlock(now(), renderApiRequest(
              event.callNumber,
              event.provider,
              event.url,
              event.request,
            ));
            break;

          case "api_response":
            if (streamingStarted) {
              println("");
              streamingStarted = false;
            }
            printBlock(now(), renderApiResponse(
              event.provider,
              event.url,
              event.stopReason,
              event.usage,
              event.content,
              event.raw,
            ));
            break;

          case "api_error": {
            const shortUrl = event.url ? event.url.replace(/^https?:\/\//, "") : "unknown";
            printBlock(now(), [red(`api error (${event.provider} ${shortUrl}): ${event.error}`)]);
            break;
          }

          case "status":
            // No live zone — skip
            break;

          case "text":
            if (!streamingStarted) {
              // Print assistant message header, then stream content inline
              printBlock(now(), [
                bold("message"),
                `${INDENT}role: "assistant"`,
                `${INDENT}content:`,
              ]);
              process.stdout.write(" ".repeat(TIME_WIDTH) + INDENT2);
              streamingStarted = true;
            }
            process.stdout.write(event.text);
            fullText += event.text;
            break;

          case "tool_call":
            printBlock(now(), renderToolStart(event.name, event.input));
            break;

          case "tool_result": {
            printBlock(now(), renderToolResult(event.result));
            break;
          }

          case "tool_result_message":
            printBlock(now(), renderToolResultMessage(event.results));
            break;

          case "metrics":
            break;

          case "turn_end": {
            if (streamingStarted) {
              println(""); // end the streamed line
              streamingStarted = false;
            }
            const m = event.metrics;
            const provider = event.provider;
            const model = event.model;
            const { turnLine, sessionLine } = formatTurnFooter(
              { inputTokens: m.inputTokens, outputTokens: m.outputTokens, costUsd: m.costUsd, ttftMs: m.ttftMs },
              { inputTokens: agent.sessionInputTokens, outputTokens: agent.sessionOutputTokens, costUsd: agent.sessionCostUsd },
              provider,
              model,
            );
            printBlock(now(), [turnLine]);
            printBlock(now(), [sessionLine]);
            break;
          }

          case "error":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [red(`⚠ ${event.error}`)]);
            break;

          case "interrupted":
            if (streamingStarted) { println(""); streamingStarted = false; }
            if (fullText) {
              printBlock(now(), renderAssistantMessage(fullText));
              fullText = "";
            }
            printBlock(now(), [red("⊘ Interrupted")]);
            break;
        }
      }
    } catch (err: any) {
      if (streamingStarted) { println(""); streamingStarted = false; }
      printBlock(now(), [red(`⚠ ${err.message}`)]);
    } finally {
      isStreaming = false;
      abortController = null;
      printPrompt(bold(green("❯ ")));
    }
  }

  /** Idempotent shutdown — guards against double-invocation (e.g. two Ctrl+C). */
  function initiateShutdown(): void {
    if (shuttingDown) return;
    shuttingDown = true;
    // Stop accepting input immediately so keypresses during world-state save
    // don't produce confusing "empty input" warnings or trigger handlers.
    process.stdin.removeAllListeners("data");
    process.stdin.setRawMode?.(false);
    process.stdin.pause();
    // Abort any in-flight stream so the world-state fold sees the latest context.
    if (abortController) { abortController.abort(); abortController = null; }
    shutdown(agent, 0);
  }

  // Handle SIGINT and SIGTERM: fold world state then exit cleanly
  process.once("SIGINT",  initiateShutdown);
  process.once("SIGTERM", initiateShutdown);

  setupRawInput(
    (line) => { handleSubmit(line).catch(console.error); },
    () => { if (abortController) { abortController.abort(); abortController = null; } },
    initiateShutdown,
  );

  // Initial prompt — show shortcuts once at startup
  printBlock(now(), [renderStatus(false)]);
  printPrompt(bold(green("❯ ")));
}

// Entry point — only run when executed directly, not when imported in tests
if (import.meta.main) {
  runApp().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
