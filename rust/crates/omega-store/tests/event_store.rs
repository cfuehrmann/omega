//! Integration tests for `EventStore` — real file I/O via temp directories.

use omega_store::EventStore;
use omega_types::{
    OmegaEvent,
    events::{TurnEndEvent, TurnMetrics, UserMessageEvent},
};

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

// ---------------------------------------------------------------------------
// read_all tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_all_returns_empty_vec_when_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no_such_file.jsonl");
    let store = EventStore::new(path);
    #[allow(clippy::unwrap_used)]
    let events = store.read_all().await.unwrap();
    assert!(events.is_empty(), "missing file must return empty Vec");
}

#[tokio::test]
async fn read_all_propagates_non_notfound_io_error() {
    let dir = tempfile::tempdir().unwrap();
    // Placing a *directory* at the path produces an IsADirectory I/O error
    // (not NotFound) when read_to_string is called — must propagate as Err.
    let path = dir.path().join("events.jsonl");
    std::fs::create_dir(&path).unwrap();
    let store = EventStore::new(path);
    let result = store.read_all().await;
    assert!(
        result.is_err(),
        "non-NotFound I/O error must propagate as Err, not return empty Vec"
    );
}

#[tokio::test]
async fn read_all_returns_empty_vec_for_empty_file() {
    let (_guard, path) = temp_events_file();
    // Create the file but write nothing.
    std::fs::write(&path, b"").unwrap();
    let store = EventStore::new(path);
    #[allow(clippy::unwrap_used)]
    let events = store.read_all().await.unwrap();
    assert!(events.is_empty(), "empty file must return empty Vec");
}

#[tokio::test]
async fn read_all_parses_all_valid_lines() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());
    let ev1 = user_message_event("first");
    let ev2 = turn_end_event();
    #[allow(clippy::unwrap_used)]
    {
        store.append(&ev1).await.unwrap();
        store.append(&ev2).await.unwrap();
    }

    #[allow(clippy::unwrap_used)]
    let values = store.read_all().await.unwrap();
    assert_eq!(values.len(), 2, "must return one Value per event");
    assert_eq!(values[0]["type"], "user_message");
    assert_eq!(values[1]["type"], "turn_end");
}

#[tokio::test]
async fn read_all_preserves_insertion_order() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());
    // Three distinct events appended in order.
    let ev1 = user_message_event("a");
    let ev2 = turn_end_event();
    let ev3 = user_message_event("b");
    #[allow(clippy::unwrap_used)]
    {
        store.append(&ev1).await.unwrap();
        store.append(&ev2).await.unwrap();
        store.append(&ev3).await.unwrap();
    }

    #[allow(clippy::unwrap_used)]
    let values = store.read_all().await.unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(values[0]["type"], "user_message");
    assert_eq!(values[0]["content"], "a");
    assert_eq!(values[1]["type"], "turn_end");
    assert_eq!(values[2]["type"], "user_message");
    assert_eq!(values[2]["content"], "b");
}

#[tokio::test]
async fn read_all_skips_malformed_lines_and_returns_rest() {
    let (_guard, path) = temp_events_file();
    // Write two valid lines with a malformed one sandwiched between them.
    let content = "{\"type\":\"user_message\",\"time\":\"t\",\"content\":\"a\"}\nnot-json\n{\"type\":\"turn_end\",\"time\":\"t\",\"metrics\":{\"inputTokens\":1,\"outputTokens\":1}}\n";
    std::fs::write(&path, content).unwrap();
    let store = EventStore::new(path);
    #[allow(clippy::unwrap_used)]
    let values = store.read_all().await.unwrap();
    // Malformed line is silently skipped; 2 valid lines remain.
    assert_eq!(values.len(), 2, "malformed line must be skipped silently");
    assert_eq!(values[0]["type"], "user_message");
    assert_eq!(values[1]["type"], "turn_end");
}

#[tokio::test]
async fn read_all_skips_blank_lines() {
    let (_guard, path) = temp_events_file();
    // File with leading/trailing blank lines and one between events.
    let content = "\n{\"type\":\"user_message\",\"time\":\"t\",\"content\":\"x\"}\n\n";
    std::fs::write(&path, content).unwrap();
    let store = EventStore::new(path);
    #[allow(clippy::unwrap_used)]
    let values = store.read_all().await.unwrap();
    assert_eq!(values.len(), 1, "blank lines must be skipped");
    assert_eq!(values[0]["type"], "user_message");
}

#[tokio::test]
async fn read_all_round_trips_written_events() {
    let (_guard, path) = temp_events_file();
    let store = EventStore::new(path.clone());
    let event = user_message_event("round-trip");
    #[allow(clippy::unwrap_used)]
    store.append(&event).await.unwrap();

    #[allow(clippy::unwrap_used)]
    let values = store.read_all().await.unwrap();
    assert_eq!(values.len(), 1);
    let parsed: OmegaEvent = serde_json::from_value(values[0].clone()).unwrap();
    assert_eq!(parsed, event);
}
