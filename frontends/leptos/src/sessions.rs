//! Session-list reactive state — picker-side store separate from
//! [`crate::store::SessionStore`].
//!
//! ## Why a separate store?
//!
//! Different lifecycles:
//!
//! - `SessionStore` (conversation): reset on every `WsMessage::ResetDone`;
//!   only ever describes the *currently active* session.
//! - `SessionListStore` (this module): survives across resets; a list of
//!   *all* sessions known to the server, mutated reactively as
//!   `SessionRenamed`/`SessionDeleted` envelopes arrive.
//!
//! Folding the two would force the conversation reducer to ignore most
//! of its own input (and vice versa). Honest types: keep them disjoint.
//!
//! ## Pure reducers ↔ reactive wrapper
//!
//! [`apply_renamed`], [`apply_deleted`], and [`is_active`] are pure free
//! functions that operate on `Vec<SessionListItem>` / refs. They are
//! directly mutation-tested without DOM or signal infrastructure.
//!
//! [`SessionListStore::apply`] is the wire-driven dispatcher; it routes
//! the two relevant [`WsMessage`] variants through `RwSignal::update` to
//! the pure reducers, and no-ops on every other variant. Mutation tests
//! exercise each match arm directly via `with_owner`-scoped tests.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::protocol::WsMessage;

// ---------------------------------------------------------------------------
// Wire shape
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/sessions` JSON array.
///
/// Field-name projection mirrors `omega-server::router::SessionListItem`
/// (camelCase). Unknown fields the server may add later are ignored
/// (default `serde` behaviour) — preserving forward compatibility for
/// fields like `description` that are already optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub dir: String,
    pub last_activity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure reducers (cargo-mutants targets)
// ---------------------------------------------------------------------------

/// Set `name` on the entry whose `dir` matches; no-op otherwise.
///
/// Returns `true` iff a matching entry was found. The boolean isn't read
/// by `SessionListStore::apply` (server-confirmed updates assume the
/// server only emits `session_renamed` for known dirs), but exposing it
/// makes the no-match arm directly observable in tests.
pub fn apply_renamed(items: &mut [SessionListItem], dir: &str, name: &str) -> bool {
    for item in items.iter_mut() {
        if item.dir == dir {
            item.name = Some(name.to_string());
            return true;
        }
    }
    false
}

/// Remove the entry whose `dir` matches; no-op otherwise.
///
/// Returns `true` iff at least one entry was removed.
pub fn apply_deleted(items: &mut Vec<SessionListItem>, dir: &str) -> bool {
    let before = items.len();
    items.retain(|item| item.dir != dir);
    items.len() != before
}

/// Decide whether `item` is the active session, given the conversation
/// store's current `session_info.dir` (`None` if no active session).
#[must_use]
pub fn is_active(item: &SessionListItem, current_dir: Option<&str>) -> bool {
    match current_dir {
        Some(d) => item.dir == d,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Reactive wrapper
// ---------------------------------------------------------------------------

/// Plain-data view of [`SessionListStore`]. Used by tests to assert
/// against, and (eventually) by debug surfaces to dump.
#[allow(dead_code)] // consumed by tests + future debug-panel snapshots
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListState {
    pub sessions: Vec<SessionListItem>,
    pub loading: bool,
    pub last_error: Option<String>,
}

/// Reactive container for the picker's view of all sessions.
///
/// Cheaply [`Copy`] (each signal is a slotmap handle); pass by value
/// into closures and contexts.
///
/// ## Fetch-generation race fix
///
/// `GET /api/sessions` is triggered by [`Effect`]s in `picker.rs` whenever
/// the active session changes. WS broadcasts (`SessionRenamed`,
/// `SessionDeleted`) can arrive *while* a fetch is in flight, mutating the
/// list locally before the GET response lands. Without coordination, the
/// GET would clobber the WS-applied mutation.
///
/// The `fetch_generation` counter solves this: every list-mutating method
/// (`set_sessions`, `set_error`, `apply` for `SessionRenamed` /
/// `SessionDeleted`) calls [`bump_generation`], invalidating any fetch
/// whose captured generation no longer matches the live one. Callers
/// snapshot the generation at fetch start via [`begin_loading`] and pass
/// it back to [`finish_loading_if_current`].
#[derive(Debug, Clone, Copy)]
pub struct SessionListStore {
    pub sessions: RwSignal<Vec<SessionListItem>>,
    pub loading: RwSignal<bool>,
    pub last_error: RwSignal<Option<String>>,
    /// Monotonically-increasing fetch generation. Bumped by every
    /// list-mutating operation. Used to discard stale fetch results
    /// (see struct-level docs).
    pub fetch_generation: RwSignal<u64>,
}

impl Default for SessionListStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionListStore {
    /// Construct with all signals at default values. Must run inside a
    /// leptos reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            last_error: RwSignal::new(None),
            fetch_generation: RwSignal::new(0),
        }
    }

    /// Route one server-emitted [`WsMessage`] through the pure
    /// reducers. Two variants matter (`SessionRenamed`, `SessionDeleted`);
    /// every other frame is a no-op for the picker.
    ///
    /// Both write-arms bump [`fetch_generation`] so any in-flight
    /// `GET /api/sessions` whose generation predates the mutation is
    /// dropped on completion (see struct-level docs).
    pub fn apply(&self, msg: &WsMessage) {
        match msg {
            WsMessage::SessionRenamed { session_dir, name } => {
                self.sessions.update(|v| {
                    apply_renamed(v, session_dir, name);
                });
                self.bump_generation();
            }
            WsMessage::SessionDeleted { session_dir } => {
                self.sessions.update(|v| {
                    apply_deleted(v, session_dir);
                });
                self.bump_generation();
            }
            _ => {}
        }
    }

    /// Replace the list with a fresh fetch. Clears any prior error and
    /// bumps the fetch generation (so any *other* fetch in flight is
    /// invalidated). Test-only seam — production callers go through
    /// [`finish_loading_if_current`] which checks the generation token.
    #[allow(dead_code)] // consumed by unit tests
    pub fn set_sessions(&self, items: Vec<SessionListItem>) {
        self.sessions.set(items);
        self.last_error.set(None);
        self.bump_generation();
    }

    /// Record an error and clear the loading flag. Bumps the fetch
    /// generation so concurrent fetches with this error's generation
    /// don't accidentally overwrite it.
    pub fn set_error(&self, message: String) {
        self.last_error.set(Some(message));
        self.loading.set(false);
        self.bump_generation();
    }

    /// Mark a fetch as in-flight, clear any prior error, and return the
    /// generation token the caller must pass back to
    /// [`finish_loading_if_current`] / [`fail_loading_if_current`].
    #[must_use]
    pub fn begin_loading(&self) -> u64 {
        let next = self.bump_generation();
        self.loading.set(true);
        self.last_error.set(None);
        next
    }

    /// Apply a successful fetch's result *iff* `token` is still the
    /// current generation. Stale results (a more recent mutation has
    /// happened since the fetch started) are silently discarded.
    pub fn finish_loading_if_current(&self, token: u64, items: Vec<SessionListItem>) {
        if self.fetch_generation.get_untracked() != token {
            return;
        }
        self.sessions.set(items);
        self.last_error.set(None);
        self.loading.set(false);
    }

    /// Record a fetch error *iff* `token` is still the current
    /// generation. Stale errors are silently discarded.
    pub fn fail_loading_if_current(&self, token: u64, message: String) {
        if self.fetch_generation.get_untracked() != token {
            return;
        }
        self.last_error.set(Some(message));
        self.loading.set(false);
    }

    /// Increment the fetch generation and return the new value.
    /// Public so tests can poke it directly; production callers should
    /// reach for the higher-level methods above.
    pub fn bump_generation(&self) -> u64 {
        let next = self.fetch_generation.get_untracked().wrapping_add(1);
        self.fetch_generation.set(next);
        next
    }

    /// Untracked snapshot — used by tests and for debug dumps.
    #[allow(dead_code)] // consumed by tests + future debug-panel snapshots
    #[must_use]
    pub fn snapshot(&self) -> SessionListState {
        SessionListState {
            sessions: self.sessions.get_untracked(),
            loading: self.loading.get_untracked(),
            last_error: self.last_error.get_untracked(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use leptos::reactive::owner::Owner;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn with_owner<F: FnOnce()>(f: F) {
        let owner = Owner::new();
        owner.with(f);
    }

    fn item(dir: &str, name: Option<&str>) -> SessionListItem {
        SessionListItem {
            dir: dir.into(),
            last_activity: "2024-01-01T00:00:00.000Z".into(),
            name: name.map(str::to_string),
            description: None,
            resumed_from: None,
        }
    }

    // ---- pure: apply_renamed ------------------------------------------------

    #[wasm_bindgen_test]
    fn apply_renamed_sets_name_on_matching_dir() {
        let mut v = vec![item("a", None), item("b", None)];
        let hit = apply_renamed(&mut v, "b", "beta");
        assert!(hit);
        assert_eq!(v[0].name, None);
        assert_eq!(v[1].name.as_deref(), Some("beta"));
    }

    #[wasm_bindgen_test]
    fn apply_renamed_overwrites_existing_name() {
        let mut v = vec![item("a", Some("old"))];
        let hit = apply_renamed(&mut v, "a", "new");
        assert!(hit);
        assert_eq!(v[0].name.as_deref(), Some("new"));
    }

    #[wasm_bindgen_test]
    fn apply_renamed_no_match_returns_false_and_leaves_list_alone() {
        let mut v = vec![item("a", Some("alpha"))];
        let hit = apply_renamed(&mut v, "missing", "x");
        assert!(!hit);
        assert_eq!(v[0].name.as_deref(), Some("alpha"));
    }

    #[wasm_bindgen_test]
    fn apply_renamed_only_first_match_when_dirs_collide() {
        // Sessions are identified by dir; collisions shouldn't happen
        // server-side, but the function must still terminate
        // deterministically. The implementation early-returns on the
        // first match — locking that in defends against a `loop` →
        // `iter().for_each()` mutation that would otherwise rename
        // every item on a duplicate-dir list.
        let mut v = vec![item("a", None), item("a", None)];
        let hit = apply_renamed(&mut v, "a", "x");
        assert!(hit);
        assert_eq!(v[0].name.as_deref(), Some("x"));
        assert_eq!(v[1].name, None);
    }

    // ---- pure: apply_deleted ------------------------------------------------

    #[wasm_bindgen_test]
    fn apply_deleted_removes_matching_dir() {
        let mut v = vec![item("a", None), item("b", None), item("c", None)];
        let hit = apply_deleted(&mut v, "b");
        assert!(hit);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].dir, "a");
        assert_eq!(v[1].dir, "c");
    }

    #[wasm_bindgen_test]
    fn apply_deleted_no_match_returns_false_and_leaves_list_alone() {
        let mut v = vec![item("a", None), item("b", None)];
        let hit = apply_deleted(&mut v, "missing");
        assert!(!hit);
        assert_eq!(v.len(), 2);
    }

    #[wasm_bindgen_test]
    fn apply_deleted_removes_every_match_when_dirs_collide() {
        let mut v = vec![item("a", None), item("a", None), item("b", None)];
        let hit = apply_deleted(&mut v, "a");
        assert!(hit);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].dir, "b");
    }

    // ---- pure: is_active ----------------------------------------------------

    #[wasm_bindgen_test]
    fn is_active_true_when_dir_matches_current() {
        assert!(is_active(&item("a", None), Some("a")));
    }

    #[wasm_bindgen_test]
    fn is_active_false_when_dir_does_not_match() {
        assert!(!is_active(&item("a", None), Some("b")));
    }

    #[wasm_bindgen_test]
    fn is_active_false_when_current_is_none() {
        assert!(!is_active(&item("a", None), None));
    }

    // ---- reactive: SessionListStore::apply (match arms) ---------------------

    #[wasm_bindgen_test]
    fn apply_renamed_message_updates_matching_item() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_sessions(vec![item("a", None), item("b", None)]);
            s.apply(&WsMessage::SessionRenamed {
                session_dir: "b".into(),
                name: "beta".into(),
            });
            let snap = s.snapshot();
            assert_eq!(snap.sessions[0].name, None);
            assert_eq!(snap.sessions[1].name.as_deref(), Some("beta"));
        });
    }

    #[wasm_bindgen_test]
    fn apply_deleted_message_removes_matching_item() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_sessions(vec![item("a", None), item("b", None)]);
            s.apply(&WsMessage::SessionDeleted {
                session_dir: "a".into(),
            });
            let snap = s.snapshot();
            assert_eq!(snap.sessions.len(), 1);
            assert_eq!(snap.sessions[0].dir, "b");
        });
    }

    #[wasm_bindgen_test]
    fn apply_unrelated_message_is_a_noop() {
        // Locks down the `_ => {}` arm: arbitrary frames must not
        // mutate the list. Mutating the catch-all to call one of the
        // two reducers would be caught by this test.
        with_owner(|| {
            let s = SessionListStore::new();
            let before = vec![item("a", Some("alpha")), item("b", None)];
            s.set_sessions(before.clone());
            s.apply(&WsMessage::Ready);
            s.apply(&WsMessage::ResetDone);
            s.apply(&WsMessage::Text { index: 0, text: "x".into() });
            assert_eq!(s.snapshot().sessions, before);
        });
    }

    // ---- reactive: setters --------------------------------------------------

    #[wasm_bindgen_test]
    fn set_sessions_replaces_list_and_clears_error() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_error("prior".into());
            assert!(s.snapshot().last_error.is_some());

            s.set_sessions(vec![item("a", None)]);
            let snap = s.snapshot();
            assert_eq!(snap.sessions.len(), 1);
            assert!(snap.last_error.is_none());
        });
    }

    #[wasm_bindgen_test]
    fn set_error_records_message_and_clears_loading() {
        with_owner(|| {
            let s = SessionListStore::new();
            let _ = s.begin_loading();
            assert!(s.snapshot().loading);

            s.set_error("oops".into());
            let snap = s.snapshot();
            assert_eq!(snap.last_error.as_deref(), Some("oops"));
            assert!(!snap.loading);
        });
    }

    #[wasm_bindgen_test]
    fn begin_and_finish_loading_toggle_the_flag() {
        with_owner(|| {
            let s = SessionListStore::new();
            assert!(!s.snapshot().loading);
            let token = s.begin_loading();
            assert!(s.snapshot().loading);
            s.finish_loading_if_current(token, vec![]);
            assert!(!s.snapshot().loading);
        });
    }

    #[wasm_bindgen_test]
    fn begin_loading_clears_prior_error() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_error("prior".into());
            let _ = s.begin_loading();
            assert!(s.snapshot().last_error.is_none());
        });
    }

    // ---- fetch-generation race fix ----------------------------------------

    #[wasm_bindgen_test]
    fn finish_loading_if_current_applies_when_generation_matches() {
        with_owner(|| {
            let s = SessionListStore::new();
            let token = s.begin_loading();
            s.finish_loading_if_current(token, vec![item("a", None)]);
            let snap = s.snapshot();
            assert_eq!(snap.sessions.len(), 1);
            assert!(!snap.loading);
        });
    }

    #[wasm_bindgen_test]
    fn finish_loading_if_current_drops_stale_result() {
        // Race scenario: GET /api/sessions starts → apply_deleted runs
        // (bumps generation) → GET response arrives. The post-bump
        // result must NOT clobber the deletion.
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_sessions(vec![item("a", None), item("b", None)]);

            let token = s.begin_loading(); // captured at fetch start
            // While the fetch is in flight, server broadcasts a delete:
            s.apply(&WsMessage::SessionDeleted {
                session_dir: "b".into(),
            });
            // Fetch returns with the *pre-delete* snapshot:
            s.finish_loading_if_current(token, vec![item("a", None), item("b", None)]);

            let snap = s.snapshot();
            // The deletion must still be reflected: stale fetch dropped.
            assert_eq!(snap.sessions.len(), 1);
            assert_eq!(snap.sessions[0].dir, "a");
        });
    }

    #[wasm_bindgen_test]
    fn fail_loading_if_current_drops_stale_error() {
        with_owner(|| {
            let s = SessionListStore::new();
            let token = s.begin_loading();
            // Server-confirmed delete races in:
            s.apply(&WsMessage::SessionDeleted {
                session_dir: "x".into(),
            });
            // Stale fetch error tries to land:
            s.fail_loading_if_current(token, "net error".into());
            // Error must NOT have been recorded:
            assert!(s.snapshot().last_error.is_none());
        });
    }

    #[wasm_bindgen_test]
    fn fail_loading_if_current_records_error_when_generation_matches() {
        with_owner(|| {
            let s = SessionListStore::new();
            let token = s.begin_loading();
            s.fail_loading_if_current(token, "net error".into());
            let snap = s.snapshot();
            assert_eq!(snap.last_error.as_deref(), Some("net error"));
            assert!(!snap.loading);
        });
    }

    #[wasm_bindgen_test]
    fn apply_renamed_message_bumps_generation() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_sessions(vec![item("a", None)]);
            let before = s.fetch_generation.get_untracked();
            s.apply(&WsMessage::SessionRenamed {
                session_dir: "a".into(),
                name: "alpha".into(),
            });
            let after = s.fetch_generation.get_untracked();
            assert!(after > before, "rename must bump generation: {before} -> {after}");
        });
    }

    #[wasm_bindgen_test]
    fn apply_deleted_message_bumps_generation() {
        with_owner(|| {
            let s = SessionListStore::new();
            s.set_sessions(vec![item("a", None)]);
            let before = s.fetch_generation.get_untracked();
            s.apply(&WsMessage::SessionDeleted {
                session_dir: "a".into(),
            });
            let after = s.fetch_generation.get_untracked();
            assert!(after > before, "delete must bump generation: {before} -> {after}");
        });
    }

    #[wasm_bindgen_test]
    fn unrelated_message_does_not_bump_generation() {
        // Locks down the `_ => {}` arm at the generation level: bumping
        // unnecessarily would make every fetch stale on every WS frame.
        with_owner(|| {
            let s = SessionListStore::new();
            let before = s.fetch_generation.get_untracked();
            s.apply(&WsMessage::Ready);
            s.apply(&WsMessage::ResetDone);
            assert_eq!(s.fetch_generation.get_untracked(), before);
        });
    }

    #[wasm_bindgen_test]
    fn bump_generation_returns_new_value() {
        with_owner(|| {
            let s = SessionListStore::new();
            let g0 = s.fetch_generation.get_untracked();
            let g1 = s.bump_generation();
            let g2 = s.bump_generation();
            assert_eq!(g1, g0 + 1);
            assert_eq!(g2, g1 + 1);
            assert_eq!(s.fetch_generation.get_untracked(), g2);
        });
    }

    // ---- wire-shape round trip ---------------------------------------------

    #[wasm_bindgen_test]
    fn session_list_item_deserialises_from_server_shape() {
        // Mirrors `omega-server::router::SessionListItem` JSON output.
        let json = r#"[
            {"dir":"d1","lastActivity":"2024-01-01T00:00:00.000Z","name":"alpha"},
            {"dir":"d2","lastActivity":"2024-01-02T00:00:00.000Z","resumedFrom":"d1"}
        ]"#;
        let v: Vec<SessionListItem> = serde_json::from_str(json).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name.as_deref(), Some("alpha"));
        assert_eq!(v[1].resumed_from.as_deref(), Some("d1"));
        assert!(v[0].description.is_none());
    }
}
