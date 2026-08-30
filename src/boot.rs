//! Backend-neutral boot policy shared by both binaries.
//!
//! `src/main.rs` (the TUI binary) and `src/bin/gtk.rs` (the GTK binary) are
//! thin shims around [`crate::CoordApp`], but process-level policy —
//! `--version` handling, subprocess hardening, and the panic log — has
//! nothing to do with which rendering backend is in use. Issue #4 lifted
//! that policy out of `src/main.rs` (which used to be the only binary that
//! ran it) into this module so both shims run the same boot sequence.
//!
//! What deliberately does **not** live here: the `catch_unwind` wrapper and
//! the post-restore stderr one-liner in `src/main.rs`. Those exist because
//! the TUI takes over the terminal and the default panic hook's stderr
//! write lands inside the alternate screen — a GTK process has no such
//! problem, and wrapping the GLib main loop in `catch_unwind` would be
//! wrong.

use std::sync::OnceLock;

/// Returns `true` when `args` contains `--version` or `-V`.
///
/// Extracted from `main` so the flag-matching logic itself is unit
/// testable — `main`'s body (terminal takeover, panic hook, `catch_unwind`)
/// isn't something a `#[test]` can exercise directly.
pub fn wants_version<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|a| a.as_ref() == "--version" || a.as_ref() == "-V")
}

/// The `--version`/`-V` output, e.g. `coord-tui 0.4.71`.
///
/// #1239 (PKG-3): reads `CARGO_PKG_VERSION`, which cargo populates from
/// `tui/Cargo.toml`'s `[package] version`. The release workflow
/// (`.github/workflows/release-tui.yml`) stamps that field from the pushed
/// `vX.Y.Z` tag before building — the same tag the Python wheel's
/// setuptools-scm version derives from — so `coord-tui --version` and
/// `coord --version` agree on a tagged release build. There is
/// deliberately no separate version literal here to drift out of sync
/// with.
pub fn version_string() -> String {
    format!("coord-tui {}", env!("CARGO_PKG_VERSION"))
}

/// Force non-interactive mode on every subprocess coord-tui (or any tool it
/// spawns) launches. Without these, an SSH passphrase or HTTPS credential
/// prompt from a child git/ssh process can grab the TTY, corrupting the TUI
/// display or hanging silently waiting for input that never arrives — and
/// under GTK, the same prompt from a headless child process just hangs
/// invisibly instead of failing fast.
///
/// - GIT_TERMINAL_PROMPT=0       — git itself never asks for credentials
/// - GIT_SSH_COMMAND BatchMode   — ssh fails fast instead of prompting
///   (10 s ConnectTimeout keeps a misconfigured remote from hanging)
/// - SSH_ASKPASS=/bin/false      — any GUI password helper fails too
///
/// The user can still load their key into ssh-agent before launching
/// coord-tui for normal workflows; these env vars just guarantee the
/// failure mode is "fast and visible" instead of "frozen TTY".
pub fn harden_subprocess_env() {
    // SAFETY: set_var is `unsafe` in recent stdlib — single-threaded
    // setup before any work begins, so no data race.
    unsafe {
        std::env::set_var("GIT_TERMINAL_PROMPT", "0");
        std::env::set_var(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=10",
        );
        std::env::set_var("SSH_ASKPASS", "/bin/false");
    }
}

/// Stashes the one-line panic summary so that a caller which wraps its run
/// loop in `catch_unwind` (only `src/main.rs`'s TUI binary does — see the
/// module docs) can retrieve it *after* the terminal has been restored, by
/// which point the default hook's stderr write would otherwise have landed
/// inside the alternate screen and scrolled offscreen. `OnceLock` is
/// panic-safe (no mutex that could deadlock inside the hook).
pub static PANIC_MSG: OnceLock<String> = OnceLock::new();

/// Installs a panic hook that persists any panic to
/// `~/.coord/coord-tui-panic.log` before the process goes down.
///
/// IMPORTANT: we do NOT chain to the Rust default hook here. The default
/// hook writes directly to stderr, which — for the TUI binary — is inside
/// the alternate-screen buffer while the TUI is running. That output is
/// invisible after terminal teardown, and it corrupts the TUI display if
/// the panic fires mid-render before teardown. Instead we log to a file
/// (always readable) and let the caller decide what, if anything, to print
/// afterwards (the TUI binary prints a clean one-liner to stderr *after*
/// `catch_unwind` returns, by which point quadraui has already restored the
/// terminal; the GTK binary has no such handoff to make and prints nothing
/// extra).
pub fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        // Stash the one-line summary for callers that print a post-restore
        // message — see `PANIC_MSG`'s docs.
        let _ = PANIC_MSG.set(info.to_string());

        if let Some(home) = std::env::var_os("HOME") {
            let log_dir = std::path::Path::new(&home).join(".coord");
            let _ = std::fs::create_dir_all(&log_dir);
            let log_path = log_dir.join("coord-tui-panic.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                use std::io::Write;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = writeln!(
                    f,
                    "\n--- unix_ts={} ---\n{}\n{}",
                    ts,
                    info,
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }
        // No default_hook call — see doc comment above.
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_version_detects_long_flag() {
        assert!(wants_version(["--version"]));
    }

    #[test]
    fn wants_version_detects_short_flag() {
        assert!(wants_version(["-V"]));
    }

    #[test]
    fn wants_version_ignores_unrelated_args() {
        assert!(!wants_version(["--help"]));
        assert!(!wants_version(Vec::<&str>::new()));
    }

    #[test]
    fn wants_version_finds_flag_anywhere_in_argv() {
        // argv[0] (the program path) is already skipped by the caller, but
        // the flag can still appear after other args (e.g. `coord-tui -x
        // --version`) — it shouldn't have to be first.
        assert!(wants_version(["-x", "--version"]));
    }

    #[test]
    fn version_string_matches_cargo_pkg_version() {
        // Cargo populates CARGO_PKG_VERSION from tui/Cargo.toml's [package]
        // version at compile time — see release-tui.yml's version-stamp
        // step, which rewrites that field from the release tag before
        // building. This test guards the format (`coord-tui <version>`),
        // not a hardcoded version number.
        assert_eq!(
            version_string(),
            format!("coord-tui {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(version_string().starts_with("coord-tui "));
    }

    /// Restores a set of env vars to their pre-test state on drop (even if
    /// the test panics partway through an assertion), so
    /// `harden_subprocess_env_sets_all_three_vars` doesn't leak
    /// `GIT_TERMINAL_PROMPT`/`GIT_SSH_COMMAND`/`SSH_ASKPASS` into the rest of
    /// the test binary's process — `cargo test` runs tests as threads within
    /// one process by default, so an unrestored `set_var` here would be
    /// visible to every other test that runs afterwards.
    struct EnvVarGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvVarGuard {
        fn capture(names: &[&'static str]) -> Self {
            Self {
                saved: names
                    .iter()
                    .map(|&name| (name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                // SAFETY: same single-threaded-w.r.t.-this-var-access
                // reasoning as `harden_subprocess_env` — this guard only
                // ever touches vars it captured itself, immediately before
                // this restore, from the test thread that set them.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    #[test]
    fn harden_subprocess_env_sets_all_three_vars() {
        // Acceptance criterion: assert the three vars via a unit test on
        // `harden_subprocess_env` rather than by launching a process.
        //
        // These three vars are process-global state, so capture and restore
        // them via `EnvVarGuard` — see its doc comment — instead of leaving
        // them set for the remainder of the test binary's run.
        let _guard = EnvVarGuard::capture(&["GIT_TERMINAL_PROMPT", "GIT_SSH_COMMAND", "SSH_ASKPASS"]);

        harden_subprocess_env();
        assert_eq!(std::env::var("GIT_TERMINAL_PROMPT").as_deref(), Ok("0"));
        assert_eq!(
            std::env::var("GIT_SSH_COMMAND").as_deref(),
            Ok("ssh -o BatchMode=yes -o ConnectTimeout=10")
        );
        assert_eq!(std::env::var("SSH_ASKPASS").as_deref(), Ok("/bin/false"));
    }
}
