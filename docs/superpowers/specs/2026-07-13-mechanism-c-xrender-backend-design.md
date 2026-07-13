# Mechanism C — command-based (X11/XRender) rendering backend, Slice 1 — design

Status: approved design, pre-implementation.
Date: 2026-07-13.
Builds on: Phase 1 (damage-based GL partial redraw, merged), Route 1 (X11 pixel-rect
present — superseded for the remote case by this backend; see "Relationship").

## Problem

rt is slow over `ssh -X` and Terminator is fast — on the *same board, over the same
link*. Measured at the X protocol level (`xtrace` on the milkv, same "hello" text):

| | rt | Terminator |
|---|---|---|
| Total client→server bytes | **2.51 MB** | **48 KB** |
| Text/glyph draw requests | **0** | 18 (`AddGlyphs`+`CompositeGlyphs`) = 2 KB |
| `PutImage` (pixel bitmaps) | **38 = 2.50 MB** | 4 (minor) |
| Biggest single request | **`PutImage` 960×600 = 2.20 MB** | 27 KB (one-time) |

rt ships **pixels**; Terminator ships **drawing commands** (upload each glyph once,
then draw by glyph-index reference). ~1250× less data for text. The 2.2 MB
full-window `PutImage` *is* the ~4 s remote menu.

Neither GLX nor Wayland offers an escape: **indirect GLX** (remote GL commands) is
disabled by default on modern X servers and never supported shader-based OpenGL —
which rt uses — so modern GL can't be remoted; it falls back to client-side software
GL + pixel transfer. **Wayland** has no drawing-command layer at all (clients render
buffers; remoting via waypipe/VNC ships compressed *pixels*). So the only living
command-based fast path for a text UI is **X11's 2D drawing protocol (XRender)** —
exactly what Cairo/Qt-xcb/Terminator use.

## Goal

- Make rt over `ssh -X` fast — typing, scrolling, scrollback, resize, multi-pane —
  by rendering the terminal grid as **X drawing commands (XRender glyph sets +
  fills)**, not GL-rendered pixels. Match Terminator's wire profile: KB of commands,
  not MB of pixels.
- **The local path is untouched.** The existing OpenGL glyph-atlas renderer
  (winit + glutin + glow + egui_glow) stays byte-for-byte for local/Wayland/hardware.
  Mechanism C is a *separate* backend, selected only for the remote case.

## Non-goals (Slice 1)

- **Chrome remotely** — the egui overlays (menu / preferences / manual / search /
  border instruments / patch-bay) are `egui_glow` and cannot render without GL. Slice
  1 degrades them gracefully (see below); full remote chrome is **Slice 2**.
- **Translucency** — needs an ARGB visual + compositor (Terminator's approach);
  **Slice 3**.
- **Color emoji** — XRender `A8` glyphs are coverage-only (grayscale AA); colour
  emoji don't render in colour. Documented limitation.
- **Any change to the local GL renderer's behaviour or output.**

## Architecture

A `Backend` trait with two implementations, selected at startup. Everything above the
renderer is shared and unchanged:

```
winit window + event loop + input + session + PTY + damage   ← SHARED, unchanged
                        │
                 trait Backend
                 ┌──────┴───────┐
            GlBackend        XRenderBackend            ← the only new leaf
        (today's render.rs,  (x11rb RENDER: glyph sets
         a mechanical wrap,   + fills, draws into winit's
         byte-identical)      X11 window; NO GL context)
```

- **`draw_panes` already computes *what* to draw** (which cell, glyph, colours,
  position). The `Backend` trait exposes the primitive ops it needs — `fill_rect`,
  `draw_glyph_run`, `draw_cursor`, `present`, `cell_size`, `resize` — so "what" stays
  shared and only "how" differs (GL quad vs XRender command).
- **winit stays** for both backends. `XRenderBackend` draws into winit's *existing*
  X11 window via `x11rb` (window id from the raw handle, as Route 1 already does) —
  no forked event loop, no reimplemented input.
- **The remote backend creates no GL context** — no glutin, no glow, no GLX-over-ssh.
  This both removes a class of ssh cost and is why egui chrome is degraded in Slice 1.
- **`GlBackend` is a near-mechanical wrap** of the current `render.rs`; local output
  is byte-identical.

### Backend selection

Auto-detect the X connection, with an explicit override:

- **Local unix socket** (`DISPLAY=:0`, `/tmp/.X11-unix`) → `GlBackend`. Local behaviour
  is therefore provably unchanged.
- **TCP / forwarded** (`ssh -X` presents as `localhost:10.x` over TCP) → `XRenderBackend`.
- **Override:** `--backend gl|xrender` or `RT_BACKEND=gl|xrender` forces either, for
  the rare misdetection.
- **Wayland** (no `$DISPLAY`, `WAYLAND_DISPLAY` set) → `GlBackend` unchanged (mechanism
  C is X11-only).

## XRender rendering (the core)

rt already produces exactly what XRender consumes.

- **Text via glyph sets — reusing rt's rasterisation.** rt already rasterises each
  glyph to a coverage (alpha) bitmap with `fontdue` for its GL atlas. `XRenderBackend`
  runs the *same* rasterisation but uploads each glyph to an XRender **`GlyphSet`**
  via `RenderAddGlyphs` — **once per unique (char, bold, italic)**, keyed exactly like
  today's glyph cache. Coverage bitmaps are XRender's `A8` glyph format, a direct
  handoff. Each frame draws text with **`RenderCompositeGlyphs`**: runs of
  *glyph-index + position*, composited through a solid-colour source Picture (the fg
  colour) onto the window. Glyphs are pixel-identical to the GL path (same fontdue
  output).
- **Backgrounds, cursor, selection — fills.** Cell backgrounds are
  `RenderFillRectangles` (colour + rects, server-side), batched into same-colour runs.
  The block cursor is a fill plus re-compositing the covered glyph in the inverse
  colour; selection is cells drawn with the selection background. Underline/strike are
  fill lines. All of this is the same "what" `draw_panes` already computes.
- **Damage-incremental, and cheap even when it isn't.** Reuses Phase 1's cell damage:
  a keystroke sends fills+glyphs for only the changed cells (the X window holds the
  rest server-side — the same preservation insight Route 1 used) — a few hundred
  bytes. And unlike Route 1, **a *full* redraw is also cheap** — a whole screen of
  glyph-index runs is a few KB of commands, not a 2.2 MB blit — so **resize,
  scrollback, and first-paint are fast** (the cases Route 1 could not fix).
- **Flicker / double-buffering.** Slice 1 draws damaged cells directly into the window
  (each cell's fill-then-glyph is visually atomic; only changed cells are touched). If
  tearing appears, the refinement is to draw into a server-side back **Pixmap** and
  `CopyArea` the damage rect to the window — and because the pixmap lives on the
  server, that copy is a tiny `CopyArea` request over the wire, not pixels. Flicker-free
  stays command-cheap.

## Chrome degradation (Slice 1)

On `XRenderBackend`, anything drawn via `egui_glow` is skipped, never crashed. Most
*function* is backend-agnostic and keeps working:

- **Works:** typing, scrollback, mouse selection **and its highlight**, keyboard
  copy/paste (clipboard is x11rb/arboard, not egui), splits/tabs, resize, pane
  dividers and titlebars (drawn by the renderer as fills+text → XRender draws them).
- **Skipped until Slice 2:** the context menu, preferences, manual, search bar, and
  the border instruments / patch-bay (all `egui_glow`). Opening one is a no-op with a
  one-line hint — never a hang.

So remotely you get a fully usable multi-pane terminal; only the egui overlays are
absent.

## Reuse vs. new

- **Reused unchanged:** fontdue rasterisation, the glyph-cache keying, colour
  resolution, the Phase-1 damage accumulator, session / selection / scrollback / input
  (all backend-agnostic).
- **New:** the `Backend` trait, the `XRenderBackend` impl (x11rb RENDER: glyph sets,
  `CompositeGlyphs`, `FillRectangles`, `CopyArea`), backend selection, and a
  mechanical `GlBackend` wrap of `render.rs`.

## Testing / verification

- **Command-not-pixels regression (the on-point test):** run `XRenderBackend` under
  Xvfb, drive text, capture with `xtrace`, and assert we emit
  `CompositeGlyphs`/`FillRectangles` and **~zero `PutImage`**, total wire bytes in KB
  not MB. The measurement that proved the problem becomes the guard against regressing
  it.
- **Correctness:** glyph rendering is inherited (same fontdue output); a visual capture
  under Xvfb compares text renders.
- **Perf (the real gate):** the milkv over actual `ssh -X` — typing/scroll/menu/resize
  latency vs. Terminator; and the `xtrace` byte-count as an objective proxy.
- **Local unchanged:** a unix socket always selects `GlBackend`; local output and
  behaviour are byte-identical (the wrap is mechanical).

## Phasing

- **Slice 1 (this spec):** `Backend` trait + auto-selection + `XRenderBackend` for the
  terminal grid (text/bg/cursor/selection/scrollback/splits/dividers/titlebars). Chrome
  degraded.
- **Slice 2:** chrome remotely — an XRender-drawn context menu, and egui-to-pixmap
  (present only its region, only on change) for preferences/manual.
- **Slice 3:** translucency (ARGB visual + compositor).

## Relationship to Phase 1 and Route 1

- **Phase 1** (damage accumulator, cell damage) is reused directly to drive
  incremental XRender drawing.
- **Route 1** (GL readback + `XPutImage` of the damage rect) proved the
  server-side-preservation insight but ships pixels; for the **remote** case it is
  **superseded** by `XRenderBackend` (remote → XRender, no GL, no readback). Route 1's
  present path remains only in `GlBackend` for any local-X11-software-GL use; whether to
  keep or retire it is a minor cleanup, out of scope here.

## Risks / open questions

- **The `Backend` trait extraction is the main risk.** `render.rs`/`draw_panes` are
  tangled with GL specifics (scissor, atlas, begin/end frame). The trait must come out
  cleanly with `GlBackend` as a near-mechanical wrap, verified local-byte-identical.
  This is the part to do carefully and review hardest.
- **Window visual on the remote backend.** We create winit's window *without* a glutin
  GL context and draw XRender to a TrueColor visual; confirm winit hands us a usable
  visual with no GL config (winit `Window` is GL-agnostic, so it should).
- **Glyph coverage limits.** Grayscale AA, bold/italic, underline/strike map cleanly;
  colour emoji do not (A8 coverage only) — documented.
- **No RENDER extension** (ancient servers) — fall back to core-X text or the pixel
  path; rare, noted.
- **Detection edge cases** — the unix-vs-TCP heuristic is covered by the `--backend`
  override.
