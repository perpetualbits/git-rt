# Damage-based Rendering — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On software GL, make a terminal update's render+present cost scale with the number of changed cells (a keystroke ≈ a few ms) instead of the whole window (~250 ms), by threading the engine's existing damage through to a scissored partial redraw and an EGL buffer-age partial present — with a full-redraw fallback that keeps the hardware-GPU path byte-for-byte unchanged.

**Architecture:** `alacritty_terminal` already tracks per-line damage. Task 1 surfaces it on `Snapshot`. Task 2 adds a pure pixel-rect accumulator (`rt/damage.rs`) that unions cell damage + cursor + chrome + overlays into a coalesced damage set, falling back to `Full` for anything uncertain. Task 3 adds scissored-redraw primitives to `render.rs`. Task 4 adds an EGL buffer-age partial-present path. Task 5 wires it into `main.rs`, **gated on `renderer.is_software()`** so hardware GPUs keep today's exact full-redraw path. Task 6 is the offscreen pixel-identical correctness gate plus the milkv/Xvfb perf check.

**Tech Stack:** Rust, `alacritty_terminal` (vendored), `glow` (GL), `glutin` 0.32.3 (EGL surface: `buffer_age()`, `swap_buffers_with_damage()`), `winit` 0.30, `egui` 0.35.

## Global Constraints

- **Hardware-GPU path must stay byte-for-byte identical.** The entire damage fast-path (partial redraw + partial present) is taken **only when `active.renderer.is_software()` is true**. When false, `redraw()` runs exactly today's code. This is the primary regression guard — copy this value verbatim into every gating decision.
- **Damage is always falsifiable to `Full`.** Any uncertainty (scroll, resize, first frame, display-offset ≠ 0, newspaper columns `columns_of(id) > 1`, an open egui overlay, `buffer_age() == 0`, a missing EGL extension, a present error) resolves to a full redraw + full swap. A correct full frame always beats a corrupt partial one.
- **`swap_buffers_with_damage` is only a compositor *hint*; it does not preserve buffers or render partially.** Buffer preservation is our responsibility via `buffer_age()`: the back buffer we get is `age` frames old, so we must redraw the union of the last `age` frames' damage. `age == 0` → redraw everything.
- **Phase 1 target is local / Wayland software GL, verified via Xvfb on the milkv.** X11-over-ssh (indirect GLX) present is **Phase 2 and out of scope here.**
- Mechanism **A** only (preserved-buffer + scissored redraw + swap-with-damage). Reserves **B** (persistent FBO) and **C** (XRender) are documented in the spec and **not built**.
- Pixel coordinates in this plan are **physical pixels, top-left origin** (winit/`content_bounds` convention). GL scissor and EGL damage rects are **bottom-left origin** — every conversion flips Y explicitly. This flip is the single most bug-prone line; it has its own pure unit test.
- Workflow per task: branch off is already done (work on the current feature branch); `cargo test -p <crate>` green; commit with the message shown. Do **not** merge/push — the user runs the release workflow (branch → merge `--no-ff` → push) separately.

---

## File Structure

- `crates/rt-engine/src/lib.rs` — **modify.** Add `Damage` enum + `CellDamage` struct; add `damage` field to `Snapshot`; refactor `snapshot()` to extract `capture_locked()`; add `TermPane::render_snapshot()`.
- `crates/rt/src/damage.rs` — **create.** Pure, GL-free damage accumulator: `PxRect`, `FrameDamage`, `DamageAccumulator`. Fully unit-testable without a GL context.
- `crates/rt/src/render.rs` — **modify.** Add pure `scissor_box()`; add `begin_frame_scissored()` and `clear_scissor()`; keep `begin_frame()`/`end_frame()` unchanged.
- `crates/rt/src/main.rs` — **modify.** Own a `DamageAccumulator` + last-two-frames damage history on `Active`; feed damage sources; branch `redraw()` on `is_software()` + `FrameDamage`; add the buffer-age partial-present helper.
- `crates/rt/tests/damage_pixel_identity.rs` — **create.** `#[ignore]`d headless-EGL integration test: single-cell change, damage-redraw framebuffer must be pixel-identical to a full redraw. The spec's correctness gate; run on a machine with (software) GL.

---

## Task 1: rt-engine exposes damage on the snapshot

**Files:**
- Modify: `crates/rt-engine/src/lib.rs` (Snapshot struct ~116; `snapshot()` ~448; new methods after it)
- Test: `crates/rt-engine/src/lib.rs` (new `#[cfg(test)] mod damage_tests` at end of file)

**Interfaces:**
- Produces:
  - `pub enum Damage { Full, Lines(Vec<CellDamage>) }`, `impl Default for Damage` → `Full`.
  - `pub struct CellDamage { pub line: usize, pub left: usize, pub right: usize }` (inclusive `left..=right`, in viewport cell coords).
  - `impl Damage { pub fn contains_line(&self, line: usize) -> bool; pub fn is_full(&self) -> bool }`.
  - `Snapshot.damage: Damage` (public field; `#[derive(Default)]` on `Snapshot` still holds since `Damage: Default`).
  - `TermPane::render_snapshot(&self) -> Snapshot` — like `snapshot()` but populates `damage` from `Term::damage()` and calls `Term::reset_damage()`, all under **one** lock. Precise `Lines` only when single-column and `display_offset == 0`; otherwise `Full`.
- Consumes: `alacritty_terminal::term::{TermDamage, Term}` (already a dependency); `LineDamageBounds { line, left, right }`.

**Design note (why not fold damage into `snapshot()`):** `Term::damage()` is `&mut self` and has side effects (it records cursor damage and advances `last_cursor`); it must be called **exactly once per frame**. `snapshot()` has five call sites (render, `cell_at`, search, scroll math) — resetting/consuming damage there would corrupt tracking. So `snapshot()` stays a pure read (its `damage` field defaults to `Full`), and only the render path calls `render_snapshot()`.

- [ ] **Step 1: Write the failing tests**

Add at the end of `crates/rt-engine/src/lib.rs`:

```rust
#[cfg(test)]
mod damage_tests {
    use super::*;

    #[test]
    fn damage_default_is_full() {
        assert_eq!(Damage::default(), Damage::Full);
        assert!(Snapshot::default().damage.is_full());
    }

    #[test]
    fn contains_line_semantics() {
        assert!(Damage::Full.contains_line(7)); // Full covers everything
        let d = Damage::Lines(vec![
            CellDamage { line: 2, left: 0, right: 3 },
            CellDamage { line: 5, left: 1, right: 1 },
        ]);
        assert!(d.contains_line(2));
        assert!(d.contains_line(5));
        assert!(!d.contains_line(3));
        assert!(!d.is_full());
    }

    // Real PTY, so poll with a bounded budget. Proves: render_snapshot()
    // clears the engine's initial Full and converges to precise Lines on an
    // idle pane (i.e. reset_damage() actually runs).
    #[test]
    fn render_snapshot_resets_and_converges_to_lines() {
        let pane = TermPane::spawn(
            Some(("sh".into(), vec!["-c".into(), "printf 'hello\\n'; sleep 30".into()])),
            None,
            20,
            5,
        )
        .expect("spawn test pane");

        // Wait for the child's output to reach the grid.
        let mut saw_hello = false;
        for _ in 0..200 {
            if pane.snapshot().to_text().contains("hello") {
                saw_hello = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(saw_hello, "child output never reached the grid");

        // Drain the initial Full frame(s). On an idle pane the damage must
        // stop being Full within a few frames — that only happens if
        // reset_damage() runs each call.
        let mut converged = None;
        for _ in 0..50 {
            let d = pane.render_snapshot().damage;
            if !d.is_full() {
                converged = Some(d);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let d = converged.expect("idle pane never converged off Full");
        assert!(matches!(d, Damage::Lines(_)), "idle damage should be Lines, got {d:?}");

        // A line we never wrote to (row 4, below "hello") must not be damaged
        // on a now-idle pane — precise damage, not blanket.
        let d2 = pane.render_snapshot().damage;
        assert!(!d2.contains_line(4), "unwritten line 4 should be undamaged: {d2:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rt-engine damage_tests 2>&1 | tail -20`
Expected: FAIL — `cannot find type Damage`, `cannot find struct CellDamage`, `no method render_snapshot`, `no field damage on Snapshot`.

- [ ] **Step 3: Add the `Damage`/`CellDamage` types and the `damage` field**

Add just above the `Snapshot` struct (~line 115):

```rust
/// Which cells changed since the previous rendered frame, in the pane's
/// viewport cell coordinates. `Full` means "repaint everything" — the honest,
/// always-correct answer for the first frame, a scroll, a resize, newspaper
/// columns, or anything the engine can't describe precisely. `Lines` is a
/// per-row inclusive changed-column span (`left..=right`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    Full,
    Lines(Vec<CellDamage>),
}

impl Default for Damage {
    fn default() -> Self {
        Damage::Full // a fresh snapshot makes no promises: repaint all
    }
}

impl Damage {
    /// Does this damage cover viewport row `line`? `Full` covers every row.
    pub fn contains_line(&self, line: usize) -> bool {
        match self {
            Damage::Full => true,
            Damage::Lines(v) => v.iter().any(|d| d.line == line),
        }
    }

    /// Is this the "repaint everything" variant?
    pub fn is_full(&self) -> bool {
        matches!(self, Damage::Full)
    }
}

/// One damaged span on a single viewport row: columns `left..=right` (inclusive)
/// of row `line` changed. Mirrors `alacritty_terminal`'s `LineDamageBounds` in
/// the pane's own cell space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellDamage {
    pub line: usize,
    pub left: usize,
    pub right: usize,
}
```

Add the field to `Snapshot` (the struct already `#[derive(Default)]`; `Damage: Default` keeps that valid):

```rust
pub struct Snapshot {
    pub cols: usize,              // number of columns captured
    pub rows: Vec<Vec<SnapCell>>, // one inner Vec per visible screen line
    pub cursor: Option<CursorPos>, // cursor location, if visible
    pub damage: Damage,           // cells changed since the previous rendered frame
}
```

- [ ] **Step 4: Refactor `snapshot()` to `capture_locked()` and add `render_snapshot()`**

Rename the existing `pub fn snapshot(&self) -> Snapshot { let term = self.term.lock(); ... }` so the body that builds the grid takes an already-locked `&Term`. Concretely, change the signature line and the lock line:

```rust
    /// Capture the current visible grid as a [`Snapshot`] for rendering or
    /// testing. Locks the `Term` briefly, copies out the visible cells, and
    /// releases — it never hands out a reference into shared state.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock(); // read access to the grid
        self.capture_locked(&term)
    }

    /// Build a [`Snapshot`] from an already-locked `Term`. Split out so
    /// `render_snapshot()` can capture the grid and the damage under one lock.
    /// The `damage` field is left at its `Full` default here; only
    /// `render_snapshot()` fills it.
    fn capture_locked(&self, term: &alacritty_terminal::term::Term<crate::Proxy>) -> Snapshot {
```

Then the existing body (`let cols = term.columns();` onward) stays **exactly as is** — it already reads through `term`. Delete only the original `let term = self.term.lock();` line that used to start the body (now moved into `snapshot()`).

> Interface note: substitute the concrete `Term<...>` generic actually used in this file. Find it with `grep -n 'Mutex<.*Term' crates/rt-engine/src/lib.rs` (the field `self.term`'s type) and use that exact type for the `term:` parameter.

Add `render_snapshot()` immediately after `snapshot()`:

```rust
    /// Like [`snapshot`](Self::snapshot) but also captures the terminal's damage
    /// (which cells changed since the last call) and resets it, so the next call
    /// reports damage relative to this frame. Call this **once per pane per
    /// frame** from the render path only — `Term::damage()` mutates damage state.
    ///
    /// Precise `Damage::Lines` is produced only for the ordinary case: a single
    /// column of grid, scrolled to the bottom (`display_offset == 0`), where a
    /// damaged viewport row maps 1:1 onto snapshot row `line`. Any other case
    /// (scrolled into history, mid-resize) already comes back as `Full` from the
    /// engine, which the renderer honours by repainting everything.
    pub fn render_snapshot(&self) -> Snapshot {
        use alacritty_terminal::term::TermDamage;
        let mut term = self.term.lock(); // exclusive: damage() is &mut
        let mut snap = self.capture_locked(&term); // grid + cursor (immutable borrow ends here)
        let damage = match term.damage() {
            TermDamage::Full => Damage::Full,
            TermDamage::Partial(iter) => {
                // Collect before reset_damage(): the iterator borrows the damage
                // buffer, and reset_damage() needs to borrow it mutably.
                let lines: Vec<CellDamage> = iter
                    .filter(|b| b.is_damaged())
                    .map(|b| CellDamage { line: b.line, left: b.left, right: b.right })
                    .collect();
                Damage::Lines(lines)
            }
        };
        term.reset_damage(); // next frame's damage is relative to this one
        snap.damage = damage;
        snap
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rt-engine damage_tests 2>&1 | tail -20`
Expected: PASS (3 tests). If `render_snapshot_resets_and_converges_to_lines` is flaky under a loaded CI box, its budget (200×10 ms + 50×10 ms) is generous; a genuine failure means reset isn't running.

Also confirm nothing else broke: `cargo build -p rt-engine 2>&1 | tail -5` → no errors (the five existing `snapshot()` callers are unchanged; they now get `damage: Full` by default, which they ignore).

- [ ] **Step 6: Commit**

```bash
git add crates/rt-engine/src/lib.rs
git commit -m "feat(engine): expose per-line damage on Snapshot via render_snapshot()"
```

---

## Task 2: pure pixel-rect damage accumulator (`rt/damage.rs`)

**Files:**
- Create: `crates/rt/src/damage.rs`
- Modify: `crates/rt/src/main.rs` (add `mod damage;` near the other `mod` declarations at the top)
- Test: `crates/rt/src/damage.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `rt_engine::{Damage, CellDamage}` (Task 1).
- Produces:
  - `pub struct PxRect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }` (physical px, top-left origin) with `pub fn right(&self)`, `pub fn bottom(&self)`, `pub fn intersects(&self, other: &PxRect) -> bool`, `pub fn union(&self, other: &PxRect) -> PxRect`, `pub fn is_empty(&self) -> bool`.
  - `pub enum FrameDamage { Full, Rects(Vec<PxRect>) }` with `pub fn bbox(&self) -> Option<PxRect>` (bounding box of all rects; `None` for `Full` or empty).
  - `pub struct DamageAccumulator` with:
    - `pub fn new() -> Self`
    - `pub fn begin_frame(&mut self)` — clears state for a new frame (`full = false`, rects empty)
    - `pub fn mark_full(&mut self)`
    - `pub fn add_rect(&mut self, r: PxRect)`
    - `pub fn add_cells(&mut self, damage: &Damage, origin_x: i32, origin_y: i32, cell_w: i32, cell_h: i32)` — maps a pane's cell damage to pixel rects at the pane's content origin; `Damage::Full` → `mark_full()`.
    - `pub fn add_cell_span(&mut self, line: usize, left: usize, right: usize, origin_x: i32, origin_y: i32, cell_w: i32, cell_h: i32)`
    - `pub fn is_full(&self) -> bool`
    - `pub fn finish(&self) -> FrameDamage` — coalesces overlapping/touching rects; returns `Full` if marked full.

- [ ] **Step 1: Write the failing tests**

Create `crates/rt/src/damage.rs` with only the tests first (so the module compiles for a red run, put a `// impl below` stub — actually write tests referencing the API; they fail to compile until Step 3). Contents:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rt_engine::{CellDamage, Damage};

    #[test]
    fn cell_span_maps_to_pixels() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        // Row 2, cols 3..=5, pane at (10,20), 8x16 cells.
        acc.add_cell_span(2, 3, 5, 10, 20, 8, 16);
        match acc.finish() {
            FrameDamage::Rects(rs) => {
                assert_eq!(rs.len(), 1);
                let r = rs[0];
                assert_eq!(r.x, 10 + 3 * 8); // 34
                assert_eq!(r.y, 20 + 2 * 16); // 52
                assert_eq!(r.w, (5 - 3 + 1) * 8); // 3 cols → 24
                assert_eq!(r.h, 16);
            }
            FrameDamage::Full => panic!("expected Rects, got Full"),
        }
    }

    #[test]
    fn engine_full_propagates() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        acc.add_cells(&Damage::Full, 0, 0, 8, 16);
        assert!(acc.is_full());
        assert!(matches!(acc.finish(), FrameDamage::Full));
    }

    #[test]
    fn engine_lines_map_each_span() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        let d = Damage::Lines(vec![
            CellDamage { line: 0, left: 0, right: 0 },
            CellDamage { line: 9, left: 2, right: 4 },
        ]);
        acc.add_cells(&d, 0, 0, 8, 16);
        match acc.finish() {
            FrameDamage::Rects(rs) => assert_eq!(rs.len(), 2),
            FrameDamage::Full => panic!("expected Rects"),
        }
    }

    #[test]
    fn overlapping_rects_coalesce() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        acc.add_rect(PxRect { x: 0, y: 0, w: 10, h: 10 });
        acc.add_rect(PxRect { x: 5, y: 5, w: 10, h: 10 }); // overlaps the first
        match acc.finish() {
            FrameDamage::Rects(rs) => {
                assert_eq!(rs.len(), 1, "overlapping rects should merge");
                assert_eq!(rs[0], PxRect { x: 0, y: 0, w: 15, h: 15 });
            }
            FrameDamage::Full => panic!("expected Rects"),
        }
    }

    #[test]
    fn disjoint_rects_stay_separate() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        acc.add_rect(PxRect { x: 0, y: 0, w: 5, h: 5 });
        acc.add_rect(PxRect { x: 100, y: 100, w: 5, h: 5 });
        match acc.finish() {
            FrameDamage::Rects(rs) => assert_eq!(rs.len(), 2),
            FrameDamage::Full => panic!("expected Rects"),
        }
    }

    #[test]
    fn empty_and_zero_size_rects_dropped() {
        let mut acc = DamageAccumulator::new();
        acc.begin_frame();
        acc.add_rect(PxRect { x: 0, y: 0, w: 0, h: 10 }); // zero width → dropped
        assert!(matches!(acc.finish(), FrameDamage::Rects(rs) if rs.is_empty()));
    }

    #[test]
    fn bbox_of_rects() {
        let fd = FrameDamage::Rects(vec![
            PxRect { x: 10, y: 10, w: 5, h: 5 },
            PxRect { x: 100, y: 50, w: 20, h: 20 },
        ]);
        let b = fd.bbox().unwrap();
        assert_eq!(b, PxRect { x: 10, y: 10, w: 110, h: 60 }); // (10,10)..(120,70)
        assert!(FrameDamage::Full.bbox().is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

First register the module so the file compiles: add `mod damage;` to `crates/rt/src/main.rs` alongside the other top-level `mod` lines (find them: `grep -nE '^mod ' crates/rt/src/main.rs`).

Run: `cargo test -p rt --lib damage 2>&1 | tail -20`
Expected: FAIL — compile errors (`PxRect`, `FrameDamage`, `DamageAccumulator` not found).

- [ ] **Step 3: Implement the accumulator**

Prepend the implementation to `crates/rt/src/damage.rs` (above the `#[cfg(test)] mod tests`):

```rust
//! Pure, GL-free damage accumulation. Collects the pixel regions that changed
//! this frame (from engine cell damage, the cursor, animated chrome, etc.) and
//! coalesces them into a small set of rectangles the renderer can scissor to.
//! No GL, no winit — just integer rectangle math, so it is fully unit-tested.

use rt_engine::Damage;

/// A rectangle in **physical pixels, top-left origin** (winit convention).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    /// Do the two rectangles overlap or touch (shared edge counts, so touching
    /// rects merge into one scissor region rather than two adjacent passes)?
    pub fn intersects(&self, other: &PxRect) -> bool {
        self.x <= other.right()
            && other.x <= self.right()
            && self.y <= other.bottom()
            && other.y <= self.bottom()
    }
    /// Smallest rectangle covering both.
    pub fn union(&self, other: &PxRect) -> PxRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        PxRect { x, y, w: right - x, h: bottom - y }
    }
}

/// The coalesced damage for one frame.
pub enum FrameDamage {
    Full,
    Rects(Vec<PxRect>),
}

impl FrameDamage {
    /// Bounding box of all damage rects, or `None` for `Full`/empty. Phase 1's
    /// scissored redraw uses this single box (see the renderer); multi-rect
    /// scissoring is a later refinement.
    pub fn bbox(&self) -> Option<PxRect> {
        match self {
            FrameDamage::Full => None,
            FrameDamage::Rects(rs) => {
                let mut it = rs.iter().filter(|r| !r.is_empty());
                let first = *it.next()?;
                Some(it.fold(first, |acc, r| acc.union(r)))
            }
        }
    }
}

/// Accumulates this frame's damage. Reused across frames via `begin_frame()`.
pub struct DamageAccumulator {
    full: bool,
    rects: Vec<PxRect>,
}

impl DamageAccumulator {
    pub fn new() -> Self {
        Self { full: false, rects: Vec::new() }
    }

    /// Start a fresh frame's accumulation.
    pub fn begin_frame(&mut self) {
        self.full = false;
        self.rects.clear();
    }

    pub fn mark_full(&mut self) {
        self.full = true;
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Add a pixel rectangle. Empty rects and additions after `mark_full()` are
    /// ignored (once full, individual rects are moot).
    pub fn add_rect(&mut self, r: PxRect) {
        if self.full || r.is_empty() {
            return;
        }
        self.rects.push(r);
    }

    /// Map one cell span (`left..=right` inclusive on row `line`) to a pixel
    /// rect at a pane's content origin and add it.
    pub fn add_cell_span(
        &mut self,
        line: usize,
        left: usize,
        right: usize,
        origin_x: i32,
        origin_y: i32,
        cell_w: i32,
        cell_h: i32,
    ) {
        if right < left {
            return; // undamaged span
        }
        let cols = (right - left + 1) as i32;
        self.add_rect(PxRect {
            x: origin_x + left as i32 * cell_w,
            y: origin_y + line as i32 * cell_h,
            w: cols * cell_w,
            h: cell_h,
        });
    }

    /// Fold a pane's engine damage in. `Full` marks the whole frame full.
    pub fn add_cells(
        &mut self,
        damage: &Damage,
        origin_x: i32,
        origin_y: i32,
        cell_w: i32,
        cell_h: i32,
    ) {
        match damage {
            Damage::Full => self.mark_full(),
            Damage::Lines(lines) => {
                for d in lines {
                    self.add_cell_span(d.line, d.left, d.right, origin_x, origin_y, cell_w, cell_h);
                }
            }
        }
    }

    /// Coalesce and return this frame's damage. Repeatedly merges any two rects
    /// that overlap or touch until no more merges are possible, so the renderer
    /// scissors a handful of regions instead of hundreds of tiny ones.
    pub fn finish(&self) -> FrameDamage {
        if self.full {
            return FrameDamage::Full;
        }
        let mut merged: Vec<PxRect> = Vec::new();
        for r in self.rects.iter().filter(|r| !r.is_empty()) {
            let mut cur = *r;
            let mut i = 0;
            while i < merged.len() {
                if merged[i].intersects(&cur) {
                    cur = merged[i].union(&cur);
                    merged.swap_remove(i); // re-test cur against the rest
                } else {
                    i += 1;
                }
            }
            merged.push(cur);
        }
        FrameDamage::Rects(merged)
    }
}

impl Default for DamageAccumulator {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rt --lib damage 2>&1 | tail -20`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/damage.rs crates/rt/src/main.rs
git commit -m "feat(rt): add pure pixel-rect damage accumulator (rt/damage.rs)"
```

---

## Task 3: scissored-redraw primitives in `render.rs`

**Files:**
- Modify: `crates/rt/src/render.rs` (add `scissor_box()` free fn; add `begin_frame_scissored()` and `clear_scissor()` on `Renderer` near `begin_frame` ~392; keep `begin_frame`/`end_frame` unchanged)
- Test: `crates/rt/src/render.rs` (inline `#[cfg(test)] mod scissor_tests` — pure, no GL)

**Interfaces:**
- Consumes: `crate::damage::PxRect` (Task 2).
- Produces:
  - `pub fn scissor_box(r: PxRect, screen_h: i32) -> (i32, i32, i32, i32)` — converts a top-left-origin pixel rect to GL's `(x, y, w, h)` bottom-left-origin scissor box. **Free function** (no `&self`) so it is unit-testable without a GL context.
  - `Renderer::begin_frame_scissored(&mut self, bg: Color, bbox: PxRect)` — like `begin_frame` but **does not clear the whole window**: enables `SCISSOR_TEST`, sets the scissor to `bbox`, and clears only `bbox` to the premultiplied background. The subsequent `end_frame()` draw is clipped to the scissor. The previous frame's pixels outside `bbox` are preserved (present layer guarantees preservation — Task 4).
  - `Renderer::clear_scissor(&mut self)` — disables `SCISSOR_TEST` and resets the scissor to the full window. Called after present so egui / the next `begin_frame` start clean.

**Design note:** Phase 1 scissors to the **single bounding box** of the frame's damage (`FrameDamage::bbox()`), not each rect. `end_frame()` issues one `draw_arrays` of all geometry; GL clips fragments to the scissor box, so the software rasteriser only fills the damaged region — the win we want. Per-rect scissoring (multiple draw passes) is a later refinement; the bbox of a keystroke's damage is a couple of adjacent cells, so over-draw is negligible.

- [ ] **Step 1: Write the failing test (pure Y-flip math)**

Add at the end of `crates/rt/src/render.rs`:

```rust
#[cfg(test)]
mod scissor_tests {
    use super::scissor_box;
    use crate::damage::PxRect;

    #[test]
    fn flips_y_to_bottom_left_origin() {
        // 800x600 window. A 24x16 rect at top-left-origin (34, 52).
        // GL origin is bottom-left, so gl_y = screen_h - (y + h) = 600 - 68 = 532.
        let (x, y, w, h) = scissor_box(PxRect { x: 34, y: 52, w: 24, h: 16 }, 600);
        assert_eq!((x, y, w, h), (34, 532, 24, 16));
    }

    #[test]
    fn top_row_maps_to_top_of_gl_buffer() {
        // A rect flush against the top (y=0, h=16) sits at gl_y = 600 - 16 = 584.
        let (_, y, _, _) = scissor_box(PxRect { x: 0, y: 0, w: 8, h: 16 }, 600);
        assert_eq!(y, 584);
    }

    #[test]
    fn bottom_row_maps_to_gl_zero() {
        // A rect flush against the bottom (y = 584, h = 16) maps to gl_y = 0.
        let (_, y, _, _) = scissor_box(PxRect { x: 0, y: 584, w: 8, h: 16 }, 600);
        assert_eq!(y, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt --lib scissor 2>&1 | tail -15`
Expected: FAIL — `cannot find function scissor_box`.

- [ ] **Step 3: Implement `scissor_box`, `begin_frame_scissored`, `clear_scissor`**

Add the free function near the top of `render.rs` (after the imports, before `struct Renderer`):

```rust
/// Convert a top-left-origin pixel rectangle to a GL scissor box, which is
/// **bottom-left origin**. `screen_h` is the window height in physical pixels.
/// Returns `(x, y, width, height)` ready for `glScissor`.
pub fn scissor_box(r: crate::damage::PxRect, screen_h: i32) -> (i32, i32, i32, i32) {
    let gl_y = screen_h - (r.y + r.h); // flip Y: top-left px → bottom-left GL
    (r.x, gl_y, r.w, r.h)
}
```

Add the two methods inside `impl Renderer`, right after `begin_frame` (~line 408):

```rust
    /// Begin a frame that only repaints the damaged bounding box `bbox`. Unlike
    /// [`begin_frame`], this does **not** clear the whole window — it enables
    /// the scissor test, clips to `bbox`, and clears only that region to the
    /// premultiplied background. Everything outside `bbox` keeps the previous
    /// frame's pixels (the present layer guarantees the back buffer is
    /// preserved). The following `end_frame()` draw is clipped to `bbox` too.
    pub fn begin_frame_scissored(&mut self, bg: Color, bbox: crate::damage::PxRect) {
        self.verts.clear(); // drop last frame's geometry
        let a = bg.3; // premultiplied clear, matching begin_frame
        let (sx, sy, sw, sh) = scissor_box(bbox, self.screen.1 as i32);
        unsafe {
            self.gl.viewport(0, 0, self.screen.0 as i32, self.screen.1 as i32);
            self.gl.enable(glow::SCISSOR_TEST); // clip clears AND draws to bbox
            self.gl.scissor(sx, sy, sw, sh);
            self.gl.enable(glow::BLEND);
            self.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            self.gl.clear_color(bg.0 * a, bg.1 * a, bg.2 * a, a);
            self.gl.clear(glow::COLOR_BUFFER_BIT); // scissor confines this to bbox
        }
    }

    /// Reset scissor state to the full window. Call after presenting a scissored
    /// frame so egui and the next `begin_frame`/`begin_frame_scissored` start
    /// from a known-clean state.
    pub fn clear_scissor(&mut self) {
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.scissor(0, 0, self.screen.0 as i32, self.screen.1 as i32);
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt --lib scissor 2>&1 | tail -15`
Expected: PASS (3 tests). Also `cargo build -p rt 2>&1 | tail -5` → clean (the new methods are unused for now; allow the `dead_code` warning or wire in Task 5 immediately after).

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/render.rs
git commit -m "feat(rt): scissored-redraw primitives (scissor_box, begin_frame_scissored)"
```

---

## Task 4: EGL buffer-age partial present

**Files:**
- Modify: `crates/rt/src/main.rs` (add a `present_with_damage` helper; imports)
- Test: manual/board (partial present needs a live EGL surface; the correctness gate is Task 6). No unit test — the logic is a thin wrapper over glutin whose branches are all fallbacks to the tested full path.

**Interfaces:**
- Consumes: `active.surface: glutin::surface::Surface<WindowSurface>`, `active.context: PossiblyCurrentContext`, `crate::damage::PxRect`, `glutin::prelude::GlSurface` (for `buffer_age`/`swap_buffers`), `glutin::surface::Rect`.
- Produces: `fn present_with_damage(active: &mut Active, rects: &[PxRect]) -> bool` — returns `true` if it performed a partial-damage swap, `false` if the caller must fall back to a full redraw + full swap (age 0, non-EGL surface, or error). Also `fn buffer_age(active: &Active) -> u32` thin accessor.

**Design note — the buffer-age contract (from the spec + glutin docs):**
- `swap_buffers_with_damage(&ctx, &rects)` is a **compositor hint only**; it does not preserve or partially render. Preservation is inferred from `buffer_age()`:
  - `age == 0` → the back buffer is new/unknown → we **cannot** do a partial redraw this frame (return `false`; caller does a full redraw).
  - `age == n (n ≥ 1)` → the back buffer holds the frame from `n` swaps ago → the caller must have redrawn the **union of the last `n` frames' damage** into it. This plan keeps a 2-deep history (Task 5) and treats `age > HISTORY_DEPTH` as "too old" → full redraw.
- `swap_buffers_with_damage` lives on glutin's **concrete EGL surface**, not the `GlSurface` trait, so we match the `Surface::Egl` / `PossiblyCurrentContext::Egl` variants. A non-EGL surface (GLX under X11) returns `false` here — that path is Phase 2.

- [ ] **Step 1: Add imports**

At the top of `main.rs`, ensure these are in scope (add if missing):

```rust
use glutin::prelude::GlSurface; // buffer_age(), swap_buffers()
use glutin::surface::Rect as GlRect; // EGL damage rect (bottom-left origin)
```

- [ ] **Step 2: Implement the present helpers**

Add as free functions (or `impl App`) near `redraw` in `main.rs`:

```rust
/// The age of the back buffer: how many swaps ago its contents were last drawn.
/// 0 means "unknown / brand new" — the whole buffer must be redrawn.
fn buffer_age(active: &Active) -> u32 {
    active.surface.buffer_age()
}

/// Present a scissored frame, hinting the compositor with the damage rects.
/// Returns `true` on a successful partial-damage swap; `false` if the caller
/// must fall back to a full redraw + full `swap_buffers` (age 0, non-EGL
/// surface, or a swap error). `rects` are physical px, top-left origin; they are
/// converted to EGL's bottom-left-origin `Rect` here.
fn present_with_damage(active: &mut Active, rects: &[crate::damage::PxRect]) -> bool {
    use glutin::context::PossiblyCurrentContext;
    use glutin::surface::Surface;

    let screen_h = active.window.inner_size().height as i32;
    let egl_rects: Vec<GlRect> = rects
        .iter()
        .map(|r| {
            let y = screen_h - (r.y + r.h); // flip to bottom-left origin
            GlRect::new(r.x, y, r.w, r.h)
        })
        .collect();

    // swap_buffers_with_damage is only on the concrete EGL surface/context.
    match (&active.surface, &active.context) {
        (Surface::Egl(egl_surface), PossiblyCurrentContext::Egl(egl_ctx)) => {
            match egl_surface.swap_buffers_with_damage(egl_ctx, &egl_rects) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("swap_buffers_with_damage failed ({e}); full swap next frame");
                    false
                }
            }
        }
        _ => false, // GLX / other backend: Phase 2 territory
    }
}
```

> Interface note: confirm the exact context enum path with `grep -n 'enum PossiblyCurrentContext' ~/.cargo/registry/src/*/glutin-0.32.3/src/context.rs`. If rt stores the context as a type alias, match its real variant name. The `Surface::Egl` variant exists because rt builds glutin with the `egl` feature (see `crates/rt/Cargo.toml:48`).

- [ ] **Step 3: Build**

Run: `cargo build -p rt 2>&1 | tail -15`
Expected: clean build. `present_with_damage`/`buffer_age` are unused until Task 5 — allow the `dead_code` warning or proceed straight to Task 5.

- [ ] **Step 4: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): EGL buffer-age partial-present helper (Wayland/local)"
```

---

## Task 5: wire damage into `redraw()` (gated on software GL)

**Files:**
- Modify: `crates/rt/src/main.rs` — `Active` struct (add fields ~228 area), `redraw()` (~2167), the pane loop (~2201), the present tail (~2556), and the input/scroll/resize/overlay handlers that must invalidate to `Full`.
- Test: covered by Task 6 (offscreen pixel identity) + manual perf.

**Interfaces:**
- Consumes: `DamageAccumulator`, `FrameDamage`, `PxRect` (Task 2); `render_snapshot()` + `Damage` (Task 1); `begin_frame_scissored`/`clear_scissor`/`scissor_box` (Task 3); `present_with_damage`/`buffer_age` (Task 4); `renderer.is_software()`, `renderer.cell_size()`, `session.visible_rects()`, `session.content_rect()`, `session.columns_of()`.
- Produces: a `redraw()` that, **only when `renderer.is_software()`**, may take the partial path; otherwise runs today's full path unchanged.

**State to add to `Active`** (near the other render state, ~line 228):

```rust
    damage: crate::damage::DamageAccumulator, // this frame's accumulated pixel damage
    damage_history: std::collections::VecDeque<crate::damage::FrameDamage>, // last frames' damage, for buffer-age
    force_full: bool,                         // next frame must be a full redraw (scroll/resize/overlay/etc.)
```

Initialize where `Active` is constructed: `damage: crate::damage::DamageAccumulator::new(), damage_history: std::collections::VecDeque::new(), force_full: true` (first frame is full).

**The rule for `force_full`:** set `active.force_full = true` in every handler that changes the whole window or defeats cell-precise damage. Concretely, add `if let Some(active) = self.active.as_mut() { active.force_full = true; }` (or set the field directly where `active` is already borrowed) in these existing handlers:
- `WindowEvent::Resized` (~1144) and the surface `resize` path.
- scroll / display-offset changes (~1192–1229, the scrollback handlers).
- any handler that opens/closes an egui overlay: `prefs_open`, `menu`, `manual_open`, `search_open` toggles.
- selection start/change (the mouse-drag selection handlers).
- font-size / zoom changes (they change `cell_size`).

> These are conservative: each just forces one full frame. Missing one causes a stale region, not a crash, and is caught by the offscreen identity test for the cell case; the overlay/scroll cases are visually obvious. Prefer over-marking.

- [ ] **Step 1: Restructure `redraw()` to compute a `FrameDamage`, then branch**

The current `redraw()` (~2167) does: `begin_frame(bg)` → pane loop drawing into `renderer.verts` → chrome → `end_frame()` → egui → `swap_buffers()`. Refactor so the pane-drawing body is reusable and the frame is bracketed by a damage decision.

Replace the head of `redraw()` (from `active.renderer.begin_frame(bg);` down to just before the pane loop) with a damage-planning block. Full new structure of `redraw()`:

```rust
    fn redraw(&mut self) {
        let Some(active) = self.active.as_mut() else { return };
        let cfg_bg = active.settings.background;
        let bg = Color::rgb(cfg_bg[0], cfg_bg[1], cfg_bg[2]).with_alpha(active.settings.background_opacity);
        let size = active.window.inner_size();
        let bounds = content_bounds(size);

        // Decide the frame's damage. The partial path is taken ONLY on software
        // GL with an EGL surface whose buffer we can trust to be preserved.
        // Everything else → full redraw (today's path), which is always correct.
        let overlay_open = active.prefs_open || active.menu.is_some() || active.manual_open || active.search_open;
        let (cell_w, cell_h) = active.renderer.cell_size();
        let (cw, ch) = (cell_w as i32, cell_h as i32);

        // Build this frame's damage from the panes (also fetches the snapshots
        // the draw pass reuses, so render_snapshot() runs exactly once/pane).
        active.damage.begin_frame();
        let mut snapshots: Vec<(PaneId, PxRectSnap)> = Vec::new(); // see note below
        if active.force_full || overlay_open || !active.renderer.is_software() {
            active.damage.mark_full();
        }
        for (id, rect) in active.session.visible_rects(bounds) {
            let content = active.session.content_rect(rect);
            let snap = active.session.pane(id).map(|p| p.render_snapshot()); // ONCE per pane
            if let Some(snap) = &snap {
                if active.session.columns_of(id) > 1 {
                    active.damage.mark_full(); // newspaper columns: cell→px mapping ambiguous
                } else {
                    active.damage.add_cells(&snap.damage, content.x as i32, content.y as i32, cw, ch);
                }
            }
            snapshots.push((id, (rect, snap)));
        }
        // Chrome that egui blends every frame (focus border, instruments) lives
        // on the pane borders; force those bands into the damage set so the
        // scissored clear+redraw always precedes egui's blend (no double-blend).
        if !active.damage.is_full() {
            for (id, (rect, _)) in &snapshots {
                let _ = id;
                for band in border_bands(*rect) {
                    active.damage.add_rect(band);
                }
            }
        }

        let frame_damage = active.damage.finish();

        // Fold in the last frames' damage per buffer age (the back buffer may be
        // older than one frame). age 0 / too old → Full.
        let plan = plan_frame(active, frame_damage);

        match plan {
            FramePlan::Full => {
                self.redraw_full(bg, bounds, snapshots); // today's exact path
            }
            FramePlan::Partial(bbox, hint_rects) => {
                self.redraw_scissored(bg, bounds, snapshots, bbox, &hint_rects);
            }
        }
        // Clear the per-frame force flag; specific handlers re-arm it.
        if let Some(active) = self.active.as_mut() {
            active.force_full = false;
        }
    }
```

Where `PxRectSnap` is a local type alias `type PxRectSnap = (Rect, Option<rt_engine::Snapshot>);` (use rt's existing pane-rect type for `Rect` — the one `visible_rects` returns). `PaneId` is the existing id type.

> **Note on the pane loop:** the existing draw body already iterates `visible_rects` and calls `pane.snapshot()`. Move that draw body into `redraw_full`/`redraw_scissored` (below), and have both consume the **pre-fetched `snapshots`** instead of calling `snapshot()` again — this guarantees `render_snapshot()` runs exactly once per pane per frame (required: it mutates engine damage state). Replace the `let snap = pane.snapshot();` line inside the moved body with the pre-fetched snapshot for that `id`.

- [ ] **Step 2: Add `border_bands`, `FramePlan`, `plan_frame`, and split the draw body**

Add helpers in `main.rs`:

```rust
use crate::damage::{DamageAccumulator, FrameDamage, PxRect};

/// The chrome bands around a pane rect that egui repaints each frame (focus
/// outline + border instruments). Kept thin so forcing them into damage stays
/// cheap. `BORDER_PX` matches the renderer's focus-outline / instrument band.
const BORDER_PX: i32 = 6;
fn border_bands(rect: Rect) -> [PxRect; 4] {
    let (x, y, w, h) = (rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32);
    [
        PxRect { x, y, w, h: BORDER_PX },                       // top
        PxRect { x, y: y + h - BORDER_PX, w, h: BORDER_PX },    // bottom
        PxRect { x, y, w: BORDER_PX, h },                       // left
        PxRect { x: x + w - BORDER_PX, y, w: BORDER_PX, h },    // right
    ]
}

enum FramePlan {
    Full,
    Partial(PxRect, Vec<PxRect>), // (scissor bbox, per-rect compositor hints)
}

/// Combine this frame's damage with the last frames' damage according to the
/// back-buffer age, and decide Full vs Partial. History depth is 2.
const HISTORY_DEPTH: u32 = 2;
fn plan_frame(active: &mut Active, this: FrameDamage) -> FramePlan {
    // Push this frame's damage into history first (newest front), cap depth.
    let age = buffer_age(active);
    let full_now = matches!(this, FrameDamage::Full);
    // Record for future frames.
    let recorded = match &this {
        FrameDamage::Full => FrameDamage::Full,
        FrameDamage::Rects(rs) => FrameDamage::Rects(rs.clone()),
    };
    active.damage_history.push_front(recorded);
    while active.damage_history.len() > HISTORY_DEPTH as usize {
        active.damage_history.pop_back();
    }

    if full_now || age == 0 || age > HISTORY_DEPTH {
        return FramePlan::Full;
    }
    // Union this frame + the previous (age-1) frames' damage into one accumulator.
    let mut acc = DamageAccumulator::new();
    acc.begin_frame();
    for fd in active.damage_history.iter().take(age as usize) {
        match fd {
            FrameDamage::Full => return FramePlan::Full, // any full in the window → full
            FrameDamage::Rects(rs) => {
                for r in rs {
                    acc.add_rect(*r);
                }
            }
        }
    }
    match acc.finish() {
        FrameDamage::Full => FramePlan::Full,
        FrameDamage::Rects(rs) => {
            if rs.is_empty() {
                // Nothing changed and buffer is fresh enough: still must present
                // something; a zero-rect partial swap is a no-op hint — safe.
                FramePlan::Partial(PxRect { x: 0, y: 0, w: 0, h: 0 }, Vec::new())
            } else {
                let bbox = FrameDamage::Rects(rs.clone()).bbox().unwrap();
                FramePlan::Partial(bbox, rs)
            }
        }
    }
}
```

Split the existing draw body into two methods that share the moved pane-drawing code:

```rust
    /// Today's exact full-window path: clear everything, draw all panes+chrome,
    /// egui, full swap. Byte-for-byte the pre-damage behaviour.
    fn redraw_full(&mut self, bg: Color, bounds: Rect, snapshots: Vec<(PaneId, PxRectSnap)>) {
        let Some(active) = self.active.as_mut() else { return };
        active.renderer.begin_frame(bg);
        Self::draw_panes(active, bounds, &snapshots); // the moved pane+chrome body
        active.renderer.end_frame();
        Self::paint_overlays_or_instruments(active); // the existing egui dispatch
        if let Err(e) = active.surface.swap_buffers(&active.context) {
            log::error!("swap_buffers failed: {e}");
        }
    }

    /// Partial path: preserve the buffer, clear+redraw only `bbox`, hint the
    /// compositor with `hint_rects`. Falls back to a full frame if the EGL
    /// partial swap is unavailable this frame.
    fn redraw_scissored(
        &mut self,
        bg: Color,
        bounds: Rect,
        snapshots: Vec<(PaneId, PxRectSnap)>,
        bbox: PxRect,
        hint_rects: &[PxRect],
    ) {
        let Some(active) = self.active.as_mut() else { return };
        active.renderer.begin_frame_scissored(bg, bbox);
        Self::draw_panes(active, bounds, &snapshots); // scissor clips to bbox
        active.renderer.end_frame();
        Self::paint_overlays_or_instruments(active); // instruments blend inside cleared bbox
        active.renderer.clear_scissor();
        if !present_with_damage(active, hint_rects) {
            // EGL partial swap unavailable/failed → guarantee correctness with a
            // full redraw + full swap this frame, and force full next frame too.
            active.renderer.begin_frame(bg);
            Self::draw_panes(active, bounds, &snapshots);
            active.renderer.end_frame();
            Self::paint_overlays_or_instruments(active);
            if let Err(e) = active.surface.swap_buffers(&active.context) {
                log::error!("swap_buffers failed: {e}");
            }
            active.force_full = true;
        }
    }
```

Extract `draw_panes(active, bounds, &snapshots)` from the current pane-drawing loop body (everything between `begin_frame` and `end_frame` in today's `redraw`, including the bell-stripe/selection/search-highlight code), taking the pre-fetched `snapshots` instead of calling `pane.snapshot()`. Extract `paint_overlays_or_instruments(active)` from the existing `if active.prefs_open { paint_egui } else if ... else { paint_instruments }` block (~2565).

> **Scissor + egui interaction (important):** `paint_overlays_or_instruments` runs egui, which sets and clears its own scissor per mesh. Call `clear_scissor()` **before** it (done above) is wrong for instruments — instruments must land inside the preserved/cleared bbox. The border-band forcing in Step 1 guarantees the instrument regions are inside `bbox` and freshly cleared, so egui blending them once is correct. `clear_scissor()` is called **after** egui in `redraw_scissored`, before present, so the next frame starts clean. (egui restoring full-window scissor internally is fine — our own draw already happened.)

- [ ] **Step 3: Build and run the existing suite**

Run: `cargo build -p rt 2>&1 | tail -20` → clean.
Run: `cargo test -p rt 2>&1 | tail -20` → existing tests still pass (damage + scissor units from Tasks 2–3).

- [ ] **Step 4: Manual smoke on hardware GL (regression guard)**

Run rt locally on the dev box (hardware GL): `cargo run -p rt`. Because `is_software()` is false, `redraw()` marks `Full` every frame → `redraw_full` → today's path. Confirm: typing, splits, scrollback, menu, prefs, resize all look identical to `main`. This is the byte-identical guarantee in practice.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): wire damage-driven partial redraw+present, gated on software GL"
```

---

## Task 6: offscreen pixel-identity correctness gate + perf verification

**Files:**
- Create: `crates/rt/tests/damage_pixel_identity.rs`
- Modify: `crates/rt/Cargo.toml` (add `[[test]]` entry with `harness = true`; add `dev-dependencies` if a headless-EGL helper is needed — reuse glutin/glow already in deps)

**Interfaces:**
- Consumes: `rt::render::{Renderer, scissor_box}`, `rt::damage::PxRect`, glutin headless EGL (pbuffer) + glow. Requires `Renderer` and `render`/`damage` modules to be reachable from an integration test — if `crates/rt` is a binary-only crate, add a thin `pub` surface: create `crates/rt/src/lib.rs` re-exporting `pub mod render; pub mod damage;` (and have `main.rs` `use rt::{render, damage};`). If a lib target already exists, just ensure `render` and `damage` are `pub`.

**What it proves (the spec's gate):** render a known grid two ways into an offscreen buffer — (a) full clear+draw, (b) full clear+draw of frame 0, then a single-cell change applied via `begin_frame_scissored` to just that cell's rect — and assert the two final framebuffers are **pixel-identical** via `glReadPixels`. Damage must never change *what* is on screen, only *how much* work produced it.

- [ ] **Step 1: Add the `#[ignore]`d integration test**

Create `crates/rt/tests/damage_pixel_identity.rs`:

```rust
//! Correctness gate for damage-based rendering: a scissored single-cell redraw
//! must produce a byte-identical framebuffer to a full redraw. Needs a live
//! (software) GL context, so it is #[ignore]d by default:
//!
//!   cargo test -p rt --test damage_pixel_identity -- --ignored
//!
//! Run it on any box with EGL + a GL driver (llvmpipe is fine; that is exactly
//! the software path we optimise). It builds a headless pbuffer surface.

use rt::damage::PxRect;
use rt::render::Renderer;

// Helper: create a headless EGL pbuffer + glow context at WxH. Returns
// (glow::Context, surface, context) kept alive by the caller. Implementation
// mirrors rt's own EGL setup in main.rs (display → config → pbuffer surface →
// context → make_current); factor it here as a test-only helper.
mod egl_headless; // see Step 2

const W: i32 = 320;
const H: i32 = 200;

#[test]
#[ignore = "needs a live GL context; run with --ignored on a GL-capable box"]
fn scissored_single_cell_equals_full_redraw() {
    let ctx = egl_headless::make(W as u32, H as u32).expect("headless EGL");
    let gl = ctx.glow(); // Arc<glow::Context>
    let blobs = egl_headless::test_fonts();
    let mut r = Renderer::new(gl.clone(), &blobs, 16.0).expect("renderer");
    r.resize(W as f32, H as f32);

    let bg = rt::render::Color::rgb(0x10, 0x10, 0x18).with_alpha(1.0);

    // --- Reference: full redraw of the "after" grid. ---
    // Draw a grid, then draw it again with cell (row 1, col 2) changed. Because
    // we cannot feed a real Term here, drive the renderer's cell-draw API
    // directly (the same calls draw_panes makes): draw a filled cell at the
    // target and read back.
    let (cw, ch) = r.cell_size();
    let (cw, ch) = (cw as i32, ch as i32);

    // Frame A (full): background + one glyph 'X' at (row 1, col 2).
    r.begin_frame(bg);
    egl_headless::draw_cell(&mut r, 2, 1, 'X'); // helper: emit one cell's quads
    r.end_frame();
    let full_px = egl_headless::read_pixels(&gl, W, H);

    // Frame B (scissored on top of a prior frame): first draw the "before"
    // frame (space at that cell), then scissor just that cell and draw 'X'.
    r.begin_frame(bg);
    egl_headless::draw_cell(&mut r, 2, 1, ' ');
    r.end_frame();
    let cell = PxRect { x: 2 * cw, y: 1 * ch, w: cw, h: ch };
    r.begin_frame_scissored(bg, cell);
    egl_headless::draw_cell(&mut r, 2, 1, 'X');
    r.end_frame();
    r.clear_scissor();
    let partial_px = egl_headless::read_pixels(&gl, W, H);

    assert_eq!(full_px.len(), partial_px.len());
    let diffs = full_px.iter().zip(&partial_px).filter(|(a, b)| a != b).count();
    assert_eq!(diffs, 0, "{diffs} pixels differ between full and scissored redraw");
}
```

- [ ] **Step 2: Add the `egl_headless` test helper**

Create `crates/rt/tests/egl_headless/mod.rs` (or `crates/rt/tests/damage_pixel_identity/egl_headless.rs` per Rust's test-module layout). It must expose:
- `make(w: u32, h: u32) -> Result<Ctx, String>` — build an EGL display (`glutin::display::Display::new` with a headless/`DisplayApiPreference::Egl`), choose a config, create a `PbufferSurface` of `w×h`, create a context, make current, and wrap a `glow::Context` via `glow::Context::from_loader_function`. Copy the loader/setup from rt's own init in `main.rs` (grep `Display::new`, `glow::Context::from_loader`).
- `Ctx::glow(&self) -> std::sync::Arc<glow::Context>`.
- `test_fonts() -> rt::render::FontBlobs` — reuse rt's bundled font blobs (grep how `main.rs` builds `FontBlobs`; expose a `pub fn embedded() -> FontBlobs` if not already public).
- `draw_cell(r: &mut Renderer, col: usize, row: usize, c: char)` — call the same public cell-emit method `draw_panes` uses to place one glyph at a cell (grep the method name in `render.rs`, e.g. `draw_glyph`/`push_cell`; make it `pub` if needed).
- `read_pixels(gl: &glow::Context, w: i32, h: i32) -> Vec<u8>` — `glReadPixels(0,0,w,h, RGBA, UNSIGNED_BYTE)` into a `vec![0u8; (w*h*4) as usize]`.

> If exposing `draw_cell` cleanly is awkward, an acceptable alternative that still proves the gate: draw a solid colored quad at the cell via a minimal public `fill_rect`-style method rather than a glyph. The property under test (scissored redraw == full redraw) is independent of *what* is drawn in the cell.

- [ ] **Step 3: Run the gate on a GL-capable box**

Run: `cargo test -p rt --test damage_pixel_identity -- --ignored 2>&1 | tail -20`
Expected: PASS — 0 pixels differ. If it can't create an EGL context (no GL on the box), the test errors out early with a clear message; run it on the milkv or a desktop with llvmpipe.

- [ ] **Step 4: Perf verification on the milkv via Xvfb**

On `ssh milkv`, run rt under Xvfb (software GL, the environment that exposed the 250 ms) and measure a keystroke's frame cost before/after. Use rt's existing frame-time logging or a quick `RUST_LOG` timer around `redraw`:

```sh
ssh milkv
cd ~/git/rt && git fetch && git checkout <this-branch> && cargo build --release -p rt
# headless X with software GL
Xvfb :99 -screen 0 1280x720x24 &
DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 RUST_LOG=rt=debug ./target/release/rt 2>frame.log &
# type into the pane (or run `while true; do echo x; sleep 1; done`) and inspect
grep -i 'frame\|redraw' frame.log | tail
```

Expected: a single-cell update's frame cost drops from ~250 ms toward a few ms (scissor confines the software rasteriser + present to the damaged bbox). If it does **not** drop, the likely cause is `buffer_age()` returning 0 under Xvfb (no `EGL_EXT_buffer_age`) → every frame falls back to Full; note this — it is exactly the Phase 2 concern (indirect/limited EGL), and Phase 1's win is validated on a Wayland/local software-GL box instead (e.g. a Weston headless session or a desktop forcing `LIBGL_ALWAYS_SOFTWARE=1` under Wayland).

- [ ] **Step 5: Commit**

```bash
git add crates/rt/tests/ crates/rt/Cargo.toml crates/rt/src/lib.rs
git commit -m "test(rt): offscreen pixel-identity gate for damage-based redraw"
```

---

## Self-Review

**1. Spec coverage** (against `2026-07-11-damage-based-rendering-design.md`):
- Component 1 (engine exposes damage, `Snapshot.damage`, reset after) → **Task 1**. ✓ (deviation: reset happens in `render_snapshot()`, called once/frame by the render path, not inside general `snapshot()` — documented, and *stronger* than the spec's wording because `snapshot()` has five callers).
- Component 2 (`rt/damage.rs` accumulator: cell + cursor + scroll/resize/first-frame→Full + selection + chrome + overlay→Full, coalesced) → **Task 2** (pure math + Full propagation + coalescing) plus **Task 5** (wiring the sources: cursor is covered because engine damage already includes the cursor cell — see alacritty `damage_cursor()`; selection/scroll/resize/overlay → `force_full`; chrome → `border_bands`). ✓
- Component 3 (render.rs partial redraw: preserve buffer, scissor, clear+redraw only damage, Full→existing) → **Task 3** + **Task 5** (`redraw_scissored`). ✓
- Component 4 Phase 1 present (Wayland/EGL `swap_buffers_with_damage` + buffer-age, full-swap fallback) → **Task 4** + `plan_frame` in **Task 5**. ✓ Phase 2 (X11 readback+XPutImage) correctly **excluded**. ✓
- Component 5 (main.rs wiring, backend/phase present selection) → **Task 5**. ✓
- Correctness gate (offscreen pixel-identical) → **Task 6**. ✓
- Perf verification (milkv/Xvfb) → **Task 6 Step 4**. ✓
- Hardware path identical / falsifiable to Full → Global Constraints + `is_software()` gate + `force_full` + every fallback. ✓
- Reserves B/C **not built** → honoured (mechanism A only). ✓

**2. Placeholder scan:** No "TBD"/"handle edge cases"/"similar to Task N". The one area with prose-not-code is Task 6's `egl_headless` helper (headless-GL boilerplate), because its exact form depends on rt's existing EGL init, which the task says to copy verbatim via named greps — the *what to expose* is fully enumerated. Acceptable: the correctness property and every public method it needs are spelled out.

**3. Type consistency:** `Damage`/`CellDamage`/`render_snapshot` (Task 1) used identically in Tasks 2 & 5. `PxRect{x,y,w,h}` / `FrameDamage::{Full,Rects}` / `DamageAccumulator` (Task 2) used identically in Tasks 3, 5, 6. `scissor_box` signature matches between Task 3 def and Task 5/6 use. `present_with_damage(active, &[PxRect]) -> bool` and `buffer_age(active) -> u32` (Task 4) match `plan_frame`/`redraw_scissored` calls (Task 5). `begin_frame_scissored`/`clear_scissor` names consistent. Glutin `Surface::Egl` / `PossiblyCurrentContext::Egl` variant match flagged for verification against the installed crate.

**Known integration risks called out for the implementer** (not gaps — decisions with fallbacks): (a) glutin context enum variant names — verify grep in Task 4; (b) `border_bands` `BORDER_PX` should match the renderer's actual focus-outline/instrument band width — tune during Task 5 Step 4; (c) `buffer_age()==0` under Xvfb defeats partial present — expected, that's the Phase-2 boundary, noted in Task 6 Step 4.
