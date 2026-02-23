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

/** Returns a short display suffix for a tool call ID (last 6 chars). */
function shortId(id: string): string {
  return id.length <= 6 ? id : `…${id.slice(-6)}`;
}

/** Render the start of a tool call (name + input only). Shown immediately when the call is issued. */
export function renderToolStart(name: string, input: any, id: string): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`tool call`)) + dim(yellow(`  [${shortId(id)}]`)));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  lines.push(yellow(`${INDENT}input: ${JSON.stringify(input)}`));
  return lines;
}

/** Render the result of a tool call. Shown with a fresh timestamp when the tool completes. */
export function renderToolResult(result: { output: string; isError: boolean }, id: string): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow(`tool result`)) + dim(yellow(`  [${shortId(id)}]`)));
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
  return dim(streaming ? "Esc to interrupt" : "Ctrl+C to quit  /sonnet /opus /codex /help");
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
const sharedPasteState = { inPaste: false, startVisualCol: 0, startCursor: 0 };

// ---------------------------------------------------------------------------
// Cursor helpers — work on character arrays to handle surrogate pairs
// ---------------------------------------------------------------------------

/** Get effective cursor position, defaulting to end of buffer. */
function getCursor(buf: { value: string; cursor?: number }): number {
  return buf.cursor ?? [...buf.value].length;
}

/** Set cursor, clamping to valid range. */
function setCursor(buf: { value: string; cursor?: number }, pos: number): void {
  const chars = [...buf.value];
  buf.cursor = Math.max(0, Math.min(pos, chars.length));
}

/** Display width of a substring (array of chars). */
function charsDisplayWidth(chars: string[]): number {
  let w = 0;
  for (const ch of chars) w += displayWidth(ch) || 1;
  return w;
}

/**
 * Emit ANSI escape sequences to move the terminal cursor from one absolute
 * visual column to another.  Works correctly across row boundaries when
 * `terminalWidth` is known.
 *
 * @param fromCol   Current absolute visual column (promptWidth + charWidth sum).
 * @param toCol     Target absolute visual column.
 * @param tw        Terminal width in columns.
 */
function moveVisualCol(fromCol: number, toCol: number, tw: number): void {
  if (fromCol === toCol) return;
  const fromRow = Math.floor(fromCol / tw);
  const toRow   = Math.floor(toCol   / tw);
  const fromC   = fromCol % tw;
  const toC     = toCol   % tw;

  // Vertical movement first.
  if (toRow < fromRow) process.stdout.write(`\x1b[${fromRow - toRow}A`);
  else if (toRow > fromRow) process.stdout.write(`\x1b[${toRow - fromRow}B`);

  // Horizontal: use CR + forward-move for robustness (avoids stopping at margins).
  if (toC === 0) {
    process.stdout.write("\r");
  } else if (toC !== fromC) {
    // CR + advance is always safe regardless of fromC.
    process.stdout.write("\r");
    process.stdout.write(`\x1b[${toC}C`);
  }
}

/** Find word boundary backward from cursor (skips spaces, then non-spaces). */
function wordBoundaryBack(chars: string[], cursor: number): number {
  let pos = cursor;
  while (pos > 0 && chars[pos - 1] === " ") pos--;
  while (pos > 0 && chars[pos - 1] !== " ") pos--;
  return pos;
}

/** Find word boundary forward from cursor (skips spaces, then non-spaces). */
function wordBoundaryForward(chars: string[], cursor: number): number {
  let pos = cursor;
  while (pos < chars.length && chars[pos] === " ") pos++;
  while (pos < chars.length && chars[pos] !== " ") pos++;
  return pos;
}

/**
 * Redraw the entire input line after an edit that may span wrapped terminal rows.
 *
 * @param chars            The full buffer character array after the edit.
 * @param logicalCursor    The char index where the cursor should end up.
 * @param terminalVisualCol The absolute visual column (from col 0) where the
 *                          terminal cursor currently sits — i.e. promptWidth +
 *                          display-width of all chars up to the current terminal
 *                          cursor position, computed from the PRE-EDIT buffer.
 *                          Only used on the wrap-safe path.
 * @param buf              Buffer object — used to read terminalWidth / promptWidth.
 *
 * When `terminalWidth` and `promptWidth` are known (wrap-safe path) we:
 *   1. Scroll up to the first row of the input (using terminalVisualCol to compute
 *      how many rows the terminal cursor is below the input start).
 *   2. CR to col 0, skip forward past the prompt.
 *   3. Rewrite the full buffer.
 *   4. Erase to end of screen (\x1b[J) to clear any wrapped-row leftovers.
 *   5. Reposition the cursor at logicalCursor.
 *
 * When terminal dimensions are unknown we fall back to the old heuristic:
 * write the tail, erase to EOL, move back.  This requires the terminal cursor
 * to already be at `logicalCursor` before the call (the caller is responsible
 * for emitting the necessary pre-move when in legacy mode).
 */
function redrawLine(
  chars: string[],
  logicalCursor: number,
  terminalVisualCol: number,
  buf: { terminalWidth?: number; promptWidth?: number },
): void {
  const tw = buf.terminalWidth;
  const pw = buf.promptWidth ?? 0;

  if (tw !== undefined && tw > 0) {
    // --- Full-line redraw (wrap-safe) ---

    const cursorVisualCol = pw + charsDisplayWidth(chars.slice(0, logicalCursor));
    const totalVisualCol  = pw + charsDisplayWidth(chars);

    // Terminal row of the terminal cursor (relative to the input's first row).
    const termRow = Math.floor(terminalVisualCol / tw);
    // Rows to scroll up to reach the input's first row.
    if (termRow > 0) process.stdout.write(`\x1b[${termRow}A`);
    // CR to column 0 of that row.
    process.stdout.write("\r");
    // Move forward past the prompt (already on screen — just advance cursor).
    if (pw > 0) process.stdout.write(`\x1b[${pw}C`);

    // Rewrite the entire buffer.
    process.stdout.write(chars.join(""));

    // Erase any leftover characters to end of screen (clears shrunken content
    // on wrapped rows that no longer carry text).
    process.stdout.write("\x1b[J");

    // Reposition cursor at logicalCursor.
    // After writing all chars the terminal cursor is at totalVisualCol.
    const totalRow  = Math.floor(totalVisualCol / tw);
    const cursorRow = Math.floor(cursorVisualCol / tw);
    const tailRows  = totalRow - cursorRow;
    if (tailRows > 0) process.stdout.write(`\x1b[${tailRows}A`);
    const targetCol  = cursorVisualCol % tw;
    const currentCol = totalVisualCol % tw;
    if (currentCol > targetCol) {
      process.stdout.write(`\x1b[${currentCol - targetCol}D`);
    } else if (targetCol > currentCol) {
      process.stdout.write(`\x1b[${targetCol - currentCol}C`);
    }
  } else {
    // --- Legacy heuristic (no terminal dimensions) ---
    // The caller must have already moved the terminal cursor to logicalCursor.
    const tail = chars.slice(logicalCursor).join("");
    const tailWidth = charsDisplayWidth(chars.slice(logicalCursor));
    process.stdout.write(tail + "\x1b[K");
    if (tailWidth > 0) process.stdout.write(`\x1b[${tailWidth}D`);
  }
}

export function parseKeys(
  chunk: string,
  callbacks: KeyCallbacks,
  buf: { value: string; cursor?: number; columns?: number; terminalWidth?: number; promptWidth?: number } = sharedBuffer,
  options: { inputEnabled?: boolean; pasteState?: { inPaste: boolean; startVisualCol: number; startCursor: number } } = {},
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
        // Record where the terminal cursor and logical cursor are at paste-start
        // so we can echo correctly when the paste ends.
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        pasteState.startVisualCol = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
        pasteState.startCursor = cursor;
        i += 5; // skip past "[200~"
        continue;
      }
      if (chunk.startsWith("[201~", i + 1)) {
        pasteState.inPaste = false;
        i += 5; // skip past "[201~"
        // Echo what was pasted and position the cursor correctly.
        // Nothing was echoed during the paste, so the terminal cursor is still
        // at pasteState.startVisualCol (the position it was at paste-start).
        //
        // We write the portion of the buffer from startCursor to the current
        // cursor directly — this covers both the legacy (no terminalWidth) and
        // wrap-safe paths.  For the wrap-safe path we then do a full redrawLine
        // which also handles any pre-existing tail beyond the cursor.
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        if (buf.terminalWidth !== undefined && buf.terminalWidth > 0) {
          // Wrap-safe: full redraw from paste-start visual col.
          redrawLine(chars, cursor, pasteState.startVisualCol, buf);
        } else {
          // Legacy: just write the pasted slice and any tail, then reposition.
          // Chars from startCursor onward need to be displayed; the terminal
          // cursor is at pasteState.startVisualCol which corresponds to startCursor.
          const toEnd = chars.slice(pasteState.startCursor).join("");
          process.stdout.write(toEnd);
          // If cursor is not at end, move back.
          const tailWidth = charsDisplayWidth(chars.slice(cursor));
          if (tailWidth > 0) process.stdout.write(`\x1b[${tailWidth}D`);
        }
        continue;
      }

      // ESC + DEL = Ctrl+Backspace (delete word backward) in many terminals
      if (i + 1 < chunk.length && chunk.charCodeAt(i + 1) === 127) {
        i++; // consume the DEL
        if (!inputEnabled) continue;
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        if (cursor === 0) continue;
        const newPos = wordBoundaryBack(chars, cursor);
        // Compute terminal visual col BEFORE splice (terminal cursor is at `cursor`)
        const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
        const deleted = chars.splice(newPos, cursor - newPos);
        const deletedWidth = charsDisplayWidth(deleted);
        buf.value = chars.join("");
        buf.cursor = newPos;
        if (buf.terminalWidth === undefined && deletedWidth > 0) {
          // Legacy: pre-move required before redrawLine
          process.stdout.write(`\x1b[${deletedWidth}D`);
        }
        redrawLine(chars, newPos, termVc, buf);
        continue;
      }

      if (i + 1 < chunk.length && chunk[i + 1] === "[") {
        // CSI sequence — parse it
        i += 2; // skip ESC [
        let params = "";
        while (i < chunk.length && !/[A-Za-z~]/.test(chunk[i])) {
          params += chunk[i];
          i++;
        }
        const final = i < chunk.length ? chunk[i] : "";

        if (!inputEnabled) continue;

        const chars = [...buf.value];
        const cursor = getCursor(buf);

        // Arrow keys: A=Up B=Down C=Right D=Left
        if (final === "D" && !params) {
          // Left arrow
          if (cursor > 0) {
            const pw = buf.promptWidth ?? 0;
            const fromCol = pw + charsDisplayWidth(chars.slice(0, cursor));
            buf.cursor = cursor - 1;
            if (buf.terminalWidth !== undefined) {
              const toCol = pw + charsDisplayWidth(chars.slice(0, cursor - 1));
              moveVisualCol(fromCol, toCol, buf.terminalWidth);
            } else {
              const w = displayWidth(chars[cursor - 1]) || 1;
              process.stdout.write(`\x1b[${w}D`);
            }
          }
          continue;
        }
        if (final === "C" && !params) {
          // Right arrow
          if (cursor < chars.length) {
            const pw = buf.promptWidth ?? 0;
            const fromCol = pw + charsDisplayWidth(chars.slice(0, cursor));
            buf.cursor = cursor + 1;
            if (buf.terminalWidth !== undefined) {
              const toCol = pw + charsDisplayWidth(chars.slice(0, cursor + 1));
              moveVisualCol(fromCol, toCol, buf.terminalWidth);
            } else {
              const w = displayWidth(chars[cursor]) || 1;
              process.stdout.write(`\x1b[${w}C`);
            }
          }
          continue;
        }

        // Ctrl+Left: \x1b[1;5D
        if (final === "D" && params === "1;5") {
          const newPos = wordBoundaryBack(chars, cursor);
          if (newPos < cursor) {
            const pw = buf.promptWidth ?? 0;
            const fromCol = pw + charsDisplayWidth(chars.slice(0, cursor));
            buf.cursor = newPos;
            if (buf.terminalWidth !== undefined) {
              const toCol = pw + charsDisplayWidth(chars.slice(0, newPos));
              moveVisualCol(fromCol, toCol, buf.terminalWidth);
            } else {
              const moveWidth = charsDisplayWidth(chars.slice(newPos, cursor));
              process.stdout.write(`\x1b[${moveWidth}D`);
            }
          }
          continue;
        }
        // Ctrl+Right: \x1b[1;5C
        if (final === "C" && params === "1;5") {
          const newPos = wordBoundaryForward(chars, cursor);
          if (newPos > cursor) {
            const pw = buf.promptWidth ?? 0;
            const fromCol = pw + charsDisplayWidth(chars.slice(0, cursor));
            buf.cursor = newPos;
            if (buf.terminalWidth !== undefined) {
              const toCol = pw + charsDisplayWidth(chars.slice(0, newPos));
              moveVisualCol(fromCol, toCol, buf.terminalWidth);
            } else {
              const moveWidth = charsDisplayWidth(chars.slice(cursor, newPos));
              process.stdout.write(`\x1b[${moveWidth}C`);
            }
          }
          continue;
        }

        // Ctrl+Delete: \x1b[3;5~
        if (final === "~" && params === "3;5") {
          if (cursor >= chars.length) continue;
          const newEnd = wordBoundaryForward(chars, cursor);
          // Compute terminal visual col BEFORE splice (terminal cursor is at `cursor`)
          const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
          chars.splice(cursor, newEnd - cursor);
          buf.value = chars.join("");
          buf.cursor = cursor;
          redrawLine(chars, cursor, termVc, buf);
          continue;
        }

        // All other CSI sequences (Up, Down, etc.) — ignore
        continue;
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
        if (buf.cursor !== undefined) buf.cursor++;
      } else {
        // Normal Enter: submit
        const line = buf.value;
        buf.value = "";
        buf.cursor = 0;
        if (buf.columns !== undefined) buf.columns = 0;
        onSubmit(line);
      }
      continue;
    }

    // Ctrl+Backspace: some terminals send 0x08
    if (code === 8) {
      const chars = [...buf.value];
      const cursor = getCursor(buf);
      if (cursor === 0) continue;
      const newPos = wordBoundaryBack(chars, cursor);
      // Compute terminal visual col BEFORE splice (terminal cursor is at `cursor`)
      const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
      const deleted = chars.splice(newPos, cursor - newPos);
      const deletedWidth = charsDisplayWidth(deleted);
      buf.value = chars.join("");
      buf.cursor = newPos;
      if (buf.terminalWidth === undefined && deletedWidth > 0) {
        process.stdout.write(`\x1b[${deletedWidth}D`);
      }
      redrawLine(chars, newPos, termVc, buf);
      continue;
    }

    if (code === 127) {                        // Backspace (DEL)
      const chars = [...buf.value];
      const cursor = getCursor(buf);
      if (cursor > 0) {
        const deleted = chars[cursor - 1];
        const w = displayWidth(deleted) || 1;
        // Compute terminal visual col BEFORE splice (terminal cursor is at `cursor`)
        const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
        chars.splice(cursor - 1, 1);
        buf.value = chars.join("");
        buf.cursor = cursor - 1;
        if (buf.columns !== undefined) buf.columns -= w;
        if (buf.terminalWidth !== undefined) {
          // Wrap-safe: full-line redraw.
          redrawLine(chars, cursor - 1, termVc, buf);
        } else if (cursor === chars.length + 1) {
          // Legacy, was at end of line — simple erase
          process.stdout.write("\b".repeat(w) + " ".repeat(w) + "\b".repeat(w));
        } else {
          // Legacy, mid-line: move back then redraw tail
          process.stdout.write(`\x1b[${w}D`);
          // For legacy, terminal cursor is at cursor-1 after the move-back above
          redrawLine(chars, cursor - 1, (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor - 1)), buf);
        }
      }
      continue;
    }

    // Printable character
    if (code >= 32 || code > 127) {
      const w = displayWidth(ch) || 1;
      const cursor = getCursor(buf);

      // Fast path: appending at end and not in paste — O(1), no spread needed.
      // We detect "cursor at end" by checking whether buf.cursor equals the
      // logical char count.  buf.cursor is always maintained in char units, and
      // buf.value.length equals the char count for pure-BMP Unicode (which
      // covers all dictation output).  For safety, we fall back to the full
      // path whenever the char is outside the BMP (surrogate pairs, emoji).
      const isBmp = ch.length === 1; // BMP chars are single JS code units
      if (!pasteState.inPaste && isBmp && buf.cursor !== undefined && buf.cursor === buf.value.length) {
        buf.value += ch;
        buf.cursor += 1;
        if (buf.columns !== undefined) buf.columns += w;
        process.stdout.write(ch);
        continue;
      }

      // General path: mid-line insert or non-BMP or paste mode.
      const chars = [...buf.value];
      chars.splice(cursor, 0, ch);
      buf.value = chars.join("");
      buf.cursor = cursor + 1;
      if (buf.columns !== undefined) buf.columns += w;
      if (!pasteState.inPaste) {
        if (cursor === chars.length - 1) {
          // Appending at end (non-BMP case) — just echo the char
          process.stdout.write(ch);
        } else {
          // Mid-line insert: write char, then redraw tail.
          process.stdout.write(ch);
          const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor + 1));
          redrawLine(chars, cursor + 1, termVc, buf);
        }
      }
    }
  }

  return buf.value;
}

/** Shared mutable buffer used by setupRawInput in production. */
const sharedBuffer: {
  value: string;
  cursor: number;
  columns: number;
  terminalWidth?: number;
  promptWidth?: number;
} = { value: "", cursor: 0, columns: 0 };

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

  // Initialise terminal width from the current stdout columns.
  sharedBuffer.terminalWidth = process.stdout.columns || undefined;

  // Keep terminal width up to date when the user resizes the window.
  process.stdout.on("resize", () => {
    sharedBuffer.terminalWidth = process.stdout.columns || undefined;
  });

  process.stdin.on("data", (chunk: string) => {
    parseKeys(chunk, { onSubmit, onEscape, onExit });
  });
}

// ---------------------------------------------------------------------------
// Input prompt
// ---------------------------------------------------------------------------

/** The visible width (columns) of the prompt prefix string, excluding ANSI codes. */
function promptVisualWidth(prefix: string): number {
  // Strip ANSI escape sequences and measure printable width.
  const stripped = prefix.replace(/\x1b\[[0-9;]*m/g, "");
  let w = 0;
  for (const ch of [...stripped]) w += displayWidth(ch) || 1;
  return w;
}

function printPrompt(prefix: string): void {
  const timeStr = dim(now().padEnd(TIME_WIDTH));
  process.stdout.write(timeStr + prefix);
  // Record where the user's input will start so redrawLine can position correctly.
  sharedBuffer.promptWidth = TIME_WIDTH + promptVisualWidth(prefix);
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
            printBlock(now(), event.message.split("\n"));
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
            printBlock(now(), renderToolStart(event.name, event.input, event.id));
            break;

          case "tool_result": {
            printBlock(now(), renderToolResult(event.result, event.id));
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
              { inputTokens: m.inputTokens, outputTokens: m.outputTokens, costUsd: m.costUsd, savedUsd: m.savedUsd, ttftMs: m.ttftMs, cacheCreationTokens: m.cacheCreationTokens, cacheReadTokens: m.cacheReadTokens },
              { inputTokens: agent.sessionInputTokens, outputTokens: agent.sessionOutputTokens, costUsd: agent.sessionCostUsd, savedUsd: agent.sessionSavedUsd, cacheCreationTokens: agent.sessionCacheCreationTokens, cacheReadTokens: agent.sessionCacheReadTokens },
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
