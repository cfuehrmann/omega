import React, { useState, useRef, useEffect } from "react";
import { Text, useInput } from "ink";
import chalk from "chalk";

interface Props {
  value: string;
  onChange: (value: string) => void;
  onSubmit?: (value: string) => void;
  placeholder?: string;
  focus?: boolean;
}

/**
 * Drop-in replacement for ink-text-input that handles rapid input
 * (paste, dictation) correctly.
 *
 * The bug in ink-text-input: it reads the `value` prop inside useInput,
 * but React hasn't re-rendered with the new value yet when the next
 * keystroke arrives. So each burst computes against the stale value.
 *
 * Fix: use a ref to track the latest value, updated synchronously
 * in the useInput handler before React re-renders.
 */
export default function FastTextInput({
  value,
  onChange,
  onSubmit,
  placeholder = "",
  focus = true,
}: Props) {
  const valueRef = useRef(value);
  const cursorRef = useRef(value.length);

  // Sync ref when value prop changes from outside (e.g. cleared after submit)
  useEffect(() => {
    valueRef.current = value;
    cursorRef.current = value.length;
  }, [value]);

  useInput(
    (input, key) => {
      if (
        key.upArrow ||
        key.downArrow ||
        (key.ctrl && input === "c") ||
        key.tab ||
        (key.shift && key.tab)
      ) {
        return;
      }

      if (key.return) {
        onSubmit?.(valueRef.current);
        return;
      }

      const current = valueRef.current;
      let cursor = cursorRef.current;
      let next = current;

      if (key.leftArrow) {
        cursor = Math.max(0, cursor - 1);
      } else if (key.rightArrow) {
        cursor = Math.min(current.length, cursor + 1);
      } else if (key.backspace || key.delete) {
        if (cursor > 0) {
          next = current.slice(0, cursor - 1) + current.slice(cursor);
          cursor--;
        }
      } else {
        next = current.slice(0, cursor) + input + current.slice(cursor);
        cursor += input.length;
      }

      // Update ref synchronously — next keystroke in same tick sees this
      valueRef.current = next;
      cursorRef.current = cursor;

      if (next !== current) {
        onChange(next);
      }
    },
    { isActive: focus }
  );

  // Render with cursor
  const display = valueRef.current;
  if (!display && placeholder) {
    return <Text>{chalk.inverse(placeholder[0])}{chalk.grey(placeholder.slice(1))}</Text>;
  }

  let rendered = "";
  for (let i = 0; i < display.length; i++) {
    rendered += i === cursorRef.current ? chalk.inverse(display[i]) : display[i];
  }
  if (cursorRef.current >= display.length) {
    rendered += chalk.inverse(" ");
  }

  return <Text>{rendered}</Text>;
}
