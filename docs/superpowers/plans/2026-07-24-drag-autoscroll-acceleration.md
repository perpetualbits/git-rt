# Drag Auto-Scroll Acceleration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While drag-selecting past a pane's top/bottom edge, make the auto-scroll accelerate the longer the pointer is held in the edge zone (capped), instead of the current flat 1 line / 35 ms — reusing the arrow-key acceleration curve and preferences.

**Architecture:** A pure helper `autoscroll_step` (in `main.rs`, beside `arrow_accel_step`) maps the accumulated `(last_dir, ticks)` accel state + the current scroll direction to a line-count and the next state — grace period, ramp, cap, and direction-flip reset, delegating the curve to `arrow_accel_step`. `autoscroll_selection` gains one `Active` field (the accel state), resets it when the pointer re-enters the grid or the drag ends, and scrolls `step` lines per gated tick instead of one.

**Tech Stack:** Rust, the existing `autoscroll_selection` drag path and `arrow_accel_step`/`arrow_accel`/`arrow_accel_max` from feature A.

## Global Constraints

- Driver is **time held**, not distance (the terminal runs vertically maximized — no runway past the edge). Do not add pointer-distance logic.
- Reuse the existing preferences `arrow_accel` (on/off) and `arrow_accel_max` (1–30). No new preference rows.
- With `arrow_accel` **off**, the step must be exactly 1 — byte-for-byte today's behavior. The feature is purely additive.
- `arrow_accel_step(repeats, max)` (`main.rs:5076`): returns 1 for `repeats < THRESHOLD` (`THRESHOLD = 4`), then `1 + (repeats - THRESHOLD)` capped at `max`; `max == 1` ⇒ 1.
- Scroll direction convention in `autoscroll_selection`: `dir = +1` when the pointer is above the pane top (scroll toward older history), `-1` when below the bottom (toward newest), `0` inside the grid.
- Pure logic (`autoscroll_step`) is TDD'd with real `cargo test -p rt`. The `autoscroll_selection` wiring is winit/event-loop code (not unit-testable here); its task ends with a build + full-workspace test + a manual checklist on dop651.
- Full gate before release: `cargo test --workspace --release -- --test-threads=1`.

---

### Task 1: Pure `autoscroll_step` helper

**Files:**
- Modify: `crates/rt/src/main.rs` — add `autoscroll_step` next to `arrow_accel_step` (`main.rs:5076`); add tests to a `#[cfg(test)]` module in `main.rs` (the bin already has test modules, e.g. `selection_tests` near `main.rs:4931` — add there or a new `mod autoscroll_tests`).

**Interfaces:**
- Consumes: `arrow_accel_step` (same file).
- Produces: `fn autoscroll_step(state: (isize, u32), dir: isize, accel: bool, max: u32) -> (usize, (isize, u32))` — returns `(lines_to_scroll_this_tick, new_state)` where `state`/`new_state` is `(last_dir, ticks)`.

- [ ] **Step 1: Write the failing test**

Add a test module to `crates/rt/src/main.rs` (e.g. after the existing test modules):

```rust
#[cfg(test)]
mod autoscroll_tests {
    use super::autoscroll_step;

    #[test]
    fn grace_then_ramps_with_held_ticks_and_threads_state() {
        // accel on, max 10. First tick (ticks 0) is in the grace band → 1 line;
        // the returned state stores the direction and the incremented tick count.
        let (s0, st1) = autoscroll_step((0, 0), 1, true, 10);
        assert_eq!(s0, 1);
        assert_eq!(st1, (1, 1));
        // Held into the ramp: at ticks 5, arrow_accel_step(5,10) = 1 + (5-4) = 2.
        assert_eq!(autoscroll_step((1, 5), 1, true, 10).0, 2);
        // Capped at max regardless of how long it is held.
        assert_eq!(autoscroll_step((1, 100), 1, true, 10).0, 10);
    }

    #[test]
    fn off_is_always_one() {
        // arrow_accel off → flat 1 line/tick (today's behavior), state still threads.
        let (s, st) = autoscroll_step((1, 50), 1, false, 10);
        assert_eq!(s, 1);
        assert_eq!(st, (1, 51));
    }

    #[test]
    fn direction_flip_resets_the_ramp() {
        // Was fast scrolling up (dir 1, ticks 50); pointer now past the bottom
        // (dir -1) → ramp resets to the grace band, new state starts at (-1, 1).
        let (s, st) = autoscroll_step((1, 50), -1, true, 10);
        assert_eq!(s, 1);
        assert_eq!(st, (-1, 1));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt autoscroll 2>&1 | tail -20`
Expected: FAIL — `autoscroll_step` not found (cannot compile).

- [ ] **Step 3: Write the implementation**

Add to `crates/rt/src/main.rs`, immediately after `arrow_accel_step`:

```rust
/// Lines to scroll this drag-auto-scroll tick, and the next accel state. `state`
/// is `(last_dir, ticks)` — the direction the ramp was built in and how many
/// consecutive gated ticks it has been held. A change of `dir` (top edge ↔
/// bottom edge) resets the ramp to the grace band. The line count follows the
/// shared arrow-accel curve (`arrow_accel_step`) so keyboard and drag accelerate
/// identically; with `accel` off it is always 1 (today's flat rate). `dir` is
/// the current, non-zero scroll direction.
fn autoscroll_step(state: (isize, u32), dir: isize, accel: bool, max: u32) -> (usize, (isize, u32)) {
    let (last_dir, ticks) = state;
    let ticks = if dir == last_dir { ticks } else { 0 }; // direction flip → restart the ramp
    let step = if accel { arrow_accel_step(ticks, max) } else { 1 };
    (step, (dir, ticks + 1))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt autoscroll 2>&1 | tail -20`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): pure autoscroll_step curve for drag auto-scroll accel"
```

---

### Task 2: Wire acceleration into `autoscroll_selection`

**Files:**
- Modify: `crates/rt/src/main.rs` — the `Active` struct (near `last_autoscroll: Instant,`, `main.rs:393`), its initializer (near `last_autoscroll: Instant::now(),`, `main.rs:1134`), and `autoscroll_selection` (`main.rs:2868`–`2901`).

**Interfaces:**
- Consumes: `autoscroll_step` (Task 1), `Active.settings.arrow_accel` / `arrow_accel_max`.
- Produces: `Active.autoscroll_accel: (isize, u32)` (the `(last_dir, ticks)` state).

- [ ] **Step 1: Add the state field + initializer**

In `Active`, beside `last_autoscroll: Instant,`:

```rust
    autoscroll_accel: (isize, u32), // drag auto-scroll ramp: (last_dir, ticks held in the edge zone)
```

In the initializer, beside `last_autoscroll: Instant::now(),`:

```rust
            autoscroll_accel: (0, 0),
```

- [ ] **Step 2: Reset the ramp at the non-scrolling exits, and scroll `step` lines per tick**

Rewrite `autoscroll_selection` (`main.rs:2868`) so it (a) resets `autoscroll_accel` to `(0, 0)` when the drag ends or the pointer is back inside the grid, and (b) scrolls `step` lines per gated tick. Full function:

```rust
    fn autoscroll_selection(active: &mut Active, now: Instant) -> bool {
        if !active.selecting {
            active.autoscroll_accel = (0, 0); // drag ended: forget the ramp
            return false;
        }
        let Some(sel) = active.selection else { return false };
        let Some(content) = Self::pane_content_rect(active, sel.pane) else { return false };
        let my = active.mouse.1;
        // +1 = scroll toward older history (pointer above top); -1 = toward newest.
        let dir: isize = if my < content.y { 1 } else if my >= content.y + content.h { -1 } else { 0 };
        if dir == 0 {
            active.autoscroll_accel = (0, 0); // back inside the grid: restart slow next time
            return false; // pointer is within the grid: normal motion handling covers it
        }
        // One gated tick per ~35ms, independent of frame rate; the number of LINES
        // per tick ramps the longer the pointer is held in the edge zone (feature C).
        if now.duration_since(active.last_autoscroll) < Duration::from_millis(35) {
            return true; // in the zone; ask the caller to keep waking us
        }
        active.last_autoscroll = now;
        let (step, new_accel) =
            autoscroll_step(active.autoscroll_accel, dir, active.settings.arrow_accel, active.settings.arrow_accel_max);
        active.autoscroll_accel = new_accel;
        let (cw, _) = active.backend.cell_size();
        let (off, col, edge_row) = {
            let Some(pane) = active.session.pane(sel.pane) else { return false };
            for _ in 0..step {
                pane.scroll(dir); // move the view `step` lines this tick
            }
            let (offset, _, screen) = pane.scroll_info();
            let col = ((active.mouse.0 - content.x) / cw).max(0.0) as usize;
            // Extend to the top row when scrolling up, the bottom row when down.
            let edge_row = if dir > 0 { 0i32 } else { screen.saturating_sub(1) as i32 };
            (offset as i32, col, edge_row)
        };
        if let Some(s) = active.selection.as_mut() {
            s.head = (col, edge_row - off); // screen edge row → absolute line
        }
        active.force_full = true; // selection + scroll: not engine-tracked damage
        true
    }
```

Note: `step` is always ≥ 1 (the helper returns 1 in the grace band and when accel is off), so the `for` loop always scrolls at least one line — identical to today when accel is off.

- [ ] **Step 3: Build**

Run: `cargo build --release -p rt --features x11 2>&1 | grep -E "^error|warning:|Finished"`
Expected: `Finished`, no errors, no new warnings.

- [ ] **Step 4: Regression gate (full workspace)**

Run: `cargo test --workspace --release -- --test-threads=1 2>&1 | grep -E "test result:|error\["`
Expected: all crates compile and pass, 0 failed.

- [ ] **Step 5: Manual verification (dop651)**

After `cargo install --path crates/rt --force`, run `rt` (ideally a **vertically maximized** window with a large scrollback):
1. Drag-select and push the pointer **past the bottom edge**, holding it there → the scroll starts slow (precise), then accelerates after a beat, and tops out (doesn't run away). The selection extends as it scrolls.
2. **Pull the pointer back up into the visible grid**, then push past the edge again → it starts slow again (the ramp reset).
3. **Past the top edge** behaves the same (accelerating toward older history).
4. Preferences → turn **"Hold-arrow acceleration" off** → drag-past-edge is now a flat 1 line/tick (exactly today's behavior). Turn it back on; raise/lower **"Max arrow speed"** and confirm the top speed changes.
5. Sanity: normal drag-select *within* the pane (not past the edge) is unchanged; a plain click/drag still selects normally.

- [ ] **Step 6: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): accelerate drag auto-scroll the longer the edge is held"
```

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-07-24-drag-autoscroll-acceleration-design.md`):
- Time-held ramp (not distance) → Task 1 curve + Task 2 tick accumulation.
- Reuse `arrow_accel_step` + `arrow_accel`/`arrow_accel_max` → Task 1 delegates; Task 2 reads the prefs. No new pref rows.
- Off ⇒ step 1 (today's behavior) → helper returns 1 when `accel == false`; loop scrolls exactly one line.
- Grace (THRESHOLD) + ramp + cap → delegated to `arrow_accel_step`.
- Reset on grid re-entry / drag end / direction flip → Task 2 resets at the two early exits; Task 1 resets on `dir != last_dir`.
- State = `(last_dir, ticks)` → the `autoscroll_accel` field.
- No conflict with anchored selection → `autoscroll_selection` is gated on `active.selecting`; unchanged.

**Placeholder scan:** no TBD/TODO; both code steps show complete code.

**Type consistency:** `autoscroll_step(state: (isize, u32), dir: isize, accel: bool, max: u32) -> (usize, (isize, u32))` matches its call in Task 2; `autoscroll_accel: (isize, u32)` matches; `arrow_accel_step` reused unchanged.

## Out of scope
- Distance-based acceleration (ruled out by the vertically-maximized constraint).
- Mouse-wheel-while-dragging (separate feature; anchored mode already covers free scrolling during selection).
