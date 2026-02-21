# UI Design — Technology-Independent Layout

This document defines the UI layout abstractly, independent of whether it's
rendered in a terminal (Ink) or a browser (HTML/CSS).

## Core Principle

The layout is a **data structure**. Renderers consume it. Adding a pane or
changing proportions means editing the descriptor, not the rendering code.

The primary renderer is Ink (terminal). The agent can read its own rendered
output as text, making the terminal the fastest iteration environment for a
text-native AI.

## Layout Zones

The screen is divided into three vertical zones, top to bottom:

### 1. Static Zone (scroll-off area)

Content that is **finished and immutable**. Once a conversation turn is
complete, a tool output is finalized, or a log entry is committed, it moves
here.

Properties:
- Grows upward (new items push old ones off screen)
- Never re-rendered after initial display
- In Ink (primary): maps to `<Static items={...}>`
- In web (future): maps to a scrollable container with append-only DOM

Content types:
- Completed assistant messages
- Completed user messages
- Finished tool invocations (with results)
- System notifications (model change, config reload, etc.)

### 2. Live Zone (active area)

The **re-rendering** area. This is where active work happens.

Subdivided horizontally:

```
┌─────────── Main Pane (flex: 3) ──────────┬── Side Pane (flex: 1) ──┐
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

Properties:
- **Main pane**: Primary interaction. Streaming text, tool output, input field.
- **Side pane**: Observability dashboard. Collapsible (keyboard toggle, e.g.
  `Ctrl+O`). When collapsed, main pane takes full width.
- Proportions defined in layout descriptor, not hardcoded.

#### Model Payload Viewer (in main pane)

At the top of the main pane (or as a collapsible overlay), a **payload
section** shows exactly what is sent to the model on the current/last call:

```
▶ Model Payload [3,847 tokens / 12.4 KB]          (click or key to expand)
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

```
 NOR │ claude-opus-4-6 │ In: 1,234 Out: 567 │ $0.042 │ TTFT: 340ms │ Session: 5m
```

Fields:
- Current mode (NOR / INS)
- Model name
- Token counts (input / output, current turn and/or session)
- Cost estimate
- Time-to-first-token for last request
- Session duration

## Layout Descriptor (TypeScript type)

```typescript
type PaneContent =
  | { type: 'chat' }           // main conversation
  | { type: 'input' }          // user input field
  | { type: 'tokens' }         // token counter widget
  | { type: 'cost' }           // cost tracker widget
  | { type: 'latency' }        // latency metrics widget
  | { type: 'logs' }           // recent log entries
  | { type: 'payload' }        // model payload viewer (system prompt, tools, history)
  | { type: 'api-trace' }      // raw API request/response viewer
  | { type: 'file-tree' }      // project file browser (future)
  | { type: 'diff' }           // file diff viewer (future)
  | { type: 'custom', id: string }  // extension point

interface Pane {
  id: string
  content: PaneContent
  flex?: number            // flex-grow value
  minWidth?: number        // minimum width in characters/pixels
  minHeight?: number       // minimum height in lines/pixels
  collapsible?: boolean    // can be hidden via keyboard shortcut
  collapsed?: boolean      // initial state
  border?: boolean         // draw border around pane
}

interface LayoutZone {
  direction: 'row' | 'column'
  panes: Pane[]
}

interface Layout {
  static: {
    enabled: boolean
    maxItems?: number      // max items to keep in static zone
  }
  live: LayoutZone
  statusBar: {
    enabled: boolean
    fields: string[]       // which fields to show
  }
}
```

## Default Layout

```typescript
const defaultLayout: Layout = {
  static: {
    enabled: true,
    maxItems: 100,
  },
  live: {
    direction: 'row',
    panes: [
      {
        id: 'main',
        content: { type: 'chat' },
        flex: 3,
        border: false,
      },
      {
        id: 'observability',
        content: { type: 'logs' },  // composite: tokens + cost + logs
        flex: 1,
        minWidth: 30,
        collapsible: true,
        collapsed: false,
        border: true,
      },
    ],
  },
  statusBar: {
    enabled: true,
    fields: ['model', 'tokens', 'cost', 'latency', 'session-time', 'shortcuts'],
  },
}
```

## Input Modes (Helix-inspired)

The UI is **modal**, inspired by the Helix editor:

### Normal Mode (default)

Navigation, commands, and pane management. Keystrokes are interpreted as
commands, not text input. The status bar shows `NOR` or similar indicator.

### Insert Mode

Text input for composing messages. Entered via `i` (or similar) from normal
mode. Exited via `Escape` back to normal mode. The status bar shows `INS`.

Mouse is supported in both modes (click to focus panes, scroll, select text).

The full keymap is deferred (see PLAN.md, Future Considerations), but the
architecture assumes modal dispatch from the start: every keystroke goes
through a mode-aware dispatcher.

## Keyboard Shortcuts (Initial Set)

**Normal mode:**

| Key | Action |
|-----|--------|
| `i` | Enter insert mode |
| `o` | Toggle observability side pane |
| `p` | Toggle model payload viewer |
| `t` | Show API trace for last request |
| `Ctrl+L` | Clear static zone |
| `Ctrl+C` | Cancel current operation / exit |
| `j/k` | Scroll history |

**Insert mode:**

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Escape` | Back to normal mode |
| `Up/Down` | Input history navigation |
| `Tab` | Autocomplete (future) |

## Rendering Contract

Any renderer (Ink is the primary; web is future) must implement:

```typescript
interface UIRenderer {
  // Initialize the renderer with a layout
  init(layout: Layout): void

  // Update a specific pane's content
  updatePane(paneId: string, content: any): void

  // Append to the static zone
  appendStatic(item: StaticItem): void

  // Update the status bar
  updateStatus(fields: Record<string, string>): void

  // Handle user input
  onInput(handler: (input: string) => void): void

  // Teardown
  destroy(): void
}
```

This contract ensures the agent core never knows or cares whether it's talking
to a terminal or a browser.
