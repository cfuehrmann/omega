import { describe, test, expect } from "bun:test";
import React, { useState } from "react";
import { render } from "ink-testing-library";
import { Box, Text } from "ink";
import TextInput from "ink-text-input";
import FastTextInput from "./fast-text-input.js";

/**
 * Reproduce: when characters arrive faster than React re-renders,
 * ink-text-input drops the beginning of the input because it computes
 * the next value from the stale `value` prop.
 *
 * FastTextInput fixes this by using a ref for synchronous value tracking.
 */

function TestApp({
  onValue,
  useFast = false,
}: {
  onValue: (v: string) => void;
  useFast?: boolean;
}) {
  const [input, setInput] = useState("");
  const InputComponent = useFast ? FastTextInput : TextInput;
  return (
    <Box>
      <Text>{"❯ "}</Text>
      <InputComponent
        value={input}
        onChange={(v: string) => {
          setInput(v);
          onValue(v);
        }}
        onSubmit={() => {}}
      />
    </Box>
  );
}

describe("ink-text-input bug: drops beginning of rapid input", () => {
  test("single paste is fine", async () => {
    let lastValue = "";
    const { stdin } = render(
      <TestApp onValue={(v) => { lastValue = v; }} />
    );
    stdin.write("hello world");
    await new Promise((r) => setTimeout(r, 100));
    expect(lastValue).toBe("hello world");
  });

  test("rapid bursts drop earlier input", async () => {
    let lastValue = "";
    const { stdin } = render(
      <TestApp onValue={(v) => { lastValue = v; }} />
    );
    stdin.write("the quick ");
    stdin.write("brown fox ");
    stdin.write("jumps over");
    await new Promise((r) => setTimeout(r, 100));
    // This demonstrates the bug — only the last burst survives
    expect(lastValue).not.toBe("the quick brown fox jumps over");
  });
});

describe("FastTextInput: handles rapid input correctly", () => {
  test("single paste works", async () => {
    let lastValue = "";
    const { stdin } = render(
      <TestApp useFast onValue={(v) => { lastValue = v; }} />
    );
    stdin.write("hello world");
    await new Promise((r) => setTimeout(r, 100));
    expect(lastValue).toBe("hello world");
  });

  test("rapid bursts preserve all characters", async () => {
    let lastValue = "";
    const { stdin } = render(
      <TestApp useFast onValue={(v) => { lastValue = v; }} />
    );
    stdin.write("the quick ");
    stdin.write("brown fox ");
    stdin.write("jumps over");
    await new Promise((r) => setTimeout(r, 100));
    expect(lastValue).toBe("the quick brown fox jumps over");
  });

  test("backspace works correctly", async () => {
    let lastValue = "";
    const { stdin } = render(
      <TestApp useFast onValue={(v) => { lastValue = v; }} />
    );
    stdin.write("helloo");
    await new Promise((r) => setTimeout(r, 50));
    stdin.write("\x7f"); // backspace
    await new Promise((r) => setTimeout(r, 100));
    expect(lastValue).toBe("hello");
  });

  test("external value reset (e.g. after submit) clears the input correctly", async () => {
    // After submit, the parent resets value to "". FastTextInput must sync
    // its ref to "" so subsequent typing starts fresh.
    let lastValue = "sentinel";

    function ResettingApp({ onValue }: { onValue: (v: string) => void }) {
      const [input, setInput] = React.useState("");
      const [submitted, setSubmitted] = React.useState(false);
      return (
        <Box>
          <FastTextInput
            value={submitted ? "" : input}
            onChange={(v: string) => {
              setInput(v);
              onValue(v);
            }}
            onSubmit={() => {
              setSubmitted(true);
              // After a brief moment, simulate parent clearing value
            }}
          />
        </Box>
      );
    }

    const { stdin } = render(<ResettingApp onValue={(v) => { lastValue = v; }} />);

    stdin.write("hello");
    await new Promise((r) => setTimeout(r, 50));
    expect(lastValue).toBe("hello");

    // Submit clears the field
    stdin.write("\r");
    await new Promise((r) => setTimeout(r, 50));

    // Now type again — should start fresh, not append to old value
    stdin.write("world");
    await new Promise((r) => setTimeout(r, 100));
    expect(lastValue).toBe("world");
  });
});
