# X11 damage-rect present (Route 1 / Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On X11 software GL, present only the damage rectangle via `glReadPixels` + `XPutImage` (no swap), so a keystroke on the milkv over X11-over-ssh drops from ~250 ms to a few milliseconds — reusing all of Phase 1's scissored rendering.

**Architecture:** Add one branch to Phase 1's `redraw()` funnel: when the surface is an X11/GLX window on software GL and an X11 present handle is available, scissor-render only the damage bbox (Phase 1's `begin_frame_scissored`), read back that bbox from the GL back buffer, and `XPutImage` it to the window. No buffer preservation is needed — the X window keeps its other pixels server-side. Everything else (hardware GPU, Wayland/EGL mechanism A, chrome via `egui_glow`) is unchanged.

**Tech Stack:** Rust, `glow` (GL, `glReadPixels`), `x11rb` (`put_image`/`get_image`, already a dep under the `x11` feature), `winit` 0.30 (raw window handle), Phase 1's damage pipeline.

## Global Constraints

- **This is spike-validated** (feasibility, cost ~0.8 ms/keystroke, correct display all confirmed on the milkv under Xvfb/softpipe). No spike gate — implement directly. Spike numbers: 240×64 rect = readback ~0.65 ms + `XPutImage` ~0.14 ms; full window ~56 ms + ~5.5 ms.
- **Reuse ALL Phase 1 plumbing unchanged:** `crate::damage::{DamageAccumulator, PxRect, FrameDamage}`, `border_bands`, `render::{scissor_box, begin_frame_scissored, clear_scissor}`, `force_full`, the `redraw()` gate, `plan_frame`, `redraw_scissored`, `redraw_full`. Do not modify Phase 1's shading.
- **Hardware GPUs and Wayland stay byte-for-byte identical.** Route 1 engages ONLY when `renderer.is_software()` AND an X11 present handle exists. Wayland (EGL) keeps mechanism A; non-software keeps the full path.
- **Always falsifiable to a full frame.** Any `glReadPixels`/`XPutImage` error, an unfamiliar depth (not 24 or 32), or a bbox we can't trust → fall back to a full-window present or plain `swap_buffers`, and re-arm `force_full`. Never corrupt the display.
- **X11 only.** `XPutImage` is X11; the module is `#[cfg(feature = "x11")]` and returns `None` on Wayland. `x11` is a default feature.
- Pixel format: `glReadPixels(GL_BACK, GL_BGRA, GL_UNSIGNED_BYTE)`; row-flip (readback is bottom-up, `XPutImage` `ZPixmap` is top-down); put with the window's depth (24 or 32).
- Coordinates: bbox is top-left physical px (Phase 1's `PxRect`); only `glReadPixels` needs `gl_y = screen_h - (y + h)`.
- Workflow per task: work on branch `damage-rendering-x11-present` (already created off main `e681478`); `cargo build -p rt` + `cargo test -p rt` green; commit with the shown message. Do not merge/push.

## File Structure

- `crates/rt/src/x11_present.rs` — **create.** The X11 present unit: `X11Present` (X connection + window + GC + depth), `try_new`, `present_rect`, and a pure `flip_rows` helper. `#[cfg(feature = "x11")]`. One responsibility: get a rendered rect from GL onto the X window.
- `crates/rt/src/main.rs` — **modify.** `mod x11_present;` decl; `Active.x11_present` field + init; relax the `redraw()` gate; `plan_frame` age-override; X11 present branches in `redraw_scissored` and `redraw_full`.
- `crates/rt/tests/x11_present_roundtrip.rs` — **create.** `#[ignore]`d integration test: render a rect → `XPutImage` → `get_image` → assert equal, under Xvfb.

---

## Task 1: the `x11_present` module

**Files:**
- Create: `crates/rt/src/x11_present.rs`
- Modify: `crates/rt/src/main.rs` (add `mod x11_present;` under the `#[cfg(feature = "x11")]` group near `mod x11_blur;`)
- Test: inline `#[cfg(test)] mod tests` in `x11_present.rs` (pure `flip_rows` test — no GL/X needed)

**Interfaces:**
- Consumes: `winit::window::Window`, `raw_window_handle`, `x11rb`, `glow`.
- Produces:
  - `pub fn flip_rows(buf: &[u8], w: usize, h: usize) -> Vec<u8>` — reverse row order of a tightly-packed `w*h*4`-byte RGBA/BGRA buffer. Pure.
  - `pub struct X11Present` with:
    - `pub fn try_new(window: &Window) -> Option<Self>` — `None` on Wayland or if the window depth is not 24/32 or X setup fails.
    - `pub fn present_rect(&self, gl: &glow::Context, x: i32, y: i32, w: i32, h: i32, screen_h: i32) -> bool` — read back `(x,y,w,h)` (top-left origin) from `GL_BACK` as BGRA, flip rows, `XPutImage` to the window. Returns `true` on success, `false` on any error (caller falls back).

- [ ] **Step 1: Write the failing test (pure row-flip)**

Create `crates/rt/src/x11_present.rs` with only the test first:

```rust
#![cfg(feature = "x11")]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_rows_reverses_row_order() {
        // 2 rows × 1 px × 4 bytes. Row 0 = [1,2,3,4], row 1 = [5,6,7,8].
        let buf = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let out = flip_rows(&buf, 1, 2);
        assert_eq!(out, vec![5, 6, 7, 8, 1, 2, 3, 4]); // rows swapped
    }

    #[test]
    fn flip_rows_single_row_unchanged() {
        let buf = [1u8, 2, 3, 4, 9, 9, 9, 9]; // 2px × 1 row
        assert_eq!(flip_rows(&buf, 2, 1), buf.to_vec());
    }
}
```

- [ ] **Step 2: Register the module and run the test to see it fail**

Add to `crates/rt/src/main.rs` next to the other X11-gated module (find `mod x11_blur;`):

```rust
#[cfg(feature = "x11")]
mod x11_present; // Route 1: X11 damage-rect present (glReadPixels + XPutImage)
```

Run: `cargo test -p rt --bin rt flip_rows 2>&1 | tail -15`
Expected: FAIL — `cannot find function flip_rows`.

- [ ] **Step 3: Implement the module**

Prepend to `crates/rt/src/x11_present.rs` (above the test module):

```rust
//! Route 1 present path: read the damage rectangle back from the GL back buffer
//! and push it to the X11 window with `XPutImage`, so only the changed pixels
//! cross the wire (fast over X11-over-ssh). No buffer preservation needed — the
//! X window keeps its other pixels server-side. X11/GLX only; `try_new` returns
//! `None` on Wayland or an unsupported visual depth.

use glow::HasContext;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateGCAux, Gcontext, ImageFormat};
use x11rb::rust_connection::RustConnection;

/// Reverse the row order of a tightly-packed `w*h*4` buffer. `glReadPixels` is
/// bottom-up; `XPutImage` `ZPixmap` is top-down.
pub fn flip_rows(buf: &[u8], w: usize, h: usize) -> Vec<u8> {
    let stride = w * 4;
    let mut out = vec![0u8; buf.len()];
    for row in 0..h {
        let src = (h - 1 - row) * stride;
        let dst = row * stride;
        out[dst..dst + stride].copy_from_slice(&buf[src..src + stride]);
    }
    out
}

/// An X11 present handle: the connection, the window, a GC, and the window depth.
pub struct X11Present {
    conn: RustConnection,
    window: u32,
    gc: Gcontext,
    depth: u8,
}

impl X11Present {
    /// Build from rt's window. `None` on Wayland, an unsupported depth (not 24/32),
    /// or if X setup fails — the caller then keeps the normal `swap_buffers` path.
    pub fn try_new(window: &Window) -> Option<Self> {
        let win = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => h.window as u32,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => return None, // Wayland: no X present path
        };
        let (conn, screen_num) = x11rb::connect(None).ok()?; // honours $DISPLAY
        let depth = conn.setup().roots[screen_num].root_depth;
        if depth != 24 && depth != 32 {
            log::info!("x11_present: depth {depth} unsupported; using swap_buffers");
            return None; // unfamiliar visual → fall back
        }
        let gc = conn.generate_id().ok()?;
        conn.create_gc(gc, win, &CreateGCAux::new()).ok()?;
        conn.flush().ok()?;
        log::info!("x11_present: ready (window={win:#x} depth={depth})");
        Some(Self { conn, window: win, gc, depth })
    }

    /// Read back `(x,y,w,h)` (top-left origin) from `GL_BACK` as BGRA and
    /// `XPutImage` it to the window. `true` on success; `false` on any error so
    /// the caller can fall back to a full present.
    pub fn present_rect(&self, gl: &glow::Context, x: i32, y: i32, w: i32, h: i32, screen_h: i32) -> bool {
        if w <= 0 || h <= 0 {
            return false;
        }
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let gl_y = screen_h - (y + h); // glReadPixels is bottom-left origin
        unsafe {
            gl.read_buffer(glow::BACK);
            gl.read_pixels(
                x, gl_y, w, h, glow::BGRA, glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut buf)),
            );
            gl.finish(); // force the scissored render + readback to complete
        }
        let data = flip_rows(&buf, w as usize, h as usize);
        let put = self.conn.put_image(
            ImageFormat::Z_PIXMAP, self.window, self.gc,
            w as u16, h as u16, x as i16, y as i16, 0, self.depth, &data,
        );
        match put.and_then(|c| c.check().map_err(Into::into)) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("x11_present put_image failed ({e:?}); full present next frame");
                false
            }
        }
    }
}
```

> Note: `put_image(...).and_then(|c| c.check()...)` waits for the server's error reply so a `BadDrawable`/`BadMatch` returns `false` rather than being swallowed. `c.check()` returns `Result<(), ReplyError>`; map it into the same error type. If the exact error-type conversion is awkward, use `let _ = self.conn.put_image(...).ok()?; self.conn.flush().ok(); true` and rely on the flush — but prefer `check()` so failures are caught.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rt --bin rt flip_rows 2>&1 | tail -15`
Expected: PASS (2 tests). Also `cargo build -p rt 2>&1 | tail -5` → clean (the struct is unused until Task 2 — a `dead_code` warning on `X11Present`/`try_new`/`present_rect` is acceptable; do NOT `#[allow]` it).

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/x11_present.rs crates/rt/src/main.rs
git commit -m "feat(rt): x11_present module (glReadPixels + XPutImage damage-rect present)"
```

---

## Task 2: wire Route 1 into `Active`, the gate, and `plan_frame`

**Files:**
- Modify: `crates/rt/src/main.rs` — `Active` struct (near `surface`/`context`), `Active` construction, the `redraw()` gate condition, `plan_frame`.

**Interfaces:**
- Consumes: `x11_present::X11Present` (Task 1); `Active.surface`, `Active.renderer.is_software()`, `plan_frame`.
- Produces: `Active.x11_present: Option<x11_present::X11Present>` (field, `#[cfg(feature = "x11")]`), and a `redraw()` that routes X11 software-GL frames to the partial path.

**Design note:** After this task, X11 software-GL frames are *planned* as partial and dispatch to `redraw_scissored`, but the actual X11 present is added in Task 3. Until then `redraw_scissored`'s existing `present_with_damage` (EGL) returns `false` on a GLX surface → it falls back to a full redraw + `swap_buffers`. So this task is safe (never corrupts) even before Task 3 lands.

- [ ] **Step 1: Add the `Active` field**

Find `surface: Surface<WindowSurface>,` in `struct Active` and add below `context`:

```rust
    #[cfg(feature = "x11")]
    x11_present: Option<x11_present::X11Present>, // Route 1: X11 damage-rect present (None on Wayland)
```

- [ ] **Step 2: Initialize it in the `Active` construction**

Find `self.active = Some(Active {` and add as the first field (it needs `&window` before `window` is moved in):

```rust
        self.active = Some(Active {
            #[cfg(feature = "x11")]
            x11_present: x11_present::X11Present::try_new(&window),
            window,
            surface,
            context,
            renderer,
            // ... rest unchanged
```

- [ ] **Step 3: Relax the `redraw()` gate to allow X11 partial**

The gate currently forces Full when the surface isn't EGL. Route 1 makes an X11 window with a present handle partial-eligible too. Replace the gate's last condition. Find:

```rust
        if active.force_full
            || overlay_open
            || !active.renderer.is_software()
            || !active.wires.is_empty()
            || !matches!(active.surface, Surface::Egl(_))
        {
```

Replace the `|| !matches!(active.surface, Surface::Egl(_))` line with a term that also accepts an X11 present handle:

```rust
            || !Self::partial_present_available(active)
```

And add this helper method in the same `impl App` block (near `plan_frame`):

```rust
    /// Whether a partial (non-full-swap) present is available this build/surface:
    /// an EGL surface (mechanism A) or an X11 present handle (Route 1). Otherwise
    /// the frame must take the full path.
    fn partial_present_available(active: &Active) -> bool {
        if matches!(active.surface, Surface::Egl(_)) {
            return true; // mechanism A (buffer_age partial swap)
        }
        #[cfg(feature = "x11")]
        if active.x11_present.is_some() {
            return true; // Route 1 (readback + XPutImage)
        }
        false
    }
```

- [ ] **Step 4: `plan_frame` — treat the X window as always-preserved**

Route 1 reads back only this frame's damage and `XPutImage`s it; the X window holds the rest, so the effective back-buffer age is 1 regardless of `buffer_age()`. Find in `plan_frame`:

```rust
        let age = Self::buffer_age(active);
```

Replace with:

```rust
        // Route 1 (X11 present) preserves the window server-side, so only this
        // frame's damage must be redrawn — treat it as age 1 regardless of the
        // GLX buffer_age (which is unusable on softpipe). EGL keeps real age.
        let age = {
            #[cfg(feature = "x11")]
            {
                if active.x11_present.is_some() { 1 } else { Self::buffer_age(active) }
            }
            #[cfg(not(feature = "x11"))]
            {
                Self::buffer_age(active)
            }
        };
```

- [ ] **Step 5: Build and verify no regression**

Run: `cargo build -p rt 2>&1 | tail -5` → clean.
Run: `cargo test -p rt 2>&1 | grep 'test result'` → existing tests pass.
Reason (write nothing, just confirm by reading): on hardware GL `is_software()` is false → gate forces Full → `redraw_full` → `swap_buffers`, unchanged. On Wayland, `x11_present` is `None` and the surface is EGL → `partial_present_available` returns true via the EGL arm exactly as before. On X11 software GL, the frame now plans as partial and dispatches to `redraw_scissored`, whose EGL `present_with_damage` returns false on GLX → safe fallback to full `swap_buffers` (until Task 3).

- [ ] **Step 6: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): make X11/GLX software-GL frames partial-eligible (Route 1 gating)"
```

---

## Task 3: present the damage rect via X11 in `redraw_scissored` and `redraw_full`

**Files:**
- Modify: `crates/rt/src/main.rs` — `redraw_scissored` (partial present) and `redraw_full` (full present).

**Interfaces:**
- Consumes: `Active.x11_present` (Task 2), `X11Present::present_rect` (Task 1), `Active.renderer.gl_ctx()` (add if missing — see Step 1), `Active.window.inner_size()`.
- Produces: X11 frames now present via `glReadPixels` + `XPutImage`; EGL/hardware unchanged.

- [ ] **Step 1: Add a GL-context accessor to `Renderer` (if not present)**

`present_rect` needs `&glow::Context`. In `crates/rt/src/render.rs`, add to `impl Renderer` (near `is_software`):

```rust
    /// Borrow the GL context (for the X11 readback present path).
    pub fn gl_ctx(&self) -> &glow::Context {
        &self.gl
    }
```

- [ ] **Step 2: X11 partial present in `redraw_scissored`**

`redraw_scissored` currently renders the scissored bbox then calls `present_with_damage` (EGL). Add an X11 branch that presents the bbox via `present_rect` *before* the EGL path. Find, in `redraw_scissored`, the block after `clear_scissor()`:

```rust
        active.renderer.clear_scissor(); // next frame starts with a clean scissor
        if !Self::present_with_damage(active, hint_rects) {
```

Insert the X11 present just before that `if`:

```rust
        active.renderer.clear_scissor(); // next frame starts with a clean scissor
        #[cfg(feature = "x11")]
        if let Some(p) = active.x11_present.as_ref() {
            let sh = active.window.inner_size().height as i32;
            if p.present_rect(active.renderer.gl_ctx(), bbox.x, bbox.y, bbox.w, bbox.h, sh) {
                return false; // presented the damage rect via XPutImage; no swap, no re-arm
            }
            // present failed → fall through to the full-redraw fallback below
        }
        if !Self::present_with_damage(active, hint_rects) {
```

The existing fallback block (full redraw + `swap_buffers` + `active.force_full = true` via the returned `true`) already handles the failure case correctly.

- [ ] **Step 3: X11 full present in `redraw_full`**

`redraw_full` currently ends with `swap_buffers`. Add an X11 branch: present the whole window via `present_rect(0,0,W,H)`, falling back to `swap_buffers` on failure. Find in `redraw_full`:

```rust
        Self::paint_overlays_or_instruments(active);
        if let Err(e) = active.surface.swap_buffers(&active.context) {
            log::error!("swap_buffers failed: {e}"); // non-fatal; log and continue
        }
    }
```

Replace with:

```rust
        Self::paint_overlays_or_instruments(active);
        #[cfg(feature = "x11")]
        if let Some(p) = active.x11_present.as_ref() {
            let sz = active.window.inner_size();
            let (w, h) = (sz.width as i32, sz.height as i32);
            if p.present_rect(active.renderer.gl_ctx(), 0, 0, w, h, h) {
                return; // presented the full window via XPutImage; no swap
            }
            // present failed → fall through to swap_buffers
        }
        if let Err(e) = active.surface.swap_buffers(&active.context) {
            log::error!("swap_buffers failed: {e}"); // non-fatal; log and continue
        }
    }
```

- [ ] **Step 4: Build and run the suite**

Run: `cargo build -p rt 2>&1 | tail -5` → clean, zero new warnings (the `dead_code` on `X11Present` from Task 1 is now gone — it's used).
Run: `cargo test -p rt 2>&1 | grep 'test result'` → all green.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/main.rs crates/rt/src/render.rs
git commit -m "feat(rt): present damage rect via glReadPixels+XPutImage on X11 (Route 1)"
```

---

## Task 4: X11 present round-trip integration test

**Files:**
- Create: `crates/rt/tests/x11_present_roundtrip.rs`

**Interfaces:**
- Consumes: `x11rb` (dev-dep — add if the test can't see it; `x11rb` is a normal dep under the `x11` feature, so an integration test built with default features links it). Uses `x11rb::put_image`/`get_image` against an Xvfb server.

**What it proves:** an image put to an X window with `put_image` reads back identical with `get_image` — i.e. the pixel format/depth/round-trip we rely on is correct on this X server. (The GL readback itself is covered by Phase 1's pixel-identity gate + the on-board perf run; this test isolates the X put/get round-trip, which is the new, X-specific risk.)

- [ ] **Step 1: Write the `#[ignore]`d round-trip test**

Create `crates/rt/tests/x11_present_roundtrip.rs`:

```rust
//! X11 put/get round-trip: put a known image to an off-screen pixmap and read it
//! back, asserting byte-identity for the depth-24 ZPixmap path Route 1 uses.
//! Needs an X server; run under Xvfb:
//!   Xvfb :99 & DISPLAY=:99 cargo test -p rt --test x11_present_roundtrip -- --ignored

#![cfg(feature = "x11")]

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateGCAux, ImageFormat};

#[test]
#[ignore = "needs an X server; run under Xvfb with --ignored"]
fn put_get_roundtrip_depth24() {
    let (conn, screen_num) = x11rb::connect(None).expect("connect to $DISPLAY");
    let screen = &conn.setup().roots[screen_num];
    let depth = screen.root_depth;
    assert!(depth == 24 || depth == 32, "test expects a 24/32-bit visual, got {depth}");

    let (w, h) = (16u16, 8u16);
    // A distinct BGRA pattern per pixel so a wrong row/byte order would show.
    let mut data = vec![0u8; (w as usize) * (h as usize) * 4];
    for (i, px) in data.chunks_mut(4).enumerate() {
        px[0] = (i & 0xff) as u8; // B
        px[1] = ((i >> 1) & 0xff) as u8; // G
        px[2] = ((i >> 2) & 0xff) as u8; // R
        px[3] = 0; // X (pad)
    }

    // Off-screen pixmap of the screen depth as the drawable (no window needed).
    let pixmap = conn.generate_id().unwrap();
    conn.create_pixmap(depth, pixmap, screen.root, w, h).unwrap();
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, pixmap, &CreateGCAux::new()).unwrap();

    conn.put_image(ImageFormat::Z_PIXMAP, pixmap, gc, w, h, 0, 0, 0, depth, &data)
        .unwrap()
        .check()
        .expect("put_image");

    let got = conn
        .get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, w, h, !0)
        .unwrap()
        .reply()
        .expect("get_image");

    // Compare the RGB bytes (the pad byte may be forced to 0xff by the server on
    // a 24-bit visual, so ignore byte 3 of each pixel).
    assert_eq!(got.data.len(), data.len());
    for (a, b) in got.data.chunks(4).zip(data.chunks(4)) {
        assert_eq!(&a[0..3], &b[0..3], "pixel RGB round-trips");
    }

    conn.free_gc(gc).ok();
    conn.free_pixmap(pixmap).ok();
}
```

- [ ] **Step 2: Verify it's skipped by default and compiles**

Run: `cargo test -p rt --test x11_present_roundtrip 2>&1 | grep -E 'test result|ignored'`
Expected: `0 passed; 0 failed; 1 ignored` (skipped without `--ignored`), and it compiled.

- [ ] **Step 3: Run it under Xvfb (on any X-capable box, incl. the milkv)**

```bash
Xvfb :99 -screen 0 640x480x24 & sleep 2
DISPLAY=:99 cargo test -p rt --test x11_present_roundtrip -- --ignored 2>&1 | grep 'test result'
```
Expected: `1 passed`. If the box has no Xvfb, run on the milkv (it has one).

- [ ] **Step 4: Commit**

```bash
git add crates/rt/tests/x11_present_roundtrip.rs
git commit -m "test(rt): X11 put/get round-trip for the ZPixmap present path"
```

---

## Task 5: perf + visual verification (controller/user-run)

**Files:** none (verification only).

**What it proves:** the end-to-end win — a keystroke on the milkv drops from ~250 ms to a few ms, and the display is correct over real X11-over-ssh.

- [ ] **Step 1: Build the branch on the milkv**

Transfer the branch (unpushed) to the milkv via `git bundle` (base `04fe7f5`, which the board has) and `cargo build --release -p rt`.

- [ ] **Step 2: Measure keystroke frame cost under Xvfb (softpipe GLX)**

```bash
Xvfb :99 -screen 0 1280x720x24 & sleep 2
DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 RUST_LOG=rt=info ./target/release/rt
# drive single-cell updates (a shell loop printing one char) and inspect frame timing
```
Expected: a single-cell update presents in a few ms (readback + `XPutImage` of the small bbox), not ~250 ms. Confirm frames take the partial (X11) path, not the full fallback.

- [ ] **Step 3: Confirm over real X11-over-ssh + visual**

On the user's real `ssh -X milkv` session, run `./target/release/rt`, type, and run `btop`. Expected: typing latency drops from ~1–2 s to responsive; the display is correct (text, cursor, chrome, patch-bay). This is the user-facing acceptance gate.

- [ ] **Step 4: Note the result** in the branch/PR description (before/after keystroke latency).

---

## Self-Review

**1. Spec coverage** (against `2026-07-12-damage-rendering-x11-present-design.md`):
- Architecture/gating (X11/GLX + software GL → Route 1) → Task 2 (`partial_present_available`, `plan_frame` age override). ✓
- Mechanism (scissor-render bbox → readback → XPutImage, no swap) → Task 1 (`present_rect`) + Task 3 (`redraw_scissored`). ✓
- Chrome (egui unchanged; `force_full` → full present) → Task 3 (`redraw_full` X11 branch); Phase 1's `force_full` already fires on chrome. ✓
- Present-path unit (`x11_present` module, X11-only, `None` on Wayland/bad depth) → Task 1. ✓
- Platform/format (X11 only; depth 24/32 else fall back) → Task 1 `try_new`. ✓
- Failure-safety (any error/unfamiliar depth → full present/swap, re-arm) → Task 1 (`present_rect` → false) + Task 3 (fallbacks) + Task 2 design note. ✓
- No preservation needed → the design's core; realized by reading back only the rendered bbox + X server persistence (Task 3 uses only the bbox). ✓
- Testing: reuse pixel-identity gate (unchanged) + X11 round-trip (Task 4) + perf/visual (Task 5). ✓
- Hardware/Wayland byte-identical → Task 2 Step 5 reasoning + the `is_software()`/EGL gating. ✓

**2. Placeholder scan:** No TBD/"handle errors"/"similar to". The one prose-not-code spot is Task 1's `put_image().check()` error-conversion note, which gives an explicit fallback form — acceptable (the exact `ReplyError` conversion depends on the x11rb version in the tree; both forms are spelled out).

**3. Type consistency:** `X11Present`, `try_new`, `present_rect(gl, x, y, w, h, screen_h) -> bool`, `flip_rows` (Task 1) used identically in Tasks 2–4. `partial_present_available(active) -> bool`, `x11_present` field (Task 2) match Task 3 uses. `renderer.gl_ctx() -> &glow::Context` (Task 3 Step 1) matches its call sites. `PxRect{x,y,w,h}` fields match Phase 1. Gate/`plan_frame`/`redraw_scissored`/`redraw_full` names match main.rs as read.

**Known integration checks for the implementer** (decisions with fallbacks, not gaps): (a) the exact `put_image(...).check()` `ReplyError` conversion — use the fallback `.ok()?` form if the `map_err(Into::into)` doesn't line up; (b) confirm `Surface::Egl` is still the right EGL-variant check in `partial_present_available` (it is, per Phase 1); (c) `glow::BGRA`/`read_buffer` exist in glow 0.17 (verified in the spike).
