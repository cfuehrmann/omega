import React, { useState, useCallback, useRef, useEffect } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "./fast-text-input.js";
import { Agent, formatPayloadSummary } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";
import { formatTokenDelta } from "./ui-logic.js";
import type Anthropic from "@anthropic-ai/sdk";

// --- Types ---

type ItemType =
  | "user"
  | "api_request"
  | "api_response"
  | "tool_call"
  | "tool_result"
  | "assistant"
  | "error"
  | "system";

interface CompletedItem {
  id: number;
  type: ItemType;
  time: string;      // HH:MM:SS
  lines: Line[];     // rendered lines (text + color + bold)
}

interface Line {
  text: string;
  color?: string;
  dimColor?: boolean;
  bold?: boolean;
}

// --- Constants ---

const TIME_WIDTH = 10; // "HH:MM:SS  "

// --- Formatting helpers ---

function now(): string {
  return new Date().toLocaleTimeString("en-GB");
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

function truncateOutput(text: string, maxLines: number = 10): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… [${lines.length - maxLines} more lines]`;
}

// Render a pseudo-JSON block for the API request
function apiRequestLines(
  callNumber: number,
  model: string,
  system: string,
  tools: Anthropic.Tool[],
  messages: Anthropic.MessageParam[],
  estimatedTokens: number,
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: `▶ API call #${callNumber}  ~${estimatedTokens.toLocaleString()} tokens`, bold: true, color: "cyan" });
  lines.push({ text: `  {`, color: "cyan" });
  lines.push({ text: `    model: "${model}",`, color: "cyan" });
  lines.push({ text: `    system: <${system.length} chars>,`, color: "cyan" });
  lines.push({ text: `    tools: [${tools.map(t => `"${t.name}"`).join(", ")}],`, color: "cyan" });
  lines.push({ text: `    max_tokens: ${config.maxOutputTokens},`, color: "cyan" });
  lines.push({ text: `    messages: [`, color: "cyan" });
  for (const msg of messages) {
    const contentSummary = summariseContent(msg.content);
    lines.push({ text: `      { role: "${msg.role}", content: ${contentSummary} },`, color: "cyan", dimColor: true });
  }
  lines.push({ text: `    ]`, color: "cyan" });
  lines.push({ text: `  }`, color: "cyan" });
  return lines;
}

function summariseContent(content: Anthropic.MessageParam["content"]): string {
  if (typeof content === "string") {
    if (content.length <= 60) return `"${content}"`;
    return `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  if (content.length === 1) {
    const b = content[0] as any;
    if (b.type === "text") {
      const t = b.text as string;
      return t.length <= 60 ? `"${t}"` : `<text: ${t.length} chars>`;
    }
    if (b.type === "tool_result") return `<tool_result>`;
    if (b.type === "tool_use") return `<tool_use: ${b.name}>`;
    return `<${b.type}>`;
  }
  // Multiple blocks — summarise by type
  const counts: Record<string, number> = {};
  for (const b of content as any[]) {
    counts[b.type] = (counts[b.type] ?? 0) + 1;
  }
  return `[${Object.entries(counts).map(([t, n]) => `${n} ${t}`).join(", ")}]`;
}

// Render a pseudo-JSON block for the API response
function apiResponseLines(
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: Anthropic.ContentBlock[],
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: `◀ response`, bold: true, color: "blue" });
  lines.push({ text: `  {`, color: "blue" });
  lines.push({ text: `    stop_reason: "${stopReason}",`, color: "blue" });
  lines.push({ text: `    usage: { input_tokens: ${usage.input_tokens}, output_tokens: ${usage.output_tokens} },`, color: "blue" });
  lines.push({ text: `    content: [`, color: "blue" });
  for (const block of content) {
    if (block.type === "text") {
      const preview = block.text.length <= 80
        ? `"${block.text.replace(/\n/g, "\\n")}"`
        : `<text: ${block.text.length} chars>`;
      lines.push({ text: `      { type: "text", text: ${preview} },`, color: "blue", dimColor: true });
    } else if (block.type === "tool_use") {
      lines.push({ text: `      { type: "tool_use", name: "${block.name}", input: ${JSON.stringify(block.input)} },`, color: "blue", dimColor: true });
    } else {
      lines.push({ text: `      { type: "${block.type}" },`, color: "blue", dimColor: true });
    }
  }
  lines.push({ text: `    ]`, color: "blue" });
  lines.push({ text: `  }`, color: "blue" });
  return lines;
}

// --- Render a single completed item ---

function ItemRow({ item }: { item: CompletedItem }) {
  return (
    <Box flexDirection="column">
      {item.lines.map((line, i) => (
        <Box key={i} flexDirection="row">
          {/* Time column — only on first line */}
          <Box width={TIME_WIDTH}>
            <Text dimColor>{i === 0 ? item.time : ""}</Text>
          </Box>
          {/* Content */}
          <Text
            color={line.color as any}
            dimColor={line.dimColor}
            bold={line.bold}
          >
            {line.text}
          </Text>
        </Box>
      ))}
    </Box>
  );
}

// --- App ---

export function App() {
  const { exit } = useApp();
  const [agent] = useState(() => new Agent());
  const [authMode, setAuthMode] = useState<string>("...");
  const [ready, setReady] = useState(false);
  const [inputState, setInput] = useState("");

  const [priorSession, setPriorSession] = useState<Session | null>(null);
  const [resumePromptDone, setResumePromptDone] = useState(false);

  const [completedItems, setCompletedItems] = useState<CompletedItem[]>([]);
  const [streamingText, setStreamingText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [activity, setActivity] = useState("");

  const abortControllerRef = useRef<AbortController | null>(null);
  const [lastCallTokens, setLastCallTokens] = useState<number | null>(null);
  const [tokenDelta, setTokenDelta] = useState<string>("");
  const [lastResponse, setLastResponse] = useState<{ text: string; dimText?: string } | null>(null);

  const nextId = useRef(0);

  const addItem = useCallback(
    (type: ItemType, time: string, lines: Line[]) => {
      setCompletedItems((prev) => [
        ...prev,
        { id: nextId.current++, type, time, lines },
      ]);
    },
    []
  );

  useEffect(() => {
    agent.init().then(async (mode) => {
      setAuthMode(mode);
      addItem("system", now(), [
        {
          text: mode === "Claude Max"
            ? `✓ Authenticated: ${mode}`
            : `⚠ Auth: ${mode}`,
          color: mode === "Claude Max" ? undefined : "red",
        },
      ]);
      const prior = await agent.checkPriorSession();
      if (prior && prior.history.length > 0) {
        setPriorSession(prior);
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
          addItem("system", now(), [{ text: `↩ Resumed session from ${savedAt} (${msgCount} messages)` }]);
        } else {
          addItem("system", now(), [{ text: "↩ Starting fresh session" }]);
        }
        setPriorSession(null);
        setResumePromptDone(true);
        setReady(true);
        setInput("");
        return;
      }

      const trimmed = value.trim();
      if (!trimmed || isStreaming || !ready) return;

      // Move last response to static zone
      if (lastResponse) {
        addItem("assistant", now(), [
          { text: lastResponse.text },
          ...(lastResponse.dimText
            ? lastResponse.dimText.split("\n").map(l => ({ text: l, dimColor: true }))
            : []),
        ]);
        setLastResponse(null);
      }

      setInput("");
      setStreamingText("");
      setIsStreaming(true);
      setTokenDelta("");
      setLastCallTokens(null);

      // User prompt — strong visual prominence with separator lines
      const sep = "─".repeat(60);
      addItem("user", now(), [
        { text: sep, color: "green", bold: true },
        { text: `❯ ${trimmed}`, color: "green", bold: true },
        { text: sep, color: "green", bold: true },
      ]);

      let fullText = "";
      const confirmTool = async () => true;
      const controller = new AbortController();
      abortControllerRef.current = controller;

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
              addItem("api_request", now(), apiRequestLines(
                event.callNumber,
                event.model,
                event.system,
                event.tools,
                event.messages,
                est,
              ));
              break;
            }

            case "api_response": {
              addItem("api_response", now(), apiResponseLines(
                event.stopReason,
                event.usage,
                event.content,
              ));
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

            case "tool_result": {
              const resultPreview = truncateOutput(event.result.output);
              addItem("tool_call", now(), [
                { text: `⚙ ${event.formatted}`, color: "yellow", bold: true },
                { text: `  ${event.result.isError ? "✗" : "✓"} ${resultPreview}`, color: "yellow", dimColor: true },
              ]);
              setActivity("");
              break;
            }

            case "metrics":
              // No longer displayed — aggregated in turn_end
              break;

            case "turn_end": {
              const m = event.metrics;
              const metricsLine = `in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)}`;
              if (fullText) {
                setLastResponse({ text: fullText, dimText: metricsLine });
                fullText = "";
                setStreamingText("");
              } else {
                addItem("assistant", now(), [{ text: metricsLine, dimColor: true }]);
              }
              break;
            }

            case "error":
              addItem("error", now(), [{ text: `⚠ ${event.error}`, color: "red" }]);
              break;

            case "interrupted":
              if (fullText) {
                addItem("assistant", now(), [{ text: fullText }]);
                fullText = "";
                setStreamingText("");
              }
              addItem("error", now(), [{ text: "⊘ Interrupted", color: "red" }]);
              break;
          }
        }
      } catch (err: any) {
        addItem("error", now(), [{ text: `⚠ ${err.message}`, color: "red" }]);
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
      <Static items={completedItems}>
        {(item) => <ItemRow key={item.id} item={item} />}
      </Static>

      <Box flexDirection="column">
        {/* Streaming response text */}
        {isStreaming && streamingText && (
          <Box>
            <Box width={TIME_WIDTH}><Text dimColor>{now()}</Text></Box>
            <Text>{streamingText}<Text dimColor>▊</Text></Text>
          </Box>
        )}

        {/* Activity indicator */}
        {isStreaming && !streamingText && (
          <Box>
            <Box width={TIME_WIDTH}><Text dimColor>{now()}</Text></Box>
            <Text dimColor>⏳ {activity || "working..."}</Text>
          </Box>
        )}

        {/* Last completed response */}
        {!isStreaming && lastResponse && (
          <Box flexDirection="column">
            <Box>
              <Box width={TIME_WIDTH}><Text dimColor>{""}</Text></Box>
              <Text>{lastResponse.text}</Text>
            </Box>
            {lastResponse.dimText && (
              <Box>
                <Box width={TIME_WIDTH}><Text>{""}</Text></Box>
                <Text dimColor>{lastResponse.dimText}</Text>
              </Box>
            )}
          </Box>
        )}

        {/* Resume prompt */}
        {priorSession && !resumePromptDone && (
          <Box flexDirection="column">
            <Box>
              <Box width={TIME_WIDTH}><Text dimColor>{now()}</Text></Box>
              <Text color="cyan">
                {"↩ Prior session: "}
                {new Date(priorSession.savedAt).toLocaleString()}
                {` (${priorSession.history.length} messages)`}
              </Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH}><Text>{""}</Text></Box>
              <Text dimColor>Resume? [Y/n] </Text>
            </Box>
          </Box>
        )}

        {/* Input prompt */}
        <Box>
          <Box width={TIME_WIDTH}><Text dimColor>{isStreaming ? "" : now()}</Text></Box>
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
