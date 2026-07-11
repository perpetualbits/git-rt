# Damage-based rendering for rt — design

Status: approved design, pre-implementation.
Date: 2026-07-11.

## Problem

rt redraws the **entire window every frame** via its custom OpenGL renderer
(glyph atlas + one `draw_arrays`, `render.rs`). On a real GPU this is fine. On a
**software GL** rasteriser (a GPU-less board, or any X11-over-ssh session where
indirect/software GL is used) it is not:

- Measured on a StarFive JH7110 (riscv64) board: **~250 ms per full-window frame.**
- Consequence: a single keystroke lags **1–2 s**, and `btop` (many frames/s) is
  unusable.
- Terminator on the same X11-over-ssh link is fast, because it redraws only the
  **cells that changed** (XRender damage), not the whole window.

Two measurements shape the design:

1. **The engine already tracks damage.** `alacritty_terminal` exposes
   `Term::damage()` → `TermDamage` (either `Full`, or per-line
   `LineDamageBounds { line, left, right }`) and `Term::reset_damage()`. rt just
   doesn't carry it through yet.
2. **The cost is mostly the *present*, not the shading.** The same software
   renderer is **cheap under headless Weston** (Wayland zero-copy present) but
   **~250 ms/frame under Xvfb** (GLX full-buffer swap). So "redraw only changed
   cells" alone is insufficient — we must also **present only the changed
   region**. Partial present is easy on Wayland/local and hard over indirect GLX.

## Goals

- Per-update render **and** present cost scales with **changed cells**, not
  window size (~1 ms for a keystroke instead of ~250 ms).
- Usable typing and `btop` on software GL, including X11-over-ssh (Phase 2).
- **Hardware-GPU path stays byte-for-byte identical** in output. Damage is an
  optimization with a full-redraw fallback; it is never a correctness dependency.

## Non-goals

- A second, non-GL rendering backend (XRender) — documented as reserve **C**, not
  built unless A/B fail.
- Changing the glyph-atlas renderer's drawing model or the visual design.

## Target & phasing (decided)

**Both, phased.**

- **Phase 1** — render-side damage + local/Wayland partial present. Lower risk,
  clear win on local software-GL and Wayland. Verified on the board via Xvfb.
- **Phase 2** — X11-over-ssh (indirect GLX) present. The hard part, isolated.
  Verified on the milkv over ssh.

Phase 1's success bar: **cheap on local software-GL / Wayland**; X11-over-ssh is
explicitly deferred to Phase 2.

## Chosen mechanism (decided): **A**, with B/C in reserve

**A. Preserved-buffer + scissored redraw + swap-with-damage** — reuse the existing
GL renderer. Preserve the back buffer across swaps; each frame re-render only the
damaged cells (`glScissor`, no full clear); present with
`swap_buffers_with_damage(&rects)` where the driver supports it, else fall back.

Reserves, with the trigger that would make us switch:

- **B. Persistent FBO + damage blit** — render into an offscreen FBO that's never
  cleared; blit/readback only the damage rect to present. *Trigger:* preserved
  back-buffer swap proves unreliable/corrupting on a target driver.
- **C. Second X11-native (XRender) backend** — damage-based, non-GL, Terminator
  style. *Trigger:* Phase-2 X11 present can't be made cheap enough with
  readback + `XPutImage`.

## Architecture / data flow

```
alacritty_terminal damage  ->  rt-engine (Snapshot + Damage)  ->
  rt damage accumulator (damage.rs)  ->  render.rs (partial redraw)  ->  present
```

### Component 1 — `rt-engine`: expose damage

- Track and return the terminal's damage alongside the `Snapshot`. Introduce a
  `Damage` value: `Full` | `Lines(Vec<LineDamageBounds-equivalent>)` in the
  pane's cell coordinate space.
- Call `Term::reset_damage()` after producing each snapshot, so damage is
  relative to the previous rendered frame.
- Interface (decided): the `Snapshot` gains a `damage: Damage` field, populated by
  `snapshot()` / `snapshot_lines()`. `Damage` is a new `rt-engine` enum:
  `Full` | `Lines(Vec<CellDamage>)`, where `CellDamage { line: usize, left:
  usize, right: usize }` mirrors `LineDamageBounds` in the pane's cell space. A
  single owner (the snapshot) carries it, so the renderer never queries the
  engine separately. `Full` is always a valid answer (scroll/resize/first frame).

### Component 2 — `rt/damage.rs` (new): the damage accumulator

rt has damage sources beyond the grid. The accumulator unions them into a
coalesced set of **pixel rectangles** for the frame:

- Engine cell-damage (mapped from cell-space to pixels via the cell metrics).
- **Cursor** moved: old cell ∪ new cell.
- **Scroll / resize / first frame** → `Full`.
- **Selection** change → the affected cell span.
- **Animated chrome** (border instruments, jacks, wire packets) → each element's
  region, only when it ticks. On software GL these are already throttled to
  ~2 fps and the cursor blink is frozen by low-power mode, so they contribute at
  most small ~2 fps rects.
- **egui overlay** open/changing (menu / preferences / search / manual) → `Full`
  for that frame (overlays are transient and user-driven; correctness over
  micro-optimisation).

Anything uncertain resolves to `Full`. Rects are merged/coalesced to avoid many
tiny scissor passes.

### Component 3 — `rt/render.rs`: partial redraw

- **Preserve** the back buffer (do not clear the whole window each frame; the
  previous frame's pixels persist).
- For the frame's damage: set `glScissor` to the union of damage rects, clear
  only those rects to background, and re-emit + draw only the geometry
  (cells, chrome) intersecting them. `glScissor` clips fragment output to the
  damaged pixels even if a quad spans outside.
- `Damage::Full` → today's full-window path, unchanged.

### Component 4 — present (phased), in `render.rs` / `main.rs`

- **Phase 1 (Wayland / EGL):** `GlSurface::swap_buffers_with_damage(&context,
  &rects)`, relying on `EGL_EXT_buffer_age` / preserved-swap so stale regions are
  known and redrawn. Cheap local present.
- **Phase 2 (X11 indirect / ssh):** where damage-swap and buffer preservation are
  unavailable, present only the damage rect via `glReadPixels(rect)` →
  `XPutImage(rect)` to the window, bypassing the GL swap for presentation.

### Component 5 — `rt/main.rs`: wiring

- Thread the accumulator into the `redraw` path.
- Select the present mechanism by backend and phase (Wayland damage-swap vs X11
  readback+XPutImage vs full swap).

## Correctness & reserves

- Damage is **always falsifiable to `Full`** — a correct full frame beats a
  corrupt partial one. A `force_full` flag covers any case damage might miss.
- Reserves **B** (FBO) and **C** (XRender) with their switch triggers, above.

## Testing / verification

- **Correctness (automated):** an offscreen render test renders a grid, applies a
  known single-cell change, and asserts the **damage-redraw framebuffer is
  pixel-identical to a full redraw** (`glReadPixels` compare). This is the gate:
  damage must never change what's on screen, only how much work produced it.
- **Performance (Phase 1):** Xvfb on the board (the proxy that exposed the
  250 ms) — a keystroke's frame cost must drop from ~250 ms to ~ms.
- **Performance (Phase 2):** the milkv over X11-over-ssh — typing latency and
  `btop` usability.
- **Regression:** the hardware-GPU path must be visually identical (full-redraw
  fallback guarantees it).

## Risks / open questions

- **Buffer preservation availability.** `EGL_EXT_buffer_age` / preserved swap may
  not exist on every target; the accumulator must fall back to `Full` when it
  can't trust preserved contents. If preservation is broadly unreliable → switch
  to reserve **B** (FBO).
- **Phase-2 present is the genuine unknown.** `swap_buffers_with_damage` is
  unlikely over indirect GLX; the `readback + XPutImage` fallback is the concrete
  plan, but its cost (readback of the damage rect on a weak CPU) must be measured
  on the milkv. If it can't beat the current cost → reserve **C**.
- **Chrome damage granularity.** The instruments live on pane borders; their
  damage rects must be tight (the border band, not the whole pane) to stay cheap.
