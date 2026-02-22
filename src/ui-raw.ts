/**
 * Raw terminal UI — no library, no live zone.
 *
 * Everything is printed to scrollback as it happens.
 * No cursor movement, no live zone, no line counting.
 * The terminal owns all layout.
 */

import { Agent } from "./agent.js";
import { config } from "./config.js";
import type { Session } from "./session.js";

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

const CSI = "\x1b[";

function sgr(...codes: number[]) {
  return `${CSI}${codes.join(";")}m`;
}

const RESET   = sgr(0);
const BOLD    = sgr(1);
const DIM_ON  = sgr(2);

function styled(s: string, ...codes: number[]): string {
  return sgr(...codes) + s + RESET;
}

function bold   (s: string) { return styled(s, 1); }
function dim    (s: string) { return styled(s, 2); }
function green  (s: string) { return styled(s, 32); }
function cyan   (s: string) { return styled(s, 36); }
function blue   (s: string) { return styled(s, 34); }
function yellow (s: string) { return styled(s, 33); }
function magenta(s: string) { return styled(s, 35); }
function red    (s: string) { return styled(s, 31); }

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

const TIME_WIDTH = 10;
const INDENT  = "  ";
const INDENT2 = "    ";
const INDENT3 = "      ";

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

function truncateOutput(text: string, maxLines = 10): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… [${lines.length - maxLines} more lines]`;
}

/** Print lines to stdout, first line gets timestamp, rest get indent. */
function printBlock(time: string, lines: string[]): void {
  for (let i = 0; i < lines.length; i++) {
    const prefix = i === 0
      ? dim(time.padEnd(TIME_WIDTH))
      : " ".repeat(TIME_WIDTH);
    process.stdout.write(prefix + lines[i] + "\n");
  }
}

function println(s: string): void {
  process.stdout.write(s + "\n");
}

// ---------------------------------------------------------------------------
// Block renderers
// ---------------------------------------------------------------------------

function renderUserMessage(content: string): string[] {
  const lines: string[] = [];
  lines.push(bold(green("message")));
  lines.push(green(`${INDENT}role: "user"`));
  lines.push(green(`${INDENT}content:`));
  for (const line of content.split("\n")) {
    lines.push(bold(green(`${INDENT2}${line}`)));
  }
  return lines;
}

function summariseContent(content: any): string {
  if (typeof content === "string") {
    return content.length <= 60 ? `"${content}"` : `<${content.length} chars>`;
  }
  if (!Array.isArray(content) || content.length === 0) return "[]";
  const parts = (content as any[]).map((b: any) => {
    if (b.type === "text")        return `text: <${b.text.length} chars>`;
    if (b.type === "tool_use")    return `tool_use: "${b.name}"`;
    if (b.type === "tool_result") return `tool_result: <${(b.content as string).length} chars>`;
    return b.type;
  });
  return `[${parts.join(", ")}]`;
}

function renderApiRequest(
  callNumber: number,
  model: string,
  system: string,
  tools: any[],
  messages: any[],
): string[] {
  const last = messages[messages.length - 1];
  const lines: string[] = [];
  lines.push(bold(cyan(`api call #${callNumber}`)));
  lines.push(cyan(`${INDENT}model: "${model}"`));
  lines.push(cyan(`${INDENT}system: <${system.length} chars>`));
  lines.push(cyan(`${INDENT}tools: [${tools.map((t: any) => `"${t.name}"`).join(", ")}]`));
  lines.push(cyan(`${INDENT}max_tokens: ${config.maxOutputTokens}`));
  lines.push(cyan(`${INDENT}messages: <${messages.length}> …`));
  if (last) {
    lines.push(dim(cyan(`${INDENT2}{ role: "${last.role}", content: ${summariseContent(last.content)} }`)));
  }
  return lines;
}

function renderApiResponse(
  stopReason: string,
  usage: { input_tokens: number; output_tokens: number },
  content: any[],
): string[] {
  const lines: string[] = [];
  lines.push(bold(blue("api response")));
  lines.push(blue(`${INDENT}stop_reason: "${stopReason}"`));
  lines.push(blue(`${INDENT}usage:`));
  lines.push(dim(blue(`${INDENT2}input_tokens: ${usage.input_tokens}`)));
  lines.push(dim(blue(`${INDENT2}output_tokens: ${usage.output_tokens}`)));
  lines.push(blue(`${INDENT}content:`));
  for (const block of content) {
    if (block.type === "text") {
      lines.push(blue(`${INDENT2}text:`));
      const preview = block.text.length <= 120 ? block.text : `<${block.text.length} chars>`;
      for (const line of preview.split("\n")) {
        lines.push(dim(blue(`${INDENT3}${line}`)));
      }
    } else if (block.type === "tool_use") {
      lines.push(blue(`${INDENT2}tool_use:`));
      lines.push(dim(blue(`${INDENT3}name: "${block.name}"`)));
      lines.push(dim(blue(`${INDENT3}input: ${JSON.stringify(block.input)}`)));
    } else {
      lines.push(dim(blue(`${INDENT2}${block.type}`)));
    }
  }
  return lines;
}

function renderToolExecution(
  name: string,
  input: any,
  result: { output: string; isError: boolean },
): string[] {
  const lines: string[] = [];
  lines.push(bold(yellow("tool execution")));
  lines.push(yellow(`${INDENT}name: "${name}"`));
  lines.push(yellow(`${INDENT}input: ${JSON.stringify(input)}`));
  lines.push(yellow(`${INDENT}result:`));
  lines.push(dim(yellow(`${INDENT2}is_error: ${result.isError}`)));
  lines.push(dim(yellow(`${INDENT2}content:`)));
  for (const line of truncateOutput(result.output).split("\n")) {
    lines.push(dim(yellow(`${INDENT3}${line}`)));
  }
  return lines;
}

function renderToolResultMessage(
  results: Array<{ tool_use_id: string; content: string; is_error: boolean }>,
): string[] {
  const lines: string[] = [];
  lines.push(bold(magenta("message")));
  lines.push(magenta(`${INDENT}role: "user"`));
  lines.push(magenta(`${INDENT}content:`));
  for (const r of results) {
    lines.push(magenta(`${INDENT2}tool_result:`));
    lines.push(dim(magenta(`${INDENT3}tool_use_id: "${r.tool_use_id}"`)));
    lines.push(dim(magenta(`${INDENT3}is_error: ${r.is_error}`)));
    lines.push(dim(magenta(`${INDENT3}content: <${r.content.length} chars>`)));
  }
  return lines;
}

function renderAssistantMessage(text: string, dimText?: string): string[] {
  const lines: string[] = [];
  lines.push(bold("message"));
  lines.push(`${INDENT}role: "assistant"`);
  lines.push(`${INDENT}content:`);
  for (const line of text.split("\n")) {
    lines.push(`${INDENT2}${line}`);
  }
  if (dimText) {
    lines.push(dim(`${INDENT}${dimText}`));
  }
  return lines;
}

function renderStatus(agent: Agent, streaming: boolean): string {
  return dim(
    `${config.model} │ in: ${agent.sessionInputTokens} out: ${agent.sessionOutputTokens} │ ${formatCost(agent.sessionCostUsd)}` +
    (streaming ? " │ Esc to interrupt" : " │ Ctrl+C to quit")
  );
}

// ---------------------------------------------------------------------------
// Raw stdin
// ---------------------------------------------------------------------------

let inputBuffer = "";

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

  process.stdin.on("data", (chunk: string) => {
    for (let i = 0; i < chunk.length; i++) {
      const ch = chunk[i];
      const code = chunk.charCodeAt(i);

      if (ch === "\x03") { onExit(); return; }  // Ctrl+C

      if (ch === "\x1b") {                       // Escape or sequence
        if (i + 1 < chunk.length && chunk[i + 1] === "[") {
          // Arrow key or other CSI sequence — skip it
          i += 2;
          while (i < chunk.length && !/[A-Za-z~]/.test(chunk[i])) i++;
        } else {
          onEscape();
        }
        continue;
      }

      if (ch === "\r" || ch === "\n") {          // Enter
        const line = inputBuffer;
        inputBuffer = "";
        // Reprint the prompt line with the submitted text (it was shown inline)
        onSubmit(line);
        continue;
      }

      if (code === 127 || code === 8) {          // Backspace
        if (inputBuffer.length > 0) {
          inputBuffer = inputBuffer.slice(0, -1);
          // Erase last char: backspace, space, backspace
          process.stdout.write("\b \b");
        }
        continue;
      }

      // Printable character
      if (code >= 32 || code > 127) {
        inputBuffer += ch;
        process.stdout.write(ch);                // echo
      }
    }
  });
}

// ---------------------------------------------------------------------------
// Input prompt
// ---------------------------------------------------------------------------

function printPrompt(prefix: string): void {
  process.stdout.write(dim(now().padEnd(TIME_WIDTH)) + prefix);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

export async function runApp(): Promise<void> {
  const agent = new Agent();

  // Init
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

  // Check for prior session
  let priorSession: Session | null = null;
  try {
    const prior = await agent.checkPriorSession();
    if (prior && prior.history.length > 0) {
      priorSession = prior;
    }
  } catch { /* no prior session */ }

  let abortController: AbortController | null = null;
  let isStreaming = false;

  async function handleSubmit(line: string): Promise<void> {
    println("");  // newline after inline-echoed input

    // Resume prompt
    if (priorSession) {
      const v = line.trim().toLowerCase();
      if (v !== "y" && v !== "yes" && v !== "" && v !== "n" && v !== "no") {
        printBlock(now(), [red("⚠ Please answer y or n")]);
        printPrompt(dim("? "));
        return;
      }
      const resume = v === "y" || v === "yes" || v === "";
      if (resume) {
        agent.resumeSession(priorSession);
        printBlock(now(), [`↩ Resumed session (${priorSession.history.length} messages)`]);
      } else {
        printBlock(now(), ["↩ Starting fresh session"]);
      }
      priorSession = null;
      // Now show status and main prompt
      printBlock(now(), [renderStatus(agent, false)]);
      printPrompt(bold(green("❯ ")));
      return;
    }

    const trimmed = line.trim();
    if (!trimmed) {
      printBlock(now(), [dim("(empty input — type a message)")]);
      printPrompt(bold(green("❯ ")));
      return;
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
    const pendingInputs = new Map<string, any>(); // tool call id → input

    try {
      for await (const event of agent.sendMessage(trimmed, confirmTool, abortController.signal)) {
        switch (event.type) {

          case "user_message":
            printBlock(now(), renderUserMessage(event.content));
            break;

          case "api_call_start":
            printBlock(now(), renderApiRequest(
              event.callNumber, event.model, event.system,
              event.tools, event.messages,
            ));
            break;

          case "api_response":
            if (streamingStarted) {
              println("");
              streamingStarted = false;
            }
            printBlock(now(), renderApiResponse(
              event.stopReason, event.usage, event.content,
            ));
            break;

          case "api_error":
            printBlock(now(), [red(`api error (${event.model}): ${event.error}`)]);
            break;

          case "status":
            // No live zone — skip
            break;

          case "text":
            if (!streamingStarted) {
              // Print assistant message header, then stream content inline
              printBlock(now(), [
                bold("message"),
                `${INDENT}role: "assistant"`,
                `${INDENT}content:`,
              ]);
              process.stdout.write(" ".repeat(TIME_WIDTH) + INDENT2);
              streamingStarted = true;
            }
            process.stdout.write(event.text);
            fullText += event.text;
            break;

          case "tool_call":
            pendingInputs.set(event.id, event.input);
            break;

          case "tool_result": {
            const input = pendingInputs.get(event.id);
            pendingInputs.delete(event.id);
            printBlock(now(), renderToolExecution(
              event.name, input, event.result,
            ));
            break;
          }

          case "tool_result_message":
            printBlock(now(), renderToolResultMessage(event.results));
            break;

          case "metrics":
            break;

          case "turn_end": {
            if (streamingStarted) {
              println(""); // end the streamed line
              streamingStarted = false;
            }
            const m = event.metrics;
            const dimText = `in: ${m.inputTokens} out: ${m.outputTokens} cost: ${formatCost(m.costUsd)} ttft: ${formatMs(m.ttftMs)}`;
            printBlock(now(), [dim(dimText)]);
            printBlock(now(), [renderStatus(agent, false)]);
            break;
          }

          case "error":
            if (streamingStarted) { println(""); streamingStarted = false; }
            printBlock(now(), [red(`⚠ ${event.error}`)]);
            break;

          case "interrupted":
            if (streamingStarted) { println(""); streamingStarted = false; }
            if (fullText) {
              printBlock(now(), renderAssistantMessage(fullText));
              fullText = "";
            }
            printBlock(now(), [red("⊘ Interrupted")]);
            break;
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

  setupRawInput(
    (line) => { handleSubmit(line).catch(console.error); },
    () => { if (abortController) { abortController.abort(); abortController = null; } },
    () => { process.stdin.setRawMode(false); process.stdout.write("\n"); process.exit(0); },
  );

  // Initial prompt — resume or normal
  if (priorSession) {
    printBlock(now(), [
      cyan(`↩ Prior session: ${new Date(priorSession.savedAt).toLocaleString()} (${priorSession.history.length} messages)`),
      dim("  Resume? [Y/n]"),
    ]);
    printPrompt(dim("? "));
  } else {
    printBlock(now(), [renderStatus(agent, false)]);
    printPrompt(bold(green("❯ ")));
  }
}

// Entry point
runApp().catch((err) => {
  console.error(err);
  process.exit(1);
});
