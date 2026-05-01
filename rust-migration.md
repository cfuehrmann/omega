# Omega — Rust Migration

*Condensed from a session covering migration rationale, architecture decisions,
testing strategy, and snapshot-review automation.*

---

## 1. Why Rust

### The TS type-system subversion problem

An LLM under pressure reaches for TypeScript's escape hatches: `as any`,
`as unknown as X`, `// @ts-ignore`, wide types (`object`, `{}`). These are
accepted at compile time and broken at runtime. This has already happened in
Omega — type-checker was absent from the gate for an extended period, and by
the time it was added (with TS 7 alpha + Go to make it fast enough) the LLM
had accumulated significant type debt. These are structural failure modes, not
discipline failures. Rust has no equivalent escape hatches; the compiler refuses.

### The multi-provider goal removes the SDK argument

The original argument for TypeScript — "the Anthropic SDK is mature and saves
work" — no longer holds once the target is Anthropic + OpenAI + Ollama + other
local LLMs. At that point you are writing wire-format code regardless of
language. Rust structs + serde + reqwest + an SSE crate are cleaner than
juggling multiple TypeScript SDKs with divergent abstractions over the same
underlying protocol.

### Gate speed is dominated by Playwright, not the language toolchain

Switching from TypeScript to Rust does not make the gate meaningfully slower.
Incremental `cargo check` and `cargo test` at Omega's scale are fast. The slow
part of the gate is browser tests, which are slow regardless of language.

### Rust-specific tooling advantages

| Tool | Value |
|---|---|
| **`insta`** | Best snapshot-testing DX in any ecosystem. `cargo insta review` TUI, inline diffs, first-class CI integration. Nothing comparable in JS/TS. |
| **`cargo mutants`** | Mutation testing that finds weak tests *and* dead code. Stryker for TS is significantly weaker. Unique force multiplier for assessing test quality. |
| **`cargo test`** | At least as fast as `bun test`, often faster via per-crate parallelism. |
| **Clippy** | Can enforce `#[deny(clippy::unwrap_used)]` etc., giving the gate teeth that TypeScript's `noUncheckedIndexedAccess` etc. never fully provided in practice. |

---

## 2. How Rust talks to LLMs (no SDK needed)

forgecode is the closest reference implementation. The pattern is simple:

- **Typed request/response structs** with `#[derive(Serialize, Deserialize)]`
- **`reqwest`** for HTTP, **`reqwest-eventsource`** (or `eventsource-stream`) for SSE streaming
- **Beta features via headers**, not a different endpoint:
  ```
  anthropic-version: 2023-06-01
  anthropic-beta: structured-outputs-2025-11-13
  anthropic-beta: interleaved-thinking-2025-05-14   (older models)
  ```
- **Provider abstraction** via a transformer pipeline: typed request structs are
  built once, then a chain of transforms (DropInvalidToolUse, SetCache,
  ReasoningTransform, etc.) normalises them per-provider before serialisation.
  This is the right pattern to adopt for Omega's multi-provider goal.

The Anthropic Messages API is straightforward JSON-over-HTTPS with an SSE
response stream. There is no aspect of it that requires a dedicated SDK.

---

## 3. All-in Rust, including the web client

### Why Leptos

Leptos uses fine-grained reactivity with signals — the same mental model as
SolidJS (the current stack). Porting existing Solid components is mostly a
syntax translation, not a paradigm change:
`createSignal` → `RwSignal`, `createEffect` → `create_effect`,
`<Show when=...>` → nearly identical. Dioxus (VDOM, React-like) and Yew
(older, less active) would require a deeper conceptual rewrite.

### The core architectural payoff: shared `omega-protocol` crate

```
crates/
├── omega-protocol/    # WsEvent, OmegaEvent, ToolCall, ToolResult — one source of truth
├── omega-core/        # agent loop; depends on omega-protocol, omega-providers
├── omega-providers/   # Anthropic, OpenAI, Ollama wire formats + transformer pipeline
├── omega-server/      # Axum + WebSocket; depends on omega-core
└── omega-web/         # Leptos SPA; depends on omega-protocol
```

`OmegaEvent` is one Rust enum, serialised on the server, deserialised on the
client. Adding a variant fails compilation in both places — exhaustively, at
every unhandled match arm. No schema files to maintain. No "did the WebSocket
field name match?" runtime surprises. This eliminates the cross-language type
friction completely.

### What you lose or work around

| Issue | Reality |
|---|---|
| **Mermaid stays JS** | One `<MermaidDiagram>` component via `wasm-bindgen`. Small, contained, unavoidable — Mermaid is a JS library. |
| **Browser testing** | See §4. |
| **WASM bundle size (~200–500 KB compressed)** | Irrelevant for a local dev tool; browser caches after first load. |
| **Dev hot-reload** | `cargo-leptos watch` — ~1–3 s incremental vs. Vite's sub-second. Acceptable for most editing. |
| **Component ecosystem** | Omega's UI is mostly custom; no dependency on Radix/shadcn to lose. |

---

## 4. Browser testing without Playwright

### The AI-testing landscape, honestly

Most AI-driven testing tools (Stagehand, browser-use, Magnitude) are built
**on top of Playwright** — they replace brittle selectors with natural-language
actions but do not remove the Node/TS dependency. Benchmark: Stagehand + Sonnet
~75% task completion vs. Playwright hand-written ~98% on the same tasks. Good
for self-healing in a TS project; not a Playwright *replacement* for this one.

### Rust-native browser driving

**`chromiumoxide`**: async (Tokio), CDP-based, no separate ChromeDriver process.
The Rust analogue of Puppeteer. Best choice for Chrome-only testing.
**`fantoccini`**: async, WebDriver-based, browser-agnostic. More portable,
slightly less feature-rich.

Prefer `chromiumoxide`: CDP is more powerful (network interception, devtools
events, performance metrics), the API is cleaner, and Omega only needs Chrome.

### LLM-as-oracle for hard-to-assert cases

For tests where the question is inherently visual or semantic ("did the Mermaid
render?", "is the markdown formatted reasonably?", "is the layout broken?"),
call the LLM with a screenshot and a structured-output schema:

```rust
let screenshot = page.screenshot(...).await?;
let result: UiAssertion = claude.ask_with_image(
    "Are all 3 messages visible in chronological order? Is the latest streaming?",
    &screenshot,
    schema_for::<UiAssertion>(),
).await?;
assert!(result.messages_visible == 3);
assert!(result.is_streaming);
```

This reuses the `omega-providers` crate already built for the agent.
Cost: ~$0.05 per LLM assertion. Use for ~5–10 tests where it earns its keep,
not for the entire suite.

### Test pyramid for all-Rust Omega

| Tier | Tool | Notes |
|---|---|---|
| Component snapshots | `insta` on rendered Leptos HTML | Covers most "check DOM" cases. Leptos is SSR-first so this is idiomatic. |
| Component logic | `cargo test` on signals/effects | Fastest feedback. |
| Integration (server ↔ client) | `cargo test` + Axum `TestServer` + Leptos renderer | No browser needed for most integration cases. |
| True e2e | `chromiumoxide` + LLM oracle for visual cases | Last mile, small suite. |

The component-snapshot tier (first row) alone replaces a surprising fraction
of current Playwright tests. Leptos components rendered to HTML in a unit test,
snapshotted with `insta`, reviewed with `cargo insta review`. No browser, no
Node, no process boundary.

---

## 5. Migration strategy: headless-agent-first

### Rationale for this phasing

1. **Smallest blast radius first.** The agent core is where TS subversion has
   caused the most pain and where Rust's type system pays off most immediately.
2. **TB2 is a near-perfect parity oracle.** Run TS Omega and Rust Omega against
   the same Terminal Bench 2 tasks. If Rust scores within noise of TS, behaviour
   is preserved. Objective, external, provider-agnostic.
3. **`events.jsonl` is already the cross-version contract.** Run both
   implementations against the same deterministic mock-LLM scripts and diff
   event logs. Byte-identical output = correct port by construction.
4. **UI question deferred until agent is solid.** Leptos, `chromiumoxide`, and
   snapshot-review machinery can be built without racing against agent stability.

### Phases

**Phase 1 — Rust headless agent; TS web UI continues**

- Build `omega-protocol`, `omega-core`, `omega-providers` (Anthropic + Ollama)
- `omega-server`: Axum + WebSocket on same protocol as today
- Use **`ts-rs`** to generate `.d.ts` from `omega-protocol` Rust structs for
  the TS web UI. When UI is later ported to Leptos, delete codegen and depend
  on `omega-protocol` directly — smooth handoff.
- Gate: TB2 parity + `insta` console snapshots of `events.jsonl` from a
  deterministic mock LLM
- **Feature-freeze TS Omega** during this window

*Done when:* TB2 parity confirmed; mock-script event sequences match TS Omega;
one week of dogfooding without regressions.

**Phase 2 — Switch daily driver, retire TS agent**

One week of dogfooding. TS agent deleted. TS web UI still runs via `ts-rs` types.

**Phase 3 — Leptos rewrite of web UI**

Server and client now share `omega-protocol` directly. Drop `ts-rs` codegen.
`insta` SSR snapshots cover Leptos components. Snapshot review automation
(§6) is already ready from Phase 1.

**Phase 4 — Browser tier; retire Playwright**

`chromiumoxide` + LLM oracle. Gate runs entirely under `cargo`. Playwright
and its Node dependency deleted.

### Key constraints

**Two providers from day one, not one.** If Phase 1 only builds Anthropic, the
provider abstraction will be Anthropic-shaped — exactly the leakage to avoid.
Building Anthropic + Ollama forces the transformer pipeline to be genuinely
abstract. Marginal extra cost on day one; very high cost to retrofit later.

**Don't redesign during the port.** Log improvement ideas in a deferred file.
Migration success criterion is parity, not improvement. Improvements happen
after parity is achieved, under the better type system.

---

## 6. Snapshot review automation

### The priming-bias problem

Within a single agent session, hiding the "narrative" of a change is impossible.
Every prior message, plan, and tool result primes the LLM. An agent told "we
refactored Foo" will rationalise almost any snapshot diff as consistent with
that refactor. This is motivated reasoning, not analysis.

### Solution: separate review session

A fresh agent session has no memory of the coding conversation. The reviewer sees:

**Sees:**
- Current state of the code (read access via tools — what exists, not the diff)
- The new rendered snapshot
- The previous accepted snapshot
- The test name / docstring (thin spec)

**Does not see:**
- Commit messages or PR descriptions
- Code diffs with `+/-` framing
- Planning notes or the coding session's conversation history

**Prompt framing:** instruct the reviewer to be skeptical — "find anything wrong"
not "is this acceptable?" Collaborative framing re-introduces bias via the prompt.

### Why session boundaries work where prompt tricks don't

The priming is in the *context window*, not just the final prompt. A separate
session breaks the causal chain completely: the reviewer cannot rationalise
around intent it never witnessed. This is the same logic that makes human
pair-review effective — the reviewer who didn't write the code sees what is
actually there.

### Practical notes

- **Trivial in Rust Omega:** sessions are independent by design. `omega review
  --snapshot=...` spawns a fresh agent with a review-focused system prompt.
- **Multi-model review:** code with Sonnet, review with Opus (or vice versa).
  Different model perspectives reduce shared blind spots.
- **Auditable:** the review session writes its own `events.jsonl`; you can
  read exactly why a diff was accepted or rejected and tune accordingly.
- **Cheap:** ~$0.05 per review session (short context, heavy system-prompt
  cache reuse across reviews).

### What separate sessions still don't fix

The **single-snapshot-looks-plausible** failure mode is unchanged. A streaming
indicator stuck on "complete" looks correct at any single rendered moment. For
these cases, explicit property-based assertions in the test are the primary
safety net. The snapshot reviewer is a second line of defence, not a substitute.

**Realistic expectation:** auto-accept ~65–70% of diffs (renamed classes, added
aria-labels, reordered attributes) without human review. Flag the rest. The
combination of explicit assertions + separate-session review covers both
semantic correctness and change-regression detection.

### Build order

Build the review machinery during Phase 1, exercised against `events.jsonl`
console snapshots (simpler text artifact, same pattern). By Phase 3 when Leptos
HTML snapshots appear, the separate-session review flow is already battle-tested.
