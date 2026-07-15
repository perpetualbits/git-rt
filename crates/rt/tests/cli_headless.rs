//! `--version` / `--help` must answer on a headless box.
//!
//! These need no display, no fonts, and no Xvfb, so they run in a normal
//! `cargo test` — unlike the XRender guards next door.
//!
//! Regression: `main()` used to build the font DB and the winit event loop
//! BEFORE parsing argv, so `rt --version` aborted with
//! `XNotSupported(XOpenDisplayFailed)` over a plain ssh with no DISPLAY. It
//! looked fine wherever a display happened to be reachable (e.g. an `ssh -X`
//! box, which is how it went unnoticed), and failed on any machine without one.

use std::process::Command;

/// Run the rt binary with the given args and no display of any kind.
fn run_headless(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rt"))
        .args(args)
        .env_remove("DISPLAY") // no X
        .env_remove("WAYLAND_DISPLAY") // no Wayland either
        .output()
        .expect("run rt")
}

#[test]
fn version_answers_without_a_display() {
    let out = run_headless(&["--version"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "rt --version must succeed with no DISPLAY; got {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with("rt "), "expected a version line, got: {stdout:?}");
}

#[test]
fn short_version_flag_answers_without_a_display() {
    let out = run_headless(&["-V"]);
    assert!(out.status.success(), "rt -V must succeed with no DISPLAY");
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("rt "));
}

#[test]
fn help_answers_without_a_display() {
    let out = run_headless(&["--help"]);
    assert!(out.status.success(), "rt --help must succeed with no DISPLAY");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--version"), "help should list the flags, got: {stdout:?}");
}
