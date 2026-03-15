/**
 * Terminal application loop — wires Agent events to terminal output.
 *
 * Imports from agent, renderer, and input modules.
 * This is the only file that ties everything together for the terminal UI.
 */

import { Agent } from "../agent.js";
import { config } from "../config.js";
import { formatTurnFooter } from "../turn-footer.js";
import { readFile } from "fs/promises";
import { makeSessionDir, findPreviousEventsFile } from "../session-dir.js";
import { exhaustiveCheck } from "../events.js";
import {
  bold, dim, green, lavender, red, yellow,
  TIME_WIDTH, INDENT, INDENT2,
  now, printBlock, println,
  renderUserMessage, renderApiRequest, renderApiResponse,
  renderToolStart, renderToolResult,
  renderAssistantMessage, renderStatus,
} from "./renderer.js";
import { parseKeys } from "./input.js";

// ---------------------------------------------------------------------------
// Input prompt
// ---------------------------------------------------------------------------

function printPrompt(prefix: string): void {
  const timeStr = dim(now().padEnd(TIME_WIDTH));
  process.stdout.write(timeStr + prefix);
}

// ---------------------------------------------------------------------------
// Raw stdin setup
// ---------------------------------------------------------------------------

function setupRawInput(
  onSubmit: (line: string) => void,
  onEscape: () => void,
  onBufferCleared: () => void,
  onExit: () => void,
): void {
  if (!process.stdin.setRawMode) {
    console.error("Error: stdin is not a TTY. Run in an interactive terminal.");
    process.exit(1);
  }
  process.stdin.setRawMode(true);
  process.stdin.resume();
  process.stdin.setEncoding("utf-8");

  // Enable bracketed paste mode so pasted newlines don't trigger submit
  process.stdout.write("\x1b[?2004h");

  process.stdin.on("data", (chunk: string) => {
    parseKeys(chunk, { onSubmit, onEscape, onExit, onBufferCleared });
  });
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

/** Clean exit — flushes session_end event, then exits. */
async function shutdown(agent: Agent | null, code: number = 0): Promise<never> {
  process.stdout.write("\r\x1b[2K");
  process.stdout.write("\x1b[?2004l");
  if (agent) await agent.emitSessionEnd("clean").catch(() => {});
  process.exit(code);
}

// ---------------------------------------------------------------------------
// Main application loop
// ---------------------------------------------------------------------------

export async function runApp(): Promise<void> {
  const sessionPaths = await makeSessionDir();
  const agent = new Agent(
    undefined,   // streamProvider
    null,        // _sessionDir (legacy placeholder)
    undefined,   // openAiCaller (use default)
    sessionPaths.contextFile,
    sessionPaths.eventsFile,
  );

  try {
    const mode = await agent.init();
    printBlock(now(), [
      mode === "Claude Max"
        ? `✓ Authenticated: ${mode}`
        : red(`⚠ Auth: ${mode}`),
    ]);
  } catch (err: any) {
    printBlock(now(), [red(`⚠ Auth error: ${err.message}`)]);
  }

  // Warn if the previous session ended abnormally (no session_end, or outcome = "error").
  const prevEventsPath = await findPreviousEventsFile(sessionPaths.dir);
  if (prevEventsPath) {
    try {
      const raw = await readFile(prevEventsPath, "utf-8");
      const lines = raw.trim().split("\n").filter(Boolean);
      if (lines.length > 0) {
        const last = JSON.parse(lines[lines.length - 1]);
        if (last.type !== "session_end") {
          printBlock(now(), [
            yellow(`⚠ Previous session has no session_end — it may have crashed.`),
            yellow(`  Inspect ${prevEventsPath} for details.`),
          ]);
        } else if (last.outcome === "error") {
          printBlock(now(), [
            yellow(`⚠ Previous session ended with an error: ${last.reason ?? "(no reason)"}`),
            yellow(`  Inspect ${prevEventsPath} for details.`),
          ]);
        }
      }
    } catch {
      // Previous session file unreadable — nothing to warn about.
    }
  }

  await agent.loadSystemPromptAppend().catch(() => {});

  let abortController: AbortController | null = null;
  let isStreaming = false;
  let shuttingDown = false;

  async function readClipboard(): Promise<string> {
    try {
      const proc = Bun.spawnSync(["wl-paste", "--no-newline"], { stderr: "ignore" });
      if (proc.exitCode !== 0) return "";
      return proc.stdout.toString();
    } catch {
      return "";
    }
  }

  async function handleSubmit(line: string): Promise<void> {
    println("");

    let trimmed = line.trim();
    if (!trimmed) {
      const clipboard = (await readClipboard()).trim();
      if (!clipboard) {
        printBlock(now(), [dim("(empty input and clipboard — type a message or copy something)")]);
        printPrompt(bold(lavender("❯ ")));
        return;
      }
      // Echo the clipboard content as if typed, then submit it
      process.stdout.write(clipboard);
      println("");
      trimmed = clipboard;
    }
    if (isStreaming) {
      printPrompt(bold(lavender("❯ ")));
      return;
    }

    isStreaming = true;
    const confirmTool = async () => true;
    abortController = new AbortController();
    let fullText = "";
    let streamingStarted = false;

    try {
      for await (const event of agent.sendMessage(trimmed, confirmTool, abortController.signal)) {
        switch (event.type) {

          case "user_message":
            printBlock(now(), renderUserMessage());
            break;

          case "llm_call":
            printBlock(now(), renderApiRequest(
              event.provider,
              event.url,
              event.request,
              event.model,
            ));
            break;

          case "llm_response":
            if (streamingStarted) {
              println("");
              streamingStarted = false;
            }
            printBlock(now(), renderApiResponse(
              event.provider,
              event.url,
              event.stopReason,
              event.usage,
              event.model,
            ));
            break;

          case "llm_error": {
            if (streamingStarted) { println(""); streamingStarted = false; }
            const shortUrl = event.url ? event.url.replace(/^https?:\/\//, "") : "unknown";
            printBlock(now(), [red(`api error (${event.provider} ${shortUrl}): ${event.error}`)]);
            break;
          }

          case "model_changed":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [`Switched to ${event.provider} ${event.model}`]);
            break;

          case "oauth_token_expired":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim("OAuth token expired/revoked — refreshing...")]);
            break;

          case "oauth_refreshed":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim("Token refreshed, retrying...")]);
            break;

          case "compact_user_start":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim("Compacting context…")]);
            break;

          case "compact_user_done":
            if (streamingStarted) { println(""); streamingStarted = false; }
            if (event.messagesAfter === event.messagesBefore) {
              printBlock(now(), [dim(`Context compacted: ${event.messagesBefore} → ${event.messagesAfter} messages (no change)`)]);
            } else {
              printBlock(now(), [dim(`Context compacted: ${event.messagesBefore} → ${event.messagesAfter} messages`)]);
            }
            break;

          case "compact_user_error":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [red(`⚠ Compaction failed: ${event.error}`)]);
            break;

          case "compact_auto_start":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Auto-compacting context (${event.messagesBefore} messages)…`)]);
            break;

          case "compact_auto_done":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Context auto-compacted: ${event.messagesBefore} → ${event.messagesAfter} messages`)]);
            break;

          case "compact_auto_error":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [yellow(`⚠ Auto-compaction failed (rolling truncation fallback): ${event.error}`)]);
            break;

          case "text":
            if (!streamingStarted) {
              process.stdout.write(dim(now().padEnd(TIME_WIDTH)) + bold("text") + "\n");
              process.stdout.write(" ".repeat(TIME_WIDTH) + INDENT);
              streamingStarted = true;
            }
            process.stdout.write(event.text);
            fullText += event.text;
            break;

          case "assistant_text":
            // Persisted full-text event emitted after streaming completes.
            // The text was already displayed via "text" fragments — ignore.
            break;

          case "tool_call":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), renderToolStart(event.name, event.input, event.id));
            break;

          case "tool_result": {
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), renderToolResult({ output: event.output, isError: event.isError }, event.id));
            break;
          }

          case "llm_retry":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Retrying (attempt ${event.attempt})… ${event.error}`)]);
            break;

          case "session_start":
            // session_start is logged at init() time; if streamed, show it compactly
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Session started (${event.authMode})`)]);
            break;

          case "session_end":
            // session_end is emitted at shutdown; normally not visible in the stream
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Session ended (${event.outcome})`)]);
            break;

          case "turn_end": {
            if (streamingStarted) {
              println("");
              streamingStarted = false;
            }
            const m = event.metrics;
            const { turnLine, sessionLine } = formatTurnFooter(
              { inputTokens: m.inputTokens, outputTokens: m.outputTokens, cacheCreationTokens: m.cacheCreationTokens, cacheReadTokens: m.cacheReadTokens },
              { inputTokens: agent.sessionInputTokens, outputTokens: agent.sessionOutputTokens, cacheCreationTokens: agent.sessionCacheCreationTokens, cacheReadTokens: agent.sessionCacheReadTokens },
              agent.getProvider(),
              agent.getActiveModel(),
            );
            printBlock(now(), [turnLine]);
            printBlock(now(), [sessionLine]);
            break;
          }

          case "agent_error":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [red(`⚠ ${event.error}`)]);
            break;

          case "turn_interrupted":
            if (streamingStarted) { println(""); streamingStarted = false; }
            if (fullText) {
              printBlock(now(), renderAssistantMessage(fullText));
              fullText = "";
            }
            printBlock(now(), [red("⊘ Interrupted")]);
            break;

          default:
            // Compile-time exhaustiveness check: TypeScript will error here if
            // any OmegaEvent or StreamSignal variant is not handled above.
            exhaustiveCheck(event);
        }
      }
    } catch (err: any) {
      if (streamingStarted) { println(""); streamingStarted = false; }
      printBlock(now(), [red(`⚠ ${err.message}`)]);
    } finally {
      isStreaming = false;
      abortController = null;
      printPrompt(bold(lavender("❯ ")));
    }
  }

  function initiateShutdown(): void {
    if (shuttingDown) return;
    shuttingDown = true;
    process.stdin.removeAllListeners("data");
    process.stdin.setRawMode?.(false);
    process.stdin.pause();
    if (abortController) { abortController.abort(); abortController = null; }
    shutdown(agent, 0);
  }

  process.once("SIGINT",  initiateShutdown);
  process.once("SIGTERM", initiateShutdown);

  setupRawInput(
    (line) => { handleSubmit(line).catch(console.error); },
    // onEscape: buffer was already empty — abort turn if one is running
    () => { if (abortController) { abortController.abort(); abortController = null; } },
    // onBufferCleared: Esc cleared a non-empty buffer — cursor stays on the same
    // line; the ❯ glyph is already visible, nothing to reprint
    () => {},
    initiateShutdown,
  );

  printBlock(now(), [renderStatus(false)]);
  printPrompt(bold(lavender("❯ ")));
}
