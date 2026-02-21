import React, { useState, useCallback, useRef, useEffect } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "./fast-text-input.js";
import { Agent, type AgentEvent, type TurnMetrics, type ApiCallPayload, formatPayloadSummary } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";
import { shouldHandleShortcut } from "./ui-logic.js";

// --- Types ---

interface CompletedItem {
  id: number;
  type: "turn" | "tool_call" | "tool_result" | "tool_rejected" | "error" | "user" | "separator";
  text: string;
  dimText?: string;
}

// --- PayloadPanel ---

const PANEL_MAX_MSGS = 20;

function PayloadPanel({ payload, callNumber }: { payload: ApiCallPayload; callNumber: number }) {
  const summary = formatPayloadSummary(payload);
  const msgs = summary.messageSummaries;
  const truncated = msgs.length > PANEL_MAX_MSGS;
  const visible = truncated ? msgs.slice(-PANEL_MAX_MSGS) : msgs;
  return (
    <Box flexDirection="column" borderStyle="single" borderColor="cyan" paddingX={1}>
      <Text bold color="cyan">Payload inspector — API call #{callNumber}</Text>
      <Text>  Model: <Text bold>{payload.model}</Text>  Est. tokens: <Text bold>{summary.estimatedTokens.toLocaleString()}</Text>  Tools: {summary.toolCount}</Text>
      <Text>  System: {summary.systemChars} chars  Messages: {summary.messageCount}</Text>
      <Text dimColor>  ─── messages ───</Text>
      {truncated && <Text dimColor>  … {msgs.length - PANEL_MAX_MSGS} earlier messages hidden</Text>}
      {visible.map((m, i) => (
        <Text key={i}>
          {"  "}
          <Text bold color={m.role === "user" ? "green" : "blue"}>{m.role}</Text>
          <Text dimColor>{` (~${m.tokenEstimate}t) `}</Text>
          <Text dimColor>{m.preview}</Text>
        </Text>
      ))}
      <Text dimColor>  i or q to close</Text>
    </Box>
  );
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

  // Tool confirmation state
  const [pendingTool, setPendingTool] = useState<{
    name: string;
    formatted: string;
  } | null>(null);
  const confirmResolveRef = useRef<((approved: boolean) => void) | null>(null);

  // Abort controller for the current stream — replaced on each new message
  const abortControllerRef = useRef<AbortController | null>(null);

  // Payload panel: last API call params + toggle (never auto-opens)
  const [lastPayload, setLastPayload] = useState<ApiCallPayload | null>(null);
  const [lastCallNumber, setLastCallNumber] = useState(0);
  const [showPayloadPanel, setShowPayloadPanel] = useState(false);

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

      const controller = new AbortController();
      abortControllerRef.current = controller;

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
        for await (const event of agent.sendMessage(trimmed, confirmTool, controller.signal)) {
          switch (event.type) {
            case "api_call_start": {
              // Save payload for the inspector panel (panel stays closed — user opens with i)
              const payload = {
                model: event.model,
                system: event.system,
                tools: event.tools,
                messages: event.messages,
              };
              setLastPayload(payload);
              setLastCallNumber(event.callNumber);
              // Stronger visual separator: bold turn marker
              const est = formatPayloadSummary(payload).estimatedTokens;
              addItem(
                "separator",
                `▶ API call #${event.callNumber}  ~${est.toLocaleString()} tokens`
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
              break;

            case "tool_rejected":
              addItem("tool_rejected", `⊘ ${event.name} rejected`);
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
    [agent, isStreaming, pendingTool, addItem, ready, lastResponse, priorSession, resumePromptDone]
  );

  useInput((input, key) => {
    if (key.escape) {
      if (pendingTool && confirmResolveRef.current) {
        // Escape rejects a pending tool confirmation
        confirmResolveRef.current(false);
        confirmResolveRef.current = null;
        setPendingTool(null);
      } else if (isStreaming && abortControllerRef.current) {
        // Escape interrupts the current stream
        abortControllerRef.current.abort();
        abortControllerRef.current = null;
      }
    }
    // i/q only fire as shortcuts when the prompt is empty (not mid-typing)
    const shortcutCtx = { inputValue: inputState, isStreaming, hasPendingTool: !!pendingTool, isReady: ready, resumeDone: resumePromptDone };
    if (shouldHandleShortcut("i", shortcutCtx)) {
      if (lastPayload) setShowPayloadPanel((v) => !v);
    }
    if (shouldHandleShortcut("q", shortcutCtx) && showPayloadPanel) {
      setShowPayloadPanel(false);
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

        {/* Tool confirmation */}
        {pendingTool && (
          <Box flexDirection="column" marginBottom={0}>
            <Text color="yellow">{"🔧 "}{pendingTool.formatted}</Text>
            <Text dimColor>{"  Allow? [Y/n] "}</Text>
          </Box>
        )}

        {/* Payload inspector — pinned just above the input box */}
        {showPayloadPanel && lastPayload && (
          <PayloadPanel payload={lastPayload} callNumber={lastCallNumber} />
        )}

        {/* Input prompt */}
        <Box>
          <Text
            bold={!isStreaming || !!pendingTool}
            dimColor={isStreaming && !pendingTool}
            color={
              priorSession && !resumePromptDone ? "cyan"
              : pendingTool ? "yellow"
              : isStreaming ? undefined
              : !ready ? undefined
              : "green"
            }
          >
            {priorSession && !resumePromptDone ? "? "
             : pendingTool ? "? "
             : isStreaming ? "… "
             : !ready ? "… "
             : "❯ "}
          </Text>
          <TextInput
            value={inputState}
            onChange={setInput}
            onSubmit={handleSubmit}
            focus={!isStreaming || !!pendingTool || !resumePromptDone}
            placeholder={
              priorSession && !resumePromptDone ? "y/n"
              : pendingTool ? "y/n"
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
          {isStreaming && !pendingTool
            ? " │ Esc to interrupt"
            : lastPayload && !isStreaming
              ? " │ i inspect │ Ctrl+C quit"
              : " │ Ctrl+C quit"}
        </Text>
      </Box>
    </>
  );
}
