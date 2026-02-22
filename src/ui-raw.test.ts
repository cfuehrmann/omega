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
});
