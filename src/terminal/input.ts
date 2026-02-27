/**
 * Raw terminal key-input parsing and line editing.
 *
 * Pure I/O logic — no Agent, no rendering of agent events.
 * Exported for unit tests and for use by the terminal app.
 */

// ---------------------------------------------------------------------------
// Display-width helper
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Visual column movement
// ---------------------------------------------------------------------------

/**
 * Emit ANSI escape sequences to move the terminal cursor from one absolute
 * visual column to another.  Works correctly across row boundaries when
 * `terminalWidth` is known.
 */
function moveVisualCol(fromCol: number, toCol: number, tw: number): void {
  if (fromCol === toCol) return;
  const fromRow = Math.floor(fromCol / tw);
  const toRow   = Math.floor(toCol   / tw);
  const fromC   = fromCol % tw;
  const toC     = toCol   % tw;

  if (toRow < fromRow) process.stdout.write(`\x1b[${fromRow - toRow}A`);
  else if (toRow > fromRow) process.stdout.write(`\x1b[${toRow - fromRow}B`);

  if (toC === 0) {
    process.stdout.write("\r");
  } else if (toC !== fromC) {
    process.stdout.write("\r");
    process.stdout.write(`\x1b[${toC}C`);
  }
}

// ---------------------------------------------------------------------------
// Word boundary helpers
// ---------------------------------------------------------------------------

/** Find word boundary backward from cursor (skips spaces, then non-spaces). */
function wordBoundaryBack(chars: string[], cursor: number): number {
  let pos = cursor;
  while (pos > 0 && chars[pos - 1] === " ") pos--;
  while (pos > 0 && chars[pos - 1] !== " ") pos--;
  return pos;
}

/** Find word boundary forward from cursor (skips non-spaces, then spaces). */
function wordBoundaryForward(chars: string[], cursor: number): number {
  let pos = cursor;
  while (pos < chars.length && chars[pos] === " ") pos++;
  while (pos < chars.length && chars[pos] !== " ") pos++;
  return pos;
}

// ---------------------------------------------------------------------------
// Line redraw
// ---------------------------------------------------------------------------

/**
 * Redraw the entire input line after an edit that may span wrapped terminal rows.
 *
 * When `terminalWidth` and `promptWidth` are known (wrap-safe path):
 *   1. Scroll up to the first row of the input.
 *   2. CR to col 0, skip forward past the prompt.
 *   3. Rewrite the full buffer.
 *   4. Erase to end of screen (\x1b[J).
 *   5. Reposition the cursor at logicalCursor.
 *
 * When terminal dimensions are unknown: falls back to the old heuristic
 * (write tail, erase to EOL, move back).  Caller must have already moved
 * the terminal cursor to logicalCursor before calling in legacy mode.
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
    const cursorVisualCol = pw + charsDisplayWidth(chars.slice(0, logicalCursor));
    const totalVisualCol  = pw + charsDisplayWidth(chars);

    const termRow = Math.floor(terminalVisualCol / tw);
    if (termRow > 0) process.stdout.write(`\x1b[${termRow}A`);
    process.stdout.write("\r");
    if (pw > 0) process.stdout.write(`\x1b[${pw}C`);

    process.stdout.write(chars.join(""));
    process.stdout.write("\x1b[J");

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
    // Legacy heuristic
    const tail = chars.slice(logicalCursor).join("");
    const tailWidth = charsDisplayWidth(chars.slice(logicalCursor));
    process.stdout.write(tail + "\x1b[K");
    if (tailWidth > 0) process.stdout.write(`\x1b[${tailWidth}D`);
  }
}

// ---------------------------------------------------------------------------
// Key parsing
// ---------------------------------------------------------------------------

/** Callbacks for key input parsing. */
export interface KeyCallbacks {
  onSubmit: (line: string) => void;
  onEscape: () => void;
  onExit: () => void;
}

/** Persistent bracketed-paste state shared across calls in production. */
export const sharedPasteState = { inPaste: false, startVisualCol: 0, startCursor: 0 };

/** Shared mutable buffer used by setupRawInput in production. */
export const sharedBuffer: {
  value: string;
  cursor: number;
  columns: number;
  terminalWidth?: number;
  promptWidth?: number;
} = { value: "", cursor: 0, columns: 0 };

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
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        pasteState.startVisualCol = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
        pasteState.startCursor = cursor;
        i += 5;
        continue;
      }
      if (chunk.startsWith("[201~", i + 1)) {
        pasteState.inPaste = false;
        i += 5;
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        if (buf.terminalWidth !== undefined && buf.terminalWidth > 0) {
          redrawLine(chars, cursor, pasteState.startVisualCol, buf);
        } else {
          const toEnd = chars.slice(pasteState.startCursor).join("");
          process.stdout.write(toEnd);
          const tailWidth = charsDisplayWidth(chars.slice(cursor));
          if (tailWidth > 0) process.stdout.write(`\x1b[${tailWidth}D`);
        }
        continue;
      }

      // ESC + DEL = Ctrl+Backspace (delete word backward) in many terminals
      if (i + 1 < chunk.length && chunk.charCodeAt(i + 1) === 127) {
        i++;
        if (!inputEnabled) continue;
        const chars = [...buf.value];
        const cursor = getCursor(buf);
        if (cursor === 0) continue;
        const newPos = wordBoundaryBack(chars, cursor);
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

      if (i + 1 < chunk.length && chunk[i + 1] === "[") {
        i += 2;
        let params = "";
        while (i < chunk.length && !/[A-Za-z~]/.test(chunk[i])) {
          params += chunk[i];
          i++;
        }
        const final = i < chunk.length ? chunk[i] : "";

        if (!inputEnabled) continue;

        const chars = [...buf.value];
        const cursor = getCursor(buf);

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

        // Delete: \x1b[3~  (forward-delete one char)
        if (final === "~" && params === "3") {
          if (cursor < chars.length) {
            const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
            chars.splice(cursor, 1);
            buf.value = chars.join("");
            buf.cursor = cursor;
            redrawLine(chars, cursor, termVc, buf);
          }
          continue;
        }

        // Ctrl+Delete: \x1b[3;5~
        if (final === "~" && params === "3;5") {
          if (cursor >= chars.length) continue;
          const newEnd = wordBoundaryForward(chars, cursor);
          const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
          chars.splice(cursor, newEnd - cursor);
          buf.value = chars.join("");
          buf.cursor = cursor;
          redrawLine(chars, cursor, termVc, buf);
          continue;
        }

        // All other CSI sequences — ignore
        continue;
      } else {
        if (!pasteState.inPaste) onEscape();
      }
      continue;
    }

    if (!inputEnabled) continue;

    if (ch === "\r" || ch === "\n") {
      if (pasteState.inPaste) {
        buf.value += "\n";
        if (buf.cursor !== undefined) buf.cursor++;
      } else {
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
        const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor));
        chars.splice(cursor - 1, 1);
        buf.value = chars.join("");
        buf.cursor = cursor - 1;
        if (buf.columns !== undefined) buf.columns -= w;
        if (buf.terminalWidth !== undefined) {
          redrawLine(chars, cursor - 1, termVc, buf);
        } else if (cursor === chars.length + 1) {
          process.stdout.write("\b".repeat(w) + " ".repeat(w) + "\b".repeat(w));
        } else {
          process.stdout.write(`\x1b[${w}D`);
          redrawLine(chars, cursor - 1, (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor - 1)), buf);
        }
      }
      continue;
    }

    // Printable character
    if (code >= 32 || code > 127) {
      const w = displayWidth(ch) || 1;
      const cursor = getCursor(buf);

      // Fast path: appending at end (BMP chars only) — O(1), no spread needed.
      const isBmp = ch.length === 1;
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
          process.stdout.write(ch);
        } else {
          process.stdout.write(ch);
          const termVc = (buf.promptWidth ?? 0) + charsDisplayWidth(chars.slice(0, cursor + 1));
          redrawLine(chars, cursor + 1, termVc, buf);
        }
      }
    }
  }

  return buf.value;
}
