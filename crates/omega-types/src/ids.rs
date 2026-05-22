//! Identity primitives for Omega — Phase 1.
//!
//! These types are the canonical schema definition for session and event
//! identity.  Changes here are breaking changes to `events.jsonl`.
//!
//! See `docs/sessionref-design-proposal.html` for the full rationale.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Leaf ID types ────────────────────────────────────────────────────────────

/// Unique identifier for an Omega session.  Wraps a UUID v7.
///
/// Newtype — prevents accidental mixing with [`EventId`] at compile time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

/// Unique identifier for a single event within any session.  Wraps a UUID v7.
///
/// Newtype — prevents accidental mixing with [`SessionId`] at compile time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub Uuid);

// ── SessionRef ───────────────────────────────────────────────────────────────

/// A reference to an Omega session, optionally pinned to a specific event.
///
/// - `event_id: None`    → refers to the session as a whole
/// - `event_id: Some(e)` → refers to a specific moment; `e` is the UUID of
///   an event in `session_id`'s `events.jsonl`.
///
/// # JSON representation
///
/// Session-only:  `{"sessionId":"<uuid>"}`
/// Event-pinned:  `{"sessionId":"<uuid>","eventId":"<uuid>"}`
///
/// # String representation (`Display` / `FromStr`)
///
/// Session-only:  `<session-uuid>`
/// Event-pinned:  `<session-uuid>#<event-uuid>`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<EventId>,
}

impl SessionRef {
    /// Construct a session-only reference (no event pinning).
    #[must_use]
    pub fn session(session_id: SessionId) -> Self {
        Self {
            session_id,
            event_id: None,
        }
    }

    /// Construct an event-pinned reference.
    #[must_use]
    pub fn event(session_id: SessionId, event_id: EventId) -> Self {
        Self {
            session_id,
            event_id: Some(event_id),
        }
    }
}

// ── Display / FromStr ────────────────────────────────────────────────────────

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Display format: `<session-uuid>` or `<session-uuid>#<event-uuid>`.
///
/// Used in tracing output, error messages, and UI chips.
/// Round-trips through [`FromStr`].
impl fmt::Display for SessionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.event_id {
            None => write!(f, "{}", self.session_id),
            Some(eid) => write!(f, "{}#{}", self.session_id, eid),
        }
    }
}

/// Error type returned when [`SessionRef`]'s [`FromStr`] impl rejects its input.
#[derive(Debug, PartialEq)]
pub struct SessionRefParseError;

impl fmt::Display for SessionRefParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid SessionRef: expected <uuid> or <uuid>#<uuid>")
    }
}

impl std::error::Error for SessionRefParseError {}

/// Parse `<session-uuid>` or `<session-uuid>#<event-uuid>`.
///
/// Both halves must be well-formed UUIDs; anything else returns
/// [`SessionRefParseError`].
impl FromStr for SessionRef {
    type Err = SessionRefParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pu = |u: &str| Uuid::parse_str(u).map_err(|_| SessionRefParseError);
        match s.split_once('#') {
            None => Ok(Self::session(SessionId(pu(s)?))),
            Some((sid, eid)) => Ok(Self::event(SessionId(pu(sid)?), EventId(pu(eid)?))),
        }
    }
}

// ── Origin ───────────────────────────────────────────────────────────────────

/// Records how this session came to exist.
///
/// Lives as a field on the session's first [`SessionStartedEvent`](crate::events::SessionStartedEvent)
/// in `events.jsonl`.  Single source of truth; UI display is free via the
/// existing event-rendering path.
///
/// # Serde
///
/// Uses `#[serde(tag = "type")]` — the discriminator lives under the key
/// `"type"` in a nested object on `SessionStartedEvent`.
///
/// `Root` → `{"type":"root"}`
/// `SubagentOf` → `{"type":"subagent_of","parent":{...}}`
///
/// # Extensibility
///
/// `#[non_exhaustive]` ensures that adding `ForkOf` in a future phase produces
/// a compile error in any match outside this crate that lacks a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Origin {
    /// Top-level session: started directly by the user.
    Root,

    /// This session was spawned as a subagent by the given parent session.
    ///
    /// `parent.event_id` is `Some(spawn_event)` — the UUID of the
    /// `SubagentSpawned` event in the parent's `events.jsonl`.  This is
    /// contractually required; a `None` `event_id` is a construction error.
    ///
    /// Use [`Origin::subagent_of`] to construct this variant.
    SubagentOf { parent: SessionRef },
}

impl Origin {
    /// Constructs a [`SubagentOf`](Origin::SubagentOf) variant, enforcing that
    /// the parent ref carries a concrete spawn-event ID.
    ///
    /// # Panics
    ///
    /// Panics if `parent.event_id` is `None`.  Callers must supply the
    /// spawn event ID from the parent session.
    #[must_use]
    pub fn subagent_of(parent: SessionRef) -> Self {
        assert!(
            parent.event_id.is_some(),
            "SubagentOf requires a concrete spawn-event ID in parent.event_id"
        );
        Self::SubagentOf { parent }
    }
}

// ── LoggedEvent envelope ─────────────────────────────────────────────────────

/// The on-disk unit stored in `events.jsonl`.
///
/// Every event appended since Phase 1 is wrapped in this envelope.  The
/// inner [`OmegaEvent`](crate::events::OmegaEvent) is flattened into the same
/// JSON object so that the `"type"` discriminator remains a top-level key.
///
/// # Old-log compatibility
///
/// Lines written before Phase 1 lack an `"eventId"` field.  They deserialise
/// with `event_id: None`.  This is the **only** intentional serde default in
/// this module — old events genuinely have no ID.  Do not add more defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggedEvent {
    /// Stable identity for this event.  `None` only for pre-Phase-1 log lines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<EventId>,

    /// The event payload.  Flattened so `"type"` remains a top-level key.
    #[serde(flatten)]
    pub event: crate::events::OmegaEvent,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Unit tests live here rather than at the crate level because
    // Display/FromStr/Origin::subagent_of are pure functions where the
    // crate-level test setup (agent, provider, etc.) would be
    // disproportionate overhead.
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    // ──────────────────────────────────────────────────────────────────
    // SessionId / EventId Display
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn session_id_display_is_uuid_string() {
        let uuid = Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap();
        let sid = SessionId(uuid);
        assert_eq!(sid.to_string(), "018f4c2e-3a1b-7d00-8000-abcdef012345");
    }

    #[test]
    fn event_id_display_is_uuid_string() {
        let uuid = Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap();
        let eid = EventId(uuid);
        assert_eq!(eid.to_string(), "018f4c2f-1a2b-7e00-8000-fedcba987654");
    }

    // ──────────────────────────────────────────────────────────────────
    // SessionRef Display / FromStr round-trips
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn session_ref_display_session_only() {
        let uuid = Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap();
        let r = SessionRef::session(SessionId(uuid));
        assert_eq!(r.to_string(), "018f4c2e-3a1b-7d00-8000-abcdef012345");
    }

    #[test]
    fn session_ref_display_event_pinned() {
        let sid = SessionId(Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap());
        let eid = EventId(Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap());
        let r = SessionRef::event(sid, eid);
        assert_eq!(
            r.to_string(),
            "018f4c2e-3a1b-7d00-8000-abcdef012345#018f4c2f-1a2b-7e00-8000-fedcba987654"
        );
    }

    #[test]
    fn session_ref_round_trip_session_only() {
        let uuid = Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap();
        let r = SessionRef::session(SessionId(uuid));
        let s = r.to_string();
        let back: SessionRef = s.parse().unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn session_ref_round_trip_event_pinned() {
        let sid = SessionId(Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap());
        let eid = EventId(Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap());
        let r = SessionRef::event(sid, eid);
        let s = r.to_string();
        let back: SessionRef = s.parse().unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn session_ref_parse_error_on_invalid_uuid() {
        let result = "not-a-uuid".parse::<SessionRef>();
        assert_eq!(result, Err(SessionRefParseError));
    }

    #[test]
    fn session_ref_parse_error_display_message() {
        let msg = SessionRefParseError.to_string();
        // Pin the message so the mutant that replaces fmt with Ok(Default::default())
        // (producing an empty string) is caught.
        assert!(
            msg.contains("uuid"),
            "error message must mention uuid, got: {msg}"
        );
    }

    #[test]
    fn session_ref_parse_error_on_invalid_event_uuid() {
        let result = "018f4c2e-3a1b-7d00-8000-abcdef012345#not-a-uuid".parse::<SessionRef>();
        assert_eq!(result, Err(SessionRefParseError));
    }

    // ──────────────────────────────────────────────────────────────────
    // Origin construction invariant
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn origin_subagent_of_requires_event_id() {
        let sid = SessionId(Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap());
        let eid = EventId(Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap());
        let parent = SessionRef::event(sid, eid);
        let o = Origin::subagent_of(parent.clone());
        assert_eq!(o, Origin::SubagentOf { parent });
    }

    #[test]
    #[should_panic(expected = "SubagentOf requires a concrete spawn-event ID")]
    fn origin_subagent_of_panics_without_event_id() {
        let sid = SessionId(Uuid::parse_str("018f4c2e-3a1b-7d00-8000-abcdef012345").unwrap());
        let parent = SessionRef::session(sid);
        let _ = Origin::subagent_of(parent);
    }

    // ──────────────────────────────────────────────────────────────────
    // LoggedEvent envelope serialisation / deserialisation
    // ──────────────────────────────────────────────────────────────────

    #[test]
    fn logged_event_serialises_event_id_at_top_level_alongside_type() {
        use crate::events::{OmegaEvent, UserMessageEvent};
        let eid = EventId(Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap());
        let le = LoggedEvent {
            event_id: Some(eid),
            event: OmegaEvent::UserMessage(UserMessageEvent {
                time: "2025-01-01T00:00:00.000Z".into(),
                content: "hello".into(),
            }),
        };
        let v = serde_json::to_value(&le).unwrap();
        assert_eq!(v["eventId"], "018f4c2f-1a2b-7e00-8000-fedcba987654");
        assert_eq!(v["type"], "user_message");
        assert_eq!(v["content"], "hello");
        // eventId and the OmegaEvent fields are at the same JSON level
        assert!(
            v.get("event").is_none(),
            "must not nest under an 'event' key"
        );
    }

    #[test]
    fn logged_event_omits_event_id_when_none() {
        use crate::events::{OmegaEvent, UserMessageEvent};
        let le = LoggedEvent {
            event_id: None,
            event: OmegaEvent::UserMessage(UserMessageEvent {
                time: "2025-01-01T00:00:00.000Z".into(),
                content: "hello".into(),
            }),
        };
        let v = serde_json::to_value(&le).unwrap();
        assert!(
            v.get("eventId").is_none(),
            "eventId must be absent when None"
        );
        assert_eq!(v["type"], "user_message");
    }

    #[test]
    fn logged_event_deserialises_without_event_id() {
        // Represents a pre-Phase-1 log line that lacks eventId.
        let json = serde_json::json!({
            "type": "user_message",
            "time": "2025-01-01T00:00:00.000Z",
            "content": "hello"
        });
        let le: LoggedEvent = serde_json::from_value(json).unwrap();
        assert!(le.event_id.is_none());
        match le.event {
            crate::events::OmegaEvent::UserMessage(e) => assert_eq!(e.content, "hello"),
            _ => panic!("expected UserMessage"),
        }
    }

    #[test]
    fn logged_event_round_trips() {
        use crate::events::{OmegaEvent, UserMessageEvent};
        let eid = EventId(Uuid::parse_str("018f4c2f-1a2b-7e00-8000-fedcba987654").unwrap());
        let le = LoggedEvent {
            event_id: Some(eid),
            event: OmegaEvent::UserMessage(UserMessageEvent {
                time: "2025-01-01T00:00:00.000Z".into(),
                content: "hello".into(),
            }),
        };
        let json = serde_json::to_string(&le).unwrap();
        let back: LoggedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(le, back);
    }
}
