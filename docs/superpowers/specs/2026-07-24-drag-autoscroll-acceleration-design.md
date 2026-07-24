# Drag auto-scroll acceleration — design

**Date:** 2026-07-24
**Status:** approved (design), pending implementation plan
**Scope:** feature **C** of the three-part selection vision (A = anchored
selection, shipped v0.3.12; B = clipboard history, shipped v0.3.13).

## Problem

While drag-selecting, pushing the pointer past the pane's top/bottom edge
auto-scrolls so you can select more than a screenful — but at a **flat 1 line
per 35 ms** (~28 lines/s) regardless of how hard/long you push
(`autoscroll_selection`, `crates/rt/src/main.rs:2868`). Selecting across a large
scrollback that way is slow.

## Key constraint (from brainstorming)

The obvious fix elsewhere — accelerate by *distance* past the edge (pointer
position as a throttle) — does **not** work here: the user and colleagues run
the terminal **vertically maximized**, so there is essentially no screen runway
above or below the pane. The pointer clamps at the screen edge a few pixels past
the content and cannot travel further. Distance-based acceleration is therefore
out.

## Decisions (from brainstorming)

1. **Driver: time held.** While the pointer sits in the thin edge zone (in the
   padding just past the content, where it parks against the screen edge), the
   scroll speed ramps the longer it is held there, capped. This works with zero
   runway and mirrors the arrow-key acceleration curve (feature A) for
   consistency.
2. **Controls: shared.** Reuse the existing `arrow_accel` (on/off) and
   `arrow_accel_max` preferences — one "acceleration" concept covering both
   keyboard and drag. No new preference rows.

## Mechanism

Keep `autoscroll_selection`'s edge-zone detection and 35 ms pacing tick exactly
as they are. Change only the amount scrolled per tick:

- Today: `pane.scroll(dir)` once per gated tick (1 line).
- New: scroll `step` lines per gated tick, where
  `step = if arrow_accel { arrow_accel_step(ticks, arrow_accel_max) } else { 1 }`
  (implemented as calling `pane.scroll(dir)` `step` times, or an equivalent
  scroll-by-N), then extend the selection head to the edge row as today.

`arrow_accel_step(repeats, max)` (`main.rs:5076`) already maps a hold count to a
line count: it returns 1 for `repeats < THRESHOLD` (`THRESHOLD = 4`), then ramps
`+1` per repeat up to `max`.

## Feel

- `ticks` counts consecutive 35 ms ticks the pointer has been in the edge zone.
- With `THRESHOLD = 4` (reused), the first ~4 ticks (~140 ms) stay at 1 line/tick
  — a grace period so a quick nudge past the edge is precise — then it ramps
  `+1` line/tick up to `arrow_accel_max`.
- At max 10 that tops out ~285 lines/s; the max slider (1–30) governs the ceiling.
- **Slow/stop:** pull the pointer back up into the visible grid → it leaves the
  zone → the counter resets to 0, so the next push starts slow again.
- **Direction change** (top edge → bottom edge) also resets the counter.

## State

One new `Active` field carrying both the tick count and the direction it was
accumulated in — e.g. `autoscroll_accel: (isize, u32)` = `(last_dir, ticks)` (or
two fields). Detecting a direction flip requires remembering the previous `dir`,
so the counter and the direction are tracked together. Reset `ticks` to 0
whenever:
- the pointer is inside the grid (`dir == 0`), or
- the drag ends (`!selecting`), or
- `dir` differs from the stored `last_dir` (direction flip).

Otherwise increment `ticks` on each gated (actually-scrolling) tick and store the
current `dir`. No other state. (The existing `last_autoscroll: Instant` continues
to pace the 35 ms tick.)

## Preferences

Reuses `arrow_accel` and `arrow_accel_max` unchanged. With acceleration **off**,
`step` is forced to 1 — byte-for-byte today's behavior (1 line / 35 ms). The
feature is purely additive: off ⇒ unchanged.

## No conflicts

`autoscroll_selection` is gated on `active.selecting` (a held plain drag-select).
The anchored-selection mode (`active.composing`) drives scrolling through its own
keyboard path (`scroll_head_into_view`) and never enters `autoscroll_selection`,
so features A and C do not interact.

## What is reused vs new

**Reused:**
- `arrow_accel_step` (the ticks→lines curve).
- The `arrow_accel` / `arrow_accel_max` preferences.
- `autoscroll_selection`'s edge-zone detection, 35 ms `last_autoscroll` gate, and
  head-to-edge extension.
- `pane.scroll(dir)`.

**New:**
- The `autoscroll_ticks` counter + its reset rules.
- The per-tick loop that scrolls `step` lines instead of one.

## Testing

- **Pure (TDD):** extract the tick→step + reset decision into a small pure helper
  and unit-test: `ticks < THRESHOLD` ⇒ step 1 (grace); ramps `+1` per tick up to
  `max`; `arrow_accel` off ⇒ always 1; reset on `dir == 0` / direction flip.
  (`arrow_accel_step` itself is already unit-tested.)
- **Integration (manual, dop651):** on a **vertically maximized** terminal,
  drag-select past the bottom into a large scrollback and confirm it starts slow,
  accelerates, tops out at the max, and resets to slow when the pointer is pulled
  back into the grid; with "Hold-arrow acceleration" **off**, it stays a flat
  1 line/tick (today's behavior); the same holds dragging past the top.

## Out of scope

- Distance-based (pointer-position) acceleration — ruled out by the
  vertically-maximized constraint.
- Mouse-wheel-while-dragging as a separate fast-scroll gesture — a different
  feature; the anchored-selection mode already covers "scroll freely while
  building a selection."
