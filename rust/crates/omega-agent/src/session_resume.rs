//! Session-resumption helpers (pure functions over event lists).
//!
//! Mirrors `src/session-resume.ts` for the parts that don't touch the
//! agent or the LLM. Phase 1d.1a ports only [`extract_last_model_and_effort`];
//! the basis-extraction and summary-extraction helpers land in 1d.1b.

use omega_protocol::OmegaEvent;

/// The last model and effort explicitly set during a session, by scanning
/// its event list.
///
/// Each `Option` is `None` when no `model_changed` / `effort_changed`
/// event was found — callers should fall back to their default.  The
/// scan is left-to-right so the *latest* change wins.
///
/// Mirrors `extractLastModelAndEffort` in `src/session-resume.ts`.
#[must_use]
pub fn extract_last_model_and_effort(events: &[OmegaEvent]) -> (Option<String>, Option<String>) {
    let mut model: Option<String> = None;
    let mut effort: Option<String> = None;
    for event in events {
        match event {
            OmegaEvent::ModelChanged(ev) => model = Some(ev.model.clone()),
            OmegaEvent::EffortChanged(ev) => effort = Some(ev.effort.clone()),
            _ => {}
        }
    }
    (model, effort)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omega_protocol::events::{EffortChangedEvent, ModelChangedEvent, UserMessageEvent};

    fn user_msg(content: &str) -> OmegaEvent {
        OmegaEvent::UserMessage(UserMessageEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            content: content.to_owned(),
        })
    }

    fn model_changed(model: &str) -> OmegaEvent {
        OmegaEvent::ModelChanged(ModelChangedEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            model: model.to_owned(),
        })
    }

    fn effort_changed(effort: &str) -> OmegaEvent {
        OmegaEvent::EffortChanged(EffortChangedEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            effort: effort.to_owned(),
        })
    }

    #[test]
    fn empty_event_list_returns_none_for_both() {
        let (m, e) = extract_last_model_and_effort(&[]);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }

    #[test]
    fn returns_none_when_no_change_events_present() {
        let evs = vec![user_msg("hi"), user_msg("there")];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }

    #[test]
    fn returns_last_model_when_multiple_changes() {
        let evs = vec![
            model_changed("claude-sonnet-4-6"),
            user_msg("between"),
            model_changed("claude-opus-4-7"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(e, None);
    }

    #[test]
    fn returns_last_effort_when_multiple_changes() {
        let evs = vec![
            effort_changed("low"),
            effort_changed("medium"),
            effort_changed("high"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e.as_deref(), Some("high"));
    }

    #[test]
    fn model_and_effort_are_independent_keys() {
        // Interleaved order — neither key should overwrite the other.
        let evs = vec![
            model_changed("claude-opus-4-6"),
            effort_changed("xhigh"),
            user_msg("noise"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(e.as_deref(), Some("xhigh"));
    }

    #[test]
    fn later_event_overrides_earlier_for_same_key() {
        // Specifically pin the "latest wins" direction so a mutation
        // that picks the first match (or breaks early) is killed.
        let evs = vec![
            model_changed("first"),
            model_changed("second"),
            model_changed("third"),
        ];
        let (m, _) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("third"));
    }

    #[test]
    fn unrelated_event_types_are_ignored() {
        let evs = vec![user_msg("hello"), user_msg("world")];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }
}
