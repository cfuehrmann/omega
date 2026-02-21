import React, { useState, useCallback, useRef, useEffect } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "./fast-text-input.js";
import { Agent, type AgentEvent, type TurnMetrics, formatPayloadSummary } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";
import { formatTokenDelta } from "./ui-logic.js";

// --- Types ---

interface CompletedItem {
  id: number;
  type: "turn" | "tool_call" | "tool_result" | "tool_rejected" | "error" | "user" | "separator";
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
  const [inputState, setInput] = useState("");

  // Resume prompt state
  const [priorSession, setPriorSession] = useState<Session | null>(null);
  const [resumePromptDone, setResumePromptDone] = useState(false);

  // All completed items go to Static (scrollback). Never put streaming text here.
  const [completedItems, setCompletedItems] = useState<CompletedItem[]>([]);

  // Live zone state — only one of these is active at a time
  const [streamingText, setStreamingText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [activity, setActivity] = useState("");

  // Abort controller for the current stream — replaced on each new message
  const abortControllerRef = useRef<AbortController | null>(null);

  // Token delta tracking: previous call's estimated tokens for Δ display
  const [lastCallTokens, setLastCallTokens] = useState<number | null>(null);
  const [tokenDelta, setTokenDelta] = useState<string>("");

  // Last completed response — shown in live zone until next message
  const [lastResponse, setLastResponse] = useState<{ text: string; dimText?: string } | null>(null);

  // Initialize agent (auth + check for prior session)
  useEffect(() => {
    agent.init().then(async (mode) => {
      setAuthMode(mode);
      // Show auth mode as first completed item so it's always visible
      setCompletedItems((prev) => [
        ...prev,
        {
          id: prev.length,
          type: mode === "Claude Max" ? "turn" as const : "error" as const,
          text: mode === "Claude Max"
            ? `✓ Authenticated: ${mode}`
            : `⚠ Auth: ${mode}${mode.includes("api-key") ? " (pay-per-token)" : ""}`,
        },
      ]);
      // Check for a resumable session before marking ready
      const prior = await agent.checkPriorSession();
      if (prior && prior.history.length > 0) {
        setPriorSession(prior);
        // ready stays false until resume prompt is resolved
      } else {
        setResumePromptDone(true);
        setReady(true);
      }
    }).catch((err) => {
      setAuthMode(`error: ${err.message}`);
      setResumePromptDone(true);
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
      // Handle resume prompt
      if (priorSession && !resumePromptDone) {
        const v = value.trim().toLowerCase();
        const resume = v === "y" || v === "yes" || v === "";
        if (resume) {
          agent.resumeSession(priorSession);
          const msgCount = priorSession.history.length;
          const savedAt = new Date(priorSession.savedAt).toLocaleString();
          addItem("turn", `↩ Resumed session from ${savedAt} (${msgCount} messages)`);
        } else {
          addItem("turn", "↩ Starting fresh session");
        }
        setPriorSession(null);
        setResumePromptDone(true);
        setReady(true);
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
      setTokenDelta("");
      setLastCallTokens(null);  // reset so first call of new turn shows no delta

      addItem("user", `❯ ${trimmed}`);

      let fullText = "";
      // Audit log: one entry per tool call this turn, shown as a summary at turn_end
      const auditLog: Array<{ formatted: string; ok: boolean; ms: number }> = [];

      const controller = new AbortController();
      abortControllerRef.current = controller;

      // No confirmation needed — everything is auto-approved.
      // The confirmTool callback is required by the agent interface but never called.
      const confirmTool = async () => true;

      try {
        for await (const event of agent.sendMessage(trimmed, confirmTool, controller.signal)) {
          switch (event.type) {
            case "api_call_start": {
              const est = formatPayloadSummary({
                model: event.model,
                system: event.system,
                tools: event.tools,
                messages: event.messages,
              }).estimatedTokens;
              setTokenDelta(formatTokenDelta(est, lastCallTokens));
              setLastCallTokens(est);
              const timestamp = new Date().toLocaleTimeString("en-GB");
              addItem(
                "separator",
                `▶ API call #${event.callNumber}  ~${est.toLocaleString()} tok  ${timestamp}`
              );
              break;
            }

            case "status":
              setActivity(event.message);
              break;

            case "text":
              fullText += event.text;
              setStreamingText(fullText);
              setActivity("");
              break;

            case "tool_call":
              setActivity(`${event.name}…`);
              break;

            case "tool_result":
              auditLog.push({
                formatted: event.formatted,
                ok: !event.result.isError,
                ms: Math.round(event.result.durationMs),
              });
              setActivity("");
              break;

            case "metrics":
              // Show duration as a dim line after each API call separator
              addItem("tool_result", "", `  ${formatMs(event.metrics.totalMs)}`);
              break;

            case "turn_end": {
              const m = event.metrics;
              const auditParts = auditLog.map(
                (t) => `${t.formatted} ${t.ok ? "✓" : "✗"} ${t.ms}ms`
              );
              const toolSummary = auditParts.length > 0
                ? `  🔧 ${auditParts.join("  ·  ")}`
                : "";
              const metricsLine = `  in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)} total: ${formatMs(m.totalMs)}`;
              const dimText = [toolSummary, metricsLine].filter(Boolean).join("\n");
              if (fullText) {
                setLastResponse({ text: fullText, dimText });
                fullText = "";
                setStreamingText("");
              } else {
                addItem("turn", "", dimText);
              }
              break;
            }

            case "error":
              addItem("error", `⚠ ${event.error}`);
              break;

            case "interrupted":
              if (fullText) {
                addItem("turn", fullText);
                fullText = "";
                setStreamingText("");
              }
              addItem("error", "⊘ Interrupted");
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
    [agent, isStreaming, addItem, ready, lastResponse, priorSession, resumePromptDone]
  );

  useInput((input, key) => {
    if (key.escape && isStreaming && abortControllerRef.current) {
      abortControllerRef.current.abort();
      abortControllerRef.current = null;
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
              bold={item.type === "separator"}
              color={
                item.type === "separator"
                  ? "cyan"
                  : item.type === "error"
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
        {isStreaming && streamingText && (
          <Box marginBottom={0}>
            <Text>
              {streamingText}
              <Text dimColor>▊</Text>
            </Text>
          </Box>
        )}

        {/* Activity indicator (no streaming text yet) */}
        {isStreaming && !streamingText && (
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

        {/* Resume prompt */}
        {priorSession && !resumePromptDone && (
          <Box flexDirection="column" marginBottom={0}>
            <Text color="cyan">
              {"↩ Prior session found: "}
              {new Date(priorSession.savedAt).toLocaleString()}
              {` (${priorSession.history.length} messages)`}
            </Text>
            <Text dimColor>{"  Resume? [Y/n] "}</Text>
          </Box>
        )}

        {/* Input prompt */}
        <Box>
          <Text
            bold={!isStreaming}
            dimColor={isStreaming}
            color={
              priorSession && !resumePromptDone ? "cyan"
              : isStreaming ? undefined
              : !ready ? undefined
              : "green"
            }
          >
            {priorSession && !resumePromptDone ? "? "
             : isStreaming ? "… "
             : !ready ? "… "
             : "❯ "}
          </Text>
          <TextInput
            value={inputState}
            onChange={setInput}
            onSubmit={handleSubmit}
            focus={!isStreaming || !resumePromptDone}
            placeholder={
              priorSession && !resumePromptDone ? "y/n"
              : isStreaming ? ""
              : "message"
            }
          />
        </Box>
      </Box>

      {/* Status bar */}
      <Box marginTop={1}>
        <Text dimColor>
          {config.model} │ {authMode} │ in: {agent.sessionInputTokens} out:{" "}
          {agent.sessionOutputTokens} │ {formatCost(agent.sessionCostUsd)}
          {tokenDelta ? ` │ ${tokenDelta}` : ""}
          {isStreaming ? " │ Esc to interrupt" : " │ Ctrl+C quit"}
        </Text>
      </Box>
    </>
  );
}
