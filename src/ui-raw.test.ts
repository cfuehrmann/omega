/**
 * Unit tests for key-input parsing in ui-raw.ts.
 *
 * Tests the exported `parseKeys` function — pure, no stdin required.
 */

import { describe, it, expect } from "bun:test";
import { parseKeys } from "./ui-raw.js";

describe("parseKeys", () => {
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
});
