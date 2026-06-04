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
use tokio::sync::Notify;

use omega_types::events::MonitorStopReason;

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
    /// A monitor self-terminated (its process exited on its own — Phase 2
    /// exit-status capture).  Enqueued by the waiter task **only** for a
    /// natural exit (a `Running` → `Stopped` transition it observed itself);
    /// agent-initiated stops / session shutdown set `Stopped` *before*
    /// killing, so the waiter suppresses the (bogus SIGKILL) item for those.
    /// The loop drains this into a projected `MonitorStopped` event.
    Stopped {
        /// Id of the monitor that exited.
        monitor_id: String,
        /// Classified outcome: `ProcessExited` (normal exit, any code) or
        /// `Crashed` (killed by a signal).
        reason: MonitorStopReason,
        /// Process exit code when it exited normally; `None` when killed by
        /// a signal (the signal number is not carried in the frozen schema).
        exit_code: Option<i32>,
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
/// Outcome of [`MonitorManager::mark_stopped_natural`] — whether the waiter's
/// observed exit was the monitor's own (and therefore worth reporting) or a
/// kill the agent/shutdown already accounted for (and therefore suppressed).
enum NaturalStop {
    /// The monitor was already `Stopped` (agent stop / shutdown got there
    /// first); the waiter drops the redundant stop item.
    Suppress,
    /// This call performed the `Running` → `Stopped` transition; `pgid` is the
    /// group to reap for orphaned grandchildren.
    Transitioned { pgid: Option<u32> },
}

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
    /// Fired (one permit) whenever a drainable item is pushed onto the
    /// pending queue — the parked main loop selects on this to wake and
    /// drain.  Kept **outside** the inner lock so the loop never blocks the
    /// reader/waiter tasks.
    item_notify: Notify,
    /// Fired whenever the live-monitor set shrinks **without** enqueuing a
    /// drainable item (an agent/UI stop or session shutdown).  Lets a parked
    /// loop re-evaluate park-vs-terminate instead of hanging when the last
    /// live monitor is killed with an empty queue.
    roster_notify: Notify,
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
            item_notify: Notify::new(),
            roster_notify: Notify::new(),
        })
    }

    /// Wake source the parked loop selects on for **queued items**.  A permit
    /// is stored if no one is waiting, so a line pushed just before the loop
    /// parks is not lost.
    #[must_use]
    pub fn notify_item(&self) -> &Notify {
        &self.item_notify
    }

    /// Wake source the parked loop selects on for a **roster shrink** that
    /// enqueued nothing (agent/UI stop, shutdown).
    #[must_use]
    pub fn notify_roster(&self) -> &Notify {
        &self.roster_notify
    }

    /// Number of monitors currently believed `Running`.  The loop parks iff
    /// this is non-zero (or the queue is non-empty); otherwise nothing can
    /// ever fire and it terminates.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.lock()
            .monitors
            .values()
            .filter(|e| e.status == MonitorStatus::Running)
            .count()
    }

    /// Number of items currently waiting in the pending queue (non-destructive).
    ///
    /// Unlike [`drain_pending`](Self::drain_pending) this does not consume the
    /// queue; it exists so callers (and tests) can observe that an item has
    /// been enqueued without taking it.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.lock().pending.len()
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

        // Waiter: reap the child (no zombies), classify its exit, and — for a
        // *natural* self-termination — enqueue a `Stopped` item so the loop can
        // project a `MonitorStopped` event (single-writer: the waiter never
        // writes events itself, it only enqueues).  Phase 2 exit-status
        // capture: `ProcessExited` (normal exit, any code) vs `Crashed`
        // (killed by a signal).
        {
            let mgr = Arc::clone(self);
            let mid = id.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                let (reason, exit_code) = match &status {
                    Ok(st) => match st.code() {
                        // Normal exit (any code, including non-zero).
                        Some(code) => (MonitorStopReason::ProcessExited, Some(code)),
                        // No code => terminated by a signal (e.g. SIGSEGV,
                        // SIGKILL). `signal()` is the carrier; the frozen
                        // schema has no field for it, so exit_code stays None.
                        None => (MonitorStopReason::Crashed, None),
                    },
                    // `wait()` itself failed (should not happen for a child we
                    // spawned); treat conservatively as a crash.
                    Err(_) => (MonitorStopReason::Crashed, None),
                };
                // Only the natural Running->Stopped transition enqueues. If the
                // agent/shutdown already set Stopped, the exit we observe here
                // is the SIGKILL *we* sent — suppress the bogus item.
                if let NaturalStop::Transitioned { pgid } = mgr.mark_stopped_natural(&mid) {
                    // The group leader exited on its own but may have left
                    // background grandchildren reparented to init; kill the
                    // group to reap them (no orphans), then enqueue.
                    if let Some(gid) = pgid {
                        kill_group(gid);
                    }
                    mgr.enqueue_stopped(&mid, reason, exit_code);
                }
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
        drop(inner);
        self.item_notify.notify_one();
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
        drop(inner);
        self.item_notify.notify_one();
    }

    /// Mark a monitor Stopped **iff** it was still `Running`, reporting the
    /// transition to the waiter.
    ///
    /// Returns `Transitioned { pgid }` (the entry's process-group id, itself
    /// optional) when this call performed the `Running` → `Stopped` transition
    /// — i.e. a natural self-exit the waiter observed first.  Returns
    /// `Suppress` when the monitor was unknown or already `Stopped` (agent stop
    /// / shutdown got there first), so the waiter drops the redundant item.
    fn mark_stopped_natural(&self, id: &str) -> NaturalStop {
        let mut inner = self.lock();
        match inner.monitors.get_mut(id) {
            Some(entry) if entry.status == MonitorStatus::Running => {
                entry.status = MonitorStatus::Stopped;
                NaturalStop::Transitioned { pgid: entry.pgid }
            }
            _ => NaturalStop::Suppress,
        }
    }

    /// Enqueue a `Stopped` item for a self-terminated monitor and wake the
    /// parked loop.  Called by the waiter only after a natural transition.
    fn enqueue_stopped(&self, id: &str, reason: MonitorStopReason, exit_code: Option<i32>) {
        {
            let mut inner = self.lock();
            inner.pending.push_back(PendingItem::Stopped {
                monitor_id: id.to_owned(),
                reason,
                exit_code,
            });
        }
        self.item_notify.notify_one();
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
        // The live set shrank but we enqueued nothing drainable: wake any
        // parked loop so it re-evaluates park-vs-terminate (covers a UI
        // KillMonitor of the last live monitor while parked).
        self.roster_notify.notify_one();
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
        if !killed.is_empty() {
            // Roster shrank without enqueuing: wake a parked loop to terminate.
            self.roster_notify.notify_one();
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

    /// Spawn a monitor through the tool and return its id.
    async fn spawn_mon(ctx: &ToolCtx, command: &str) -> String {
        let r = execute_tool(
            "monitor",
            json!({ "description": "d", "command": command }),
            None,
            Some(ctx),
        )
        .await;
        started_event(&r).id.clone()
    }

    #[tokio::test]
    async fn natural_exit_enqueues_stopped_with_exit_code() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        // Exit with a specific non-zero code; the waiter must classify it as a
        // normal `ProcessExited` and carry the code through the queue.
        let id = spawn_mon(&ctx, "exit 7").await;
        let items = accumulate(&mgr, DL, 1, |i| matches!(i, PendingItem::Stopped { .. })).await;
        let stopped: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                PendingItem::Stopped {
                    monitor_id,
                    reason,
                    exit_code,
                } => Some((monitor_id, reason.clone(), *exit_code)),
                _ => None,
            })
            .collect();
        assert_eq!(stopped.len(), 1, "exactly one Stopped item");
        assert_eq!(stopped[0].0, &id);
        assert_eq!(stopped[0].1, MonitorStopReason::ProcessExited);
        assert_eq!(stopped[0].2, Some(7), "normal exit carries its code");
        assert_eq!(mgr.status(&id), Some(MonitorStatus::Stopped));
    }

    #[tokio::test]
    async fn signal_death_enqueues_crashed_without_code() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        // Kill the shell with SIGKILL: no exit code => `Crashed`.
        let _id = spawn_mon(&ctx, "kill -9 $$").await;
        let items = accumulate(&mgr, DL, 1, |i| matches!(i, PendingItem::Stopped { .. })).await;
        let stopped = items
            .iter()
            .find_map(|i| match i {
                PendingItem::Stopped {
                    reason, exit_code, ..
                } => Some((reason.clone(), *exit_code)),
                _ => None,
            })
            .expect("a Stopped item");
        assert_eq!(stopped.0, MonitorStopReason::Crashed);
        assert_eq!(stopped.1, None, "signal death carries no exit code");
    }

    #[tokio::test]
    async fn agent_stop_suppresses_the_natural_stopped_item() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let id = spawn_mon(&ctx, "sleep 100").await;
        // Let the process come up, then stop it. The SIGKILL the waiter later
        // observes must NOT be reported as a `Stopped` item (single-writer: the
        // agent already logged MonitorStopped(AgentStopped) via the tool).
        assert!(
            poll_until(DL, || mgr.live_count() == 1).await,
            "monitor should be live"
        );
        assert!(mgr.stop(&id), "stop reports it was live");
        // Give the waiter ample time to observe the kill and (wrongly, if buggy)
        // enqueue.
        let items = accumulate(&mgr, Duration::from_millis(400), 1, |i| {
            matches!(i, PendingItem::Stopped { .. })
        })
        .await;
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, PendingItem::Stopped { .. })),
            "agent-killed monitor must not enqueue a Stopped item, got {items:?}"
        );
    }

    #[tokio::test]
    async fn natural_exit_reaps_orphaned_grandchildren() {
        let (tmp, ctx, mgr) = ctx_with_manager();
        let pidfile = tmp.path().join("grand.pid");
        // The shell backgrounds a grandchild then exits *without* waiting. The
        // grandchild is reparented to init but stays in the leader's process
        // group, so the natural-exit `kill_group` must reap it (no orphan).
        let cmd = format!("sleep 100 & echo $! > {}; exit 0", pidfile.display());
        let id = spawn_mon(&ctx, &cmd).await;
        assert!(poll_until(DL, || pidfile.exists()).await, "pidfile written");
        let grand = poll_read_pid(&pidfile).await.expect("grandchild pid");
        assert!(
            poll_until(DL, || mgr.status(&id) == Some(MonitorStatus::Stopped)).await,
            "leader should self-exit"
        );
        assert!(
            poll_until(DL, || !process_alive(grand)).await,
            "orphaned grandchild {grand} must be reaped by the natural-exit kill_group"
        );
    }

    #[tokio::test]
    async fn live_count_and_pending_len_track_state() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        assert_eq!(mgr.live_count(), 0);
        assert_eq!(mgr.pending_len(), 0);
        let id = spawn_mon(&ctx, "printf 'one\\n'; sleep 100").await;
        assert!(
            poll_until(DL, || mgr.live_count() == 1).await,
            "one live monitor"
        );
        assert!(
            poll_until(DL, || mgr.pending_len() >= 1).await,
            "the stdout line is queued (observable without draining)"
        );
        let drained = mgr.drain_pending();
        assert!(!drained.is_empty(), "drain returns the queued line");
        assert_eq!(mgr.pending_len(), 0, "drain empties the queue");
        assert!(mgr.stop(&id));
        assert_eq!(mgr.live_count(), 0, "stop drops the live count");
    }

    #[tokio::test]
    async fn item_notify_fires_when_a_line_is_enqueued() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let _id = spawn_mon(&ctx, "printf 'x\\n'; sleep 100").await;
        // Once a line is queued, `notify_one` has stored a permit, so a parked
        // loop selecting on this source wakes immediately.
        assert!(
            poll_until(DL, || mgr.pending_len() >= 1).await,
            "line queued"
        );
        let woke = tokio::time::timeout(DL, mgr.notify_item().notified())
            .await
            .is_ok();
        assert!(woke, "enqueuing a line must fire item_notify");
    }

    #[tokio::test]
    async fn roster_notify_fires_when_a_monitor_is_stopped() {
        let (_tmp, ctx, mgr) = ctx_with_manager();
        let id = spawn_mon(&ctx, "sleep 100").await;
        assert!(poll_until(DL, || mgr.live_count() == 1).await, "live");
        assert!(mgr.stop(&id));
        // A stop shrinks the live set without enqueuing a drainable item, so it
        // must fire roster_notify to wake a parked loop into re-evaluating.
        let woke = tokio::time::timeout(DL, mgr.notify_roster().notified())
            .await
            .is_ok();
        assert!(woke, "stopping a monitor must fire roster_notify");
    }

    #[tokio::test]
    async fn shutdown_fires_roster_notify_only_when_it_reaps_something() {
        // No live monitors: shutdown reaps nothing, so roster_notify must NOT
        // fire (a parked loop would otherwise spin needlessly).  This pins the
        // `if !killed.is_empty()` guard.
        let (_tmp, _ctx, mgr) = ctx_with_manager();
        assert!(mgr.shutdown().is_empty());
        let spurious =
            tokio::time::timeout(Duration::from_millis(150), mgr.notify_roster().notified())
                .await
                .is_ok();
        assert!(
            !spurious,
            "shutdown reaping nothing must not fire roster_notify"
        );

        // With a live monitor: shutdown shrinks the live set, so it must fire.
        let (_tmp2, ctx2, mgr2) = ctx_with_manager();
        spawn_mon(&ctx2, "sleep 100").await;
        assert!(poll_until(DL, || mgr2.live_count() == 1).await, "live");
        assert_eq!(mgr2.shutdown().len(), 1);
        let woke = tokio::time::timeout(DL, mgr2.notify_roster().notified())
            .await
            .is_ok();
        assert!(
            woke,
            "shutdown that reaps a monitor must fire roster_notify"
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
