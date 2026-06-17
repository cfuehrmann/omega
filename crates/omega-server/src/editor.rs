//! External-editor prompt composition for the web UI.
//!
//! The browser's "Editor" button POSTs to `/api/compose`; the server
//! launches the operator's configured editor on a temp file seeded with the
//! current draft, waits for the editor to close, and returns the edited text
//! tagged with the operator's intent (derived from the exit code): a normal
//! exit (`0`) means *send immediately*, while a force-quit with
//! [`BACKOUT_EXIT_CODE`] (Helix `:cq!`) means *keep as draft, do not send* —
//! a loss-free backout.
//!
//! ## Where the temp file lives
//!
//! The seed file is created in the **server's working directory** (the
//! project CWD), not the system temp dir. TUI editors anchor in-buffer path
//! completion at the *current document's* directory rather than the working
//! directory, so a temp file under `/tmp` would complete paths against
//! `/tmp`. Anchoring it at the cwd lets the operator tab-complete project
//! files while composing. The `omega-prompt-*.md` name is gitignored and the
//! file is unlinked when the request completes.
//!
//! ## Editor resolution
//!
//! [`resolve_editor_command`] consults, in order: `OMEGA_EDITOR` → `VISUAL`
//! → `EDITOR`, taking the first that is set and non-blank.  `OMEGA_EDITOR`
//! is the recommended knob (set it in `~/.config/omega/.env`); `VISUAL` /
//! `EDITOR` are honoured as conventional fallbacks.
//!
//! ## The terminal caveat
//!
//! The editor runs on the **server host**, spawned by the `omega-server`
//! process, which has no controlling terminal of its own.  A TUI editor
//! (Helix, nvim, vim) must therefore be wrapped in a terminal command:
//!
//! ```text
//! OMEGA_EDITOR="foot hx"            # foot runs `hx <tmpfile>` and blocks
//! OMEGA_EDITOR="alacritty -e nvim" # likewise for alacritty + nvim
//! ```
//!
//! GUI editors need their blocking flag instead, e.g. `code --wait` or
//! `gedit -w`.  A bare `EDITOR=nvim` (or `hx`/`vim`) cannot work from the
//! server: with no terminal to attach to, the editor's stdin is detached
//! ([`std::process::Stdio::null`]) so it exits immediately rather than
//! hanging the request, and [`compose_with_editor`] returns an error that
//! names the terminal-wrapper fix.  Because the editor opens on the server
//! host, the feature only makes sense when the browser and server share a
//! machine (the usual setup).
//!
//! ## Mutation-test split
//!
//! The pure helpers ([`resolve_editor_command`], [`split_editor_command`],
//! [`classify_exit_code`]) carry the mutation-testing budget.
//! [`compose_with_editor`] is the process/tempfile I/O edge —
//! `#[mutants::skip]`, exercised end-to-end by the `/api/compose` integration
//! test with a fake editor.

/// Environment variables consulted to find the editor command, in
/// precedence order.  `OMEGA_EDITOR` wins over the conventional
/// `VISUAL` / `EDITOR` so operators can configure a server-specific
/// terminal-wrapped command without disturbing their shell editor.
pub const EDITOR_ENV_VARS: [&str; 3] = ["OMEGA_EDITOR", "VISUAL", "EDITOR"];

/// Resolve the editor command string, using `lookup` to read env vars.
///
/// Returns the value of the first variable in [`EDITOR_ENV_VARS`] that is
/// present and non-blank.  Pure — `lookup` is injected so the precedence
/// logic is unit- and mutation-testable without touching the process
/// environment.
#[must_use]
pub fn resolve_editor_command<F>(lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    EDITOR_ENV_VARS
        .iter()
        .find_map(|var| lookup(var).filter(|v| !v.trim().is_empty()))
}

/// Split an editor command string into `(program, args)` by whitespace.
///
/// Returns `None` when the command is blank.  The caller appends the
/// temp-file path as the final argument.
///
/// Whitespace splitting is deliberately simple: it handles the common
/// cases (`hx`, `foot hx`, `alacritty -e hx`, `code --wait`) but not
/// quoted paths containing spaces.  Operators whose editor lives at a
/// spaced path should point `OMEGA_EDITOR` at a wrapper script.
#[must_use]
pub fn split_editor_command(command: &str) -> Option<(String, Vec<String>)> {
    let mut parts = command.split_whitespace().map(str::to_owned);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// Return `true` when an editor command is available in the current process
/// environment — i.e. at least one of [`EDITOR_ENV_VARS`] is set and
/// non-blank.  Pure delegation to [`resolve_editor_command`] so the
/// precedence logic stays in one place and is not duplicated at call sites.
///
/// `#[mutants::skip]`: trivial delegation that reads process-global
/// `std::env`. The substantive precedence logic lives in
/// [`resolve_editor_command`], which is unit-tested through an injectable
/// lookup closure precisely because real-env mutation is racy under
/// parallel tests. This shim only does `.is_some()`, and the one spot it
/// touches real env means a hermetic `--lib` unit test can't pin its
/// `true`/`false` body replacements; the wiring is exercised only by the
/// env-controlled integration/e2e paths the `-- --lib` recipe excludes.
#[must_use]
#[mutants::skip]
pub fn is_editor_configured() -> bool {
    resolve_editor_command(|var| std::env::var(var).ok()).is_some()
}

/// Exit code an editor uses to signal a **loss-free backout**: keep the
/// composed text as the draft but do *not* send it. This is Helix's default
/// `:cq!` (force-quit) code; vim's `:cq` matches too. Normal exit (`0`)
/// means "send"; any other code is treated as a launch/runtime error.
pub const BACKOUT_EXIT_CODE: i32 = 1;

/// How the editor's exit code maps to a compose disposition.
///
/// Pure classifier so the exit-code policy is unit- and mutation-testable
/// without spawning a process. `None` means "neither a clean send nor a
/// deliberate backout" — i.e. an error the caller should surface with the
/// terminal-wrapper hint.
#[derive(Debug, PartialEq, Eq)]
pub enum ExitDisposition {
    /// Editor exited normally (`0`): send the composed text immediately.
    Send,
    /// Editor force-quit with [`BACKOUT_EXIT_CODE`]: keep as draft, no send.
    Backout,
}

/// Classify an editor process exit code into a [`ExitDisposition`].
///
/// `code` is `None` when the process was terminated by a signal (no exit
/// code), which is always an error. Pure.
#[must_use]
pub fn classify_exit_code(code: Option<i32>) -> Option<ExitDisposition> {
    match code {
        Some(0) => Some(ExitDisposition::Send),
        Some(BACKOUT_EXIT_CODE) => Some(ExitDisposition::Backout),
        _ => None,
    }
}

/// Outcome of a successful external-editor compose session.
///
/// Carries the composed text together with the operator's intent, derived
/// from the editor's exit code (see [`classify_exit_code`]).
#[derive(Debug, PartialEq, Eq)]
pub enum ComposeOutcome {
    /// Editor exited normally (`0`): the client sends this immediately,
    /// as if the Send button had been clicked.
    Send(String),
    /// Editor force-quit with [`BACKOUT_EXIT_CODE`] (Helix `:cq!`): the
    /// client keeps this as the draft (persisted to localStorage) without
    /// sending, so the operator backs out without losing what they wrote.
    Backout(String),
}

/// Normalize text written by the external editor before it is sent or kept
/// as a draft.
///
/// Strips trailing whitespace from every line (including a stray `\r` from
/// CRLF endings), normalizes line endings to LF, and drops trailing blank
/// lines and the final newline. Leading whitespace and interior blank lines
/// are preserved, so intentional indentation and paragraph breaks survive.
///
/// Trailing whitespace is never meaningful in a prompt — it only wastes
/// tokens and adds noise, and markdown's two-trailing-spaces hard break is
/// irrelevant to a model reading raw text. Pure, so the policy is unit- and
/// mutation-testable without spawning an editor.
#[must_use]
pub fn strip_trailing_whitespace(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

/// Launch the configured editor on a temp file seeded with `draft`, wait
/// for it to exit, and return the edited contents tagged with the operator's
/// intent.
///
/// The exit code decides the [`ComposeOutcome`]: `0` → [`ComposeOutcome::Send`]
/// (send immediately), [`BACKOUT_EXIT_CODE`] → [`ComposeOutcome::Backout`]
/// (keep as draft, do not send). In both cases the file contents are passed
/// through [`strip_trailing_whitespace`] — including when the operator quit
/// without saving, in which case the file still holds the seed, so a backout
/// is loss-free.
///
/// # Errors
///
/// Returns `Err(message)` when:
/// - no editor is configured (`OMEGA_EDITOR` / `VISUAL` / `EDITOR` unset),
/// - the temp file cannot be created or seeded,
/// - the editor cannot be spawned,
/// - the editor exits with a status that is neither `0` nor
///   [`BACKOUT_EXIT_CODE`] (or is killed by a signal), or
/// - the edited file cannot be read back.
#[mutants::skip] // process/tempfile I/O edge — pure helpers carry the budget.
pub async fn compose_with_editor(draft: &str) -> Result<ComposeOutcome, String> {
    let command = resolve_editor_command(|var| std::env::var(var).ok()).ok_or_else(|| {
        "no editor configured: set OMEGA_EDITOR (e.g. \"foot hx\"), VISUAL, or EDITOR".to_owned()
    })?;
    let (program, args) =
        split_editor_command(&command).ok_or_else(|| "editor command is blank".to_owned())?;

    // Seed a temp file with the current draft so the operator can continue
    // editing what they had already typed in the browser.
    //
    // The file is created in the server's working directory (the project
    // CWD), *not* the system temp dir. TUI editors anchor in-buffer path
    // completion at the current document's directory (Helix:
    // `doc.path().parent()`), so a temp file under `/tmp` would complete
    // against `/tmp`. Placing it at the cwd lets the operator tab-complete
    // project files while composing. The `omega-prompt-*.md` name is
    // gitignored and the file is unlinked on drop, so the project tree is
    // only transiently touched while the editor is open.
    let cwd = std::env::current_dir().map_err(|e| format!("determine working directory: {e}"))?;
    let tmp = tempfile::Builder::new()
        .prefix("omega-prompt-")
        .suffix(".md")
        .tempfile_in(&cwd)
        .map_err(|e| format!("create temp file: {e}"))?;
    let path = tmp.path().to_path_buf();
    tokio::fs::write(&path, draft)
        .await
        .map_err(|e| format!("seed temp file: {e}"))?;

    // Detach the child's stdin from the server's. The server has no
    // controlling terminal, so a bare TUI editor (nvim, vim, hx) launched
    // without a terminal wrapper would otherwise read the server's non-tty
    // stdin and block forever, hanging the `/api/compose` request. With a
    // null stdin it instead hits EOF immediately and exits non-zero, so the
    // operator gets a prompt error they can act on. A correctly wrapped
    // command (`foot nvim`, `alacritty -e hx`, `code --wait`) opens its own
    // terminal/pty and is unaffected.
    // `kill_on_drop(true)`: if this handler future is ever dropped while the
    // editor is still open (e.g. server shutdown with the request in flight),
    // tear the editor down with it instead of leaving an orphan editing an
    // already-unlinked temp file.
    let status = tokio::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| format!("launch editor '{program}': {e}"))?;
    let disposition = classify_exit_code(status.code()).ok_or_else(|| {
        format!(
            "editor '{program}' exited with {status}. If this is a terminal \
             editor (nvim, vim, hx), the server has no terminal to run it in \
             — wrap it in one, e.g. OMEGA_EDITOR=\"foot {program}\" or \
             \"alacritty -e {program}\"; GUI editors need a blocking flag \
             like \"code --wait\". (To back out without sending, exit with \
             code {BACKOUT_EXIT_CODE} — e.g. Helix `:cq!`.)"
        )
    })?;

    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("read prompt file: {e}"))?;
    let content = strip_trailing_whitespace(&raw);

    Ok(match disposition {
        ExitDisposition::Send => ComposeOutcome::Send(content),
        ExitDisposition::Backout => ComposeOutcome::Backout(content),
    })
}

// ---------------------------------------------------------------------------
// Unit tests for the pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Justification for inline test block: `resolve_editor_command` and
    // `split_editor_command` are pure functions with no HTTP-observable
    // surface of their own — the `/api/compose` integration test exercises
    // them only through one configured editor command at a time, so the
    // precedence ladder and the arg-splitting edge cases (blank, extra
    // whitespace, multi-arg) are tested directly here.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::{
        ExitDisposition, classify_exit_code, resolve_editor_command, split_editor_command,
        strip_trailing_whitespace,
    };
    use std::collections::HashMap;

    /// Build a `lookup` closure backed by a fixed map.
    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |var: &str| map.get(var).cloned()
    }

    #[test]
    fn resolve_prefers_omega_editor_over_visual_and_editor() {
        let lookup = lookup_from(&[
            ("OMEGA_EDITOR", "foot hx"),
            ("VISUAL", "gvim"),
            ("EDITOR", "vim"),
        ]);
        assert_eq!(resolve_editor_command(lookup), Some("foot hx".to_owned()));
    }

    #[test]
    fn resolve_falls_back_to_visual_when_omega_editor_unset() {
        let lookup = lookup_from(&[("VISUAL", "gvim"), ("EDITOR", "vim")]);
        assert_eq!(resolve_editor_command(lookup), Some("gvim".to_owned()));
    }

    #[test]
    fn resolve_falls_back_to_editor_when_others_unset() {
        let lookup = lookup_from(&[("EDITOR", "vim")]);
        assert_eq!(resolve_editor_command(lookup), Some("vim".to_owned()));
    }

    #[test]
    fn resolve_skips_blank_omega_editor_and_uses_next() {
        // A present-but-blank OMEGA_EDITOR must not win — fall through.
        let lookup = lookup_from(&[("OMEGA_EDITOR", "   "), ("VISUAL", "gvim")]);
        assert_eq!(resolve_editor_command(lookup), Some("gvim".to_owned()));
    }

    #[test]
    fn resolve_returns_none_when_nothing_set() {
        let lookup = lookup_from(&[]);
        assert_eq!(resolve_editor_command(lookup), None);
    }

    #[test]
    fn resolve_returns_none_when_all_blank() {
        let lookup = lookup_from(&[("OMEGA_EDITOR", ""), ("VISUAL", "  "), ("EDITOR", "\t")]);
        assert_eq!(resolve_editor_command(lookup), None);
    }

    #[test]
    fn split_single_program_has_no_args() {
        assert_eq!(split_editor_command("hx"), Some(("hx".to_owned(), vec![])));
    }

    #[test]
    fn split_program_with_args_preserves_order() {
        assert_eq!(
            split_editor_command("alacritty -e hx"),
            Some((
                "alacritty".to_owned(),
                vec!["-e".to_owned(), "hx".to_owned()]
            )),
        );
    }

    #[test]
    fn split_collapses_surrounding_and_internal_whitespace() {
        assert_eq!(
            split_editor_command("  foot   hx  "),
            Some(("foot".to_owned(), vec!["hx".to_owned()])),
        );
    }

    #[test]
    fn split_blank_command_is_none() {
        assert_eq!(split_editor_command(""), None);
        assert_eq!(split_editor_command("   "), None);
    }

    #[test]
    fn classify_exit_zero_is_send() {
        assert_eq!(classify_exit_code(Some(0)), Some(ExitDisposition::Send));
    }

    #[test]
    fn classify_backout_code_is_backout() {
        // Helix `:cq!` default — the deliberate loss-free backout signal.
        assert_eq!(classify_exit_code(Some(1)), Some(ExitDisposition::Backout));
    }

    #[test]
    fn classify_other_nonzero_code_is_error() {
        // Anything that is neither a clean send nor the backout code is an
        // error the caller surfaces with the terminal-wrapper hint.
        assert_eq!(classify_exit_code(Some(2)), None);
        assert_eq!(classify_exit_code(Some(127)), None);
    }

    #[test]
    fn classify_signal_termination_is_error() {
        // No exit code (killed by a signal) is always an error.
        assert_eq!(classify_exit_code(None), None);
    }

    #[test]
    fn strip_removes_per_line_trailing_spaces_and_tabs() {
        assert_eq!(strip_trailing_whitespace("a   \nb\t\nc"), "a\nb\nc");
    }

    #[test]
    fn strip_normalizes_crlf_to_lf_and_drops_the_carriage_return() {
        assert_eq!(strip_trailing_whitespace("a\r\nb\r\nc"), "a\nb\nc");
    }

    #[test]
    fn strip_drops_trailing_blank_lines_and_final_newline() {
        assert_eq!(strip_trailing_whitespace("hello\n\n\n"), "hello");
        assert_eq!(strip_trailing_whitespace("hello\n"), "hello");
    }

    #[test]
    fn strip_preserves_interior_blank_lines_and_leading_indent() {
        // Paragraph breaks and intentional indentation must survive.
        assert_eq!(
            strip_trailing_whitespace("para one\n\n    indented line  \n"),
            "para one\n\n    indented line",
        );
    }

    #[test]
    fn strip_empty_stays_empty() {
        assert_eq!(strip_trailing_whitespace(""), "");
        assert_eq!(strip_trailing_whitespace("   \n  \n"), "");
    }
}
