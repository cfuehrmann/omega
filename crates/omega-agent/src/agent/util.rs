// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

use chrono::Utc;
use omega_core::LlmRequest;
use omega_types::events::UsageIteration;
use serde_json::{Map, Value, json};

/// `#[mutants::skip]`: timestamp value (not format) is not asserted by
/// any in-process test — the format is verified indirectly in events.jsonl
/// assertion tests, but the mutation survivors produce wrong *values*,
/// not wrong formats.  CLI/server e2e suites verify real timestamps.
#[mutants::skip]
pub(in crate::agent) fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Extract compaction token counts from a `usage.iterations` slice.
///
/// Returns `(tokens_before, tokens_after, summary_tokens)` where:
/// - `tokens_before` — `input_tokens` of the `compaction` iteration
///   (old context fed to the summariser; the "before" figure).
/// - `tokens_after`  — `input_tokens` of the `message` iteration
///   (new, compacted baseline; the "after" figure).
/// - `summary_tokens` — `output_tokens` of the `compaction` iteration
///   (tokens produced by the summariser).
///
/// Any missing iteration contributes `0` to the respective field.
pub(in crate::agent) fn extract_compaction_tokens(iters: &[UsageIteration]) -> (i64, i64, i64) {
    let compaction = iters.iter().find(|it| it.iteration_type == "compaction");
    let message = iters.iter().find(|it| it.iteration_type == "message");
    (
        compaction.map_or(0, |it| it.input_tokens),
        message.map_or(0, |it| it.input_tokens),
        compaction.map_or(0, |it| it.output_tokens),
    )
}

/// Generate an 8-character lowercase hex string from 4 random bytes.
///
/// Used as the per-tool-call identifier recorded in `events.jsonl` and
/// embedded in tee-log filenames so that the two are bidirectionally
/// cross-referenceable without knowing the LLM provider's ID format.
pub(in crate::agent) fn gen_call_id() -> String {
    let bytes: [u8; 4] = rand::random();
    bytes.iter().fold(String::with_capacity(8), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Build an elided (non-wall-of-text) summary of an [`LlmRequest`] for
/// the `request_summary` field of [`LlmCallEvent`].
///
/// Mirrors `elideAnthropicRequest` in the TypeScript reference
/// (`src/agent.ts`, commits 50622a9 / 5f1e40a).
///
/// * `system`  → `"[N block(s), X chars, cache_control: ephemeral]"`
///   (the last system block always carries the cache marker)
/// * `tools`   → array of `{name, description: "[N chars]", input_schema:
///               "[elided]"}` with `cache_control: "ephemeral"` on the last
///   entry (matches the wire format produced by `build_wire_tools`)
/// * `messages` → `"[N message(s), X chars, cache_control on msg[N-1]]"`
///   (the last content block of the last message always carries the marker)
/// * Top-level scalar fields (`model`, `max_tokens`, `thinking`, …) are
///   forwarded verbatim.
pub(in crate::agent) fn elide_request(req: &LlmRequest) -> Value {
    // ---- system ---------------------------------------------------------
    // The last system block always receives `cache_control: ephemeral`
    // (see `build_system_blocks` in omega-core/src/anthropic.rs).
    let system_val = if let Some(sys) = &req.system {
        let blocks = sys.len();
        let chars: usize = sys.iter().map(|b| b.chars().count()).sum();
        let label = if blocks == 1 { "block" } else { "blocks" };
        Value::String(format!(
            "[{blocks} {label}, {chars} chars, cache_control: ephemeral]"
        ))
    } else {
        Value::Null
    };

    // ---- tools ----------------------------------------------------------
    // The last tool definition always receives `cache_control: ephemeral`
    // (see `build_wire_tools` in omega-core/src/anthropic.rs).
    let last_tool_idx = req.tools.len().saturating_sub(1);
    let tools_val: Vec<Value> = req
        .tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let desc_chars = t.description.chars().count();
            if i == last_tool_idx {
                json!({
                    "name": t.name,
                    "description": format!("[{desc_chars} chars]"),
                    "input_schema": "[elided]",
                    "cache_control": "ephemeral",
                })
            } else {
                json!({
                    "name": t.name,
                    "description": format!("[{desc_chars} chars]"),
                    "input_schema": "[elided]",
                })
            }
        })
        .collect();

    // ---- messages -------------------------------------------------------
    // The last content block of the last message always receives
    // `cache_control: ephemeral` (see `build_wire_messages` in
    // omega-core/src/anthropic.rs).
    let msg_count = req.messages.len();
    let msg_label = if msg_count == 1 {
        "message"
    } else {
        "messages"
    };
    let msg_chars = serde_json::to_string(&req.messages).map_or(0, |s| s.chars().count());
    let cache_note = if msg_count > 0 {
        format!(", cache_control on msg[{}]", msg_count - 1)
    } else {
        String::new()
    };
    let messages_val = Value::String(format!(
        "[{msg_count} {msg_label}, {msg_chars} chars{cache_note}]"
    ));

    // ---- top-level scalars ----------------------------------------------
    let mut map = Map::new();
    map.insert("model".to_owned(), Value::String(req.model.clone()));
    map.insert(
        "max_tokens".to_owned(),
        Value::Number(req.config.max_tokens.into()),
    );
    if let Some(n) = req
        .config
        .temperature
        .and_then(|t| serde_json::Number::from_f64(f64::from(t)))
    {
        map.insert("temperature".to_owned(), Value::Number(n));
    }
    // thinking: adaptive or budget
    if req.config.adaptive_thinking {
        map.insert("thinking".to_owned(), json!({ "type": "adaptive" }));
    } else if let Some(budget) = req.config.thinking_budget {
        map.insert(
            "thinking".to_owned(),
            json!({ "type": "enabled", "budget_tokens": budget }),
        );
    }
    if let Some(effort) = &req.config.effort {
        map.insert("effort".to_owned(), Value::String(effort.clone()));
    }
    if let Some(cm) = &req.context_management {
        map.insert("context_management".to_owned(), cm.clone());
    }
    // elided compound fields
    map.insert("system".to_owned(), system_val);
    if !tools_val.is_empty() {
        map.insert("tools".to_owned(), Value::Array(tools_val));
    }
    map.insert("messages".to_owned(), messages_val);

    Value::Object(map)
}

#[cfg(test)]
mod gen_call_id_tests {
    //! Inline carve-out tests for [`gen_call_id`].
    //!
    //! Justification for carve-out: `gen_call_id` is a private helper whose
    //! output is embedded in `LlmCallEvent.tool_call_id` and tee-log filenames.
    //! Asserting the exact length/alphabet via the e2e surface (`MockProvider`)
    //! would require parsing event payloads from a full agent run, adding
    //! substantial setup for a property that is far simpler to pin inline.
    //! The uniqueness property also relies on randomness, which the e2e surface
    //! cannot control.

    use super::gen_call_id;

    #[test]
    fn gen_call_id_returns_exactly_8_hex_chars() {
        let id = gen_call_id();
        assert_eq!(id.len(), 8, "expected 8 chars, got {id:?}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "non-hex character in {id:?}"
        );
        // lowercase only — `{b:02x}` produces lowercase
        assert_eq!(id, id.to_ascii_lowercase(), "must be lowercase hex: {id:?}");
    }

    #[test]
    fn gen_call_id_successive_calls_differ() {
        // With 4 random bytes per call the probability of collision in two
        // successive calls is 1 / 2^32, which is negligible in CI.
        let a = gen_call_id();
        let b = gen_call_id();
        assert_ne!(
            a, b,
            "two successive gen_call_id() calls produced the same value: {a:?}"
        );
    }
}

#[cfg(test)]
mod elide_request_tests {
    //! Inline carve-out tests for [`elide_request`].
    //!
    //! Justification for carve-out: `elide_request` is a private pure function
    //! whose pluralisation and empty-tools branches are not directly observable
    //! downstream (CLI/server e2e tests don't snapshot
    //! `LlmCall.request_summary`).  These tests pin the branches that survive
    //! `cargo mutants -p omega-agent` otherwise.

    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::elide_request;
    use omega_core::{ContentBlock, LlmRequest, Message, ModelConfig, Role, ToolDefinition};

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
        }
    }

    fn make_request(messages: Vec<Message>, tools: Vec<ToolDefinition>) -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-4-6".to_owned(),
            messages,
            system: Some(vec!["hello".to_owned()]),
            tools,
            config: ModelConfig::default(),
            context_management: None,
        }
    }

    #[test]
    fn singular_message_label() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.starts_with("[1 message,"), "singular: {s}");
        assert!(!s.contains("messages,"), "plural leaked: {s}");
    }

    #[test]
    fn plural_messages_label() {
        let req = make_request(vec![user_msg("a"), user_msg("b")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.starts_with("[2 messages,"), "plural: {s}");
    }

    #[test]
    fn messages_label_includes_cache_control_note() {
        let req = make_request(vec![user_msg("a"), user_msg("b")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.contains("cache_control on msg[1]"), "cache note: {s}");
    }

    #[test]
    fn empty_messages_label_has_no_cache_note() {
        let req = make_request(vec![], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(!s.contains("cache_control"), "unexpected cache note: {s}");
    }

    #[test]
    fn singular_system_block_label() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["system"].as_str().expect("string");
        assert!(s.starts_with("[1 block,"), "singular: {s}");
        assert!(!s.contains("blocks,"), "plural leaked: {s}");
    }

    #[test]
    fn system_label_includes_cache_control() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["system"].as_str().expect("string");
        assert!(s.contains("cache_control: ephemeral"), "cache missing: {s}");
    }

    #[test]
    fn empty_tools_omits_tools_key() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        assert!(
            v.as_object().expect("object").get("tools").is_none(),
            "empty tools must not produce a `tools` key, got {v:?}"
        );
    }

    #[test]
    fn non_empty_tools_includes_tools_key() {
        let tool = ToolDefinition {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "read_file");
        assert_eq!(arr[0]["description"], "[12 chars]");
    }

    #[test]
    fn last_tool_has_cache_control() {
        let tool_a = ToolDefinition {
            name: "tool_a".to_owned(),
            description: "first".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let tool_b = ToolDefinition {
            name: "tool_b".to_owned(),
            description: "second".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool_a, tool_b]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr.len(), 2);
        assert!(
            arr[0].get("cache_control").is_none(),
            "first tool must not have cache_control"
        );
        assert_eq!(
            arr[1]["cache_control"], "ephemeral",
            "last tool must have cache_control"
        );
    }

    #[test]
    fn single_tool_has_cache_control() {
        let tool = ToolDefinition {
            name: "only".to_owned(),
            description: "sole tool".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr[0]["cache_control"], "ephemeral");
    }
}
