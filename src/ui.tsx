import React, { useState, useCallback, useRef } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "ink-text-input";
import { Agent, type AgentEvent, type TurnMetrics } from "./agent.js";
import { config } from "./config.js";

// --- Types ---

interface CompletedItem {
  id: number;
  type: "turn" | "tool_call" | "tool_result" | "tool_rejected" | "error";
  text: string;
  dimText?: string;
}

// --- Formatting ---

function formatCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(3)}`;
}

function formatMs(ms: number | null): string {
  if (ms === null) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function truncateOutput(text: string, maxLines: number = 20): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return (
    lines.slice(0, maxLines).join("\n") +
    `\n... [${lines.length - maxLines} more lines]`
  );
}

// --- App ---

export function App() {
  const { exit } = useApp();
  const [agent] = useState(() => new Agent());
  const [input, setInput] = useState("");
  const [completedItems, setCompletedItems] = useState<CompletedItem[]>([]);
  const [streamingText, setStreamingText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [itemId, setItemId] = useState(0);

  // Tool confirmation state
  const [pendingTool, setPendingTool] = useState<{
    name: string;
    formatted: string;
  } | null>(null);
  const confirmResolveRef = useRef<((approved: boolean) => void) | null>(null);

  const nextId = useCallback(() => {
    const id = itemId;
    setItemId((i) => i + 1);
    return id;
  }, [itemId]);

  const addItem = useCallback(
    (
      type: CompletedItem["type"],
      text: string,
      dimText?: string
    ) => {
      setCompletedItems((prev) => [
        ...prev,
        { id: prev.length, type, text, dimText },
      ]);
    },
    []
  );

  const handleSubmit = useCallback(
    async (value: string) => {
      // Handle tool confirmation
      if (pendingTool && confirmResolveRef.current) {
        const v = value.trim().toLowerCase();
        if (v === "y" || v === "yes" || v === "") {
          confirmResolveRef.current(true);
        } else {
          confirmResolveRef.current(false);
        }
        confirmResolveRef.current = null;
        setPendingTool(null);
        setInput("");
        return;
      }

      const trimmed = value.trim();
      if (!trimmed || isStreaming) return;

      setInput("");
      setStreamingText("");
      setIsStreaming(true);

      addItem("turn", `❯ ${trimmed}`);

      let fullText = "";

      const confirmTool = (
        name: string,
        input: any,
        formatted: string
      ): Promise<boolean> => {
        return new Promise((resolve) => {
          confirmResolveRef.current = resolve;
          setPendingTool({ name, formatted });
        });
      };

      try {
        for await (const event of agent.sendMessage(trimmed, confirmTool)) {
          switch (event.type) {
            case "text":
              fullText += event.text;
              setStreamingText(fullText);
              break;

            case "tool_pending":
              // Pause streaming text display while waiting for confirmation
              if (fullText) {
                addItem("turn", fullText);
                fullText = "";
                setStreamingText("");
              }
              break;

            case "tool_call":
              addItem("tool_call", `🔧 ${event.formatted}`);
              break;

            case "tool_result":
              addItem(
                "tool_result",
                truncateOutput(event.result.output),
                `  ${event.name} ${event.result.isError ? "✗" : "✓"} ${Math.round(event.result.durationMs)}ms`
              );
              break;

            case "tool_rejected":
              addItem("tool_rejected", `⊘ ${event.name} rejected`);
              break;

            case "metrics":
              if (fullText) {
                const m = event.metrics;
                addItem(
                  "turn",
                  fullText,
                  `  in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)} total: ${formatMs(m.totalMs)}`
                );
                fullText = "";
                setStreamingText("");
              }
              break;

            case "error":
              addItem("error", `⚠ ${event.error}`);
              break;
          }
        }
      } catch (err: any) {
        addItem("error", `⚠ ${err.message}`);
      } finally {
        setStreamingText("");
        setIsStreaming(false);
      }
    },
    [agent, isStreaming, pendingTool, addItem]
  );

  useInput((_, key) => {
    if (key.escape) {
      if (pendingTool && confirmResolveRef.current) {
        confirmResolveRef.current(false);
        confirmResolveRef.current = null;
        setPendingTool(null);
      } else {
        exit();
      }
    }
  });

  return (
    <>
      {/* Static zone: completed items */}
      <Static items={completedItems}>
        {(item) => (
          <Box key={item.id} flexDirection="column">
            <Text
              color={
                item.type === "error"
                  ? "red"
                  : item.type === "tool_call"
                    ? "yellow"
                    : item.type === "tool_result"
                      ? "gray"
                      : item.type === "tool_rejected"
                        ? "red"
                        : undefined
              }
            >
              {item.text}
            </Text>
            {item.dimText && <Text dimColor>{item.dimText}</Text>}
          </Box>
        )}
      </Static>

      {/* Live zone */}
      <Box flexDirection="column">
        {/* Streaming response */}
        {isStreaming && streamingText && !pendingTool && (
          <Box marginBottom={0}>
            <Text>
              {streamingText}
              <Text dimColor>▊</Text>
            </Text>
          </Box>
        )}

        {/* Tool confirmation prompt */}
        {pendingTool && (
          <Box flexDirection="column" marginBottom={0}>
            <Text color="yellow">
              {"🔧 "}
              {pendingTool.formatted}
            </Text>
            <Text dimColor>
              {"  Allow? [Y/n] "}
            </Text>
          </Box>
        )}

        {/* Input */}
        <Box>
          <Text bold color={pendingTool ? "yellow" : isStreaming ? "gray" : "green"}>
            {pendingTool ? "? " : "❯ "}
          </Text>
          <TextInput
            value={input}
            onChange={setInput}
            onSubmit={handleSubmit}
            placeholder={
              pendingTool
                ? "y/n"
                : isStreaming
                  ? "waiting..."
                  : "message"
            }
          />
        </Box>
      </Box>

      {/* Status bar */}
      <Box marginTop={1}>
        <Text dimColor>
          {config.model} │ in: {agent.sessionInputTokens} out:{" "}
          {agent.sessionOutputTokens} │ {formatCost(agent.sessionCostUsd)}
        </Text>
      </Box>
    </>
  );
}
