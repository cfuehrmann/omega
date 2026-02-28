/**
 * Raw terminal key-input parsing — minimal append-only line editor.
 *
 * Design: the prompt is not a full line editor. It exists only for short
 * commands ("go", "act on item X"). Real composition happens in an external
 * editor; clipboard paste (bracketed paste) is the primary input path.
 *
 * Supported operations:
 *   - Printable characters  — append at end
 *   - Backspace             — delete last character
 *   - Enter                 — submit
 *   - Esc                   — context-sensitive cancel (see KeyCallbacks.onEscape)
 *   - Ctrl+C                — exit
 *   - Bracketed paste       — accumulate then display on close marker
 *
 * Everything else (arrow keys, word-jump, forward-delete, etc.) is silently
 * ignored. This is intentional: the prompt must not train navigation habits
 * that conflict with the operator's editor (e.g. Helix hjkl).
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
  if (cp < 32 || (cp >= 0x7f && cp < 0xa0)) return 0;
  if (cp >= 0x1100 && cp <= 0x115f) return 2;
  if (cp === 0x2329 || cp === 0x232a) return 2;
  if (cp >= 0x2e80 && cp <= 0x303e) return 2;
  if (cp >= 0x3041 && cp <= 0x33bf) return 2;
  if (cp >= 0x33ff && cp <= 0xfe4f) return 2;
  if (cp >= 0xfe51 && cp <= 0xfe6f) return 2;
  if (cp >= 0xff01 && cp <= 0xff60) return 2;
  if (cp >= 0xffe0 && cp <= 0xffe6) return 2;
  if (cp >= 0x1f004 && cp <= 0x1f9ff) return 2;
  if (cp >= 0x20000 && cp <= 0x2fffd) return 2;
  if (cp >= 0x30000 && cp <= 0x3fffd) return 2;
  return 1;
}

// ---------------------------------------------------------------------------
// Key parsing
// ---------------------------------------------------------------------------

/**
 * Callbacks for key input parsing.
 *
 * onEscape is context-sensitive:
 *   - buffer non-empty          → clear the buffer (caller redraws prompt)
 *   - buffer empty, streaming   → abort the in-flight turn
 *   - buffer empty, idle        → no-op
 * The caller decides which case applies; parseKeys signals by calling onEscape
 * only when the buffer is already empty (it clears the buffer itself otherwise).
 */
interface KeyCallbacks {
  onSubmit: (line: string) => void;
  onEscape: () => void;
  onExit: () => void;
  /** Called after parseKeys clears a non-empty buffer via Esc. Redraw the prompt. */
  onBufferCleared?: () => void;
}

/** Persistent bracketed-paste state shared across calls in production. */
const sharedPasteState = { inPaste: false };

/** Shared mutable buffer used by setupRawInput in production. */
const sharedBuffer: {
  value: string;
} = { value: "" };

/**
 * Parse a raw terminal chunk and dispatch to callbacks.
 *
 * Exported for unit testing without requiring a real TTY.
 *
 * @param chunk     Raw bytes from stdin (UTF-8 string).
 * @param callbacks Handlers for submit, escape, exit, and buffer-cleared.
 * @param buf       Current input buffer (defaults to shared module buffer).
 * @returns         Updated buffer value after processing.
 */
export function parseKeys(
  chunk: string,
  callbacks: KeyCallbacks,
  buf: { value: string } = sharedBuffer,
  options: { inputEnabled?: boolean; pasteState?: { inPaste: boolean } } = {},
): string {
  const { onSubmit, onEscape, onExit, onBufferCleared } = callbacks;
  const inputEnabled = options.inputEnabled ?? true;
  const pasteState = options.pasteState ?? sharedPasteState;

  for (let i = 0; i < chunk.length; i++) {
    const ch = chunk[i];
    const code = chunk.charCodeAt(i);

    // Ctrl+C — exit immediately
    if (ch === "\x03") { onExit(); return buf.value; }

    if (ch === "\x1b") {
      // Bracketed paste start: \x1b[200~
      if (chunk.startsWith("[200~", i + 1)) {
        pasteState.inPaste = true;
        i += 5;
        continue;
      }
      // Bracketed paste end: \x1b[201~
      if (chunk.startsWith("[201~", i + 1)) {
        pasteState.inPaste = false;
        i += 5;
        // Display the newly-pasted content
        if (inputEnabled) {
          process.stdout.write(buf.value);
        }
        continue;
      }

      // Skip all other CSI sequences (\x1b[ ... letter) silently
      if (i + 1 < chunk.length && chunk[i + 1] === "[") {
        i += 2;
        while (i < chunk.length && !/[A-Za-z~]/.test(chunk[i])) i++;
        continue;
      }

      // Plain Esc (not inside a paste)
      if (!pasteState.inPaste) {
        if (buf.value.length > 0) {
          // Clear the buffer and erase the line visually
          const w = [...buf.value].reduce((s, c) => s + (displayWidth(c) || 1), 0);
          buf.value = "";
          process.stdout.write(`\x1b[${w}D\x1b[K`);
          onBufferCleared?.();
        } else {
          onEscape();
        }
      }
      continue;
    }

    if (!inputEnabled) continue;

    // Enter — submit (or insert newline inside a paste)
    if (ch === "\r" || ch === "\n") {
      if (pasteState.inPaste) {
        buf.value += "\n";
        continue;
      }
      const line = buf.value;
      buf.value = "";
      onSubmit(line);
      continue;
    }

    // Backspace — delete last character
    if (code === 127) {
      const chars = [...buf.value];
      if (chars.length > 0) {
        const deleted = chars[chars.length - 1];
        const w = displayWidth(deleted) || 1;
        chars.pop();
        buf.value = chars.join("");
        process.stdout.write("\b".repeat(w) + " ".repeat(w) + "\b".repeat(w));
      }
      continue;
    }

    // Printable character — append at end
    if (code >= 32 || code > 127) {
      buf.value += ch;
      if (!pasteState.inPaste) {
        process.stdout.write(ch);
      }
    }
  }

  return buf.value;
}
