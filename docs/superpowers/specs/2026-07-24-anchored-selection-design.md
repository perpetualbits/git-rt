# Anchored selection — design

**Date:** 2026-07-24
**Status:** approved (design), pending implementation plan
**Scope:** feature **A** of the three-part selection vision. Feature **B**
(clipboard history in the titlebar) and feature **C** (drag auto-scroll
acceleration) are deferred to their own spec → plan → build cycles.

## Problem

Selecting text that spans more than a screenful is awkward. Today you must
hold the left button and drag past the pane edge, waiting on a fixed 35 ms
edge auto-scroll (`autoscroll_selection`, `crates/rt/src/main.rs:2650`) that
crawls at one constant speed. Selecting a long key, a multi-screen listing,
or "everything from here to the very end" is slow and imprecise.

The idea: **decouple the two endpoints from a single held drag.** Drop a start
anchor, navigate freely to the target (keyboard *and* scrollbar), then set the
end. No terminal does this.

## Model

The existing `Selection` struct (`crates/rt/src/main.rs:399`) already has
everything the endpoints need:

- `pane: PaneId` — the pane the selection lives in
- `anchor: (usize, i32)` / `head: (usize, i32)` — **absolute** (col, buffer line)
- `block: bool` — rectangular vs row-major

Because endpoints are anchored to *absolute* buffer lines, an anchor stays
pinned to its content while the view scrolls — the core requirement of a
"set start, then navigate away" mode. No new geometry is required.

New state: an optional compose handle beside the current `selection`, e.g.

```rust
composing: Option<Selection>,  // Some while an anchored selection is being built
```

While `composing` is `Some`, the focused pane is in a **modal** selection
state: keyboard input drives the selection instead of reaching the shell.

## Interaction

### Entering

- **Shift+click** (press then release with no drag) drops the *start* anchor at
  the clicked cell and enters compose. The anchor's `head` starts equal to the
  anchor.
- **Shift+drag** is unchanged — a normal linear drag-select.
- Resolution: a Shift+press is *pending* until either the pointer moves past the
  click threshold (→ it becomes a normal drag-select, no compose) or the button
  releases with no motion (→ the anchor drops and compose begins). So the same
  gesture start can still become a plain drag; only a clean click enters the
  mode.
- Holding **Ctrl** on the first Shift+click makes the selection **rectangular**
  (`block = true`), consistent with today's Ctrl-drag block select
  (`crates/rt/src/main.rs:1980`). Block is fixed at the start click; there is no
  live linear/block toggle in v1.

### Composing

All keyboard input is captured by the mode (nothing leaks to the shell) until
commit or cancel:

- **Arrows** move the **head** live: Left/Right by one cell, Up/Down by one row.
  The highlight grows as the head moves and the view **auto-scrolls to keep the
  head visible**. Holding an arrow **accelerates** through the arrow-accel path
  (`arrow_accel_step` / `arrow_hold`, `crates/rt/src/main.rs:3089`, `4690`),
  honoring the `arrow_accel` on/off and `arrow_accel_max` preferences (1:1 when
  acceleration is off).
- **Home / End** move the head to the start / end of its line.
- **PageUp / PageDown** move the head by a screenful.
- **Ctrl+Home / Ctrl+End** move the head to the top / bottom of the buffer — so
  "select from here to the very end" is two keystrokes.
- **Scrollbar drag** and **wheel** scroll the viewport freely; the head stays
  pinned to its absolute cell (it may scroll off-screen — that is fine).
- A **Shift+click** places the head directly at the clicked cell (including in
  scrollback after you have scrolled there).

### Committing

A second **Shift+click**, or **Enter**, finalizes the selection and copies its
text to **both CLIPBOARD and PRIMARY** (the goal is text ready to paste
anywhere, not just middle-click primary). The finalized selection remains
highlighted afterward, exactly like a completed drag-select, so it can be seen
and re-copied.

### Cancelling

**Esc**, or a **plain (non-Shift) left-click** anywhere, discards the in-progress
selection, exits compose, and touches no clipboard.

### Multi-pane

The compose is bound to the pane where the first Shift+click landed (via
`Selection::pane`). A Shift+click in a *different* pane cancels the current
compose and starts a fresh anchor there. A plain click anywhere cancels.

## Titlebar indicator

While composing, the focused pane's titlebar shows a compact status:

- linear: `◉ selecting · 42 lines`
- block: `◉ selecting · 12×40`

Non-clickable in v1. The clickable clipboard-history affordance is feature B.

## What is reused vs new

**Reused:**
- Absolute-line anchoring in `Selection` — survives scrolling unchanged.
- The scroll-to-follow logic inside `autoscroll_selection` — now driven by
  keyboard head movement rather than pointer edge proximity.
- `arrow_accel_step` / `arrow_hold` for held-arrow acceleration.
- `selection_text` for producing the copied text (already skips wide-char
  spacers and rejoins soft-wrapped lines).
- `Selection::contains` for drawing the highlight.
- The clipboard/PRIMARY store paths already used on drag-select release.

**New:**
- The `composing` state + its state machine (enter / navigate / commit / cancel).
- Capturing keyboard input while composing, branching arrow handling away from
  `feed_input` toward head movement.
- The titlebar compose indicator (linear line count / block dimensions).

## Testing

- Head movement: arrows move the head the right number of cells/rows; Home/End,
  Page keys, Ctrl+Home/End land where specified.
- Absolute anchoring: after dropping an anchor and scrolling the view (wheel /
  scrollbar), the anchor still maps to the same buffer content; the committed
  text matches.
- Commit copies to both CLIPBOARD and PRIMARY; the text equals a drag-select of
  the same range (linear and block).
- Cancel (Esc and plain click) leaves the clipboards untouched and clears
  compose state.
- Modal capture: while composing, arrow keys do **not** reach the shell; after
  commit/cancel they do again.
- Block: Ctrl on the first Shift+click yields a rectangular selection whose text
  matches a Ctrl-drag of the same corners.
- Indicator text reflects linear line count vs block dimensions.

## Out of scope (own turns)

- **B** — clipboard history: a ring of recent clippings surfaced (and clickable)
  in the titlebar.
- **C** — drag auto-scroll acceleration: ramp the existing fixed 35 ms edge
  auto-scroll so it speeds up the longer/further the pointer pushes past the
  edge, mirroring the arrow-accel curve.
