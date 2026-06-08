// ---------------------------------------------------------------------------
// SCHEMA-8 Phase 3: per-block streaming accumulators
//
// Each [`Provider`] stream is a sequence of indexed `content_block_start` /
// `..._delta` / `content_block_stop` events.  The agent collects each block
// into its own `BlockSlot` keyed by the API's `index`, then assembles the
// assistant message in index order.  This replaces the legacy flat
// accumulators (`text_buf`, `current_thinking`, `completed_thinking_blocks`,
// `tool_uses`) that grouped by kind and reordered interleaved blocks.
//
// Phase 3 staging:
//   * commit 3a (this commit) introduces the slots in parallel with the flat
//     accumulators; the flat path still wins for context.jsonl assembly so
//     all 6 Phase-0 goldens stay byte-equal.
//   * commit 3e drops the flat path and locks the interleaved-thinking
//     golden.
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;

use omega_types::OmegaEvent;
use omega_types::events::{
    LlmResponseDiscardedEvent, TextBlockEvent, ThinkingBlockEvent, ToolUseBlockEvent,
};
use serde_json::Value;

use super::util::{gen_call_id, now_iso};

/// One in-flight assistant content block, keyed by the provider's
/// `content_block_start.index`.  Variants mirror the three
/// `content_block_start` shapes Anthropic emits.
///
/// `sealed` flips to `true` on the matching `*BlockComplete` signal.  An
/// unsealed slot at the moment a stream is abandoned (`LlmRetry` /
/// `LlmError` / `TurnInterrupted` mid-stream) yields a
/// `partial: true` block event in Phase 3 commit 3d.
#[derive(Debug, Clone)]
pub(in crate::agent) enum BlockSlot {
    Text {
        text: String,
        sealed: bool,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
        sealed: bool,
    },
    ToolUse {
        /// Omega-layer identifier (provider-agnostic), minted on
        /// `ToolUseBlockStart` so the same id flows through the
        /// streaming partial-event path, the sealed `ToolUseBlock`
        /// event, and the downstream `ToolCall` / `ToolResult` events.
        tool_call_id: String,
        /// LLM-issued identifier from the provider's `tool_use` block.
        /// Echoed back verbatim in `ContentBlock::ToolResult.tool_use_id`
        /// (protocol layer).
        tool_use_id: String,
        name: String,
        input: Value,
        sealed: bool,
    },
}

/// Append a text delta to slot `idx`, creating an empty `Text` slot if
/// missing.  Logs and ignores type mismatches (defensive — providers
/// shouldn't send a `Text` delta against a non-text slot).
pub(in crate::agent) fn append_text_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    delta: &str,
) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Text {
        text: String::new(),
        sealed: false,
    });
    if let BlockSlot::Text { text, .. } = slot {
        text.push_str(delta);
    }
    // Type mismatch: provider sent a `Text` delta against a slot already
    // typed as Thinking/ToolUse.  Drop — a provider bug we can't recover
    // from cleanly here.  Phase 3 commit 3e's index-ordered assembly will
    // surface anything that slips through as a context-record discrepancy
    // detected by goldens.
}

/// Append a thinking delta to slot `idx`, creating an empty `Thinking`
/// slot if missing.
pub(in crate::agent) fn append_thinking_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    delta: &str,
) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Thinking {
        thinking: String::new(),
        signature: None,
        sealed: false,
    });
    if let BlockSlot::Thinking { thinking, .. } = slot {
        thinking.push_str(delta);
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Mark a `Text` slot sealed.  Creates an empty `Text` slot if missing
/// (an empty text block is rare but legal — the provider is telling us
/// it's done either way).
///
/// `#[mutants::skip]`: This function mutates a slot stored inside a
/// `BTreeMap` that is private to the streaming accumulation loop.  Its
/// observable effect (setting `sealed = true`) is only detectable
/// through the abandonment-closer path, which requires the streaming
/// signal path to be exercised.  The `MockProvider`-based tests bypass
/// real SSE parsing and never emit raw `Signal::TextBlockComplete`
/// events, so the sealed/unsealed distinction is invisible to them.
/// Covered by the CLI / server end-to-end suites instead.
#[mutants::skip]
pub(in crate::agent) fn seal_text_slot(slots: &mut BTreeMap<usize, BlockSlot>, idx: usize) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Text {
        text: String::new(),
        sealed: false,
    });
    if let BlockSlot::Text { sealed, .. } = slot {
        *sealed = true;
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Mark a `Thinking` slot sealed and record its signature.  Creates an
/// empty `Thinking` slot if missing (rare but legal).
pub(in crate::agent) fn seal_thinking_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    sig: String,
) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Thinking {
        thinking: String::new(),
        signature: None,
        sealed: false,
    });
    if let BlockSlot::Thinking {
        signature, sealed, ..
    } = slot
    {
        *signature = Some(sig);
        *sealed = true;
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Open an unsealed `ToolUse` slot at `idx` on `ToolUseBlockStart`,
/// minting a fresh `tool_call_id` so it's available before any input
/// deltas arrive.  Idempotent on retry: a re-`Start` for the same index
/// gets a fresh `tool_call_id` (correct — different attempt).
///
/// `#[mutants::skip]` on the body: the returned `tool_call_id` is
/// a generated correlation key used internally; tests do not assert
/// its exact value.  The slot-insertion side-effect is exercised only
/// through the real SSE signal path (`Signal::ToolUseBlockStart`),
/// which `MockProvider` bypasses.  Covered by CLI/server e2e suites.
#[mutants::skip]
pub(in crate::agent) fn open_tool_use_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    tool_use_id: String,
    name: String,
) -> String {
    let tool_call_id = gen_call_id();
    slots.insert(
        idx,
        BlockSlot::ToolUse {
            tool_call_id: tool_call_id.clone(),
            tool_use_id,
            name,
            input: Value::Null,
            sealed: false,
        },
    );
    tool_call_id
}

/// Seal a `ToolUse` slot on `ToolUseBlockComplete`, populating `input`.
/// Returns the `tool_call_id` that was minted at open time so the caller
/// can include it in the emitted `ToolUseBlockEvent`.  If the slot is
/// missing (provider bug: Complete without Start), synthesize a fresh
/// `tool_call_id` and insert the slot sealed.
pub(in crate::agent) fn seal_tool_use_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    tool_use_id: String,
    name: String,
    input: Value,
) -> String {
    if let Some(BlockSlot::ToolUse {
        tool_call_id,
        input: i,
        sealed,
        ..
    }) = slots.get_mut(&idx)
    {
        *i = input;
        *sealed = true;
        return tool_call_id.clone();
    }
    let tool_call_id = gen_call_id();
    slots.insert(
        idx,
        BlockSlot::ToolUse {
            tool_call_id: tool_call_id.clone(),
            tool_use_id,
            name,
            input,
            sealed: true,
        },
    );
    tool_call_id
}

/// SCHEMA-8 Phase 3 commit 3d: build the abandonment-closer event
/// sequence for a response stream that was cut short by
/// `LlmRetry` / `LlmError` / `TurnInterrupted` before the provider
/// could surface its terminal `LlmResponse`.
///
/// For each UNSEALED `BlockSlot` left in `slots` (in index order),
/// emit a `partial: true` variant of the corresponding
/// `TextBlock` / `ThinkingBlock` / `ToolUseBlock` event so the
/// consumer has explicit closure for every opened block.  Sealed
/// slots had their final `partial: false` event emitted on their
/// `*BlockComplete` signal and are skipped here to avoid duplicate
/// emission.
///
/// Finally, append the `LlmResponseDiscarded` marker so the
/// consumer knows the response stream was abandoned.  Always
/// emitted when this helper is called, even if `slots` was empty
/// — the caller is expected to gate on `response_started`.
pub(in crate::agent) fn make_abandonment_closers(
    slots: BTreeMap<usize, BlockSlot>,
) -> Vec<OmegaEvent> {
    let mut events: Vec<OmegaEvent> = slots
        .into_values()
        .filter_map(|slot| match slot {
            BlockSlot::Text {
                text,
                sealed: false,
            } if !text.is_empty() => Some(OmegaEvent::TextBlock(TextBlockEvent {
                time: now_iso(),
                text,
                partial: true,
            })),
            BlockSlot::Thinking {
                thinking,
                signature,
                sealed: false,
            } if !thinking.is_empty() => Some(OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
                time: now_iso(),
                thinking,
                signature,
                partial: true,
            })),
            BlockSlot::ToolUse {
                tool_call_id,
                tool_use_id,
                name,
                input,
                sealed: false,
            } => Some(OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
                time: now_iso(),
                tool_call_id,
                tool_use_id,
                name,
                input,
                partial: true,
            })),
            _ => None,
        })
        .collect();
    events.push(OmegaEvent::LlmResponseDiscarded(
        LlmResponseDiscardedEvent { time: now_iso() },
    ));
    events
}

#[cfg(test)]
mod abandonment_closer_tests {
    //! Inline tests pinning [`make_abandonment_closers`]'s emission contract
    //! (SCHEMA-8 Phase 3 commit 3d).
    //!
    //! Justification for carve-out: `make_abandonment_closers` is a private
    //! function exercised at mid-stream retry time.  The integration tests in
    //! `tests/internal.rs` exercise only the streaming-loop wiring around this
    //! helper; the per-slot emission decisions (text/thinking/tool-use empty vs.
    //! non-empty, sealed vs. unsealed) are not observable through
    //! `Agent::send_message` / `MockProvider` without constructing specific
    //! slot maps that the real loop cannot easily produce.
    //!
    //! The integration tests in `tests/internal.rs` exercise only the
    //! streaming-loop wiring around this helper, not the per-slot emission
    //! decisions.  Phase 8 (`cargo mutants -p omega-agent`) flagged seven
    //! survivors that escape the integration tests:
    //!
    //! * the `!text.is_empty()` guard (3 mutants: replace-with-true,
    //!   replace-with-false, `delete !`)
    //! * the `!thinking.is_empty()` guard (3 mutants)
    //! * the `BlockSlot::ToolUse { sealed: false, .. }` match arm (1 mutant
    //!   `delete match arm`)
    //!
    //! The first two groups are real gaps: the existing
    //! `script_mid_stream_retry` golden replays mid-stream retry but its
    //! oracle is `context.jsonl` byte-equality, which says nothing about
    //! the `events.jsonl` partial-block emission decisions.  The third
    //! group is defensive code that the current stream loop can never
    //! reach (`insert_tool_use_slot` always seals on insert) but whose
    //! contract is part of Phase 3 commit 3d's spec — we pin it here so
    //! future changes to the seal discipline can't silently drop it.

    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::wildcard_enum_match_arm
    )]

    use super::{BlockSlot, make_abandonment_closers};
    use omega_types::OmegaEvent;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn expect_discarded(ev: &OmegaEvent) {
        match ev {
            OmegaEvent::LlmResponseDiscarded(_) => {}
            other => panic!("expected LlmResponseDiscarded, got {other:?}"),
        }
    }

    #[test]
    fn empty_slot_map_emits_only_discarded_marker() {
        // The closer pair degrades to a single marker when nothing was
        // accumulated before the abandon.
        let events = make_abandonment_closers(BTreeMap::new());
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_nonempty_text_slot_emits_partial_text_block() {
        // Catches `agent.rs:215:18 !text.is_empty() -> false` and the
        // matching `delete !` mutation: with either mutation the partial
        // TextBlock disappears and only LlmResponseDiscarded would remain.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: "hello world".to_owned(),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::TextBlock(t) => {
                assert_eq!(t.text, "hello world");
                assert!(t.partial, "abandonment TextBlock must be partial");
            }
            other => panic!("expected TextBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn unsealed_empty_text_slot_emits_no_text_block() {
        // Catches `agent.rs:215:18 !text.is_empty() -> true`: with the
        // mutation an empty TextBlock would slip through.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: String::new(),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(
            events.len(),
            1,
            "empty text slot must not emit a TextBlock event"
        );
        expect_discarded(&events[0]);
    }

    #[test]
    fn sealed_text_slot_is_skipped() {
        // A sealed slot has already had its `partial:false` TextBlock
        // emitted on `TextBlockComplete`; the closer must not re-emit.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: "complete".to_owned(),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_nonempty_thinking_slot_emits_partial_thinking_block() {
        // Catches `agent.rs:224:18 !thinking.is_empty() -> false` and
        // the matching `delete !` mutation.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "deep thought".to_owned(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::ThinkingBlock(t) => {
                assert_eq!(t.thinking, "deep thought");
                assert_eq!(t.signature, None);
                assert!(t.partial, "abandonment ThinkingBlock must be partial");
            }
            other => panic!("expected ThinkingBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn unsealed_thinking_slot_preserves_signature_when_present() {
        // The signature can arrive on `signature_delta` before the
        // model stops streaming; abandonment must forward it untouched.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "half-baked".to_owned(),
                signature: Some("sig-xyz".to_owned()),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        match &events[0] {
            OmegaEvent::ThinkingBlock(t) => {
                assert_eq!(t.signature.as_deref(), Some("sig-xyz"));
                assert!(t.partial);
            }
            other => panic!("expected ThinkingBlock, got {other:?}"),
        }
    }

    #[test]
    fn unsealed_empty_thinking_slot_emits_no_thinking_block() {
        // Catches `agent.rs:224:18 !thinking.is_empty() -> true`.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: String::new(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(
            events.len(),
            1,
            "empty thinking slot must not emit a ThinkingBlock event"
        );
        expect_discarded(&events[0]);
    }

    #[test]
    fn sealed_thinking_slot_is_skipped() {
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "complete".to_owned(),
                signature: Some("sig".to_owned()),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_tool_use_slot_emits_partial_tool_use_block() {
        // Catches `agent.rs:230:13 delete match arm BlockSlot::ToolUse { sealed: false, .. }`.
        //
        // The current stream loop never produces an unsealed ToolUse
        // slot (`insert_tool_use_slot` always sets `sealed: true`), but
        // SCHEMA-8 Phase 3 commit 3d's contract still covers this case
        // for forward compatibility (e.g. partial `input_json` arriving
        // at abandonment time in a future schema).  Constructing the
        // slot map directly is the only way to exercise the arm.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tool-id-1".to_owned(),
                name: "calc".to_owned(),
                input: json!({"x": 1}),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::ToolUseBlock(t) => {
                assert_eq!(t.tool_call_id, "tc-1");
                assert_eq!(t.tool_use_id, "tool-id-1");
                assert_eq!(t.name, "calc");
                assert_eq!(t.input, json!({"x": 1}));
                assert!(t.partial, "abandonment ToolUseBlock must be partial");
            }
            other => panic!("expected ToolUseBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn sealed_tool_use_slot_is_skipped() {
        // The normal stream path: ToolUseBlockComplete inserts the slot
        // sealed and emits a `partial:false` ToolUseBlock immediately,
        // so abandonment must not re-emit.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tool-id-1".to_owned(),
                name: "calc".to_owned(),
                input: json!({}),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn mixed_slots_emit_in_block_index_order() {
        // Phase 2's wire-shape invariant: assistant blocks are persisted
        // in API-declared index order.  The closer pair must respect
        // the same order even when slots are inserted out of order.
        let mut slots = BTreeMap::new();
        slots.insert(
            2,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tu".to_owned(),
                name: "n".to_owned(),
                input: json!({}),
                sealed: false,
            },
        );
        slots.insert(
            0,
            BlockSlot::Text {
                text: "t0".to_owned(),
                sealed: false,
            },
        );
        slots.insert(
            1,
            BlockSlot::Thinking {
                thinking: "th1".to_owned(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], OmegaEvent::TextBlock(_)));
        assert!(matches!(events[1], OmegaEvent::ThinkingBlock(_)));
        assert!(matches!(events[2], OmegaEvent::ToolUseBlock(_)));
        expect_discarded(&events[3]);
    }
}
