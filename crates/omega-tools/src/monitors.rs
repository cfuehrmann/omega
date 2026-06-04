//! Async-monitor runtime (Phase 1 of the Async Monitors feature).
//!
//! A [`MonitorManager`] owns, per session:
//!
//! * a **pending queue** — every monitor stdout line / stderr chunk is
//!   enqueued here; and
//! * a **roster** — the ephemeral in-memory list of live monitors
//!   (id, description, command, status, start time, event-fired count,
//!   stderr tail).
//!
//! ## Single-writer rule (design §4)
//! Monitors **never** append to `events.jsonl`.  Background reader tasks only
//! push to the in-memory pending queue and mutate the roster.  The agent's
//! main loop is the single writer; in Phase 2 it will *drain* this queue at
//! the two legal boundaries and turn the items into `MonitorDelivery` /
//! `MonitorStderr` events.  Phase 1 ships the runtime + the two tools but
//! does **not** wire that drain.
//!
//! ## Ownership decision (the Phase 1 → Phase 2 bridge)
//! The queue and roster live together in a `MonitorManager` held behind an
//! `Arc`, stored in [`ToolCtx::monitors`](crate::ToolCtx).  This mirrors how
//! `python_repl` state is threaded (`Option<Arc<…>>` on `ToolCtx`) and is the
//! *smallest* ownership model that satisfies both Phase 1 and Phase 2:
//!
//! * the `monitor` / `stop_monitor` tools reach it now via the `ctx` handle;
//! * Phase 2's main loop will hold the *same* `Arc`, clone it into every
//!   `ToolCtx` it builds, and call [`MonitorManager::drain_pending`] at each
//!   boundary plus [`MonitorManager::shutdown`] on session end — no
//!   re-architecting required.
//!
//! Deliberately **not** a module-level `static` (unlike the background-process
//! registry in `state.rs`): a global would leak the roster across sessions and
//! across tests, violating the *ephemeral roster* requirement (§10 — a fresh
//! process starts with an empty roster, liveness is never reconstructed) and
//! making tests non-deterministic.
//!
//! ## Clean cutover (no half-wired sessions)
//! Phase 1 keeps monitors **invisible** to real sessions: the two tools are
//! *not* in `ALL_TOOL_NAMES` / `DEFAULT_TOOL_NAMES` / any preset, and
//! `tool_definitions` never emits their schemas, so the model can never select
//! or call them.  A session is therefore either *unaware* of monitors (today)
//! or *all-in* (after the later cutover that exposes the tools alongside the
//! loop drain) — never half-wired with a `monitor()` that spawns a process
//! whose output nothing drains.  See the design doc §4 / §11 Phase 1 note.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::process_util::kill_group;

/// Maximum number of stderr lines retained in a monitor's roster tail.
/// Older lines are dropped (the full stderr stream still flows through the
/// pending queue toward `MonitorStderr` events in Phase 2).
const STDERR_TAIL_MAX: usize = 20;

/// RFC3339 millisecond UTC timestamp, matching the format used elsewhere in
/// `omega-tools` for event timestamps.
pub(crate) fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Lifecycle status of a monitor in the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorStatus {
    /// The process is (believed to be) running.
    Running,
    /// The process has been stopped (agent stop, session-end reap, or it
    /// exited / was killed and the waiter task observed the exit).
    Stopped,
}

/// One item in the pending queue, tagged by source kind.
///
/// Phase 2's drain maps `Stdout` items into a `MonitorDelivery` (projected to
/// `role: user`) and `Stderr` items into `MonitorStderr` (diagnostic,
/// non-projected).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingItem {
    /// A single stdout line from a monitor.
    Stdout {
        /// Id of the monitor that produced the line.
        monitor_id: String,
        /// The line, with its trailing newline stripped.
        line: String,
    },
    /// A single stderr line (chunk) from a monitor.
    Stderr {
        /// Id of the monitor that produced the chunk.
        monitor_id: String,
        /// The stderr line, with its trailing newline stripped.
        chunk: String,
    },
}

/// Public, cloneable snapshot of one roster entry (for UI / WS / tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Stable per-session monitor id.
    pub id: String,
    /// Human description supplied to `monitor()`.
    pub description: String,
    /// The shell command being run.
    pub command: String,
    /// Current lifecycle status.
    pub status: MonitorStatus,
    /// RFC3339 start time.
    pub started_at: String,
    /// Number of stdout lines delivered to the queue so far.
    pub fired_count: u64,
    /// Most recent stderr lines (bounded by `STDERR_TAIL_MAX`, oldest first).
    pub stderr_tail: Vec<String>,
}

/// Internal roster entry.
#[derive(Debug)]
struct MonitorEntry {
    description: String,
    command: String,
    status: MonitorStatus,
    started_at: String,
    fired_count: u64,
    stderr_tail: VecDeque<String>,
    /// Process-group id (== the spawned `bash` pid because we spawn with
    /// `process_group(0)`).  `None` only if the OS failed to report a pid.
    pgid: Option<u32>,
}

/// Mutable manager state behind a single lock.
#[derive(Debug)]
struct ManagerInner {
    monitors: HashMap<String, MonitorEntry>,
    pending: VecDeque<PendingItem>,
    next_seq: u64,
}

/// Returned by [`MonitorManager::spawn`] so the caller can build the
/// `MonitorStarted` event with the *same* id and start time recorded in the
/// roster.
pub struct SpawnedMonitor {
    /// The freshly-assigned monitor id.
    pub id: String,
    /// The roster start timestamp (RFC3339).
    pub started_at: String,
}

/// Owns the pending queue and the live-monitor roster for one session.
#[derive(Debug)]
pub struct MonitorManager {
    inner: Mutex<ManagerInner>,
}

impl MonitorManager {
    /// Create an empty manager wrapped in an `Arc` (the shared handle form
    /// used everywhere — `spawn` needs `Arc<Self>` to hand clones to its
    /// background reader tasks).
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ManagerInner {
                monitors: HashMap::new(),
                pending: VecDeque::new(),
                next_seq: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerInner> {
        // Recover the guard even if a previous holder panicked: the manager
        // state is a plain queue + roster, so a poisoned lock carries no
        // broken invariant we need to abort on.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Spawn `command` as a background OS process in its own process group,
    /// register it in the roster, and start reader tasks that enqueue its
    /// stdout/stderr.  Returns **immediately** with the new monitor id; no
    /// process output is awaited here.
    ///
    /// # Errors
    /// Returns an error string if the process could not be spawned.
    pub fn spawn(
        self: &Arc<Self>,
        description: &str,
        command: &str,
    ) -> Result<SpawnedMonitor, String> {
        let mut cmd = Command::new("bash");
        cmd.args(["-c", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("monitor: failed to spawn command: {e}"))?;

        let pgid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let started_at = now_iso();

        let id = {
            let mut inner = self.lock();
            inner.next_seq += 1;
            let id = format!("m{}", inner.next_seq);
            inner.monitors.insert(
                id.clone(),
                MonitorEntry {
                    description: description.to_owned(),
                    command: command.to_owned(),
                    status: MonitorStatus::Running,
                    started_at: started_at.clone(),
                    fired_count: 0,
                    stderr_tail: VecDeque::new(),
                    pgid,
                },
            );
            id
        };

        if let Some(stdout) = stdout {
            let mgr = Arc::clone(self);
            let mid = id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    mgr.push_stdout(&mid, line);
                }
            });
        }

        if let Some(stderr) = stderr {
            let mgr = Arc::clone(self);
            let mid = id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    mgr.push_stderr(&mid, line);
                }
            });
        }

        // Waiter: reap the child (no zombies) and mark the roster entry
        // Stopped when the process exits on its own.  Uses the SIGCHLD-driven
        // `wait`; if the monitor was already stopped (agent stop / shutdown)
        // the status is left as Stopped.
        {
            let mgr = Arc::clone(self);
            let mid = id.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                mgr.mark_stopped(&mid);
            });
        }

        Ok(SpawnedMonitor { id, started_at })
    }

    fn push_stdout(&self, id: &str, line: String) {
        let mut inner = self.lock();
        if let Some(entry) = inner.monitors.get_mut(id) {
            entry.fired_count += 1;
        }
        inner.pending.push_back(PendingItem::Stdout {
            monitor_id: id.to_owned(),
            line,
        });
    }

    fn push_stderr(&self, id: &str, chunk: String) {
        let mut inner = self.lock();
        if let Some(entry) = inner.monitors.get_mut(id) {
            entry.stderr_tail.push_back(chunk.clone());
            // We push exactly one line at a time, so the tail can exceed the
            // cap by at most one — a single `pop_front` restores it. (An `if`
            // rather than a `while` deliberately: a `while` here lets a
            // boundary mutation degrade into an infinite drain on an empty
            // deque, which mutation testing scores as a timeout rather than a
            // clean catch.)
            if entry.stderr_tail.len() > STDERR_TAIL_MAX {
                entry.stderr_tail.pop_front();
            }
        }
        inner.pending.push_back(PendingItem::Stderr {
            monitor_id: id.to_owned(),
            chunk,
        });
    }

    /// Mark a monitor Stopped if it is still known (idempotent; leaves an
    /// already-Stopped entry untouched).
    fn mark_stopped(&self, id: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.monitors.get_mut(id) {
            entry.status = MonitorStatus::Stopped;
        }
    }

    /// Stop one monitor: kill its whole process tree and mark it Stopped.
    ///
    /// Returns `true` if the monitor was **live** and was stopped (the caller
    /// should log a `MonitorStopped`).  Returns `false` — a no-op — if the id
    /// is unknown or the monitor was already stopped.
    pub fn stop(&self, id: &str) -> bool {
        let pgid = {
            let mut inner = self.lock();
            match inner.monitors.get_mut(id) {
                Some(entry) if entry.status == MonitorStatus::Running => {
                    entry.status = MonitorStatus::Stopped;
                    entry.pgid
                }
                _ => return false,
            }
        };
        if let Some(gid) = pgid {
            kill_group(gid);
        }
        true
    }

    /// Session-end reaping: kill every live monitor's process tree and mark
    /// them Stopped.  Returns the ids that were live and got killed.
    pub fn shutdown(&self) -> Vec<String> {
        let killed: Vec<(String, Option<u32>)> = {
            let mut inner = self.lock();
            inner
                .monitors
                .iter_mut()
                .filter(|(_, e)| e.status == MonitorStatus::Running)
                .map(|(id, e)| {
                    e.status = MonitorStatus::Stopped;
                    (id.clone(), e.pgid)
                })
                .collect()
        };
        for (_, pgid) in &killed {
            if let Some(gid) = pgid {
                kill_group(*gid);
            }
        }
        killed.into_iter().map(|(id, _)| id).collect()
    }

    /// Drain and return all pending queue items (Phase 2's loop calls this at
    /// each boundary; tests use it to observe enqueued stdout/stderr).
    pub fn drain_pending(&self) -> Vec<PendingItem> {
        let mut inner = self.lock();
        inner.pending.drain(..).collect()
    }

    /// Snapshot the live roster (for UI / WS / tests).
    #[must_use]
    pub fn roster(&self) -> Vec<MonitorInfo> {
        let inner = self.lock();
        inner
            .monitors
            .iter()
            .map(|(id, e)| MonitorInfo {
                id: id.clone(),
                description: e.description.clone(),
                command: e.command.clone(),
                status: e.status,
                started_at: e.started_at.clone(),
                fired_count: e.fired_count,
                stderr_tail: e.stderr_tail.iter().cloned().collect(),
            })
            .collect()
    }

    /// Status of a single monitor, or `None` if the id is unknown.
    #[must_use]
    pub fn status(&self, id: &str) -> Option<MonitorStatus> {
        let inner = self.lock();
        inner.monitors.get(id).map(|e| e.status)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used, // test assertions
    clippy::unwrap_used, // test assertions
    clippy::panic, // test assertions
)]
mod tests {
    //! End-to-end tests: the two tools are exercised through
    //! [`crate::execute_tool`] (per the AGENTS.md mandate), and the monitor
    //! runtime is observed through the shared `MonitorManager` handle that the
    //! `ToolCtx` carries — exactly the seam the agent loop will use in Phase 2.

    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use omega_types::events::OmegaEvent;

    use super::*;
    use crate::{ToolCtx, execute_tool};

    /// Build a `ToolCtx` wired to a fresh `MonitorManager`, returning both.
    fn ctx_with_manager() -> (tempfile::TempDir, ToolCtx, Arc<MonitorManager>) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let mut ctx = ToolCtx::new(tmp.path(), "testmon0");
        let mgr = MonitorManager::new();
        ctx.monitors = Some(Arc::clone(&mgr));
        (tmp, ctx, mgr)
    }

    /// Pull the single `MonitorStarted` event out of a tool result.
    fn started_event(r: &crate::ToolResult) -> &omega_types::events::MonitorStartedEvent {
        match r.extra_events.as_slice() {
            [OmegaEvent::MonitorStarted(e)] => e,
            other => panic!("expected exactly one MonitorStarted event, got {other:?}"),
        }
    }

    /// True if the process is alive (or a not-yet-reaped zombie).
    fn process_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Poll `f` every 20ms until it returns `true` or `deadline` elapses.
    async fn poll_until(deadline: Duration, mut f: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            if f() {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Drain the manager repeatedly, accumulating items, until at least
    /// `want` of `kind` have been seen or the deadline elapses.
    async fn accumulate(
        mgr: &MonitorManager,
        deadline: Duration,
        want: usize,
        is_kind: impl Fn(&PendingItem) -> bool,
    ) -> Vec<PendingItem> {
        let mut acc: Vec<PendingItem> = Vec::new();
        let start = Instant::now();
        loop {
            acc.extend(mgr.drain_pending());
            if acc.iter().filter(|i| is_kind(i)).count() >= want {
                return acc;
            }
            if start.elapsed() >= deadline {
                return acc;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    const DL: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn monitor_returns_immediately_with_id_and_logs_started() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let started = Instant::now();
        let r = execute_tool(
            "monitor",
            json!({ "description": "watch logs", "command": "sleep 100" }),
            None,
            Some(&ctx),
        )
        .await;
        // Immediate return: spawning + registering must not block on output.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "monitor() did not return promptly"
        );
        assert!(!r.is_error, "monitor errored: {}", r.content);

        let ev = started_event(&r);
        assert!(!ev.id.is_empty(), "id must be non-empty");
        assert_eq!(ev.description, "watch logs");
        assert_eq!(ev.command, "sleep 100");
        // now_iso() must produce a real RFC3339 timestamp, not an empty string.
        assert!(
            ev.time.contains('T') && ev.time.ends_with('Z'),
            "bad time: {}",
            ev.time
        );
        // The tool result text surfaces the id to the model.
        assert!(
            r.content.contains(&ev.id),
            "result should mention id: {}",
            r.content
        );
        // The roster knows about the live monitor.
        assert_eq!(mgr.status(&ev.id), Some(MonitorStatus::Running));

        mgr.shutdown();
    }

    #[tokio::test]
    async fn monitor_rejects_missing_fields_and_missing_manager() {
        let (_tmp, ctx, _mgr) = ctx_with_manager();
        // Missing description.
        let r = execute_tool("monitor", json!({ "command": "true" }), None, Some(&ctx)).await;
        assert!(r.is_error && r.content.contains("description"));
        // Missing command.
        let r = execute_tool("monitor", json!({ "description": "x" }), None, Some(&ctx)).await;
        assert!(r.is_error && r.content.contains("command"));
        // No ctx at all.
        let r = execute_tool(
            "monitor",
            json!({ "description": "x", "command": "true" }),
            None,
            None,
        )
        .await;
        assert!(r.is_error);
        // ctx present but monitors disabled.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let ctx_no_mon = ToolCtx::new(tmp.path(), "nomon");
        let r = execute_tool(
            "monitor",
            json!({ "description": "x", "command": "true" }),
            None,
            Some(&ctx_no_mon),
        )
        .await;
        assert!(r.is_error && r.content.contains("not enabled"));
    }

    #[tokio::test]
    async fn stdout_lines_reach_pending_queue_and_increment_fired() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": "printf 'a\\nb\\nc\\n'" }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();

        let items = accumulate(&mgr, DL, 3, |i| matches!(i, PendingItem::Stdout { .. })).await;
        let lines: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                PendingItem::Stdout { monitor_id, line } if *monitor_id == id => {
                    Some(line.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            vec!["a", "b", "c"],
            "stdout lines must reach the queue in order"
        );

        assert!(
            poll_until(DL, || mgr
                .roster()
                .iter()
                .any(|m| m.id == id && m.fired_count == 3))
            .await,
            "fired_count must reach 3, got roster {:?}",
            mgr.roster()
        );
        mgr.shutdown();
    }

    #[tokio::test]
    async fn stderr_lines_reach_queue_and_roster_tail() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": "printf 'x\\ny\\n' 1>&2" }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();

        let items = accumulate(&mgr, DL, 2, |i| matches!(i, PendingItem::Stderr { .. })).await;
        let chunks: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                PendingItem::Stderr { monitor_id, chunk } if *monitor_id == id => {
                    Some(chunk.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(chunks, vec!["x", "y"], "stderr chunks must reach the queue");

        let tail = mgr
            .roster()
            .into_iter()
            .find(|m| m.id == id)
            .expect("roster")
            .stderr_tail;
        assert_eq!(
            tail,
            vec!["x".to_string(), "y".to_string()],
            "roster stderr_tail"
        );
        mgr.shutdown();
    }

    #[tokio::test]
    async fn stderr_tail_is_capped_to_last_n() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let n = STDERR_TAIL_MAX + 5; // 25 lines
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": format!("for i in $(seq 1 {n}); do echo line$i 1>&2; done") }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();

        // Wait until all n stderr chunks have been processed (roster tail and
        // pending queue are updated atomically per line).
        accumulate(&mgr, DL, n, |i| matches!(i, PendingItem::Stderr { .. })).await;

        let tail = mgr
            .roster()
            .into_iter()
            .find(|m| m.id == id)
            .expect("roster")
            .stderr_tail;
        assert_eq!(
            tail.len(),
            STDERR_TAIL_MAX,
            "tail must be capped at STDERR_TAIL_MAX"
        );
        assert_eq!(
            tail.first().map(String::as_str),
            Some("line6"),
            "oldest retained line"
        );
        assert_eq!(
            tail.last().map(String::as_str),
            Some("line25"),
            "newest line"
        );
        mgr.shutdown();
    }

    #[tokio::test]
    async fn stop_monitor_kills_tree_and_logs_agent_stopped() {
        use omega_types::events::MonitorStopReason;

        let (tmp, ctx, mgr) = ctx_with_manager();
        let pidfile = tmp.path().join("child.pid");
        let cmd = format!("sleep 100 & echo $! > {}; wait", pidfile.display());
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": cmd }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();

        // Read the grandchild (sleep) pid — proves we kill the whole tree,
        // not just the bash leader.
        assert!(
            poll_until(DL, || pidfile.exists()).await,
            "pidfile never appeared"
        );
        let child_pid: u32 = poll_read_pid(&pidfile).await.expect("child pid");
        assert!(
            process_alive(child_pid),
            "sleep child should be alive before stop"
        );

        let r = execute_tool("stop_monitor", json!({ "id": id }), None, Some(&ctx)).await;
        assert!(!r.is_error, "stop_monitor errored: {}", r.content);
        match r.extra_events.as_slice() {
            [OmegaEvent::MonitorStopped(e)] => {
                assert_eq!(e.id, id);
                assert_eq!(e.reason, MonitorStopReason::AgentStopped);
                assert!(e.time.contains('T') && e.time.ends_with('Z'));
            }
            other => panic!("expected one MonitorStopped, got {other:?}"),
        }
        assert_eq!(mgr.status(&id), Some(MonitorStatus::Stopped));
        assert!(
            poll_until(DL, || !process_alive(child_pid)).await,
            "sleep child {child_pid} should be killed (whole tree reaped)"
        );
    }

    #[tokio::test]
    async fn stop_monitor_unknown_and_already_stopped_are_noops() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        // Unknown id: ok, no event.
        let r = execute_tool("stop_monitor", json!({ "id": "nope" }), None, Some(&ctx)).await;
        assert!(!r.is_error, "unknown stop should be ok");
        assert!(r.extra_events.is_empty(), "no-op must emit no event");

        // Spawn, stop once (event), stop again (no-op, no event).
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": "sleep 100" }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();
        let first = execute_tool(
            "stop_monitor",
            json!({ "id": id.clone() }),
            None,
            Some(&ctx),
        )
        .await;
        assert_eq!(first.extra_events.len(), 1, "first stop logs an event");
        let second = execute_tool("stop_monitor", json!({ "id": id }), None, Some(&ctx)).await;
        assert!(!second.is_error);
        assert!(second.extra_events.is_empty(), "second stop is a no-op");
        mgr.shutdown();
    }

    #[tokio::test]
    async fn shutdown_reaps_all_live_monitors() {
        let (tmp, ctx, mgr) = ctx_with_manager();
        let mut pids = Vec::new();
        let mut ids = Vec::new();
        for k in 0..2 {
            let pidfile = tmp.path().join(format!("c{k}.pid"));
            let cmd = format!("sleep 100 & echo $! > {}; wait", pidfile.display());
            let r = execute_tool(
                "monitor",
                json!({ "description": "d", "command": cmd }),
                None,
                Some(&ctx),
            )
            .await;
            ids.push(started_event(&r).id.clone());
            assert!(poll_until(DL, || pidfile.exists()).await, "pidfile {k}");
            pids.push(poll_read_pid(&pidfile).await.expect("pid"));
        }

        let killed = mgr.shutdown();
        assert_eq!(killed.len(), 2, "shutdown must report both live monitors");
        for id in &ids {
            assert!(killed.contains(id), "shutdown must include {id}");
            assert_eq!(mgr.status(id), Some(MonitorStatus::Stopped));
        }
        for pid in pids {
            assert!(
                poll_until(DL, || !process_alive(pid)).await,
                "pid {pid} must be reaped"
            );
        }
        // Second shutdown is a no-op: nothing live remains.
        assert!(mgr.shutdown().is_empty(), "second shutdown reaps nothing");
    }

    #[tokio::test]
    async fn natural_exit_marks_monitor_stopped() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": "printf done\\n" }),
            None,
            Some(&ctx),
        )
        .await;
        let id = started_event(&r).id.clone();
        assert!(
            poll_until(DL, || mgr.status(&id) == Some(MonitorStatus::Stopped)).await,
            "monitor should be marked Stopped after natural exit"
        );
    }

    /// Poll until the pidfile parses to a u32.
    async fn poll_read_pid(path: &std::path::Path) -> Option<u32> {
        let start = Instant::now();
        loop {
            if let Ok(s) = std::fs::read_to_string(path)
                && let Ok(pid) = s.trim().parse::<u32>()
            {
                return Some(pid);
            }
            if start.elapsed() >= DL {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
