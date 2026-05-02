//! Integration tests for the Phase 1d.1e pause / continue / abort seam.
//!
//! Each test pushes a deterministic transcript onto `MockProvider`, then
//! drives the resulting `send_message` stream while interleaving
//! `ControlHandle::request_pause` / `request_continue` / `request_abort`
//! calls.  Async streams advance only under `.next().await`, so the
//! tests can step the agent forward one item at a time and inject
//! control-handle calls at deterministic points.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::wildcard_enum_match_arm,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod common;

use std::path::Path;

use common::{
    MockProvider, collect_stream, make_llm_response, make_test_agent, make_tool_use_items, tags,
};
use futures::StreamExt;
use omega_core::{AgentItem, ContentBlock, Message, Role};
use omega_protocol::{ContinueMode, OmegaEvent};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers specific to this file
// ---------------------------------------------------------------------------

/// Drive the stream until an event matching `pred` is yielded, or the
/// stream ends.  Returns every item collected so far, including the
/// matching one (last entry).
async fn drive_until<F>(
    stream: &mut std::pin::Pin<Box<dyn futures::Stream<Item = AgentItem> + Send + '_>>,
    mut pred: F,
) -> Vec<AgentItem>
where
    F: FnMut(&AgentItem) -> bool,
{
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let stop = pred(&item);
        out.push(item);
        if stop {
            return out;
        }
    }
    out
}

fn is_event(item: &AgentItem, want: &str) -> bool {
    matches!(item, AgentItem::Event(_)) && tags(std::slice::from_ref(item)) == vec![want]
}

/// Drive a freshly-constructed stream forward until just before the
/// pause seam fires.  `send_message` is lazy: `reset_for_turn` runs on
/// the first poll, so any pause/continue/abort state set BEFORE the
/// first poll is wiped.  Driving past `ToolCall` guarantees the turn
/// has entered the agentic loop, the tool is about to dispatch, and
/// the caller can now set control state that the seam will observe.
async fn drive_to_pre_seam(
    stream: &mut std::pin::Pin<Box<dyn futures::Stream<Item = AgentItem> + Send + '_>>,
) -> Vec<AgentItem> {
    drive_until(stream, |it| is_event(it, "ToolCall")).await
}

/// Push a transcript that drives one full pause cycle: turn 1 issues a
/// tool_use call, turn 2 (after continue) issues a final text response.
fn arrange_one_cycle_transcript(provider: &MockProvider, tool_path: &Path) {
    provider.push_response(make_tool_use_items(
        "tu_1",
        "read_file",
        json!({ "path": tool_path.to_string_lossy() }),
    ));
    provider.push_response(vec![Ok(make_llm_response(
        "end_turn",
        Some("done"),
        20,
        10,
    ))]);
}

/// Read every persisted event from `events.jsonl`.
fn read_events(path: &Path) -> Vec<OmegaEvent> {
    let raw = std::fs::read_to_string(path).expect("read events.jsonl");
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<OmegaEvent>(l).expect("parse event"))
        .collect()
}

fn count_pause_requested(path: &Path) -> usize {
    read_events(path)
        .iter()
        .filter(|e| matches!(e, OmegaEvent::PauseRequested(_)))
        .count()
}

/// A "scratch" file written into `cwd` so `read_file` succeeds without
/// the test depending on filesystem-error formatting.
fn write_scratch(cwd: &Path) -> std::path::PathBuf {
    let p = cwd.join("scratch.txt");
    std::fs::write(&p, "hello").expect("write scratch");
    p
}

// ---------------------------------------------------------------------------
// Test 1 — manual continue (suspended → manual mode), no interjection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_then_manual_continue_no_interjection() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());

    // Drive past turn entry (where reset_for_turn clears stale state),
    // then arm the pause.  The seam fires when the next iteration
    // appends tool_results.
    let pre_seam = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;

    let pre_pause = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(
        tags(&pre_pause).contains(&"TurnPaused"),
        "expected TurnPaused before suspend, got {:?}",
        tags(&pre_pause)
    );

    // Now suspended.  Issuing continue resumes the seam in manual mode.
    handle.request_continue(None);

    let post: Vec<AgentItem> = collect_stream(stream).await;
    let all_tags: Vec<_> = tags(&pre_seam)
        .into_iter()
        .chain(tags(&pre_pause))
        .chain(tags(&post))
        .collect();

    // Sanity: TurnPaused → TurnContinued, no interjected UserMessage.
    let paused_idx = all_tags.iter().position(|t| *t == "TurnPaused").unwrap();
    let continued_idx = all_tags.iter().position(|t| *t == "TurnContinued").unwrap();
    assert!(paused_idx < continued_idx, "{:?}", all_tags);
    let between = &all_tags[paused_idx + 1..continued_idx];
    assert!(
        !between.contains(&"UserMessage"),
        "no interjected UserMessage expected: {:?}",
        between
    );

    // Mode = Manual.
    let cont = post
        .iter()
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::TurnContinued(c) => Some(c.mode.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(cont, ContinueMode::Manual);

    // Turn ends normally.
    assert!(all_tags.last().copied() == Some("TurnEnd"));
}

// ---------------------------------------------------------------------------
// Test 2 — manual continue with interjection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_then_manual_continue_with_interjection() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let pre_seam = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let pre = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(tags(&pre).contains(&"TurnPaused"));

    handle.request_continue(Some("interject!".into()));
    let post = collect_stream(stream).await;

    // Verify shape: TurnPaused, UserMessage(interjection), TurnContinued{Manual}.
    let combined: Vec<AgentItem> = pre_seam
        .into_iter()
        .chain(pre.into_iter())
        .chain(post.into_iter())
        .collect();
    let combined_tags = tags(&combined);
    let paused_idx = combined_tags
        .iter()
        .position(|t| *t == "TurnPaused")
        .unwrap();
    let continued_idx = combined_tags
        .iter()
        .position(|t| *t == "TurnContinued")
        .unwrap();
    let between = &combined_tags[paused_idx + 1..continued_idx];
    assert_eq!(between, &["UserMessage"]);

    // Interjection content lands as a UserMessage.
    let user_msg_text = combined
        .iter()
        .skip(paused_idx + 1)
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::UserMessage(u) => Some(u.content.clone()),
                _ => None,
            },
            _ => None,
        });
    assert_eq!(user_msg_text.as_deref(), Some("interject!"));

    // Mode = Manual.
    let cont = combined
        .iter()
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::TurnContinued(c) => Some(c.mode.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(cont, ContinueMode::Manual);

    // The next LLM call sees the interjection in messages.
    let requests = provider.take_requests();
    assert_eq!(requests.len(), 2, "two LLM calls expected");
    let second = &requests[1];
    let last_user_msg = second
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .expect("at least one user msg");
    let saw_interjection = last_user_msg.content.iter().any(|b| match b {
        ContentBlock::Text { text } => text == "interject!",
        _ => false,
    });
    assert!(
        saw_interjection,
        "interjection should be in second LLM call's messages: {:?}",
        last_user_msg
    );
}

// ---------------------------------------------------------------------------
// Test 3 — pre-commit continue (auto mode), no interjection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pre_commit_continue_no_interjection() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let pre_seam = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    handle.request_continue(None); // beats the seam — mode=Auto.

    let post = collect_stream(stream).await;
    let items: Vec<AgentItem> = pre_seam.into_iter().chain(post.into_iter()).collect();
    let t = tags(&items);

    // Both TurnPaused and TurnContinued must appear; no UserMessage between
    // (other than the initial one before LlmCall).
    let paused_idx = t.iter().position(|x| *x == "TurnPaused").unwrap();
    let continued_idx = t.iter().position(|x| *x == "TurnContinued").unwrap();
    assert!(paused_idx < continued_idx);
    let between = &t[paused_idx + 1..continued_idx];
    assert!(
        !between.contains(&"UserMessage"),
        "no interjection: {:?}",
        between
    );

    let mode = items
        .iter()
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::TurnContinued(c) => Some(c.mode.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(mode, ContinueMode::Auto);
}

// ---------------------------------------------------------------------------
// Test 4 — pre-commit continue with interjection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pre_commit_continue_with_interjection() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let pre_seam = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    handle.request_continue(Some("inject".into()));

    let post = collect_stream(stream).await;
    let items: Vec<AgentItem> = pre_seam.into_iter().chain(post.into_iter()).collect();
    let t = tags(&items);

    let paused_idx = t.iter().position(|x| *x == "TurnPaused").unwrap();
    let continued_idx = t.iter().position(|x| *x == "TurnContinued").unwrap();
    let between = &t[paused_idx + 1..continued_idx];
    assert_eq!(between, &["UserMessage"]);

    let mode = items
        .iter()
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::TurnContinued(c) => Some(c.mode.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(mode, ContinueMode::Auto);
}

// ---------------------------------------------------------------------------
// Test 5 — abort during pause emits TurnInterrupted{Aborted}.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_then_abort_emits_turn_interrupted() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    // Only need one LLM call — the abort kills the turn before the second.
    provider.push_response(make_tool_use_items(
        "tu_1",
        "read_file",
        json!({ "path": scratch.to_string_lossy() }),
    ));

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let _ = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let pre = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(tags(&pre).contains(&"TurnPaused"));

    handle.request_abort();

    let post = collect_stream(stream).await;
    let all_tags: Vec<_> = tags(&pre).into_iter().chain(tags(&post)).collect();
    assert!(all_tags.contains(&"TurnInterrupted"), "{:?}", all_tags);
    assert!(!all_tags.contains(&"TurnContinued"), "{:?}", all_tags);
}

// ---------------------------------------------------------------------------
// Test 6 — request_pause is idempotent when already pending.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn idempotent_request_pause_already_pending() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let _ = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    handle.request_pause().await; // no-op — already pending
    handle.request_continue(None);
    let _ = collect_stream(stream).await;

    let events_path = tmp.path().join("events.jsonl");
    assert_eq!(count_pause_requested(&events_path), 1);
}

// ---------------------------------------------------------------------------
// Test 7 — request_pause logs even when no turn is running.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_pause_logs_with_no_turn_running() {
    let (agent, _provider, tmp) = make_test_agent();
    let handle = agent.controls();
    handle.request_pause().await;
    drop(agent);

    let events_path = tmp.path().join("events.jsonl");
    assert_eq!(count_pause_requested(&events_path), 1);
}

// ---------------------------------------------------------------------------
// Test 8 — pause state resets between turns.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_state_resets_between_turns() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Turn 1: text-only response → no tool_use → no seam fires.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("hi"), 5, 5))]);

    let handle = agent.controls();
    handle.request_pause().await; // set, but never observed by a seam.

    let stream = agent.send_message("first".into(), CancellationToken::new());
    let t1 = tags(&collect_stream(stream).await);
    assert!(!t1.contains(&"TurnPaused"), "no seam this turn: {:?}", t1);
    assert!(t1.contains(&"TurnEnd"));

    // Turn 2: tool_use + final text. Seam must NOT pause (state was
    // reset by the TurnGuard at the end of turn 1).
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);
    let stream2 = agent.send_message("second".into(), CancellationToken::new());
    let t2 = tags(&collect_stream(stream2).await);
    assert!(!t2.contains(&"TurnPaused"), "{:?}", t2);
    assert!(t2.contains(&"TurnEnd"));
}

// ---------------------------------------------------------------------------
// Test 9 — drop-guard releases a suspended seam.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_guard_releases_suspended_seam() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("first".into(), CancellationToken::new());
    let _ = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let pre = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(
        tags(&pre).contains(&"TurnPaused"),
        "must reach suspended seam: {:?}",
        tags(&pre)
    );

    // Drop the stream while the seam is suspended.  TurnGuard::drop
    // must clear state so subsequent turns work.
    drop(stream);

    // Drain anything left in the provider queue (the second response
    // was never consumed) and push fresh transcripts for a new turn.
    let _ = provider.take_requests();
    provider.responses.lock().unwrap().clear();
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 5, 5))]);

    let stream2 = agent.send_message("again".into(), CancellationToken::new());
    let t = tags(&collect_stream(stream2).await);
    assert!(t.contains(&"TurnEnd"));
    assert!(!t.contains(&"TurnPaused"));
}

// ---------------------------------------------------------------------------
// Test 10 — events.jsonl preserves the full pause/continue ordering.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_jsonl_pause_continue_ordering() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let _ = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let _ = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    handle.request_continue(Some("hello again".into()));
    let _ = collect_stream(stream).await;

    let events = read_events(&tmp.path().join("events.jsonl"));
    let names: Vec<&'static str> = events
        .iter()
        .map(|e| match e {
            OmegaEvent::PauseRequested(_) => "PauseRequested",
            OmegaEvent::UserMessage(_) => "UserMessage",
            OmegaEvent::LlmCall(_) => "LlmCall",
            OmegaEvent::LlmResponse(_) => "LlmResponse",
            OmegaEvent::ToolCall(_) => "ToolCall",
            OmegaEvent::ToolResult(_) => "ToolResult",
            OmegaEvent::TurnPaused(_) => "TurnPaused",
            OmegaEvent::TurnContinued(_) => "TurnContinued",
            OmegaEvent::TurnEnd(_) => "TurnEnd",
            OmegaEvent::TurnInterrupted(_) => "TurnInterrupted",
            _ => "Other",
        })
        .collect();

    // Expected sequence (loose match — assert relative ordering of the
    // pause/continue block).  PauseRequested precedes everything because
    // it fires before send_message is even called.
    let pr = names.iter().position(|n| *n == "PauseRequested").unwrap();
    let tp = names.iter().position(|n| *n == "TurnPaused").unwrap();
    let user_msgs: Vec<usize> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (*n == "UserMessage").then_some(i))
        .collect();
    let tc = names.iter().position(|n| *n == "TurnContinued").unwrap();

    assert!(pr < tp, "PauseRequested before TurnPaused: {:?}", names);
    assert!(tp < tc, "TurnPaused before TurnContinued: {:?}", names);

    // Two UserMessage events: the initial "hi" and the "hello again"
    // interjection.  The interjection sits between TurnPaused and
    // TurnContinued.
    assert_eq!(user_msgs.len(), 2, "{:?}", names);
    assert!(user_msgs[0] < tp);
    assert!(tp < user_msgs[1] && user_msgs[1] < tc);

    assert!(names.last() == Some(&"TurnEnd"));
}

// ---------------------------------------------------------------------------
// Test 11 — multiple pause cycles in one turn.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_pause_cycles_in_one_turn() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    // Three LLM calls: tool_use, tool_use, end_turn.
    provider.push_response(make_tool_use_items(
        "tu_1",
        "read_file",
        json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push_response(make_tool_use_items(
        "tu_2",
        "read_file",
        json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("done"), 5, 5))]);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());

    // Cycle 1: drive past first ToolCall, arm pause, await TurnPaused.
    let pre_seam1 = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let pre1 = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(tags(&pre1).contains(&"TurnPaused"));
    handle.request_continue(None);

    // Cycle 2: drive past the SECOND ToolCall, arm pause again.
    let between = drive_until(&mut stream, |it| is_event(it, "ToolCall")).await;
    handle.request_pause().await;
    let pre2 = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;
    assert!(tags(&pre2).contains(&"TurnPaused"));
    handle.request_continue(None);

    let post = collect_stream(stream).await;
    let combined_tags: Vec<_> = tags(&pre_seam1)
        .into_iter()
        .chain(tags(&pre1))
        .chain(tags(&between))
        .chain(tags(&pre2))
        .chain(tags(&post))
        .collect();

    let paused_count = combined_tags.iter().filter(|t| **t == "TurnPaused").count();
    let continued_count = combined_tags
        .iter()
        .filter(|t| **t == "TurnContinued")
        .count();
    assert_eq!(paused_count, 2, "{:?}", combined_tags);
    assert_eq!(continued_count, 2, "{:?}", combined_tags);
    assert_eq!(combined_tags.last().copied(), Some("TurnEnd"));
}

// ---------------------------------------------------------------------------
// Test 12 — request_abort during tool dispatch, no prior pause.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_abort_during_tool_dispatch_no_prior_pause() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    // Drive past turn entry (which would otherwise reset our cancel),
    // then abort — the next cancel check at loop top yields
    // TurnInterrupted{Aborted}.
    let pre_seam = drive_to_pre_seam(&mut stream).await;
    handle.request_abort();

    let post = collect_stream(stream).await;
    let items: Vec<AgentItem> = pre_seam.into_iter().chain(post.into_iter()).collect();
    let t = tags(&items);
    assert!(t.contains(&"TurnInterrupted"), "{:?}", t);

    // Confirm the reason is Aborted.
    let reason = items
        .iter()
        .find_map(|it| match it {
            AgentItem::Event(b) => match b.as_ref() {
                OmegaEvent::TurnInterrupted(ti) => Some(ti.reason.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(reason, Some(omega_protocol::InterruptReason::Aborted));
}

// ---------------------------------------------------------------------------
// Test 13 — request_continue without a prior pause is inert.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_continue_without_prior_pause_is_inert() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("hi"), 5, 5))]);

    let handle = agent.controls();
    handle.request_continue(Some("noise".into()));

    let stream = agent.send_message("hi".into(), CancellationToken::new());
    let items = collect_stream(stream).await;
    let t = tags(&items);

    assert!(!t.contains(&"TurnPaused"), "{:?}", t);
    assert!(!t.contains(&"TurnContinued"), "{:?}", t);
    assert!(t.contains(&"TurnEnd"), "{:?}", t);
}

// ---------------------------------------------------------------------------
// Test 14 — request_pause while suspended is idempotent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_pause_while_suspended_is_idempotent() {
    let (mut agent, provider, tmp) = make_test_agent();
    let scratch = write_scratch(tmp.path());
    arrange_one_cycle_transcript(&provider, &scratch);

    let handle = agent.controls();
    let mut stream = agent.send_message("hi".into(), CancellationToken::new());
    let _ = drive_to_pre_seam(&mut stream).await;
    handle.request_pause().await;
    let _ = drive_until(&mut stream, |it| is_event(it, "TurnPaused")).await;

    // Now suspended.  A second request_pause must be a no-op.
    handle.request_pause().await;

    handle.request_continue(None);
    let _ = collect_stream(stream).await;

    let events_path = tmp.path().join("events.jsonl");
    assert_eq!(count_pause_requested(&events_path), 1);
}

// ---------------------------------------------------------------------------
// Sanity: keep an unused symbol referenced so the import isn't pruned
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _ref_unused(_p: Arc<MockProvider>, _m: Message) {}
