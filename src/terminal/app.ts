/**
 * Terminal application loop — wires Agent events to terminal output.
 *
 * Imports from agent, renderer, and input modules.
 * This is the only file that ties everything together for the terminal UI.
 */

import { Agent } from "../agent.js";
import { config } from "../config.js";
import { formatTurnFooter } from "../turn-footer.js";
import { checkDiagnostics } from "../diagnosis.js";

import { clearContextStore } from "../context-store.js";
import { clearSessionEvents } from "../session-event.js";
import { exhaustiveCheck } from "../events.js";
import {
  bold, dim, green, red, yellow,
  TIME_WIDTH, INDENT, INDENT2,
  now, printBlock, println,
  renderUserMessage, renderApiRequest, renderApiResponse,
  renderToolStart, renderToolResult,
  renderAssistantMessage, renderStatus,
} from "./renderer.js";
import {
  parseKeys, sharedBuffer, sharedPasteState,
  type KeyCallbacks,
} from "./input.js";

// ---------------------------------------------------------------------------
// Input prompt
// ---------------------------------------------------------------------------

/** The visible width (columns) of the prompt prefix string, excluding ANSI codes. */
function promptVisualWidth(prefix: string): number {
  const stripped = prefix.replace(/\x1b\[[0-9;]*m/g, "");
  let w = 0;
  for (const ch of [...stripped]) w += (ch.codePointAt(0)! < 0x7f ? 1 : 1); // ASCII safe
  return stripped.length; // simple for prompt (all ASCII)
}

function printPrompt(prefix: string): void {
  const timeStr = dim(now().padEnd(TIME_WIDTH));
  process.stdout.write(timeStr + prefix);
  sharedBuffer.promptWidth = TIME_WIDTH + prefix.replace(/\x1b\[[0-9;]*m/g, "").length;
}

// ---------------------------------------------------------------------------
// Raw stdin setup
// ---------------------------------------------------------------------------

function setupRawInput(
  onSubmit: (line: string) => void,
  onEscape: () => void,
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

  sharedBuffer.terminalWidth = process.stdout.columns || undefined;

  process.stdout.on("resize", () => {
    sharedBuffer.terminalWidth = process.stdout.columns || undefined;
  });

  process.stdin.on("data", (chunk: string) => {
    parseKeys(chunk, { onSubmit, onEscape, onExit });
  });
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

/** Clean exit — exits immediately, no LLM calls. */
function shutdown(code: number = 0): never {
  process.stdout.write("\r\x1b[2K");
  process.stdout.write("\x1b[?2004l");
  process.exit(code);
}

// ---------------------------------------------------------------------------
// Main application loop
// ---------------------------------------------------------------------------

export async function runApp(): Promise<void> {
  await clearContextStore(); // fresh session — discard previous session's context
  await clearSessionEvents(); // fresh session — discard previous session's events
  const agent = new Agent();

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

  // Warn if any diagnostic snapshots exist from prior crashed sessions
  const diagFiles = await checkDiagnostics();
  if (diagFiles.length > 0) {
    printBlock(now(), [
      yellow(`⚠ Diagnostic snapshot(s) from a previous crash:`),
      ...diagFiles.map(f => yellow(`  ${f}`)),
      yellow(`  Read these files before debugging the error.`),
      yellow(`  Delete them once the issue is resolved.`),
    ]);
  }

  await agent.loadWorldState().catch(() => {});

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
        printPrompt(bold(green("❯ ")));
        return;
      }
      // Echo the clipboard content as if typed, then submit it
      process.stdout.write(clipboard);
      println("");
      trimmed = clipboard;
    }
    if (isStreaming) {
      printPrompt(bold(green("❯ ")));
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
            printBlock(now(), renderUserMessage(event.content));
            break;

          case "llm_call":
            printBlock(now(), renderApiRequest(
              event.provider,
              event.url,
              event.request,
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
              event.content,
              event.raw,
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

          case "session_compacted":
            if (streamingStarted) { println(""); streamingStarted = false; }
            if (event.newCount === event.originalCount) {
              printBlock(now(), [dim(`Context is already short (${event.originalCount} messages) — nothing compacted.`)]);
            } else {
              printBlock(now(), [dim(`Context compacted: ${event.originalCount} → ${event.newCount} messages`)]);
            }
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

          case "tool_call":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), renderToolStart(event.name, event.input, event.id));
            break;

          case "tool_result": {
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), renderToolResult(event.result, event.id));
            break;
          }

          case "llm_retry":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Retrying (attempt ${event.attempt})… ${event.error}`)]);
            break;

          case "diagnostic_written":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Diagnostic written: ${event.path}`)]);
            break;

          case "context_view_trimmed":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Context trimmed: ${event.originalMessages} → ${event.keptMessages} messages`)]);
            break;

          case "session_start":
            // session_start is logged at init() time; if streamed, show it compactly
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [dim(`Session started (${event.authMode})`)]);
            break;

          case "turn_end": {
            if (streamingStarted) {
              println("");
              streamingStarted = false;
            }
            const m = event.metrics;
            const provider = event.provider;
            const model = event.model;
            const { turnLine, sessionLine } = formatTurnFooter(
              { inputTokens: m.inputTokens, outputTokens: m.outputTokens, costUsd: m.costUsd, savedUsd: m.savedUsd, ttftMs: m.ttftMs, cacheCreationTokens: m.cacheCreationTokens, cacheReadTokens: m.cacheReadTokens },
              { inputTokens: agent.sessionInputTokens, outputTokens: agent.sessionOutputTokens, costUsd: agent.sessionCostUsd, savedUsd: agent.sessionSavedUsd, cacheCreationTokens: agent.sessionCacheCreationTokens, cacheReadTokens: agent.sessionCacheReadTokens },
              provider,
              model,
            );
            printBlock(now(), [turnLine]);
            printBlock(now(), [sessionLine]);
            // If diagnostic snapshots exist, remind the operator after every turn
            const diagFiles = await checkDiagnostics();
            if (diagFiles.length > 0) {
              const names = diagFiles.map(f => f.replace(/^diagnosis\//, "").replace(/\.json$/, ""));
              printBlock(now(), [red(`⚠ ${diagFiles.length} diagnostic snapshot(s): ${names.join(", ")}`)]);
            }
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
      printPrompt(bold(green("❯ ")));
    }
  }

  function initiateShutdown(): void {
    if (shuttingDown) return;
    shuttingDown = true;
    process.stdin.removeAllListeners("data");
    process.stdin.setRawMode?.(false);
    process.stdin.pause();
    if (abortController) { abortController.abort(); abortController = null; }
    shutdown(0);
  }

  process.once("SIGINT",  initiateShutdown);
  process.once("SIGTERM", initiateShutdown);

  setupRawInput(
    (line) => { handleSubmit(line).catch(console.error); },
    () => { if (abortController) { abortController.abort(); abortController = null; } },
    initiateShutdown,
  );

  printBlock(now(), [renderStatus(false)]);
  printPrompt(bold(green("❯ ")));
}
