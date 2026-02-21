# UI Design — Layout and Interaction

This document describes the UI layout and interaction model. The primary
(and currently only) renderer is Ink (terminal).

Formal TypeScript interfaces for layout abstraction are deferred until a
second renderer (browser) is added. At that point, we extract the abstraction
from the working Ink implementation rather than guessing at it upfront.

## Layout Zones

The screen is divided into three vertical zones, top to bottom:

### 1. Static Zone (scroll-off area)

Content that is **finished and immutable**. Once a conversation turn is
complete, a tool output is finalized, or a log entry is committed, it moves
here.

Properties:
- Grows upward (new items push old ones off screen)
- Never re-rendered after initial display
- In Ink: maps to `<Static items={...}>`

Content types:
- Completed assistant messages
- Completed user messages
- Finished tool invocations (with results)
- System notifications (model change, config reload, etc.)

### 2. Live Zone (active area)

The **re-rendering** area. This is where active work happens.

In its full form (M3+), subdivided horizontally:

```
┌─────────── Main Pane ────────────────────┬── Side Pane ────────────┐
│                                          │                         │
│  Streaming assistant response            │  Token Counter           │
│  ...or active tool execution output      │    Input: 1,234          │
│  ...or user input prompt                 │    Output: 567           │
│                                          │    Cache: hit            │
│                                          │                         │
│                                          │  Cost (session)          │
│                                          │    $0.042                │
│                                          │                         │
│                                          │  Latency                 │
│                                          │    TTFT: 340ms           │
│                                          │    Total: 2.1s           │
│                                          │                         │
│                                          │  Recent Logs             │
│                                          │    [DEBUG] tool:read ... │
│                                          │    [INFO] api:stream ... │
│                                          │                         │
│  ┌─ Input ─────────────────────────────┐ │                         │
│  │ > _                                 │ │                         │
│  └─────────────────────────────────────┘ │                         │
│                                          │                         │
└──────────────────────────────────────────┴─────────────────────────┘
```

**M0 version is much simpler**: just the streaming response and an input
line. No side pane, no observability dashboard. Token count shown inline
after each response.

The side pane is **collapsible and collapsed by default** (small-window-first
design). Toggled by keyboard shortcut.

#### Model Payload Viewer (M3+)

A collapsible section showing exactly what is sent to the model:

```
▶ Model Payload [3,847 tokens / 12.4 KB]          (key to expand)
```

When expanded:
```
▼ Model Payload [3,847 tokens / 12.4 KB]
  ┌─ System Prompt (428 tokens) ─────────────────────┐
  │ You are Omega, a self-improving coding agent...   │
  └───────────────────────────────────────────────────┘
  ┌─ Tool Definitions (1,204 tokens) ────────────────┐
  │ read_file, write_file, run_command, ...           │
  └───────────────────────────────────────────────────┘
  ┌─ Conversation (2,215 tokens) ────────────────────┐
  │ [12 messages, 3 tool calls]                       │
  └───────────────────────────────────────────────────┘
```

Collapsed by default. Always shows the total size. The operator can drill
into any section to see the exact text. No hidden content — if it goes to
the model, it's visible here.

### 3. Status Bar (fixed bottom)

Single line. Always visible. Dense information at a glance.

**M0 version:**
```
 claude-opus-4-6 │ In: 1,234 Out: 567 │ $0.042
```

**Full version (M3+):**
```
 NOR │ claude-opus-4-6 │ In: 1,234 Out: 567 │ $0.042 │ TTFT: 340ms │ 🟢
```

Fields (added incrementally):
- Current mode — NOR / INS (M3)
- Model name (M0)
- Token counts — input / output (M0)
- Cost estimate (M0)
- Time-to-first-token (M3)
- Provider health — 🟢 🟡 🔴 (M3)
- Session duration (M3)

## Input Modes (Helix-inspired, M3+)

The UI will be **modal**, inspired by the Helix editor:

### Normal Mode (default)

Navigation, commands, and pane management. Keystrokes are interpreted as
commands, not text input. The status bar shows `NOR`.

### Insert Mode

Text input for composing messages. Entered via `i` from normal mode.
Exited via `Escape` back to normal mode. The status bar shows `INS`.

Mouse is supported in both modes (click to focus panes, scroll).

**M0 has no modal input.** The UI starts in a simple always-input mode.
Modal dispatch is added in M3.

## Keyboard Shortcuts

### M0 (minimal)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Ctrl+C` | Exit |

### M3+ (full, normal mode)

| Key | Action |
|-----|--------|
| `i` | Enter insert mode |
| `o` | Toggle observability side pane |
| `p` | Toggle model payload viewer |
| `t` | Show API trace for last request |
| `Ctrl+L` | Clear static zone |
| `Ctrl+C` | Cancel current operation / exit |
| `j/k` | Scroll history |

### M3+ (full, insert mode)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Escape` | Back to normal mode |
| `Up/Down` | Input history navigation |
| `Tab` | Autocomplete (future) |
