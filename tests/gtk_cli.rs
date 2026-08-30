//! Black-box CLI test for coord-tui-gtk's `--version`/`-V` flag.
//!
//! Mirrors `tests/cli.rs`, which black-box-tests the plain `coord-tui`
//! binary's `--version`/`-V` wiring, for the GTK binary (#4: "Share boot
//! policy (--version, panic log, subprocess hardening) with the GTK
//! binary"). Both binaries share the same `coord_tui::boot::wants_version`/
//! `version_string` helpers, and those helpers already have unit-test
//! coverage in `src/boot.rs` — but a unit test on the helper functions can't
//! catch a break in `src/bin/gtk.rs`'s own `main()` wiring (e.g. an
//! accidental early return before the `println!`, or the `wants_version`
//! check landing after `harden_subprocess_env`/`install_panic_logger`
//! instead of before). The whole point of lifting the boot policy into
//! `coord_tui::boot` was so both binaries actually call it the same way —
//! this test is what would catch it if one of them didn't.
//!
//! Critically, this also proves `--version` returns *without opening a GTK
//! window* — `wants_version` short-circuits `main()` before
//! `quadraui::gtk::shell_runner::run_with_shell` ever runs. A CI box with no
//! display available would hang or fail if that ordering ever regressed.
//!
//! Gated by `required-features = ["gtk"]` on this test target in
//! `Cargo.toml`, so a plain `cargo test` (which does not build the `gtk`
//! feature — see `Cargo.toml`'s `[features]` doc comments) skips it
//! entirely; `.github/workflows/cargo-test.yml`'s `cargo-test-gtk` job runs
//! `cargo test --features gtk`, which picks it up alongside building the
//! `coord-tui-gtk` binary itself.

use std::process::Command;

/// Runs `coord-tui-gtk <arg>` and returns its stdout, asserting a clean exit
/// and no lingering child process (i.e. no window was opened and left
/// running).
fn run_coord_tui_gtk(arg: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_coord-tui-gtk"))
        .arg(arg)
        .output()
        .expect("failed to spawn the compiled coord-tui-gtk binary");
    assert!(
        output.status.success(),
        "coord-tui-gtk {arg} exited non-zero: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("coord-tui-gtk stdout was not valid UTF-8")
}

#[test]
fn long_flag_prints_version_to_real_stdout() {
    let stdout = run_coord_tui_gtk("--version");
    assert_eq!(
        stdout.trim(),
        format!("coord-tui {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn short_flag_prints_version_to_real_stdout() {
    let stdout = run_coord_tui_gtk("-V");
    assert_eq!(
        stdout.trim(),
        format!("coord-tui {}", env!("CARGO_PKG_VERSION"))
    );
}
