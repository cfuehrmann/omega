//! Integration tests for `EventStore` — real file I/O via temp directories.

use omega_protocol::{
    OmegaEvent,
    events::{TurnEndEvent, TurnMetrics, UserMessageEvent},
};
use omega_store::EventStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_events_file() -> (tempfile::TempDir, std::path::PathBuf) {
    #[allow(clippy::unwrap_used)]
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    (dir, path)
}

fn user_message_event(content: &str) -> OmegaEvent {
    OmegaEvent::UserMessage(UserMessageEvent {
        time: "2025-07-04T14-32-05.000Z".into(),
        content: content.to_owned(),
    })
}

fn turn_end_event() -> OmegaEvent {
    OmegaEvent::TurnEnd(TurnEndEvent {
        time: "2025-07-04T14-32-06.000Z".into(),
        metrics: TurnMetrics {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn append_writes_json_line() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());

    let event = user_message_event("hello");
    #[allow(clippy::unwrap_used)]
    store.append(&event).await.unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        !content.is_empty(),
        "events.jsonl should not be empty after append"
    );
    // Must end with a newline (JSONL format).
    assert!(content.ends_with('\n'), "JSONL line must end with newline");
}

#[tokio::test]
async fn append_writes_valid_json_that_round_trips() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());

    let event = user_message_event("round-trip content");
    #[allow(clippy::unwrap_used)]
    store.append(&event).await.unwrap();

    let line = std::fs::read_to_string(&path).unwrap();
    let parsed: OmegaEvent = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(parsed, event);
}

#[tokio::test]
async fn append_multiple_events_creates_multiple_lines() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());

    let ev1 = user_message_event("first");
    let ev2 = turn_end_event();

    #[allow(clippy::unwrap_used)]
    {
        store.append(&ev1).await.unwrap();
        store.append(&ev2).await.unwrap();
    }

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "expected exactly 2 lines");

    // Each line must round-trip as an OmegaEvent.
    let parsed1: OmegaEvent = serde_json::from_str(lines[0]).unwrap();
    let parsed2: OmegaEvent = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(parsed1, ev1);
    assert_eq!(parsed2, ev2);
}

#[tokio::test]
async fn append_creates_parent_dirs_if_needed() {
    let root = tempfile::tempdir().unwrap();
    // Deep path that does not exist yet.
    let path = root.path().join("a").join("b").join("events.jsonl");
    let store = EventStore::new(path.clone());

    #[allow(clippy::unwrap_used)]
    store.append(&user_message_event("nested")).await.unwrap();

    assert!(
        path.exists(),
        "events.jsonl should have been created inside nested dirs"
    );
}
