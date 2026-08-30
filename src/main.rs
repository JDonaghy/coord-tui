//! coord-tui — TUI binary.
//!
//! Thin shim: wires [`coord_tui::CoordApp`] to `quadraui::tui::shell_runner`.
//! All app logic lives in `CoordApp`; quadraui owns terminal
//! setup/teardown, the AppShell chrome, and the crossterm event loop.
//!
//! The boot policy shared with `src/bin/gtk.rs` (`--version` handling,
//! subprocess hardening, the panic log) lives in `coord_tui::boot` — see
//! that module's docs for why it was lifted out of here (#4).

use coord_tui::CoordApp;

fn main() {
    // `--version`/`-V` short-circuits before any of the TUI setup below
    // (env var poking, panic hook install, terminal takeover) — a version
    // check must never touch the terminal or spawn anything.
    if coord_tui::boot::wants_version(std::env::args().skip(1)) {
        println!("{}", coord_tui::boot::version_string());
        return;
    }

    coord_tui::boot::harden_subprocess_env();
    coord_tui::boot::install_panic_logger();

    // Wrap the TUI run loop in `catch_unwind` so that a panic that escapes
    // quadraui's internal recovery (e.g. during startup or shutdown) still
    // lets quadraui's Drop handlers restore the terminal before we print the
    // post-mortem.  Without this wrapper the process would abort (or the
    // `panic = "abort"` profile would kill it) and leave the terminal in raw
    // mode.
    //
    // `AssertUnwindSafe` is safe here: we immediately exit the process on the
    // Err branch; we never resume normal execution with a potentially
    // inconsistent CoordApp.
    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        quadraui::tui::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
    }));

    if run_result.is_err() {
        // At this point the terminal has been restored by quadraui's Drop
        // handler, so a plain `eprintln!` is safe and visible.
        let summary = coord_tui::boot::PANIC_MSG
            .get()
            .map(String::as_str)
            .unwrap_or("unknown panic");
        eprintln!(
            "\ncoord-tui panicked: {}\n\nFull details in ~/.coord/coord-tui-panic.log\n",
            summary
        );
        std::process::exit(101);
    }
}
