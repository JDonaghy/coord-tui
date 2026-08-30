//! coord-tui — GTK binary.
//!
//! Thin shim: wires [`coord_tui::CoordApp`] to the `quadraui::gtk`
//! shell runner. All app logic is backend-neutral and lives in `CoordApp`.
//!
//! The app-id defaults to `"org.quadraui.app"` (the runner's built-in
//! default) until quadraui #234 lands and adds a builder API for
//! custom app-ids and window titles.
//!
//! Runs the same boot policy as `src/main.rs` (`--version` handling,
//! subprocess hardening, the panic log) via `coord_tui::boot` — see that
//! module's docs for why (#4). Unlike the TUI binary, this doesn't wrap the
//! run loop in `catch_unwind`: GTK doesn't take over the terminal the way
//! the TUI does, so there is no alternate-screen/raw-mode state that needs
//! restoring before a post-mortem message can print.
use coord_tui::CoordApp;

fn main() {
    if coord_tui::boot::wants_version(std::env::args().skip(1)) {
        println!("{}", coord_tui::boot::version_string());
        return;
    }

    coord_tui::boot::harden_subprocess_env();
    coord_tui::boot::install_panic_logger();

    quadraui::gtk::shell_runner::run_with_shell(CoordApp::new(), CoordApp::shell_config());
}
