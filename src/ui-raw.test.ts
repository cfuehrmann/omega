/**
 * Unit tests for key-input parsing in ui-raw.ts.
 *
 * Tests the exported `parseKeys` function — pure, no stdin required.
 *
 * The prompt editor is intentionally minimal: append-only, backspace only.
 * Arrow keys, word-jump, Ctrl+Backspace, forward-delete are all silently
 * ignored. This is by design — see the module docstring in input.ts.
 */

import { describe, it, expect } from "bun:test";
import { parseKeys, displayWidth } from "./ui-raw.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function captureOutput(fn: () => void): string {
  const chunks: string[] = [];
  const origWrite = process.stdout.write.bind(process.stdout);
  process.stdout.write = (s: any, ...args: any[]) => {
    chunks.push(typeof s === "string" ? s : String(s));
    return true;
  };
  try { fn(); } finally { process.stdout.write = origWrite; }
  return chunks.join("");
}

// ---------------------------------------------------------------------------
// displayWidth
// ---------------------------------------------------------------------------

describe("parseKeys", () => {
  it("displayWidth returns 1 for ASCII characters", () => {
    expect(displayWidth("a")).toBe(1);
    expect(displayWidth(">")).toBe(1);
    expect(displayWidth(" ")).toBe(1);
  });

  it("displayWidth returns 2 for CJK characters", () => {
    expect(displayWidth("你")).toBe(2);
    expect(displayWidth("語")).toBe(2);
    expect(displayWidth("あ")).toBe(2);
  });

  it("displayWidth returns 1 for common non-ASCII 1-column chars", () => {
    expect(displayWidth("é")).toBe(1);
    expect(displayWidth("ñ")).toBe(1);
  });

  // -------------------------------------------------------------------------
  // Exit
  // -------------------------------------------------------------------------

  it("calls onExit once when Ctrl+C appears once", () => {
    let exits = 0;
    parseKeys("\x03", { onSubmit: () => {}, onEscape: () => {}, onExit: () => { exits++; } });
    expect(exits).toBe(1);
  });

  it("calls onExit once even when Ctrl+C appears twice in same chunk", () => {
    let exits = 0;
    parseKeys("\x03\x03", { onSubmit: () => {}, onEscape: () => {}, onExit: () => { exits++; } });
    expect(exits).toBe(1);
  });

  it("stops processing chunk after Ctrl+C (no trailing input submitted)", () => {
    let submitted = "";
    let exits = 0;
    parseKeys("\x03hello\r", {
      onSubmit: (l) => { submitted = l; },
      onEscape: () => {},
      onExit: () => { exits++; },
    });
    expect(exits).toBe(1);
    expect(submitted).toBe("");
  });

  // -------------------------------------------------------------------------
  // Submit
  // -------------------------------------------------------------------------

  it("calls onSubmit with accumulated buffer on Enter", () => {
    let submitted = "";
    parseKeys("hello\r", { onSubmit: (line) => { submitted = line; }, onEscape: () => {}, onExit: () => {} });
    expect(submitted).toBe("hello");
  });

  it("submit clears the buffer", () => {
    const buf = { value: "hi" };
    parseKeys("\r", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf);
    expect(buf.value).toBe("");
  });

  // -------------------------------------------------------------------------
  // Esc — context-sensitive cancel
  // -------------------------------------------------------------------------

  it("Esc on empty buffer calls onEscape", () => {
    let escapes = 0;
    const buf = { value: "" };
    parseKeys("\x1b", { onSubmit: () => {}, onEscape: () => { escapes++; }, onExit: () => {} }, buf);
    expect(escapes).toBe(1);
    expect(buf.value).toBe("");
  });

  it("Esc on non-empty buffer clears the buffer, does NOT call onEscape", () => {
    let escapes = 0;
    let cleared = false;
    const buf = { value: "some text" };
    parseKeys("\x1b", {
      onSubmit: () => {},
      onEscape: () => { escapes++; },
      onExit: () => {},
      onBufferCleared: () => { cleared = true; },
    }, buf);
    expect(buf.value).toBe("");
    expect(escapes).toBe(0);
    expect(cleared).toBe(true);
  });

  it("Esc on non-empty buffer erases the text visually", () => {
    const buf = { value: "abc" }; // 3 chars = 3 columns
    const out = captureOutput(() =>
      parseKeys("\x1b", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf)
    );
    // Should move cursor back 3 cols then erase to end of line
    expect(out).toContain("\x1b[3D");
    expect(out).toContain("\x1b[K");
  });

  it("Esc on non-empty buffer erases correctly for double-width chars", () => {
    const buf = { value: "你好" }; // 2 CJK chars = 4 columns
    const out = captureOutput(() =>
      parseKeys("\x1b", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf)
    );
    expect(out).toContain("\x1b[4D");
    expect(out).toContain("\x1b[K");
  });

  it("Esc is handled even when inputEnabled is false (buffer non-empty → clear)", () => {
    let escapes = 0;
    let cleared = false;
    const buf = { value: "keep" };
    parseKeys("\x1b", {
      onSubmit: () => {},
      onEscape: () => { escapes++; },
      onExit: () => {},
      onBufferCleared: () => { cleared = true; },
    }, buf, { inputEnabled: false });
    // Buffer had content → clears, does not call onEscape
    expect(buf.value).toBe("");
    expect(escapes).toBe(0);
    expect(cleared).toBe(true);
  });

  it("Esc is handled even when inputEnabled is false (buffer empty → onEscape)", () => {
    let escapes = 0;
    const buf = { value: "" };
    parseKeys("\x1b", {
      onSubmit: () => {},
      onEscape: () => { escapes++; },
      onExit: () => {},
    }, buf, { inputEnabled: false });
    expect(escapes).toBe(1);
  });

  // -------------------------------------------------------------------------
  // inputEnabled guard
  // -------------------------------------------------------------------------

  it("ignores printable/backspace/enter when input is disabled", () => {
    let submitted = "";
    const buf = { value: "keep" };
    parseKeys("a\x7f\r", {
      onSubmit: (line) => { submitted = line; },
      onEscape: () => {},
      onExit: () => {},
    }, buf, { inputEnabled: false });
    expect(submitted).toBe("");
    expect(buf.value).toBe("keep");
  });

  // -------------------------------------------------------------------------
  // Arrow keys and other navigation — silently ignored
  // -------------------------------------------------------------------------

  it("arrow keys and other CSI sequences do not call any callback", () => {
    let calls = 0;
    const cb = {
      onSubmit: () => { calls++; },
      onEscape: () => { calls++; },
      onExit:   () => { calls++; },
    };
    const buf = { value: "" };
    parseKeys("\x1b[A", cb, buf);  // Up
    parseKeys("\x1b[B", cb, buf);  // Down
    parseKeys("\x1b[C", cb, buf);  // Right
    parseKeys("\x1b[D", cb, buf);  // Left
    parseKeys("\x1b[1;5D", cb, buf); // Ctrl+Left
    parseKeys("\x1b[1;5C", cb, buf); // Ctrl+Right
    parseKeys("\x1b[3~", cb, buf);   // Delete
    parseKeys("\x1b[3;5~", cb, buf); // Ctrl+Delete
    expect(calls).toBe(0);
    expect(buf.value).toBe(""); // buffer unchanged
  });

  it("arrow keys do not modify the buffer", () => {
    const buf = { value: "hello" };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[D", cb, buf); // Left
    parseKeys("\x1b[C", cb, buf); // Right
    expect(buf.value).toBe("hello");
  });

  // -------------------------------------------------------------------------
  // Printable characters — append-only
  // -------------------------------------------------------------------------

  it("echoes printable characters and accumulates buffer", () => {
    let submitted = "";
    const buf = { value: "" };
    const cb = { onSubmit: (l: string) => { submitted = l; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("h", cb, buf);
    parseKeys("i", cb, buf);
    parseKeys("\r", cb, buf);
    expect(submitted).toBe("hi");
  });

  it("always appends at end regardless of any prior arrow-key presses", () => {
    const buf = { value: "abc" };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    // Arrow keys are silently ignored; typing still appends at end
    parseKeys("\x1b[D", cb, buf); // Left (no-op)
    parseKeys("X", cb, buf);
    expect(buf.value).toBe("abcX");
  });

  it("single stdout.write per char (no O(n) regression on large buffer)", () => {
    const bigStr = "a".repeat(5000);
    const buf = { value: bigStr };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };

    let callCount = 0;
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { callCount++; return true; };
    try {
      parseKeys("x", cb, buf);
    } finally {
      process.stdout.write = origWrite;
    }
    expect(buf.value).toBe(bigStr + "x");
    expect(callCount).toBe(1);
  });

  // -------------------------------------------------------------------------
  // Backspace — delete last character
  // -------------------------------------------------------------------------

  it("backspace does not write to stdout when buffer is empty", () => {
    const out = captureOutput(() => {
      const buf = { value: "" };
      parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf);
    });
    expect(out).toBe("");
  });

  it("backspace removes last character from buffer", () => {
    const buf = { value: "ab" };
    parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf);
    expect(buf.value).toBe("a");
    parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf);
    expect(buf.value).toBe("");
    // Another backspace on empty buffer — no change, no crash
    parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf);
    expect(buf.value).toBe("");
  });

  it("backspace emits \\b space \\b to erase the character visually", () => {
    const buf = { value: "ab" };
    const out = captureOutput(() =>
      parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf)
    );
    expect(out).toBe("\b \b");
  });

  it("backspace erases a double-width CJK character with two \\b sequences", () => {
    const buf = { value: "你" }; // 2-column CJK
    const out = captureOutput(() =>
      parseKeys("\x7f", { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} }, buf)
    );
    expect(buf.value).toBe("");
    expect(out).toBe("\b\b  \b\b");
  });

  // Ctrl+Backspace (ESC DEL or 0x08) — silently ignored
  it("Ctrl+Backspace (ESC DEL) is silently ignored — buffer unchanged", () => {
    const buf = { value: "hello world" };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    // ESC DEL: the ESC sees non-empty buffer and clears it — that's by design.
    // But 0x08 (alternative Ctrl+Backspace encoding) should be silently ignored.
    parseKeys("\x08", cb, buf);
    expect(buf.value).toBe("hello world");
  });

  // -------------------------------------------------------------------------
  // Bracketed paste
  // -------------------------------------------------------------------------

  it("does not submit on newline inside bracketed paste", () => {
    let submits = 0;
    const buf = { value: "" };
    const cb = { onSubmit: () => { submits++; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[200~line one\nline two\x1b[201~", cb, buf);
    expect(submits).toBe(0);
    expect(buf.value).toBe("line one\nline two");
  });

  it("submits accumulated paste content on Enter after paste ends", () => {
    let submitted = "";
    const buf = { value: "" };
    const cb = { onSubmit: (l: string) => { submitted = l; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[200~line one\nline two\x1b[201~", cb, buf);
    parseKeys("\r", cb, buf);
    expect(submitted).toBe("line one\nline two");
  });

  it("echoes pasted content to stdout when paste ends", () => {
    const buf = { value: "" };
    const pasteState = { inPaste: false };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    const out = captureOutput(() =>
      parseKeys("\x1b[200~hello\nworld\x1b[201~", cb, buf, { pasteState })
    );
    expect(out).toContain("hello\nworld");
  });

  it("paste into non-empty buffer appends to end", () => {
    const buf = { value: "abc" };
    const pasteState = { inPaste: false };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[200~XYZ\x1b[201~", cb, buf, { pasteState });
    expect(buf.value).toBe("abcXYZ");
  });

  it("treats newline as submit when NOT in paste mode", () => {
    let submits = 0;
    const buf = { value: "typed" };
    const cb = { onSubmit: () => { submits++; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("\r", cb, buf);
    expect(submits).toBe(1);
  });

  // -------------------------------------------------------------------------
  // wtype / raw injection (no bracketed paste markers)
  // -------------------------------------------------------------------------

  it("wtype-style raw injection: all chars buffered correctly", () => {
    const buf = { value: "" };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    for (const ch of "hello world") {
      parseKeys(ch, cb, buf);
    }
    expect(buf.value).toBe("hello world");
  });
});
