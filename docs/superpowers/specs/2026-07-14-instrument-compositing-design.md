# Server-Side Instrument Compositing (Slice 2, item 2) — Design

**Status:** built, then CORRECTED in flight — see "Correction: the full-window
composite premise was wrong" below before trusting any cost claim in this doc.

**Goal:** Bring the animated instruments / patch-bay back on the remote (XRender,
`ssh -X`) backend and make them cheap enough to be **on by default** — by drawing
them onto a **separate server-side layer** that is composited over the terminal
content, so instrument geometry is re-shipped at most 6×/sec and **never rides on
a keystroke**.

## Background

Slice 2 gave rt a native XRender chrome path so remote sessions render as X
drawing *commands*, not pixel blits. During the milkv (riscv64, `ssh -X`)
feel-test we found that drawing the animated instruments *inline on the content
back-buffer every frame* re-shipped ~15–40 KB of instrument geometry on every
content frame — so a single keystroke re-tessellated and re-sent the whole
patch-bay. The stopgap was to disable instruments on the remote backend
(`inst_remote = false`). This design removes that stopgap properly.

Local GL is unaffected: it animates instruments through egui every frame (target
"A", smooth) and keeps doing so. All work here is on `XRenderBackend`.

## Decisions (settled during brainstorming)

- **Remote animation target = "B":** a low, bounded tick rate, decoupled from
  content cost. Local GL stays at "A" (smooth, every frame).
- **Default-on remotely:** `inst_remote` flips to `true`. A patch-bay you must
  discover is not much of a feature.
- **6 fps, fixed constant** (one instrument redraw per ~166 ms). Not a config
  knob yet — promote to one later only if a milkv feel-test shows one rate does
  not fit all links.
- **Overlay behaviour:** while an overlay (menu / manual / search) is open, the
  instrument layer is **suppressed** (not composited). Keeps the present path to
  two steps and avoids instruments painting over a modal overlay.

## Architecture

### Two server-side pixmaps

`XRenderBackend` holds two server-side pixmaps instead of one:

- **`content_pix`** (24-bit, window depth) — the grid, cursor, split dividers,
  focus borders, titlebars, bell stripes, and any open overlay. This is exactly
  what `back_pixmap` is today, minus the instruments. Scissored partial redraws
  (typing) draw here, unchanged.
- **`instr_pix`** (32-bit **premultiplied ARGB**, full-window, offscreen) — the
  instrument / patch-bay layer. Transparent everywhere except where instruments
  are drawn. Redrawn only on a 6 fps tick.

### Present = copy content, composite instruments over it

`present()` becomes two server-side operations, both shipping **zero wire
pixels**:

1. `CopyArea content_pix → window` (full window, as today).
2. If the instrument layer is visible this frame:
   `RENDER Composite(OP=Over, src=instr_pix, dst=window)` (full window).

A full-window composite costs the same as a partial one over the wire (it is
server-side), so `present()` stays "always full-window" — the invariant that
fixed the half-drawn-border class of bugs.

> **WRONG — see the correction section at the end of this doc.** True over the
> wire, but silently generalised to "costs the same". A composite is X-server
> CPU proportional to the area it covers, paid on every frame including every
> keystroke. This shipped a typing regression.

No ARGB *window* visual is required; `instr_pix` is an offscreen pixmap whose
depth/format we choose. Compositing a 32-bit ARGB source `OVER` a 24-bit
destination is standard RENDER. (Whole-*window* translucency is a separate
concern — Slice 3.)

### Backend interface

Three additions to the `Backend` trait. They are **no-ops on `GlBackend`**,
which draws instruments through egui and never calls them:

- `begin_instrument_layer(&mut self)` — clear `instr_pix` to fully transparent
  and redirect subsequent draw primitives (`fill`, A8 stamps, triangles) to it.
- `end_instrument_layer(&mut self)` — restore the draw target to `content_pix`.
- `set_instrument_layer_visible(&mut self, bool)` — whether `present()`
  composites `instr_pix` this frame.

`chrome/instruments.rs::draw()` is called **unchanged** between
`begin_instrument_layer` / `end_instrument_layer`; it already draws through
`Backend` primitives, which now land on `instr_pix`.

**Premultiplied alpha (the one delicate detail).** For `OVER` to blend glows and
semi-transparent packets correctly, the primitives must write **premultiplied**
ARGB to `instr_pix`. Instrument colours today come from egui
`Color32::from_rgba_unmultiplied(...)` and rt's `Color(r,g,b,a)`; when the draw
target is `instr_pix`, the backend premultiplies (`r·a, g·a, b·a, a`) before
issuing the RENDER fill / mask composite. The A8 mask stamps (discs, rings) and
the triangle lines already carry coverage/alpha; they must composite into
`instr_pix` with the source alpha preserved rather than flattened against an
opaque background.

## Data flow per frame

The frame builder in `main.rs` already decides full-vs-scissored and already has
an `inst_remote`/overlay gate around the native instrument draw. It gains one
notion: **is this frame an instrument tick?**

- **Content frame (keystroke, output, most frames):** draw content to
  `content_pix` as today. Do **not** touch `instr_pix`. Call
  `set_instrument_layer_visible(inst_remote && !overlay_open)`. `present()`
  re-composites the `instr_pix` already resident on the server. Instrument
  geometry ships **nothing**.
- **Instrument tick (≤ 6 fps, only when `inst_remote && inst_animate &&
  !overlay_open`):** additionally wrap `chrome::instruments::draw()` in
  `begin_instrument_layer` / `end_instrument_layer`, re-shipping instrument
  geometry **once**. Then present as above.
- **Overlay open:** `set_instrument_layer_visible(false)`. `instr_pix` is left
  frozen (not cleared); present skips the composite. On overlay close, visibility
  returns and the frozen layer shows until the next tick refreshes it.

### Animation driver

`about_to_wait` schedules a repaint every ~166 ms while
`inst_remote && inst_animate` and no overlay is open, marking that frame an
instrument tick. `pump_wires` / `sample_heat` continue to advance the animation
state (they already run for their side effects). When `inst_animate` is false the
layer is drawn once (first enable / topology change) and held static — no ticks.

### Config / defaults

- `inst_remote` default flips `false → true` (`rt-config`).
- `inst_animate` default flips `false → true` — a living patch-bay ticking at
  6 fps out of the box, since that decoupled tick is the whole point of this
  slice. (Setting it back to `false` gives a static-but-visible layer; the
  mechanism supports both.)
- The 6 fps interval is a named `const` in `main.rs` (e.g.
  `INSTRUMENT_TICK = Duration::from_millis(166)`).

## Components / files

- `crates/rt/src/xrender_backend.rs` — second pixmap (`instr_pix`) + its ARGB
  Picture; `begin/end_instrument_layer`, `set_instrument_layer_visible`;
  premultiplied writes when the target is `instr_pix`; `present()` composite
  step; `recreate_back` / `resize_surface` recreate both pixmaps.
- `crates/rt/src/backend.rs` — three new trait methods with default no-op bodies
  (so `GlBackend` needs no change) or explicit no-op impls.
- `crates/rt/src/gl_backend.rs` — explicit no-op impls if not defaulted.
- `crates/rt/src/main.rs` — instrument-tick scheduling (6 fps `const`),
  wrapping the native instrument draw in begin/end on ticks, the
  `set_instrument_layer_visible` call each frame, default gating.
- `crates/rt-config/src/lib.rs` — `inst_remote` default `true` (and the
  `inst_animate` default decision).
- `crates/rt/tests/` — composite-identity test; extended xtrace decoupling +
  `PutImage == 0` guard.

## Correctness & performance gates

1. **Composite-identity (offscreen, deterministic).** Draw a known content
   pattern to `content_pix` and a known instrument primitive (e.g. one green disc
   at a known alpha) to `instr_pix`, present, read back the window pixels, and
   assert they equal the exact premultiplied `OVER` blend — transparent instrument
   pixels show content unchanged; the disc blends per its alpha. This is the
   falsifiable check on the premultiplied-alpha detail.
2. **Decoupling guard (xtrace).** A burst of keystrokes whose trace window
   contains **no** instrument tick ships **zero** instrument geometry (no RENDER
   Triangles; `CompositeGlyphs` only for text cells). A window that does contain a
   tick ships the instrument geometry exactly once. This is the falsifiable
   "typing does not re-ship instruments."
3. **`PutImage == 0`** still holds on every XRender frame (commands, not pixels).
4. **milkv feel-test.** With instruments visible and ticking at 6 fps: typing /
   `ls -alR ~` stay as fast as with instruments off; the patch-bay visibly ticks
   without stalling input; opening an overlay hides the instruments and closing it
   restores them.

## Non-goals

- Whole-window translucency / blur (needs an ARGB *window* visual + a compositor)
  — Slice 3.
- A user-configurable instrument frame rate — deferred; fixed 6 fps `const` now.
- Changing the local GL instrument path — it stays smooth egui ("A").
- Unifying the GL and native instrument draws onto one code path — out of scope.

## Alternatives considered (rejected)

- **Single back-buffer, instruments redrawn only on tick / topology change.**
  Content and instruments share pixels, so typing under a wire erases it and
  forces an instrument redraw there — the decoupling leaks and geometry rides on
  keystrokes again.
- **Instruments drawn straight onto the window each tick (no `instr_pix`).** Every
  content `CopyArea` overwrites the window region and wipes the last tick's
  instruments, so each keystroke would have to redraw them — same disease.
- **Higher frame rate (10–12 fps) or re-tessellating instruments server-side
  (target "A" remotely).** Re-fights the byte-volume battle Slice 2 just won; 6 fps
  is the honest rate for `ssh -X`.

## Correction: the full-window composite premise was wrong

Shipped with `inst_remote`/`inst_animate` defaulting on, this design made typing
lag badly on the milkv (riscv64, `ssh -X`) — worse than any prior build. Root
cause, confirmed by bisect (`inst_remote = false` restored fast typing) and then
measured directly:

**The claim "a full-window composite costs the same as a partial one" is false.**
It is true *over the wire* — a `Composite` is a fixed-size request either way,
and `PutImage` stays 0. It is not true of **X-server CPU**, which is
proportional to the area composited. `present()` runs on every frame, so an
unclipped full-window composite is an area-proportional cost **per keystroke**.

Why every gate in this design missed it: the cost is not rt's. rt's CPU is
identical with the layer on or off, so client-side profiling, the xtrace byte
counts, and the `PutImage == 0` invariant all look clean. Measured on the X
server instead (1180×780 window, ~30 keystrokes): **110ms** of server CPU with
instruments on vs **50ms** off, scaling with window area — so a large real
window is considerably worse.

The 6fps tick was never the problem; it is bounded and correct.

**Fix** (`fb3e6d6`): the full-window `CopyArea` already restores content
everywhere, so the composite only needs to re-apply instruments where they
actually are. `XRenderBackend` records each instrument primitive's extent as the
layer is drawn and installs the union as `win_pic`'s clip region; the per-tick
clear is bounded the same way. Re-measured: instrument overhead above the
instruments-off baseline fell from **+120%** to **0–30%** (within noise).

**Defaults stay OFF** pending a feel-test on the real link. The mechanism is
opt-in via `inst_remote = true`.

**Gate this design should have had:** every correctness/perf gate here measures
rt or the wire. For a backend whose whole job is to make the *server* draw, at
least one gate must measure the **server's** CPU. Sampling `/proc/<Xorg>/stat`
across a scripted keystroke burst is enough, and would have caught this before
it ever reached a feel-test.
