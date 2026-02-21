import React, { useState, useCallback, useRef, useEffect } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "ink-text-input";
import { Agent, type AgentEvent, type TurnMetrics } from "./agent.js";
import { config } from "./config.js";

// --- Types ---

interface CompletedItem {
  id: number;
  type: "turn" | "tool_call" | "tool_result" | "tool_rejected" | "error" | "user";
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
  const [authMode, setAuthMode] = useState<string>("...");
  const [ready, setReady] = useState(false);
  const [input, setInput] = useState("");

  // All completed items go to Static (scrollback). Never put streaming text here.
  const [completedItems, setCompletedItems] = useState<CompletedItem[]>([]);

  // Live zone state — only one of these is active at a time
  const [streamingText, setStreamingText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [activity, setActivity] = useState("");

  // Tool confirmation state
  const [pendingTool, setPendingTool] = useState<{
    name: string;
    formatted: string;
  } | null>(null);
  const confirmResolveRef = useRef<((approved: boolean) => void) | null>(null);

  // Last completed response — shown in live zone until next message
  const [lastResponse, setLastResponse] = useState<{ text: string; dimText?: string } | null>(null);

  // Initialize agent (auth)
  useEffect(() => {
    agent.init().then((mode) => {
      setAuthMode(mode);
      setReady(true);
    }).catch((err) => {
      setAuthMode(`error: ${err.message}`);
      setReady(true);
    });
  }, [agent]);

  const addItem = useCallback(
    (type: CompletedItem["type"], text: string, dimText?: string) => {
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
      if (!trimmed || isStreaming || !ready) return;

      // Move last response to static zone before starting new turn
      if (lastResponse) {
        addItem("turn", lastResponse.text, lastResponse.dimText);
        setLastResponse(null);
      }

      setInput("");
      setStreamingText("");
      setIsStreaming(true);

      addItem("user", `❯ ${trimmed}`);

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
        setActivity("thinking...");
        for await (const event of agent.sendMessage(trimmed, confirmTool)) {
          switch (event.type) {
            case "text":
              fullText += event.text;
              setStreamingText(fullText);
              setActivity("");
              break;

            case "tool_pending":
              // Flush text before confirmation prompt
              if (fullText) {
                addItem("turn", fullText);
                fullText = "";
                setStreamingText("");
              }
              setActivity("");
              break;

            case "tool_call":
              // Flush text before tool call
              if (fullText) {
                addItem("turn", fullText);
                fullText = "";
                setStreamingText("");
              }
              addItem("tool_call", `🔧 ${event.formatted}`);
              setActivity(`running ${event.name}...`);
              break;

            case "tool_result":
              addItem(
                "tool_result",
                truncateOutput(event.result.output),
                `  ${event.name} ${event.result.isError ? "✗" : "✓"} ${Math.round(event.result.durationMs)}ms`
              );
              setActivity("thinking...");
              break;

            case "tool_rejected":
              addItem("tool_rejected", `⊘ ${event.name} rejected`);
              setActivity("thinking...");
              break;

            case "metrics": {
              const m = event.metrics;
              const metricsLine = `  in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)} total: ${formatMs(m.totalMs)}`;
              // Don't add to static — keep in live zone via lastResponse
              if (fullText) {
                setLastResponse({ text: fullText, dimText: metricsLine });
                fullText = "";
                setStreamingText("");
              } else {
                // Metrics-only turn (e.g. after tool loop with no final text)
                addItem("turn", "", metricsLine);
              }
              break;
            }

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
        setActivity("");
      }
    },
    [agent, isStreaming, pendingTool, addItem, ready, lastResponse]
  );

  useInput((input, key) => {
    if (key.escape) {
      if (pendingTool && confirmResolveRef.current) {
        confirmResolveRef.current(false);
        confirmResolveRef.current = null;
        setPendingTool(null);
      }
    }
    if (input === "c" && key.ctrl) {
      exit();
    }
  });

  return (
    <>
      {/* Static zone: scrollback history */}
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
                        : item.type === "user"
                          ? "green"
                          : undefined
              }
            >
              {item.text}
            </Text>
            {item.dimText && <Text dimColor>{item.dimText}</Text>}
          </Box>
        )}
      </Static>

      {/* Live zone — only one thing shows at a time */}
      <Box flexDirection="column">
        {/* Streaming response text */}
        {isStreaming && streamingText && !pendingTool && (
          <Box marginBottom={0}>
            <Text>
              {streamingText}
              <Text dimColor>▊</Text>
            </Text>
          </Box>
        )}

        {/* Activity indicator (no streaming text yet) */}
        {isStreaming && !streamingText && !pendingTool && (
          <Box>
            <Text dimColor>⏳ {activity || "working..."}</Text>
          </Box>
        )}

        {/* Last completed response (stays here until next message) */}
        {!isStreaming && lastResponse && (
          <Box flexDirection="column">
            <Text>{lastResponse.text}</Text>
            {lastResponse.dimText && <Text dimColor>{lastResponse.dimText}</Text>}
          </Box>
        )}

        {/* Tool confirmation */}
        {pendingTool && (
          <Box flexDirection="column" marginBottom={0}>
            <Text color="yellow">{"🔧 "}{pendingTool.formatted}</Text>
            <Text dimColor>{"  Allow? [Y/n] "}</Text>
          </Box>
        )}

        {/* Input prompt — hidden while streaming (unless confirming tool) */}
        {isStreaming && !pendingTool ? null : (
          <Box>
            <Text bold color={pendingTool ? "yellow" : !ready ? "red" : "green"}>
              {pendingTool ? "? " : !ready ? "… " : "❯ "}
            </Text>
            <TextInput
              value={input}
              onChange={setInput}
              onSubmit={handleSubmit}
              placeholder={pendingTool ? "y/n" : "message"}
            />
          </Box>
        )}
      </Box>

      {/* Status bar */}
      <Box marginTop={1}>
        <Text dimColor>
          {config.model} │ {authMode} │ in: {agent.sessionInputTokens} out:{" "}
          {agent.sessionOutputTokens} │ {formatCost(agent.sessionCostUsd)}
          {" │ Ctrl+C quit"}
        </Text>
      </Box>
    </>
  );
}
