import React, { useState, useCallback, useRef, useEffect } from "react";
import { Box, Text, Static, useInput, useApp } from "ink";
import TextInput from "./fast-text-input.js";
import { Agent } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";
import type Anthropic from "@anthropic-ai/sdk";

// --- Types ---

type ItemType =
  | "user_message"
  | "api_request"
  | "api_response"
  | "tool_execution"
  | "tool_result_message"
  | "assistant_message"
  | "error"
  | "system";

interface CompletedItem {
  id: number;
  type: ItemType;
  time: string;
  lines: Line[];
}

interface Line {
  text: string;
  color?: string;
  dimColor?: boolean;
  bold?: boolean;
}

// --- Constants ---

const TIME_WIDTH = 10; // "HH:MM:SS  "
const INDENT = "  ";
const INDENT2 = "    ";
const INDENT3 = "      ";

// --- Helpers ---

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

// --- Line builders ---

function userMessageLines(content: string): Line[] {
  const lines: Line[] = [];
  lines.push({ text: "message", bold: true, color: "green" });
  lines.push({ text: `${INDENT}role: "user"`, color: "green" });
  lines.push({ text: `${INDENT}content:`, color: "green" });
  for (const line of content.split("\n")) {
    lines.push({ text: `${INDENT2}${line}`, color: "green", bold: true });
  }
  return lines;
}

function summariseMessageContent(content: Anthropic.MessageParam["content"]): string {
  if (typeof content === "string") {
    return content.length <= 60 ? `"${content}"` : `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  const parts = (content as any[]).map((b) => {
    if (b.type === "text") return `text: <${b.text.length} chars>`;
    if (b.type === "tool_use") return `tool_use: "${b.name}"`;
    if (b.type === "tool_result") return `tool_result: <${(b.content as string).length} chars>`;
    return b.type;
  });
  return `[${parts.join(", ")}]`;
}

function apiRequestLines(
  callNumber: number,
  model: string,
  system: string,
  tools: Anthropic.Tool[],
  messages: Anthropic.MessageParam[],
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: `api call #${callNumber}`, bold: true, color: "cyan" });
  lines.push({ text: `${INDENT}model: "${model}"`, color: "cyan" });
  lines.push({ text: `${INDENT}system: <${system.length} chars>`, color: "cyan" });
  lines.push({ text: `${INDENT}tools: [${tools.map(t => `"${t.name}"`).join(", ")}]`, color: "cyan" });
  lines.push({ text: `${INDENT}max_tokens: ${config.maxOutputTokens}`, color: "cyan" });
  const last = messages[messages.length - 1];
  lines.push({ text: `${INDENT}messages: <${messages.length}> …`, color: "cyan" });
  if (last) {
    lines.push({ text: `${INDENT2}{ role: "${last.role}", content: ${summariseMessageContent(last.content)} }`, color: "cyan", dimColor: true });
  }
  return lines;
}

function apiResponseLines(
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: Anthropic.ContentBlock[],
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: "api response", bold: true, color: "blue" });
  lines.push({ text: `${INDENT}stop_reason: "${stopReason}"`, color: "blue" });
  lines.push({ text: `${INDENT}usage:`, color: "blue" });
  lines.push({ text: `${INDENT2}input_tokens: ${usage.input_tokens}`, color: "blue", dimColor: true });
  lines.push({ text: `${INDENT2}output_tokens: ${usage.output_tokens}`, color: "blue", dimColor: true });
  lines.push({ text: `${INDENT}content:`, color: "blue" });
  for (const block of content) {
    if (block.type === "text") {
      lines.push({ text: `${INDENT2}text:`, color: "blue" });
      const preview = block.text.length <= 120 ? block.text : `<${block.text.length} chars>`;
      for (const line of preview.split("\n")) {
        lines.push({ text: `${INDENT3}${line}`, color: "blue", dimColor: true });
      }
    } else if (block.type === "tool_use") {
      lines.push({ text: `${INDENT2}tool_use:`, color: "blue" });
      lines.push({ text: `${INDENT3}name: "${block.name}"`, color: "blue", dimColor: true });
      lines.push({ text: `${INDENT3}input: ${JSON.stringify(block.input)}`, color: "blue", dimColor: true });
    } else {
      lines.push({ text: `${INDENT2}${block.type}`, color: "blue", dimColor: true });
    }
  }
  return lines;
}

function toolExecutionLines(
  name: string,
  input: any,
  formatted: string,
  result: { output: string; isError: boolean },
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: "tool execution", bold: true, color: "yellow" });
  lines.push({ text: `${INDENT}name: "${name}"`, color: "yellow" });
  lines.push({ text: `${INDENT}input: ${JSON.stringify(input)}`, color: "yellow" });
  lines.push({ text: `${INDENT}result:`, color: "yellow" });
  lines.push({ text: `${INDENT2}is_error: ${result.isError}`, color: "yellow", dimColor: true });
  lines.push({ text: `${INDENT2}content:`, color: "yellow", dimColor: true });
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push({ text: `${INDENT3}${line}`, color: "yellow", dimColor: true });
  }
  return lines;
}

function toolResultMessageLines(
  results: Array<{ tool_use_id: string; content: string; is_error: boolean }>,
): Line[] {
  const lines: Line[] = [];
  lines.push({ text: "message", bold: true, color: "magenta" });
  lines.push({ text: `${INDENT}role: "user"`, color: "magenta" });
  lines.push({ text: `${INDENT}content:`, color: "magenta" });
  for (const r of results) {
    lines.push({ text: `${INDENT2}tool_result:`, color: "magenta" });
    lines.push({ text: `${INDENT3}tool_use_id: "${r.tool_use_id}"`, color: "magenta", dimColor: true });
    lines.push({ text: `${INDENT3}is_error: ${r.is_error}`, color: "magenta", dimColor: true });
    lines.push({ text: `${INDENT3}content: <${r.content.length} chars>`, color: "magenta", dimColor: true });
  }
  return lines;
}

function assistantMessageLines(text: string, dimText?: string): Line[] {
  const lines: Line[] = [];
  lines.push({ text: "message", bold: true });
  lines.push({ text: `${INDENT}role: "assistant"` });
  lines.push({ text: `${INDENT}content:` });
  for (const line of text.split("\n")) {
    lines.push({ text: `${INDENT2}${line}` });
  }
  if (dimText) {
    for (const line of dimText.split("\n")) {
      lines.push({ text: `${INDENT}${line}`, dimColor: true });
    }
  }
  return lines;
}

// --- Item renderer ---

function ItemRow({ item }: { item: CompletedItem }) {
  return (
    <Box flexDirection="column">
      {item.lines.map((line, i) => (
        <Box key={i} flexDirection="row">
          <Box width={TIME_WIDTH} flexShrink={0}>
            <Text dimColor>{i === 0 ? item.time : ""}</Text>
          </Box>
          <Text color={line.color as any} dimColor={line.dimColor} bold={line.bold}>
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
      addItem("system", now(), [{
        text: mode === "Claude Max" ? `✓ Authenticated: ${mode}` : `⚠ Auth: ${mode}`,
        color: mode === "Claude Max" ? undefined : "red",
      }]);
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

      if (lastResponse) {
        addItem("assistant_message", now(), assistantMessageLines(
          lastResponse.text, lastResponse.dimText
        ));
        setLastResponse(null);
      }

      setInput("");
      setStreamingText("");
      setIsStreaming(true);

      let fullText = "";
      const confirmTool = async () => true;
      const controller = new AbortController();
      abortControllerRef.current = controller;

      try {
        for await (const event of agent.sendMessage(trimmed, confirmTool, controller.signal)) {
          switch (event.type) {

            case "user_message":
              addItem("user_message", now(), userMessageLines(event.content));
              break;

            case "api_call_start":
              addItem("api_request", now(), apiRequestLines(
                event.callNumber,
                event.model,
                event.system,
                event.tools,
                event.messages,
              ));
              break;

            case "api_response":
              addItem("api_response", now(), apiResponseLines(
                event.stopReason,
                event.usage,
                event.content,
              ));
              break;

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
              addItem("tool_execution", now(), toolExecutionLines(
                event.name,
                event.input,
                event.formatted,
                event.result,
              ));
              setActivity("");
              break;

            case "tool_result_message":
              addItem("tool_result_message", now(), toolResultMessageLines(event.results));
              break;

            case "metrics":
              break;

            case "turn_end": {
              const m = event.metrics;
              const dimText = `in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)}`;
              if (fullText) {
                setLastResponse({ text: fullText, dimText });
                fullText = "";
                setStreamingText("");
              } else {
                addItem("system", now(), [{ text: dimText, dimColor: true }]);
              }
              break;
            }

            case "error":
              addItem("error", now(), [{ text: `⚠ ${event.error}`, color: "red" }]);
              break;

            case "interrupted":
              if (fullText) {
                addItem("assistant_message", now(), assistantMessageLines(fullText));
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
        {/* Streaming assistant message */}
        {isStreaming && streamingText && (
          <Box flexDirection="column">
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text dimColor>{now()}</Text></Box>
              <Text bold>message</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT}role: "assistant"</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT}content:</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT2}{streamingText}<Text dimColor>▊</Text></Text>
            </Box>
          </Box>
        )}

        {/* Activity indicator */}
        {isStreaming && !streamingText && (
          <Box>
            <Box width={TIME_WIDTH} flexShrink={0}><Text dimColor>{now()}</Text></Box>
            <Text dimColor>⏳ {activity || "working..."}</Text>
          </Box>
        )}

        {/* Last completed response — shown until next message */}
        {!isStreaming && lastResponse && (
          <Box flexDirection="column">
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text bold>message</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT}role: "assistant"</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT}content:</Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text>{INDENT2}{lastResponse.text}</Text>
            </Box>
            {lastResponse.dimText && (
              <Box>
                <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
                <Text dimColor>{INDENT}{lastResponse.dimText}</Text>
              </Box>
            )}
          </Box>
        )}

        {/* Resume prompt */}
        {priorSession && !resumePromptDone && (
          <Box flexDirection="column">
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text dimColor>{now()}</Text></Box>
              <Text color="cyan">
                {"↩ Prior session: "}
                {new Date(priorSession.savedAt).toLocaleString()}
                {` (${priorSession.history.length} messages)`}
              </Text>
            </Box>
            <Box>
              <Box width={TIME_WIDTH} flexShrink={0}><Text>{""}</Text></Box>
              <Text dimColor>Resume? [Y/n] </Text>
            </Box>
          </Box>
        )}

        {/* Input prompt */}
        <Box>
          <Box width={TIME_WIDTH} flexShrink={0}>
            <Text dimColor>{isStreaming ? "" : now()}</Text>
          </Box>
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
          {isStreaming ? " │ Esc to interrupt" : " │ Ctrl+C quit"}
        </Text>
      </Box>
    </>
  );
}
