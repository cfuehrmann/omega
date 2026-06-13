//! Conversation-shape automaton for model-facing protocol correctness.
//!
//! The context the model sees must always be a *valid* Anthropic
//! conversation: roles alternate, and a `tool_use` is answered by its
//! `tool_result`s in the very next message — with the **exact** set of ids,
//! no missing, no extra, no duplicate.  All three `role:user` sources
//! (human input, monitor deliveries, tool results) must respect that.
//!
//! This module lifts that discipline out of `drive_turn`'s control flow into
//! an explicit transition function.  It is a **register/EFSM**, not a finite
//! automaton: `AwaitingToolResults` carries the pending tool-use ids as an
//! unbounded `BTreeSet` *data* field — that register is exactly the part of
//! the grammar (id-completeness over an unbounded id domain) that a finite
//! automaton cannot express.  Batches never nest, so a set suffices — no
//! stack/CFG machinery is needed.
//!
//! ## Compaction resets to a fresh baseline
//!
//! Server-side context compaction clears the *in-memory* `history` (the
//! compacted view sent to the model) while the on-disk `context.jsonl` keeps
//! the full record.  So an empty in-memory history is a valid baseline that
//! may be followed by either a user turn (a fresh session) **or** an assistant
//! turn (the post-compaction resume — the model answering the server-held
//! summary).  That is why [`ConvState::Empty`] is distinct from
//! [`ConvState::Idle`]: only `Empty` admits an assistant message as the first
//! record.
//!
//! These are pure functions, unit-tested directly: an agent-level test per
//! illegal transition would be disproportionate setup for a total function
//! over a tiny domain (a deliberate carve-out from the end-to-end testing
//! norm — the legal happy path *is* covered end-to-end, because every
//! `append_record` in the real loop now routes through `next_state`).

use std::collections::BTreeSet;

use omega_core::{ContentBlock, Message, Role};

/// State of the conversation automaton, derived purely from the context tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConvState {
    /// Empty in-memory history: a fresh session, or the baseline left by a
    /// context compaction.  Admits either a user turn or — uniquely — an
    /// assistant turn (the post-compaction resume).
    Empty,
    /// A turn ended: the last record is an assistant message with no pending
    /// tool calls.  The next record must start a new user turn.
    Idle,
    /// A `role:user` record was the last appended; the model owes the next
    /// assistant message.
    AwaitingAssistant,
    /// The last assistant message emitted tool calls; the harness owes
    /// exactly these tool-result ids before the turn may proceed.
    AwaitingToolResults { pending: BTreeSet<String> },
}

impl ConvState {
    /// Stable discriminator for forensics (`ConversationInvariantViolatedEvent.state`).
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ConvState::Empty => "empty",
            ConvState::Idle => "idle",
            ConvState::AwaitingAssistant => "awaiting_assistant",
            ConvState::AwaitingToolResults { .. } => "awaiting_tool_results",
        }
    }

    /// Pending tool-use ids (empty unless `AwaitingToolResults`).
    pub(crate) fn pending_ids(&self) -> Vec<String> {
        match self {
            ConvState::AwaitingToolResults { pending } => pending.iter().cloned().collect(),
            _ => Vec::new(),
        }
    }
}

/// A classified append, derived from a record's role and content blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Move {
    /// A `role:user` record with no tool-result blocks (human / monitor / text).
    UserInput,
    /// An assistant message with no tool-use blocks (text / thinking only).
    AssistantPlain,
    /// An assistant message emitting these tool-use ids.
    AssistantToolUse { ids: BTreeSet<String> },
    /// A `role:user` record answering these tool-use ids.
    ToolResults { ids: BTreeSet<String> },
}

impl Move {
    /// Stable discriminator for forensics (`ConversationInvariantViolatedEvent.attemptedMove`).
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Move::UserInput => "user_input",
            Move::AssistantPlain => "assistant_plain",
            Move::AssistantToolUse { .. } => "assistant_tool_use",
            Move::ToolResults { .. } => "tool_results",
        }
    }

    /// Tool-use ids carried by the move (empty for `UserInput` / `AssistantPlain`).
    pub(crate) fn ids(&self) -> Vec<String> {
        match self {
            Move::AssistantToolUse { ids } | Move::ToolResults { ids } => {
                ids.iter().cloned().collect()
            }
            _ => Vec::new(),
        }
    }
}

/// A rejected transition — the data needed for a forensic tombstone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Violation {
    /// Human-readable name of the violated rule (the rejected δ arm).
    pub rule: String,
    /// Ids pending but not answered by the attempted move.
    pub missing: Vec<String>,
    /// Ids answered by the attempted move that were never pending.
    pub extra: Vec<String>,
}

impl Violation {
    fn rule(rule: &str) -> Self {
        Violation {
            rule: rule.to_owned(),
            missing: Vec::new(),
            extra: Vec::new(),
        }
    }
}

/// Derive the conversation state from the context tail.
///
/// State is a pure projection of `history` (the canonical record), so it can
/// never drift from it and is reconstructed for free on resume.
pub(crate) fn conv_state(history: &[Message]) -> ConvState {
    match history.last() {
        None => ConvState::Empty,
        Some(msg) => match msg.role {
            Role::Assistant => {
                let pending = tool_use_ids(&msg.content);
                if pending.is_empty() {
                    ConvState::Idle
                } else {
                    ConvState::AwaitingToolResults { pending }
                }
            }
            Role::User => ConvState::AwaitingAssistant,
        },
    }
}

/// Classify an append into a [`Move`] from its role and content blocks.
pub(crate) fn classify_move(role: Role, blocks: &[ContentBlock]) -> Move {
    match role {
        Role::Assistant => {
            let ids = tool_use_ids(blocks);
            if ids.is_empty() {
                Move::AssistantPlain
            } else {
                Move::AssistantToolUse { ids }
            }
        }
        Role::User => {
            let ids = tool_result_ids(blocks);
            if ids.is_empty() {
                Move::UserInput
            } else {
                Move::ToolResults { ids }
            }
        }
    }
}

/// The transition function δ. `Ok` is the next state; `Err` is a [`Violation`].
///
/// The strict id bijection — the non-regular, register-machine part — lives in
/// the `(AwaitingToolResults, ToolResults)` arm: the answered ids must equal
/// the pending ids *exactly*.
pub(crate) fn next_state(state: &ConvState, mv: &Move) -> Result<ConvState, Violation> {
    use ConvState::{AwaitingAssistant, AwaitingToolResults, Empty, Idle};
    use Move::{AssistantPlain, AssistantToolUse, ToolResults, UserInput};

    match (state, mv) {
        // Start or continue a user turn (consecutive user records merge in
        // projection — this is the Seam-A / Seam-B drain).
        (Empty | Idle | AwaitingAssistant, UserInput) => Ok(AwaitingAssistant),
        // Assistant answers the user turn — or resumes after a compaction
        // reset (the `Empty` case).
        (Empty | AwaitingAssistant, AssistantPlain) => Ok(Idle),
        (Empty | AwaitingAssistant, AssistantToolUse { ids }) => Ok(AwaitingToolResults {
            pending: ids.clone(),
        }),
        // The rigid tool_use → tool_result pair, with the exact-bijection check.
        (AwaitingToolResults { pending }, ToolResults { ids }) => {
            if pending == ids {
                Ok(AwaitingAssistant)
            } else {
                Err(Violation {
                    rule: "tool_results ids must equal pending exactly".to_owned(),
                    missing: pending.difference(ids).cloned().collect(),
                    extra: ids.difference(pending).cloned().collect(),
                })
            }
        }
        // Injection mid-pair — the invariant the seam discipline exists to prevent.
        (AwaitingToolResults { .. }, UserInput) => Err(Violation::rule(
            "user input injected while tool results are pending (no injection mid-pair)",
        )),
        (AwaitingToolResults { .. }, AssistantPlain | AssistantToolUse { .. }) => Err(
            Violation::rule("assistant appended while tool results are pending"),
        ),
        // Assistant after a turn already ended (would be two assistants in a row).
        (Idle, AssistantPlain | AssistantToolUse { .. }) => Err(Violation::rule(
            "assistant appended after a turn already ended (two assistants in a row)",
        )),
        // Tool results with nothing pending to answer.
        (Empty | Idle | AwaitingAssistant, ToolResults { .. }) => Err(Violation::rule(
            "tool_results appended with no pending tool_use",
        )),
    }
}

/// Summarise the last `n` context records as `"role: blockKinds"` lines for
/// the forensic tombstone.
pub(crate) fn history_tail_summary(history: &[Message], n: usize) -> Vec<String> {
    let start = history.len().saturating_sub(n);
    history[start..]
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let kinds: Vec<&str> = m.content.iter().map(block_kind).collect();
            format!("{role}: {}", kinds.join(","))
        })
        .collect()
}

fn block_kind(b: &ContentBlock) -> &'static str {
    match b {
        ContentBlock::Text { .. } => "text",
        ContentBlock::Thinking { .. } => "thinking",
        ContentBlock::ToolUse { .. } => "tool_use",
        ContentBlock::ToolResult { .. } => "tool_result",
    }
}

fn tool_use_ids(blocks: &[ContentBlock]) -> BTreeSet<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(blocks: &[ContentBlock]) -> BTreeSet<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text { text: s.to_owned() }
    }
    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_owned(),
            name: "t".to_owned(),
            input: serde_json::Value::Null,
        }
    }
    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_owned(),
            content: "ok".to_owned(),
            is_error: false,
        }
    }
    fn user(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::User,
            content: blocks,
        }
    }
    fn assistant(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::Assistant,
            content: blocks,
        }
    }
    fn ids(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }
    fn awaiting(items: &[&str]) -> ConvState {
        ConvState::AwaitingToolResults {
            pending: ids(items),
        }
    }

    // --- conv_state -------------------------------------------------------

    #[test]
    fn empty_history_is_empty_state() {
        assert_eq!(conv_state(&[]), ConvState::Empty);
    }

    #[test]
    fn last_user_record_awaits_assistant() {
        assert_eq!(
            conv_state(&[user(vec![text("hi")])]),
            ConvState::AwaitingAssistant
        );
    }

    #[test]
    fn last_plain_assistant_is_idle() {
        assert_eq!(
            conv_state(&[assistant(vec![text("done")])]),
            ConvState::Idle
        );
    }

    #[test]
    fn last_assistant_tool_use_awaits_exactly_those_ids() {
        assert_eq!(
            conv_state(&[assistant(vec![
                tool_use("a"),
                text("thinking"),
                tool_use("b")
            ])]),
            awaiting(&["a", "b"]),
        );
    }

    #[test]
    fn conv_state_reads_only_the_last_record() {
        // An earlier tool_use that was already answered must not leak in.
        let history = vec![
            assistant(vec![tool_use("old")]),
            user(vec![tool_result("old")]),
        ];
        assert_eq!(conv_state(&history), ConvState::AwaitingAssistant);
    }

    // --- classify_move ----------------------------------------------------

    #[test]
    fn classify_user_text_is_user_input() {
        assert_eq!(classify_move(Role::User, &[text("hi")]), Move::UserInput);
    }

    #[test]
    fn classify_user_tool_results() {
        assert_eq!(
            classify_move(Role::User, &[tool_result("a"), tool_result("b")]),
            Move::ToolResults {
                ids: ids(&["a", "b"])
            },
        );
    }

    #[test]
    fn classify_assistant_text_is_plain() {
        assert_eq!(
            classify_move(Role::Assistant, &[text("x")]),
            Move::AssistantPlain
        );
    }

    #[test]
    fn classify_assistant_tool_use() {
        assert_eq!(
            classify_move(Role::Assistant, &[tool_use("a")]),
            Move::AssistantToolUse { ids: ids(&["a"]) },
        );
    }

    // --- next_state: legal transitions -----------------------------------

    #[test]
    fn idle_then_user_input_awaits_assistant() {
        assert_eq!(
            next_state(&ConvState::Idle, &Move::UserInput),
            Ok(ConvState::AwaitingAssistant)
        );
    }

    // --- compaction reset: Empty admits a user OR an assistant turn -------

    #[test]
    fn empty_then_user_input_starts_a_session() {
        assert_eq!(
            next_state(&ConvState::Empty, &Move::UserInput),
            Ok(ConvState::AwaitingAssistant)
        );
    }

    #[test]
    fn empty_then_assistant_is_the_compaction_resume() {
        // After compaction clears in-memory history, the post-compaction
        // assistant message is appended to an empty history — legal.
        assert_eq!(
            next_state(&ConvState::Empty, &Move::AssistantPlain),
            Ok(ConvState::Idle)
        );
        assert_eq!(
            next_state(
                &ConvState::Empty,
                &Move::AssistantToolUse { ids: ids(&["a"]) }
            ),
            Ok(awaiting(&["a"])),
        );
    }

    #[test]
    fn empty_then_tool_results_is_a_violation() {
        assert!(next_state(&ConvState::Empty, &Move::ToolResults { ids: ids(&["a"]) }).is_err());
    }

    #[test]
    fn consecutive_user_input_is_legal() {
        // Seam-A / Seam-B drain: monitor/human input batched after a turn.
        assert_eq!(
            next_state(&ConvState::AwaitingAssistant, &Move::UserInput),
            Ok(ConvState::AwaitingAssistant)
        );
    }

    #[test]
    fn assistant_plain_ends_turn() {
        assert_eq!(
            next_state(&ConvState::AwaitingAssistant, &Move::AssistantPlain),
            Ok(ConvState::Idle)
        );
    }

    #[test]
    fn assistant_tool_use_opens_pending_set() {
        assert_eq!(
            next_state(
                &ConvState::AwaitingAssistant,
                &Move::AssistantToolUse {
                    ids: ids(&["a", "b"])
                },
            ),
            Ok(awaiting(&["a", "b"])),
        );
    }

    #[test]
    fn exact_tool_results_close_the_pair() {
        assert_eq!(
            next_state(
                &awaiting(&["a", "b"]),
                &Move::ToolResults {
                    ids: ids(&["a", "b"])
                }
            ),
            Ok(ConvState::AwaitingAssistant),
        );
    }

    // --- next_state: the strict id bijection (the non-regular part) -------

    #[test]
    fn missing_id_is_a_violation() {
        let v = next_state(
            &awaiting(&["a", "b"]),
            &Move::ToolResults { ids: ids(&["a"]) },
        )
        .unwrap_err();
        assert_eq!(v.missing, vec!["b".to_owned()]);
        assert!(v.extra.is_empty());
        assert_eq!(v.rule, "tool_results ids must equal pending exactly");
    }

    #[test]
    fn extra_id_is_a_violation() {
        let v = next_state(
            &awaiting(&["a"]),
            &Move::ToolResults {
                ids: ids(&["a", "c"]),
            },
        )
        .unwrap_err();
        assert!(v.missing.is_empty());
        assert_eq!(v.extra, vec!["c".to_owned()]);
    }

    #[test]
    fn swapped_ids_report_both_missing_and_extra() {
        let v = next_state(&awaiting(&["a"]), &Move::ToolResults { ids: ids(&["b"]) }).unwrap_err();
        assert_eq!(v.missing, vec!["a".to_owned()]);
        assert_eq!(v.extra, vec!["b".to_owned()]);
    }

    // --- next_state: illegal sequencing ----------------------------------

    #[test]
    fn user_input_while_pending_is_a_violation() {
        assert!(next_state(&awaiting(&["a"]), &Move::UserInput).is_err());
    }

    #[test]
    fn assistant_while_pending_is_a_violation() {
        assert!(next_state(&awaiting(&["a"]), &Move::AssistantPlain).is_err());
        assert!(
            next_state(
                &awaiting(&["a"]),
                &Move::AssistantToolUse { ids: ids(&["b"]) }
            )
            .is_err()
        );
    }

    #[test]
    fn assistant_without_user_turn_is_a_violation() {
        assert!(next_state(&ConvState::Idle, &Move::AssistantPlain).is_err());
        assert!(
            next_state(
                &ConvState::Idle,
                &Move::AssistantToolUse { ids: ids(&["a"]) }
            )
            .is_err()
        );
    }

    #[test]
    fn tool_results_with_nothing_pending_is_a_violation() {
        assert!(next_state(&ConvState::Idle, &Move::ToolResults { ids: ids(&["a"]) }).is_err());
        assert!(
            next_state(
                &ConvState::AwaitingAssistant,
                &Move::ToolResults { ids: ids(&["a"]) }
            )
            .is_err()
        );
    }

    // --- forensic helpers -------------------------------------------------

    #[test]
    fn labels_and_ids_are_stable_for_forensics() {
        assert_eq!(awaiting(&["a", "b"]).label(), "awaiting_tool_results");
        assert_eq!(ConvState::Empty.label(), "empty");
        assert_eq!(ConvState::Idle.label(), "idle");
        assert_eq!(ConvState::AwaitingAssistant.label(), "awaiting_assistant");
        assert_eq!(awaiting(&["b", "a"]).pending_ids(), vec!["a", "b"]); // sorted
        assert_eq!(Move::UserInput.label(), "user_input");
        assert_eq!(Move::AssistantPlain.label(), "assistant_plain");
        assert_eq!(
            Move::AssistantToolUse { ids: ids(&["a"]) }.label(),
            "assistant_tool_use"
        );
        assert_eq!(
            Move::ToolResults { ids: ids(&["x"]) }.label(),
            "tool_results"
        );
        assert_eq!(
            Move::ToolResults {
                ids: ids(&["y", "x"])
            }
            .ids(),
            vec!["x", "y"]
        );
        assert!(Move::UserInput.ids().is_empty());
    }

    #[test]
    fn history_tail_summary_caps_and_formats() {
        let history = vec![
            user(vec![text("hi")]),
            assistant(vec![tool_use("a")]),
            user(vec![tool_result("a")]),
        ];
        assert_eq!(
            history_tail_summary(&history, 2),
            vec![
                "assistant: tool_use".to_owned(),
                "user: tool_result".to_owned()
            ],
        );
        // n larger than len is saturating, not panicking.
        assert_eq!(history_tail_summary(&history, 10).len(), 3);
        assert_eq!(history_tail_summary(&[], 6), Vec::<String>::new());
    }
}
