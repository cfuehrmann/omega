import React, { useState, useCallback } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "ink-text-input";
import { Agent, type TurnMetrics } from "./agent.js";

interface CompletedTurn {
  id: number;
  userMessage: string;
  assistantMessage: string;
  metrics: TurnMetrics;
}

function formatCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(3)}`;
}

function formatMs(ms: number | null): string {
  if (ms === null) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function App() {
  const { exit } = useApp();
  const [agent] = useState(() => new Agent());
  const [input, setInput] = useState("");
  const [completedTurns, setCompletedTurns] = useState<CompletedTurn[]>([]);
  const [streamingText, setStreamingText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [turnId, setTurnId] = useState(0);

  const handleSubmit = useCallback(
    async (value: string) => {
      const trimmed = value.trim();
      if (!trimmed || isStreaming) return;

      setInput("");
      setStreamingText("");
      setIsStreaming(true);

      const currentTurnId = turnId;
      setTurnId((t) => t + 1);

      let fullText = "";
      let turnMetrics: TurnMetrics | null = null;

      try {
        for await (const event of agent.sendMessage(trimmed)) {
          if (event.type === "text") {
            fullText += event.text;
            setStreamingText(fullText);
          } else if (event.type === "metrics") {
            turnMetrics = event.metrics;
          }
        }

        if (turnMetrics) {
          setCompletedTurns((prev) => [
            ...prev,
            {
              id: currentTurnId,
              userMessage: trimmed,
              assistantMessage: fullText,
              metrics: turnMetrics!,
            },
          ]);
        }
      } catch (err: any) {
        setCompletedTurns((prev) => [
          ...prev,
          {
            id: currentTurnId,
            userMessage: trimmed,
            assistantMessage: `Error: ${err.message}`,
            metrics: {
              inputTokens: 0,
              outputTokens: 0,
              costUsd: 0,
              ttftMs: null,
              totalMs: 0,
            },
          },
        ]);
      } finally {
        setStreamingText("");
        setIsStreaming(false);
      }
    },
    [agent, isStreaming, turnId]
  );

  useInput((_, key) => {
    if (key.escape) exit();
  });

  return (
    <>
      {/* Static zone: completed turns */}
      <Static items={completedTurns}>
        {(turn) => (
          <Box key={turn.id} flexDirection="column" marginBottom={1}>
            <Text>
              <Text bold color="blue">
                {"❯ "}
              </Text>
              <Text>{turn.userMessage}</Text>
            </Text>
            <Text>{turn.assistantMessage}</Text>
            <Text dimColor>
              {`  in: ${turn.metrics.inputTokens} out: ${turn.metrics.outputTokens} cost: ${formatCost(turn.metrics.costUsd)} ttft: ${formatMs(turn.metrics.ttftMs)} total: ${formatMs(turn.metrics.totalMs)}`}
            </Text>
          </Box>
        )}
      </Static>

      {/* Live zone */}
      <Box flexDirection="column">
        {/* Streaming response */}
        {isStreaming && streamingText && (
          <Box marginBottom={1}>
            <Text>
              {streamingText}
              <Text dimColor>▊</Text>
            </Text>
          </Box>
        )}

        {/* Input */}
        <Box>
          <Text bold color={isStreaming ? "gray" : "green"}>
            {"❯ "}
          </Text>
          <TextInput
            value={input}
            onChange={setInput}
            onSubmit={handleSubmit}
            placeholder={isStreaming ? "waiting..." : "message"}
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

// config import at top of file scope
import { config } from "./config.js";

