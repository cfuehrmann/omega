/**
 * Unit tests for key-input parsing in ui-raw.ts.
 *
 * Tests the exported `parseKeys` function — pure, no stdin required.
 */

import { describe, it, expect } from "bun:test";
import { parseKeys, displayWidth } from "./ui-raw.js";

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
    expect(submitted).toBe(""); // no submit after exit
  });

  it("calls onSubmit with accumulated buffer on Enter", () => {
    let submitted = "";
    parseKeys("hello\r", { onSubmit: (line) => { submitted = line; }, onEscape: () => {}, onExit: () => {} });
    expect(submitted).toBe("hello");
  });

  it("calls onEscape on bare Escape", () => {
    let escapes = 0;
    parseKeys("\x1b", { onSubmit: () => {}, onEscape: () => { escapes++; }, onExit: () => {} });
    expect(escapes).toBe(1);
  });

  it("ignores printable/backspace/enter when input is disabled", () => {
    let submitted = "";
    const buf = { value: "keep" };
    parseKeys("a\b\r", {
      onSubmit: (line) => { submitted = line; },
      onEscape: () => {},
      onExit: () => {},
    }, buf, { inputEnabled: false });
    expect(submitted).toBe("");
    expect(buf.value).toBe("keep");
  });

  it("still handles Escape when input is disabled", () => {
    let escapes = 0;
    const buf = { value: "keep" };
    parseKeys("\x1b", {
      onSubmit: () => {},
      onEscape: () => { escapes++; },
      onExit: () => {},
    }, buf, { inputEnabled: false });
    expect(escapes).toBe(1);
    expect(buf.value).toBe("keep");
  });

  it("skips arrow-key CSI sequences without calling any callback", () => {
    let calls = 0;
    const cb = {
      onSubmit: () => { calls++; },
      onEscape: () => { calls++; },
      onExit:   () => { calls++; },
    };
    parseKeys("\x1b[A", cb);  // Up arrow
    parseKeys("\x1b[B", cb);  // Down arrow
    parseKeys("\x1b[C", cb);  // Right arrow
    parseKeys("\x1b[D", cb);  // Left arrow
    expect(calls).toBe(0);
  });

  it("echoes printable characters and accumulates buffer", () => {
    let submitted = "";
    // Type "hi" then Enter
    parseKeys("h", { onSubmit: (l) => { submitted = l; }, onEscape: () => {}, onExit: () => {} });
    parseKeys("i", { onSubmit: (l) => { submitted = l; }, onEscape: () => {}, onExit: () => {} });
    parseKeys("\r", { onSubmit: (l) => { submitted = l; }, onEscape: () => {}, onExit: () => {} });
    expect(submitted).toBe("hi");
  });

  // Bracketed paste: newlines in pasted text must NOT trigger onSubmit mid-paste
  it("does not submit on newline inside bracketed paste", () => {
    let submits = 0;
    const buf = { value: "" };
    const cb = { onSubmit: () => { submits++; }, onEscape: () => {}, onExit: () => {} };
    // Simulate a paste: start marker, multi-line text, end marker
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

  it("treats newline as submit when NOT in paste mode (normal typing)", () => {
    let submits = 0;
    const buf = { value: "typed" };
    const cb = { onSubmit: () => { submits++; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("\r", cb, buf);
    expect(submits).toBe(1);
  });

  it("backspace does not write to stdout when buffer is empty (prevents erasing prompt)", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any, ...args: any[]) => {
      written.push(typeof s === "string" ? s : String(s));
      return true;
    };
    try {
      const buf = { value: "" };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      // Press backspace when buffer is empty — must not write anything
      parseKeys("\x7f", cb, buf);
      expect(written.join("")).toBe("");
      expect(buf.value).toBe("");
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("backspace removes last character from buffer and erases it visually", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any, ...args: any[]) => {
      written.push(typeof s === "string" ? s : String(s));
      return true;
    };
    try {
      const buf = { value: "ab" };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x7f", cb, buf);
      expect(buf.value).toBe("a");
      expect(written.join("")).toBe("\b \b");
      // Second backspace removes 'a'
      written.length = 0;
      parseKeys("\x7f", cb, buf);
      expect(buf.value).toBe("");
      expect(written.join("")).toBe("\b \b");
      // Third backspace with empty buffer — must not write
      written.length = 0;
      parseKeys("\x7f", cb, buf);
      expect(buf.value).toBe("");
      expect(written.join("")).toBe("");
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("backspace erases double-width character using two \\b sequences", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any, ...args: any[]) => {
      written.push(typeof s === "string" ? s : String(s));
      return true;
    };
    try {
      // '你' is a 2-column CJK character
      const buf = { value: "你", columns: 2 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x7f", cb, buf);
      expect(buf.value).toBe("");
      expect(buf.columns).toBe(0);
      // Must emit two backspace-erase-backspace sequences (4 chars each direction * 2 columns)
      expect(written.join("")).toBe("\b\b  \b\b");
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("echoes pasted content to stdout when paste ends", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any, ...args: any[]) => {
      written.push(s);
      return true;
    };
    try {
      const buf = { value: "" };
      const pasteState = { inPaste: false };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b[200~hello\nworld\x1b[201~", cb, buf, { pasteState });
      expect(written.join("")).toContain("hello\nworld");
    } finally {
      process.stdout.write = origWrite;
    }
  });

  // -----------------------------------------------------------------------
  // Cursor movement & line editing
  // -----------------------------------------------------------------------

  it("left arrow moves cursor back one character", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "abc", cursor: 3 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b[D", cb, buf); // Left arrow
      expect(buf.cursor).toBe(2);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("right arrow moves cursor forward one character", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "abc", cursor: 1 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b[C", cb, buf); // Right arrow
      expect(buf.cursor).toBe(2);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("left arrow does not go below 0", () => {
    const buf = { value: "abc", cursor: 0 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[D", cb, buf);
    expect(buf.cursor).toBe(0);
  });

  it("right arrow does not go past end", () => {
    const buf = { value: "abc", cursor: 3 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[C", cb, buf);
    expect(buf.cursor).toBe(3);
  });

  it("typing inserts at cursor position", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "ac", cursor: 1 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("b", cb, buf);
      expect(buf.value).toBe("abc");
      expect(buf.cursor).toBe(2);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("backspace at cursor mid-line deletes char before cursor", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "abc", cursor: 2 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x7f", cb, buf);
      expect(buf.value).toBe("ac");
      expect(buf.cursor).toBe(1);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Backspace deletes word backward", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "hello world", cursor: 11 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b\x7f", cb, buf); // Ctrl+Backspace (sent as ESC DEL by many terminals)
      expect(buf.value).toBe("hello ");
      expect(buf.cursor).toBe(6);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Backspace deletes word backward from middle of line", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      // cursor after "two" space: "one two| three"  (| = cursor at 7)
      // wordBoundaryBack skips non-spaces (two) → lands at 4, deletes chars 4..6
      const buf = { value: "one two three", cursor: 7 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b\x7f", cb, buf); // Ctrl+Backspace
      expect(buf.value).toBe("one  three");
      expect(buf.cursor).toBe(4);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Backspace via \\x08 deletes word backward", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "hello world", cursor: 11 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x08", cb, buf); // Some terminals send 0x08 for Ctrl+Backspace
      expect(buf.value).toBe("hello ");
      expect(buf.cursor).toBe(6);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Delete deletes word forward", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      const buf = { value: "hello world", cursor: 5 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b[3;5~", cb, buf); // Ctrl+Delete CSI sequence
      expect(buf.value).toBe("hello");
      expect(buf.cursor).toBe(5);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Delete deletes word forward from middle", () => {
    const written: string[] = [];
    const origWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (s: any) => { written.push(String(s)); return true; };
    try {
      // cursor after "one": "one| two three"  (| = cursor at 3)
      // wordBoundaryForward skips space then "two" → lands at 7, deletes chars 3..6
      const buf = { value: "one two three", cursor: 3 };
      const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
      parseKeys("\x1b[3;5~", cb, buf); // Ctrl+Delete
      expect(buf.value).toBe("one three");
      expect(buf.cursor).toBe(3);
    } finally {
      process.stdout.write = origWrite;
    }
  });

  it("Ctrl+Left moves cursor one word backward", () => {
    const buf = { value: "hello world", cursor: 11 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[1;5D", cb, buf); // Ctrl+Left CSI sequence
    expect(buf.cursor).toBe(6);
  });

  it("Ctrl+Right moves cursor one word forward", () => {
    const buf = { value: "hello world", cursor: 0 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[1;5C", cb, buf); // Ctrl+Right CSI sequence
    expect(buf.cursor).toBe(5);
  });

  it("Ctrl+Left skips trailing spaces then word", () => {
    const buf = { value: "one two  ", cursor: 9 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[1;5D", cb, buf);
    expect(buf.cursor).toBe(4);
  });

  it("Ctrl+Right skips leading spaces then word", () => {
    const buf = { value: "hello  world", cursor: 5 };
    const cb = { onSubmit: () => {}, onEscape: () => {}, onExit: () => {} };
    parseKeys("\x1b[1;5C", cb, buf);
    expect(buf.cursor).toBe(12);
  });

  it("submit resets cursor to 0", () => {
    let submitted = "";
    const buf = { value: "hello", cursor: 3 };
    const cb = { onSubmit: (l: string) => { submitted = l; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("\r", cb, buf);
    expect(submitted).toBe("hello");
    expect(buf.value).toBe("");
    expect(buf.cursor).toBe(0);
  });

  it("backward compatibility: buf without cursor field works (cursor defaults to end)", () => {
    let submitted = "";
    const buf = { value: "" };
    const cb = { onSubmit: (l: string) => { submitted = l; }, onEscape: () => {}, onExit: () => {} };
    parseKeys("hi\r", cb, buf);
    expect(submitted).toBe("hi");
  });
});
