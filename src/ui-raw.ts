/**
 * Raw terminal UI — no library.
 *
 * Architecture:
 * - Scrollback: plain stdout writes. Terminal owns history.
 * - Live zone: N lines at bottom, redrawn by moving cursor up and clearing.
 * - Input: raw stdin, buffered. Decoupled from rendering.
 * - Resize: SIGWINCH redraws live zone at new width.
 */

import { Agent } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";

// ---------------------------------------------------------------------------
// ANSI escape helpers
// ---------------------------------------------------------------------------

const ESC = "\x1b";
const CSI = `${ESC}[`;

const ansi = {
  cursorUp:       (n: number) => `${CSI}${n}A`,
  clearLine:      () => `${CSI}2K`,
  clearToEnd:     () => `${CSI}0J`,
  col1:           () => `${CSI}G`,           // move to column 1
  saveCursor:     () => `${ESC}7`,
  restoreCursor:  () => `${ESC}8`,
  bold:           (s: string) => `${CSI}1m${s}${CSI}0m`,
  dim:            (s: string) => `${CSI}2m${s}${CSI}0m`,
  color:          (code: number, s: string) => `${CSI}${code}m${s}${CSI}0m`,
  reset:          () => `${CSI}0m`,
  syncStart:      () => `${CSI}?2026h`,   // synchronized output begin
  syncEnd:        () => `${CSI}?2026l`,   // synchronized output end
};

// Named color codes
const GREEN   = 32;
const CYAN    = 36;
const BLUE    = 34;
const YELLOW  = 33;
const MAGENTA = 35;
const RED     = 31;
const DIM     = 2;

function green  (s: string) { return ansi.color(GREEN,   s); }
function cyan   (s: string) { return ansi.color(CYAN,    s); }
function blue   (s: string) { return ansi.color(BLUE,    s); }
function yellow (s: string) { return ansi.color(YELLOW,  s); }
function magenta(s: string) { return ansi.color(MAGENTA, s); }
function red    (s: string) { return ansi.color(RED,     s); }
function dim    (s: string) { return ansi.color(DIM,     s); }
function bold   (s: string) { return ansi.bold(s); }

// ---------------------------------------------------------------------------
// Terminal dimensions
// ---------------------------------------------------------------------------

function termWidth(): number  { return process.stdout.columns ?? 80; }
function termHeight(): number { return process.stdout.rows ?? 24; }

// ---------------------------------------------------------------------------
// Scrollback — just print lines
// ---------------------------------------------------------------------------

const TIME_WIDTH = 10; // "HH:MM:SS  "
const INDENT  = "  ";
const INDENT2 = "    ";
const INDENT3 = "      ";

function now(): string {
  return new Date().toLocaleTimeString("en-GB");
}

function formatCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(3)}`;
}

function formatMs(ms: number | null): string {
  if (ms === null) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function truncateOutput(text: string, maxLines = 10): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… [${lines.length - maxLines} more lines]`;
}

function truncateLine(s: string, width: number): string {
  // Strip ANSI codes to measure visual length, then truncate if needed.
  // Simple approximation: measure raw length minus escape sequences.
  const visible = s.replace(/\x1b\[[0-9;]*m/g, "");
  if (visible.length <= width) return s;
  // Truncate the raw string — imprecise with ANSI but good enough
  return s.slice(0, width - 1) + "…";
}

/** Print a block of lines to scrollback. First line gets the timestamp. */
function printBlock(time: string, lines: string[]): void {
  const w = termWidth();
  for (let i = 0; i < lines.length; i++) {
    const prefix = i === 0
      ? dim(time.padEnd(TIME_WIDTH))
      : " ".repeat(TIME_WIDTH);
    const full = prefix + lines[i];
    process.stdout.write(truncateLine(full, w) + "\n");
  }
}

// --- Block renderers ---

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

function renderApiRequest(
  callNumber: number,
  model: string,
  system: string,
  tools: any[],
  messages: any[],
): string[] {
  const last = messages[messages.length - 1];
  const lastSummary = last
    ? `{ role: "${last.role}", content: ${summariseContent(last.content)} }`
    : "";
  const lines: string[] = [];
  lines.push(bold(cyan(`api call #${callNumber}`)));
  lines.push(cyan(`${INDENT}model: "${model}"`));
  lines.push(cyan(`${INDENT}system: <${system.length} chars>`));
  lines.push(cyan(`${INDENT}tools: [${tools.map((t: any) => `"${t.name}"`).join(", ")}]`));
  lines.push(cyan(`${INDENT}max_tokens: ${config.maxOutputTokens}`));
  lines.push(cyan(`${INDENT}messages: <${messages.length}> …`));
  if (lastSummary) lines.push(dim(cyan(`${INDENT2}${lastSummary}`)));
  return lines;
}

function summariseContent(content: any): string {
  if (typeof content === "string") {
    return content.length <= 60 ? `"${content}"` : `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  const parts = content.map((b: any) => {
    if (b.type === "text") return `text: <${b.text.length} chars>`;
    if (b.type === "tool_use") return `tool_use: "${b.name}"`;
    if (b.type === "tool_result") return `tool_result: <${(b.content as string).length} chars>`;
    return b.type;
  });
  return `[${parts.join(", ")}]`;
}

function renderApiResponse(
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: any[],
): string[] {
  const lines: string[] = [];
  lines.push(bold(blue("api response")));
  lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
  lines.push(blue(`${INDENT}usage:`));
  lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
  lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
  lines.push(blue(`${INDENT}content:`));
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
    for (const line of dimText.split("\n")) {
      lines.push(dim(`${INDENT}${line}`));
    }
  }
  return lines;
}

// ---------------------------------------------------------------------------
// Live zone — redrawn lines at the bottom
// ---------------------------------------------------------------------------

interface LiveState {
  streaming: string;       // partial assistant text being streamed
  activity: string;        // "thinking…", "read_file…", etc.
  input: string;           // current input buffer
  isStreaming: boolean;
  isReady: boolean;
  resumePrompt: boolean;
  statusBar: string;
}

let liveState: LiveState = {
  streaming: "",
  activity: "",
  input: "",
  isStreaming: false,
  isReady: false,
  resumePrompt: false,
  statusBar: "",
};

// How many lines the live zone currently occupies (so we can erase them)
let liveLineCount = 0;

function renderLiveZone(): void {
  const w = termWidth();
  const lines: string[] = [];

  if (liveState.isStreaming && liveState.streaming) {
    // Streaming assistant text
    const prefix = " ".repeat(TIME_WIDTH);
    // Word-wrap the streaming text to terminal width
    const text = liveState.streaming + dim("▊");
    for (const line of wrapText(`${prefix}${text}`, w)) {
      lines.push(line);
    }
  } else if (liveState.isStreaming && liveState.activity) {
    lines.push(" ".repeat(TIME_WIDTH) + dim(`⏳ ${liveState.activity}`));
  } else if (liveState.isStreaming) {
    lines.push(" ".repeat(TIME_WIDTH) + dim("⏳ working..."));
  }

  // Input line
  const prompt = liveState.resumePrompt ? "? "
    : liveState.isStreaming ? dim("… ")
    : !liveState.isReady ? dim("… ")
    : bold(green("❯ "));
  const t = now();
  const timePrefix = liveState.isStreaming ? " ".repeat(TIME_WIDTH) : dim(t.padEnd(TIME_WIDTH));
  lines.push(timePrefix + prompt + liveState.input);

  // Status bar
  lines.push(dim(liveState.statusBar));

  // Erase previous live zone and redraw
  const out = [ansi.syncStart()];
  if (liveLineCount > 0) {
    out.push(ansi.cursorUp(liveLineCount));
    out.push(ansi.col1());
    out.push(ansi.clearToEnd());
  }
  for (const line of lines) {
    out.push(truncateLine(line, w) + "\n");
  }
  out.push(ansi.syncEnd());
  process.stdout.write(out.join(""));
  liveLineCount = lines.length;
}

function wrapText(text: string, width: number): string[] {
  // Simple wrap — split on spaces when line exceeds width
  // Strips ANSI to measure, but keeps ANSI in output (approximate)
  const visible = text.replace(/\x1b\[[0-9;]*m/g, "");
  if (visible.length <= width) return [text];
  // Naive: just split at width characters (ignoring ANSI codes)
  const result: string[] = [];
  let remaining = text;
  while (remaining.replace(/\x1b\[[0-9;]*m/g, "").length > width) {
    result.push(remaining.slice(0, width));
    remaining = " ".repeat(TIME_WIDTH) + remaining.slice(width);
  }
  result.push(remaining);
  return result;
}

function updateLive(patch: Partial<LiveState>): void {
  Object.assign(liveState, patch);
  renderLiveZone();
}

// ---------------------------------------------------------------------------
// Raw stdin input
// ---------------------------------------------------------------------------

let inputBuffer = "";

function setupRawInput(onSubmit: (line: string) => void, onEscape: () => void, onExit: () => void): void {
  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stdin.setEncoding("utf-8");

  process.stdin.on("data", (chunk: string) => {
    for (let i = 0; i < chunk.length; i++) {
      const ch = chunk[i];
      const code = chunk.charCodeAt(i);

      if (ch === "\x03") { // Ctrl+C
        onExit();
        return;
      }

      if (ch === "\x1b") { // Escape or escape sequence
        // Peek ahead — if next char arrives immediately it's a sequence
        // For simplicity, treat lone ESC as interrupt
        if (i + 1 >= chunk.length) {
          onEscape();
        }
        // Skip escape sequences (e.g. arrow keys: \x1b[A)
        if (i + 1 < chunk.length && chunk[i + 1] === "[") {
          i += 2; // skip the [ and the following letter
          while (i < chunk.length && !/[A-Za-z]/.test(chunk[i])) i++;
        }
        continue;
      }

      if (ch === "\r" || ch === "\n") { // Enter
        const line = inputBuffer;
        inputBuffer = "";
        updateLive({ input: "" });
        onSubmit(line);
        continue;
      }

      if (code === 127 || code === 8) { // Backspace
        if (inputBuffer.length > 0) {
          inputBuffer = inputBuffer.slice(0, -1);
          updateLive({ input: inputBuffer });
        }
        continue;
      }

      // Printable character (including multi-byte UTF-8 from dictation)
      if (code >= 32 || code > 127) {
        inputBuffer += ch;
        updateLive({ input: inputBuffer });
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

export async function runApp(): Promise<void> {
  const agent = new Agent();

  // Initial status bar
  function updateStatusBar(): void {
    updateLive({
      statusBar: `${config.model} │ in: ${agent.sessionInputTokens} out: ${agent.sessionOutputTokens} │ ${formatCost(agent.sessionCostUsd)}${liveState.isStreaming ? " │ Esc to interrupt" : " │ Ctrl+C quit"}`,
    });
  }

  // Resize handler
  process.on("SIGWINCH", () => {
    liveLineCount = 0; // force full redraw
    renderLiveZone();
  });

  // Start live zone
  updateLive({ isReady: false, statusBar: `${config.model} │ starting...` });

  // Init agent
  let authMode = "...";
  try {
    authMode = await agent.init();
    printBlock(now(), [
      authMode === "Claude Max"
        ? `✓ Authenticated: ${authMode}`
        : red(`⚠ Auth: ${authMode}`),
    ]);
  } catch (err: any) {
    printBlock(now(), [red(`⚠ Auth error: ${err.message}`)]);
  }

  updateStatusBar();

  // Check for prior session
  let priorSession: Session | null = null;
  let resumePromptDone = false;
  try {
    const prior = await agent.checkPriorSession();
    if (prior && prior.history.length > 0) {
      priorSession = prior;
      updateLive({
        resumePrompt: true,
        input: "",
      });
      printBlock(now(), [
        cyan(`↩ Prior session: ${new Date(prior.savedAt).toLocaleString()} (${prior.history.length} messages)`),
        dim("  Resume? [Y/n]"),
      ]);
    } else {
      resumePromptDone = true;
      updateLive({ isReady: true });
    }
  } catch {
    resumePromptDone = true;
    updateLive({ isReady: true });
  }

  let abortController: AbortController | null = null;
  let lastResponseText = "";
  let lastResponseDimText = "";

  async function handleSubmit(line: string): Promise<void> {
    // Resume prompt
    if (priorSession && !resumePromptDone) {
      const v = line.trim().toLowerCase();
      const resume = v === "y" || v === "yes" || v === "";
      if (resume) {
        agent.resumeSession(priorSession);
        printBlock(now(), [
          `↩ Resumed session from ${new Date(priorSession.savedAt).toLocaleString()} (${priorSession.history.length} messages)`,
        ]);
      } else {
        printBlock(now(), ["↩ Starting fresh session"]);
      }
      priorSession = null;
      resumePromptDone = true;
      updateLive({ resumePrompt: false, isReady: true });
      return;
    }

    const trimmed = line.trim();
    if (!trimmed || liveState.isStreaming || !liveState.isReady) return;

    // Flush last response to scrollback
    if (lastResponseText) {
      printBlock(now(), renderAssistantMessage(lastResponseText, lastResponseDimText));
      lastResponseText = "";
      lastResponseDimText = "";
    }

    updateLive({ isStreaming: true, streaming: "", activity: "" });
    updateStatusBar();

    const confirmTool = async () => true;
    abortController = new AbortController();

    let fullText = "";

    try {
      for await (const event of agent.sendMessage(trimmed, confirmTool, abortController.signal)) {
        switch (event.type) {

          case "user_message":
            printBlock(now(), renderUserMessage(event.content));
            break;

          case "api_call_start":
            updateLive({ activity: "thinking..." });
            printBlock(now(), renderApiRequest(
              event.callNumber,
              event.model,
              event.system,
              event.tools,
              event.messages,
            ));
            break;

          case "api_response":
            printBlock(now(), renderApiResponse(
              event.stopReason,
              event.usage,
              event.content,
            ));
            break;

          case "status":
            updateLive({ activity: event.message });
            break;

          case "text":
            fullText += event.text;
            updateLive({ streaming: fullText, activity: "" });
            break;

          case "tool_call":
            updateLive({ activity: `${event.name}…` });
            break;

          case "tool_result":
            printBlock(now(), renderToolExecution(
              event.name,
              event.input,
              event.result,
            ));
            updateLive({ activity: "" });
            break;

          case "tool_result_message":
            printBlock(now(), renderToolResultMessage(event.results));
            break;

          case "metrics":
            break;

          case "turn_end": {
            const m = event.metrics;
            const dimText = `in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)}`;
            if (fullText) {
              lastResponseText = fullText;
              lastResponseDimText = dimText;
              fullText = "";
              updateLive({ streaming: "" });
            } else {
              printBlock(now(), [dim(dimText)]);
            }
            break;
          }

          case "error":
            printBlock(now(), [red(`⚠ ${event.error}`)]);
            break;

          case "interrupted":
            if (fullText) {
              printBlock(now(), renderAssistantMessage(fullText));
              fullText = "";
              updateLive({ streaming: "" });
            }
            printBlock(now(), [red("⊘ Interrupted")]);
            break;
        }
        updateStatusBar();
      }
    } catch (err: any) {
      printBlock(now(), [red(`⚠ ${err.message}`)]);
    } finally {
      updateLive({ isStreaming: false, streaming: "", activity: "" });
      updateStatusBar();
    }
  }

  setupRawInput(
    (line) => { handleSubmit(line).catch(console.error); },
    () => { if (abortController) { abortController.abort(); abortController = null; } },
    () => {
      // Restore terminal state and exit
      process.stdin.setRawMode(false);
      process.stdout.write("\n");
      process.exit(0);
    },
  );
}
