# Phase 3.8 — SolidJS → Leptos selector mapping

This file is **step 1** of the Phase 3.8 visual-parity port. It
records every CSS selector from the deleted SolidJS theme
(`git show 1e3bed4:src/web/client/style.css`, 1408 lines) and
classifies it as one of:

- **pass-through** — selector exists verbatim in the Leptos class
  surface; CSS body copies over unchanged.
- **renamed** — selector exists with a different name; copy the CSS
  body, swap selector(s).
- **adapt** — selector targets a structure Leptos has but with a
  different shape; copy the rule but adjust where needed (e.g.
  attribute-selector instead of class).
- **dead** — selector targets a structure Leptos doesn't have
  (status display, metrics table, OAuth dialog, token legend,
  custom effort dropdown, retry fragment, modal-section labels,
  llm-call-modal internals, etc.). Drop entirely.

**Strategy:** keep `leptos-*` prefixes on the Leptos class names.
Rationale: 27 SSR snapshots + 32 Playwright specs lock in the
existing class strings; renaming risks breaking selectors for zero
behavioural gain. The CSS rewrite is mechanical.

**Theme:** Mocha (dark) only. Light theme dropped — SolidJS shipped
Mocha only.

**Reference:** the full SolidJS class surface used in production
markup is recoverable via
`git show 1e3bed4:src/web/client/App.tsx` (where each selector was
applied). The Leptos class surface (44 distinct values) is the
output of:

```sh
grep -rh 'class="' frontends/leptos/src/*.rs \
  | grep -oE 'class="[^"]+"' | sort -u
```

---

## Top-level

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|---|
| `:root` (Catppuccin Mocha vars) | pass-through | `:root` | Verbatim — semantic colour palette is the foundation. |
| `*` (box-sizing reset) | pass-through | `*` | Verbatim. |
| `html, body, #root` | adapt | `html, body` | Drop `#root` (Leptos mounts to `body` directly via `mount_to_body`). |
| `.app` | adapt | `main` | Leptos's root container is the bare `<main>` tag in `lib.rs::App`. Same flex column / overflow:hidden / padding shape applies to `main`. |

## Feed + blocks

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|---|
| `.feed-wrapper` | dead | — | No outer wrapper in Leptos; the feed `<section>` is the only feed-side element. |
| `.feed` | renamed | `.leptos-feed` | Copy the `flex:1; overflow-y:auto; …` rules verbatim. |
| `.block` | pass-through | `.block` | The Leptos `EventBlock` emits `class="block block-<kind>"` from `event_view::css_class_for`. |
| `.block.user` | renamed | `.block-user` | Each kind gets its own variant class (no compound `.block.user` selector — Leptos splats both classes together but the `.block-<kind>` selector alone is enough for the variant rules). |
| `.block.assist` | renamed | `.block-assistant` | |
| `.block.tool` | renamed | `.block-tool-call` | |
| `.block.result` | renamed | `.block-tool-result` | The OK case (errored tool results land in `.block-error`). |
| `.block.result-error` | renamed | `.block-error` | `kind_for` collapses errored tool_result + AgentError + LlmError + TransportError + TurnInterrupted into the single `Error` family. |
| `.block.api-call`, `.block.api-response` | adapt | `.block.block-status[data-event-type="llm_call"]`, `.block.block-assistant[data-event-type="llm_response"]` | Phase 3.9 TODO-3: per-event-type rules override the family-block colour. `llm_call` → sapphire (`--llm`); `llm_response` → sapphire border/label with `--text` body. Rules added after the family-block rules so source order wins. |
| `.block.thinking` | adapt | `[data-testid="leptos-streaming-thinking"].block-status` | Phase 3.9 TODO-3: streaming-thinking overlay gets teal (`--ctp-teal`) via attribute+class compound rule, overriding the generic `.block-status` mauve. |
| `.block.status` | renamed | `.block-status` | Direct map. |
| `.block.footer` | adapt | `.block.block-status[data-event-type="turn_end"]` | Phase 3.9 TODO-3: `turn_end` gets the muted `--border`/`--dim` palette from the `--footer` look via data-event-type rule, rather than the generic `.block-status` mauve. |
| `.block.pause-event` | adapt | `.block.block-status[data-event-type="pause_requested"]`, `…[data-event-type="turn_paused"]`, `…[data-event-type="turn_continued"]` | Phase 3.9 TODO-3: all three pause events get teal (`--ctp-teal`) via per-event-type rules. |
| `.block.error-b`, `.block.interrupt` | renamed | `.block-error` | |
| `.block.info` | adapt | `.block.block-status[data-event-type="session_started"]`, `…[data-event-type="server_started"]`, `…[data-event-type="server_stopped"]`, `…[data-event-type="compacted"]`, `…[data-event-type="resuming_session"]`, `…[data-event-type="session_resumed"]` | Phase 3.9 TODO-3: info lifecycle events get `--ctp-overlay2` border + `--ctp-subtext0` fg via per-event-type rules. |
| `.block.retry` | adapt | `.block.block-status[data-event-type="llm_retry"]` | Phase 3.9 TODO-3: llm_retry restored to `--peach` (matching SolidJS) via per-event-type rule overriding the `.block-status` mauve. |
| `.block.streaming` (streaming label cursor) | adapt | `.block-streaming` | Leptos's `StreamingTail` adds a `block-streaming` class. The `.block-label::after` blinking ● keeps Mocha's pulse keyframes. |
| `.block-label` | pass-through | `.block-label` | Verbatim. |
| `.block-label-meta` | dead | — | Not emitted; meta lives in `.block-meta`. |
| `.block-body` | pass-through | `.block-body` | Verbatim. |
| `.block-meta` | adapt | `.block-meta` | Used by Leptos for the assistant usage line, tool-result duration line. SolidJS didn't have an exact equivalent; we use the muted `--dim` foreground. |
| `.block-tool-name` | pass-through | `.block-tool-name` | Direct map; same role as SolidJS's `.tool-name`. |
| `.block-tool-input` | pass-through | `.block-tool-input` | Hosts the JSON arg preview — styled as a `<pre>` body. |
| `.block-show-more` | pass-through | `.block-show-more` | Inline expand toggle. |
| `.block-llm-call-details` | pass-through | `.block-llm-call-details` | The `<details>` element hosting the inline llm_call expander. |
| `.block-llm-call-meta` | pass-through | `.block-llm-call-meta` | The `<dl>` inside the `<details>`. |
| `.block-llm-call-open` | pass-through | `.block-llm-call-open` | "context records…" button that opens the modal. |

## Markdown body

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|---|
| `.md-body` | pass-through | `.md-body` | Verbatim. |
| `.md-body p`, `ul`, `ol`, `h1..h6`, `blockquote` | pass-through | same | Verbatim. |
| `.md-body code` | pass-through | same | Verbatim. |
| `.md-body pre` | pass-through | same | Verbatim. |
| `.md-body table`, `th`, `td`, `tr:nth-child(even) td` | pass-through | same | Verbatim. |
| `.md-body a`, `hr`, `strong`, `em` | pass-through | same | Verbatim. |
| `.md-body pre.diff-block` | pass-through | same | The diff post-mount enhancer adds `.diff-block` and `data-testid="diff-block"`. |
| `.diff-add`, `.diff-del`, `.diff-hunk`, `.diff-file`, `.diff-ctx` | pass-through | same | Verbatim. |
| `.code-copy-btn` | pass-through | same | The mermaid.js shim (`addCopyButtons`) injects this class on each `<pre>`. |
| `.mermaid-wrapper` | pass-through | same | The mermaid.js shim emits this. |
| `.mermaid-diagram`, `.mermaid-error-notice`, `.mermaid-source` | pass-through | same | All emitted by the shim, verbatim. |
| `.mermaid-diagram svg line[stroke="#444444"]`, etc. (C4 overrides) | pass-through | same | Verbatim — applies to whichever `.mermaid-diagram` SVG renders. |

## Composer

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|---|
| `.input-row` | renamed | `.leptos-composer` | Same flex/gap/align-items shape. |
| `.input-row textarea` | renamed | `.leptos-composer-input` | Same border / focus / hover transitions. |
| `.textarea-wrap` | renamed | `.leptos-composer-textarea-wrap` | Anchor for the completion popup. |
| `.fc-dropdown` | renamed | `.leptos-composer-completion` | |
| `.fc-item` | renamed | `.leptos-composer-completion-item` | |
| `.fc-item.fc-hl` | renamed | `.leptos-composer-completion-hl` | Selected via `.leptos-composer-completion-item.leptos-composer-completion-hl`. |
| `.fc-item.fc-dir` | renamed | `.leptos-composer-completion-dir` | |
| `.input-btn` | adapt | `.leptos-composer button` (descendant) | Base button look — applied to every button that appears in the composer surface. |
| `.send-btn` | adapt | `.leptos-composer-primary[data-action="send"]` | Each primary-action variant uses an attribute selector against the existing `data-action` attribute (already emitted by `composer.rs::action_tag`). No DOM change needed. |
| `.pause-btn` | adapt | `.leptos-composer-primary[data-action="pause"]` | |
| `.continue-btn` | adapt | `.leptos-composer-primary[data-action="continue"]` | |
| `.takeitback-btn` | dead | — | The "Take it back" affordance was dropped in 3.4 (recorded decision). |
| `.abort-btn` | adapt | `.leptos-composer-primary[data-action="abort"]`, `.leptos-composer-abort` | Both the primary (during PauseRequested) and the secondary (during Paused) get the red colour. |
| `.sessions-btn` | adapt | `.leptos-composer-sessions` | Phase 3.9 TODO-1: a "Sessions" button was added to `<Composer/>` at position 0 (before model/effort selects). Opens the picker modal via `PickerOpen.open()`. Styled with yellow hover to match SolidJS's sessions-btn palette. |
| `.effort-select`, `.effort-trigger`, `.effort-dropdown`, `.effort-option`, `.effort-option-selected` | dead | — | Leptos uses native `<select>` elements (`.leptos-composer-effort` / `.leptos-composer-model`); the SolidJS custom dropdown machinery has no analogue. We style the native `<select>` with `.leptos-composer-effort` / `.leptos-composer-model` rules. |

## Session picker

The Leptos picker is now a true modal overlay (Phase 3.9 TODO-1). The 3.8 CSS gave it
the visual shape of a centred panel; 3.9 adds a fixed-position backdrop, a `✕` close
button in the header, and a `PickerOpen` signal wired to the composer's "Sessions" button.

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|——|
| `.session-picker-modal` (max-width:700px) | adapt | `[data-testid="leptos-session-picker"]` | Use the testid as the stable selector — `picker.rs` doesn't emit a `.leptos-session-picker` class today. |
| `.modal-backdrop` (when picker is open) | adapt | `.picker-backdrop` | Phase 3.9 TODO-1: fixed-position dark overlay added. `PickerOpen(false)` removes it from the DOM entirely via `<Show>`. |
| `.session-picker-list` | adapt | `[data-testid="leptos-session-list"]` | The `<ul>` is testid-bearing only; targeted by attribute selector. |
| `.session-picker-search` | dead | — | No search input in 3.2; deferred to 3.6/4 polish. |
| `.session-picker-new` | adapt | `[data-testid="leptos-session-new"]` | "+ new session" button. |
| `.session-picker-item` | renamed | `.session-item` | Already the class on each `<li>`. |
| `.session-picker-item-current` | renamed | `.session-item-active` | Already the class when active. |
| `.session-picker-item-header` | dead | — | No separate header row; the row is one flex line. |
| `.session-picker-name` | renamed | `.session-item-label` | |
| `.session-picker-unnamed` | dead | — | The Leptos picker shows the dir as the label when `name` is None — same fallback, no separate styling tier. |
| `.session-picker-current-badge` | renamed | `.session-item-active-marker` | Smaller text "(active)" — close enough to the SolidJS pill for parity. |
| `.session-picker-item-btns` | dead | — | The buttons (resume/rename/delete) live inline in the row's flex layout. |
| `.session-picker-meta` | dead | — | No model/effort/turn-count row; deferred. |
| `.session-picker-desc` | dead | — | No description; deferred. |
| `.session-picker-cont` | dead | — | No "resumed from …" annotation; deferred. |
| `.session-picker-resume` | adapt | `[data-testid="leptos-session-resume"]` | |
| `.session-picker-rename` | adapt | `[data-testid="leptos-session-rename"]` | |
| `.session-picker-save` | adapt | `[data-testid="leptos-session-rename-submit"]` | |
| `.session-picker-cancel-rename` | adapt | `[data-testid="leptos-session-rename-cancel"]` | |
| `.session-picker-delete` | adapt | `[data-testid="leptos-session-delete"]` | Hover turns red. |
| `.session-picker-rename-input` | adapt | `[data-testid="leptos-session-rename-input"]` | |
| `.session-picker-loading` | dead | — | The Leptos picker doesn't show a "Loading sessions…" placeholder; the empty list state is acceptable for 3.8. |
| `.session-picker-resuming`, `-resuming-text`, `-resuming-dir`, `-cancel` | dead | — | No mid-resume picker state; the resume flow re-uses the inline conversation feed. |
| `.picker-header` | new | `.picker-header` | Wraps the `<h2>Sessions</h2>` + `+ new session` button row inside the picker panel. |
| `.picker-error` | new | `.picker-error` | Error message inside the picker panel. |
| `.session-item-edit` | new | `.session-item-edit` | Inline-rename `<span>` containing the input + save/cancel buttons. |

## Context modal

The context modal is a full-viewport overlay. The 3.8 CSS gives it a Mocha
panel look. **Note: the inline `style=` attributes hard-coded white-bg/black-fg
in `context_modal.rs`; 3.8 strips them so CSS can take over (surfaced as the
unavoidable CSS-vs-DOM mismatch in the Phase 3.8 record).**

| SolidJS selector | Status | Leptos counterpart | Notes |
|---|---|---|---|
| `.modal-backdrop` | renamed | `.leptos-context-modal-backdrop` | Same `position:fixed; inset:0; …` shape. |
| `.modal` | renamed | `.leptos-context-modal` | Centred panel; max-width 64rem; mantle background. |
| `.modal-header` | renamed | `.leptos-context-modal-header` | |
| `.modal-title` | renamed | `.leptos-context-modal-title` | |
| `.modal-close` | renamed | `.leptos-context-modal-close` | |
| `.modal-body` (scroll body) | renamed | `.leptos-context-modal-records` | The `<ul>` of records is the scrolling content. |
| `.llm-call-msg`, `.llm-call-msg-role`, `.llm-call-msg-body`, `.llm-call-msg-user`, `.llm-call-msg-assistant`, `.llm-call-msg-ts`, `.llm-call-msg-loading`, `.llm-call-separator` | renamed | `.leptos-context-modal-record`, `.leptos-context-modal-record-role`, `.leptos-context-modal-record-body`, `.leptos-context-modal-record-user`, `.leptos-context-modal-record-assistant`, `.leptos-context-modal-record-time`, `.leptos-context-modal-loading` | Same dispatch by role-class. The `<li>`'s `data-role` attribute is preserved as a parallel selector. |
| `.modal-section-label` | adapt | `.leptos-context-modal-meta` | The "N hash(es) · M bytes" line. |
| `.modal-meta`, `.modal-scroll-body`, `.modal-pre`, `.modal.tool-modal`, `.modal.llm-call-modal`, `.modal.llm-resp-modal`, `.modal.block-modal`, `.modal-header-btns` | dead | — | No tool / llm-resp / block modal kinds in Leptos; the context modal is the only modal surface. |
| `.pending-changes-modal`, `-body`, `-actions` | dead | — | Pending-changes UI not ported (Phase-4-bound). |

## Bottom panel + status display + metrics

All dead — Leptos has no bottom panel today.

| SolidJS selector | Status | Notes |
|---|---|---|
| `.bottom-panel`, `-session`, `.bp-label`, `.bp-dir` | dead | No bottom panel in 3.0–3.7. |
| `.metrics-table`, `.sm-row-label`, `.sm-header-cell`, `.sm-col-gap`, `.sm-col-val`, `.sm-compact-line`, `.sm-legend-cell`, `.sm-legend-toggle` | dead | No metrics table. |
| `.status-display`, `.status-ready` / `-streaming` / `-retrying` / `-connecting` / `-error` / `-pause-requested` / `-pause-requested-precommit` / `-paused`, `.status-row`, `.status-label` | dead | The composer's `data-turn-state` attribute is the modern replacement; deferred to a future polish if a banner is wanted. |
| `.panel-toggle-btn` | dead | No metrics-panel toggle. |

## Misc dead

| SolidJS selector | Notes |
|---|---|
| `.scroll-to-bottom` | Not implemented in Leptos (3.6 carry-forward). |
| `.reconnect-banner` | Not surfaced; transport errors stay in the debug panel. |
| `.token-legend-overlay`, `.token-legend`, `-header`, `-close`, `-table` | Not implemented. |
| `.oauth-overlay`, `.oauth-dialog`, `-title`, `-steps`, `-link`, `-code-row`, `-code-input`, `-submit-btn`, `-cancel-btn` | Not implemented. |
| `.cursor`, `.user-msg-text`, `.user-msg-body`, `.tool-seq`, `.tool-name`, `.tool-arg`, `.tool-call-content`, `.tool-result-left`, `.block-id`, `.block-model`, `.block-preview`, `.block-preview-result`, `.block-tool-row`, `.block-btn-group`, `.block-expand-btn`, `.block-retry-meta`, `.retry-fragment`, `.retry-fragment-label`, `.retry-fragment-body`, `.llm-legend-btn`, `.turn-end-line`, `.thinking-body`, `.thinking-btn`, `.modal-close` (modal-internal), `.render-error` | All target SolidJS-specific markup that the Leptos surface (3.0–3.7) doesn't emit. |
| `.block-label-row` | The Leptos `EventBlock` emits the label as a sibling `<span>` rather than inside a row container. The `.block` flex column lays them out fine. |

## Debug panel + chrome

| SolidJS selector | Status | Leptos counterpart |
|---|---|---|
| (none — debug panel is Leptos-only) | new | `[data-testid="leptos-debug-panel"]` |
| (none — `<h1>Omega (Leptos) — Phase 3.6</h1>` heading) | new | `main > h1` |

---

## What we add that SolidJS didn't have

A small set of `.leptos-*` rules with no SolidJS counterpart:

- `.leptos-feed-sentinel` — invisible sentinel `<div>` for the auto-scroll
  `scrollIntoView` Effect. Zero-height, no border.
- `.leptos-composer-completion`, `-item`, `-hl`, `-dir` — file-path popup
  inside `.leptos-composer-textarea-wrap`. Mirrors `.fc-*` rules.
- `.leptos-composer-primary`, `.leptos-composer-abort`,
  `.leptos-composer-input`, `.leptos-composer-model`,
  `.leptos-composer-effort` — composer controls.
- `.leptos-context-modal-*` (full set) — modal overlay + panel + records.
- `.leptos-context-modal-empty` — placeholder text when no records returned.
- `.leptos-context-modal-error` — fetch-error notice.
