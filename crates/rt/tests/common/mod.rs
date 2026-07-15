//! Shared X-display harness for the XRender integration tests.
//!
//! Both `xrender_commands.rs` and `instrument_compositing.rs` drive a real rt
//! against a throwaway `Xvfb`, under `xtrace`, and count wire requests. They
//! previously each carried their own copy of this setup — and the same bug in
//! it (see `display_answers`). It lives here once.
#![allow(dead_code)] // each test binary uses a different subset

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// Serialise the tests within a test binary.
///
/// Each one spawns an Xvfb AND a real rt rendering through llvmpipe. Several at
/// once on a loaded machine push rt's cold start past the run window, so it is
/// killed before it draws and the run measures zeros — which silently satisfies
/// an upper-bound assertion. Hold this for the duration of any such test.
pub fn x_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Poisoning is irrelevant here: the guard protects an external resource
    // (display numbers), not invariants of in-process data.
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

/// True when a server is actually LISTENING on `:disp`.
///
/// Deliberately not `Path::exists()` on the socket: a killed Xvfb leaves its
/// socket file behind, so existence is not liveness. Waiting on the file made
/// these tests launch rt against a dead display — rt's connect was refused, it
/// rendered nothing, and the run silently measured zeros. `connect()` is the
/// real check: a stale socket refuses it.
pub fn display_answers(disp: u32) -> bool {
    UnixStream::connect(format!("/tmp/.X11-unix/X{disp}")).is_ok()
}

/// True when nothing has claimed display `d`'s NAME: no socket file, no lock
/// file. Both outlive the server that made them, and either is enough to stop a
/// new server binding that name — Xvfb refuses on a stale lock, and xtrace's
/// proxy bind fails on a stale socket. Killed runs litter these, so a fixed
/// display guess is not reliable.
pub fn display_name_free(d: u32) -> bool {
    !Path::new(&format!("/tmp/.X11-unix/X{d}")).exists() && !Path::new(&format!("/tmp/.X{d}-lock")).exists()
}

/// Find an unclaimed display number at or after `base`.
pub fn free_display_name(base: u32) -> Option<u32> {
    (base..base + 200).find(|d| display_name_free(*d))
}

/// Spawn `Xvfb :disp` and wait until it actually answers (up to ~5 s).
/// Returns the owned child so the caller kills exactly this PID.
pub fn start_xvfb(disp: u32) -> Option<Child> {
    // Refuse a display already in use rather than silently attaching to someone
    // else's server (a parallel run, or a leftover).
    if display_answers(disp) {
        return None;
    }
    let mut child = Command::new("Xvfb")
        .arg(format!(":{disp}"))
        .args(["-screen", "0", "800x600x24", "-nolisten", "tcp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        // Xvfb exiting early (e.g. "server already active" on a stale lock file)
        // must fail loudly, not leave us polling a socket that never answers.
        if matches!(child.try_wait(), Ok(Some(_))) {
            return None;
        }
        if display_answers(disp) {
            return Some(child);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Start an Xvfb on the first display it will actually serve at or after `base`.
pub fn start_xvfb_scan(base: u32) -> Option<(u32, Child)> {
    (base..base + 200)
        .filter(|d| display_name_free(*d))
        .find_map(|d| start_xvfb(d).map(|c| (d, c)))
}

/// Block until `path` contains `needle`, or `timeout` elapses. Returns the
/// trace length at the moment it appeared (so a caller can measure only what
/// came AFTER), or `None` on timeout.
///
/// Why this exists: rt's cold start (llvmpipe + GL context + XRender init +
/// font upload) is ~1.5s idle but was measured at 3.5s under load, in a debug
/// build — which is what `cargo test` runs. Every fixed `sleep`/`timeout` here
/// was calibrated to the idle figure, so on a busy machine rt was killed or
/// screenshotted BEFORE it drew anything; the runs then measured zeros, which
/// silently satisfy `PutImage == 0` and any upper-bound guard. Wait for the
/// condition instead of guessing at a duration.
pub fn wait_for_trace(path: &Path, needle: &str, timeout: Duration) -> Option<usize> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Some(i) = s.find(needle) {
                return Some(i + needle.len());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// True if `prog` is runnable (resolves on PATH / is executable).
pub fn have(prog: &str) -> bool {
    Command::new(prog)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
}
