# Clipboard History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An in-memory clipboard history for rt: every text you copy in rt enters a bounded MRU ring; a focused-pane titlebar affordance and Ctrl+Shift+H open a native overlay of recent clips; picking one pastes it into the focused pane and promotes it to the clipboard.

**Architecture:** A pure, unit-tested `ClipHistory` ring (`crates/rt/src/clip_history.rs`, no I/O) holds the clips and formats previews. A pure overlay-geometry module (`crates/rt/src/chrome/clip_history.rs`) lays out and hit-tests the list, mirroring `chrome/menu.rs`. `main.rs` funnels every copy through one `record_clip` call, holds the ring + overlay state, and wires the affordance, the keybinding, and pick→paste+promote. Nothing is written to disk.

**Tech Stack:** Rust, winit event loop, the existing native-chrome overlay pattern (`chrome/menu.rs`), `clipboard.rs` (`store`/`store_primary`), `Session::feed_paste` (per-pane bracketed paste).

## Global Constraints

- No new dependencies. History is **in-memory only** — never serialized to disk; dropped on exit.
- Capacity `CLIP_HISTORY_MAX = 20`. `record` skips empty/whitespace-only text; a re-recorded existing clip moves to the front (MRU), it does not duplicate.
- `record` is called at the copy **sites** (`do_copy`, the drag-release PRIMARY store, `copy_selection_to_primary`), NOT inside `clipboard.store`/`store_primary` — so promoting a clip picked from the history does not re-enter capture.
- Newest clip is index 0 (front). Previews are one line: `\n`→`↵`, `\t`→space, truncated to the row width with `…`. Full clip text is only used at paste/promote time, never shown expanded.
- Pure logic (`clip_history.rs`, the overlay geometry) is TDD'd with real `cargo test`. winit event-loop wiring cannot be unit-tested here; those tasks end with a build + a concrete manual-verification checklist on dop651 (local GL).
- Build check for a wiring task: `cargo build --release -p rt --features x11` finishes clean. Full gate before release: `cargo test --workspace --release -- --test-threads=1`.
- Modules that are pure + tested get declared in BOTH `crates/rt/src/lib.rs` (so tests run in the `rt_app` lib crate) AND `crates/rt/src/main.rs` (so the bin uses them) — the established convention (see `select`, `render`).

---

### Task 1: Pure `ClipHistory` ring (`clip_history.rs`)

**Files:**
- Create: `crates/rt/src/clip_history.rs`
- Modify: `crates/rt/src/lib.rs` (add `pub mod clip_history;` beside `pub mod select;`), `crates/rt/src/main.rs` (add `mod clip_history;` beside `mod select;`)
- Test: `crates/rt/src/clip_history.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub const CLIP_HISTORY_MAX: usize = 20;`
  - `pub struct ClipHistory` with `pub fn new() -> Self`, `pub fn record(&mut self, text: String)`, `pub fn clear(&mut self)`, `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`, `pub fn get(&self, i: usize) -> Option<&str>`, `pub fn iter(&self) -> impl Iterator<Item = &str>` (newest first).

- [ ] **Step 1: Write the failing test**

Create `crates/rt/src/clip_history.rs`:

```rust
//! In-memory clipboard history: a bounded most-recently-used ring of the text
//! rt has copied this session. Pure (no I/O, nothing persisted) so it is
//! unit-testable without the event loop. See
//! docs/superpowers/specs/2026-07-24-clipboard-history-design.md.

/// Most clips kept. Older ones fall off the back as new ones arrive.
pub const CLIP_HISTORY_MAX: usize = 20;

/// A ring of recent clippings, newest at index 0. In-memory only.
#[derive(Default)]
pub struct ClipHistory {
    items: Vec<String>,
}

impl ClipHistory {
    pub fn new() -> Self {
        ClipHistory { items: Vec::new() }
    }

    /// Record a freshly-copied clip. Empty / whitespace-only text is ignored. A
    /// clip already present is moved to the front (MRU) rather than duplicated;
    /// otherwise it is pushed to the front and the ring is capped at
    /// `CLIP_HISTORY_MAX`, dropping the oldest.
    pub fn record(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(pos) = self.items.iter().position(|c| *c == text) {
            self.items.remove(pos);
        }
        self.items.insert(0, text);
        self.items.truncate(CLIP_HISTORY_MAX);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The i-th clip, newest first (`0` is the most recent), or `None`.
    pub fn get(&self, i: usize) -> Option<&str> {
        self.items.get(i).map(String::as_str)
    }

    /// Clips newest-first.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_newest_first_and_skips_blank() {
        let mut h = ClipHistory::new();
        h.record("one".into());
        h.record("two".into());
        h.record("   ".into()); // whitespace-only: ignored
        h.record("".into()); // empty: ignored
        assert_eq!(h.len(), 2);
        assert_eq!(h.get(0), Some("two"));
        assert_eq!(h.get(1), Some("one"));
        assert_eq!(h.iter().collect::<Vec<_>>(), vec!["two", "one"]);
    }

    #[test]
    fn re_recording_moves_to_front_without_duplicating() {
        let mut h = ClipHistory::new();
        h.record("a".into());
        h.record("b".into());
        h.record("a".into()); // already present → moves to front
        assert_eq!(h.len(), 2);
        assert_eq!(h.iter().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn caps_at_the_maximum_dropping_oldest() {
        let mut h = ClipHistory::new();
        for i in 0..(CLIP_HISTORY_MAX + 5) {
            h.record(format!("clip{i}"));
        }
        assert_eq!(h.len(), CLIP_HISTORY_MAX);
        assert_eq!(h.get(0), Some(format!("clip{}", CLIP_HISTORY_MAX + 4).as_str()));
        assert_eq!(h.iter().last(), Some("clip5")); // clip0..clip4 evicted
    }

    #[test]
    fn clear_empties_it() {
        let mut h = ClipHistory::new();
        h.record("x".into());
        h.clear();
        assert!(h.is_empty());
        assert_eq!(h.get(0), None);
    }
}
```

Then add the module declarations: `pub mod clip_history;` in `crates/rt/src/lib.rs` (next to `pub mod select;`) and `mod clip_history;` in `crates/rt/src/main.rs` (next to `mod select;`). Read the surrounding lines to place them consistently.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt clip_history:: 2>&1 | tail -20`
Expected: FAIL — module not yet wired / (if you wrote the impl and tests together, instead momentarily stub `record` with `todo!()` to see RED, then restore). The point is to see the tests execute and fail before trusting them.

- [ ] **Step 3: Implementation** — the impl above is complete; ensure it compiles.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt clip_history:: 2>&1 | tail -20`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/clip_history.rs crates/rt/src/lib.rs crates/rt/src/main.rs
git commit -m "feat(rt): in-memory clipboard-history ring (pure)"
```

---

### Task 2: Preview + badge formatting (`clip_history.rs`)

**Files:**
- Modify: `crates/rt/src/clip_history.rs`
- Test: `crates/rt/src/clip_history.rs` (`tests`)

**Interfaces:**
- Produces: `pub fn preview(text: &str, max_cols: usize) -> String`, `pub fn badge(text: &str) -> String`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    #[test]
    fn preview_is_one_line_and_truncated() {
        // Newlines become ↵, tabs become spaces, kept on one line.
        assert_eq!(preview("git log\n--oneline", 40), "git log↵--oneline");
        assert_eq!(preview("a\tb", 40), "a b");
        // Truncation adds an ellipsis and never exceeds max_cols chars.
        let p = preview("0123456789abcdef", 8);
        assert_eq!(p.chars().count(), 8);
        assert!(p.ends_with('…'));
        assert_eq!(p, "0123456…");
    }

    #[test]
    fn badge_counts_chars_and_lines() {
        assert_eq!(badge("hello"), "5c");
        assert_eq!(badge("a\nb\nc"), "3L·5c");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt clip_history:: 2>&1 | tail -20`
Expected: FAIL — `preview` / `badge` not found.

- [ ] **Step 3: Implementation**

Add to `crates/rt/src/clip_history.rs` (after the `impl`):

```rust
/// A one-line preview of a clip for the overlay: newlines shown as `↵`, tabs as
/// spaces, truncated to `max_cols` characters with a trailing `…`. Never spans
/// lines; the real (multi-line) text is only ever used at paste time.
pub fn preview(text: &str, max_cols: usize) -> String {
    let flat: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '\n' => '↵',
            '\r' => '↵',
            '\t' => ' ',
            c => c,
        })
        .collect();
    if flat.chars().count() <= max_cols {
        return flat;
    }
    let keep = max_cols.saturating_sub(1);
    let mut s: String = flat.chars().take(keep).collect();
    s.push('…');
    s
}

/// A compact size badge: `"<chars>c"`, prefixed with `"<lines>L·"` when the clip
/// spans more than one line.
pub fn badge(text: &str) -> String {
    let chars = text.chars().count();
    let lines = text.lines().count().max(1);
    if lines > 1 {
        format!("{lines}L·{chars}c")
    } else {
        format!("{chars}c")
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt clip_history:: 2>&1 | tail -20`
Expected: PASS — 6 tests total.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/clip_history.rs
git commit -m "feat(rt): clipboard-history preview + badge formatting"
```

---

### Task 3: Actions + keybinding (rt-config)

**Files:**
- Modify: `crates/rt-config/src/lib.rs` — the `Action` enum and the default-bindings table (near `("<Shift><Control>c", Action::Copy)`).
- Test: `crates/rt-config/src/lib.rs` (existing `#[cfg(test)] mod tests`, or add one)

**Interfaces:**
- Produces: `Action::ClipHistory`, `Action::ClearClipHistory`; a default binding `<Shift><Control>h → Action::ClipHistory`.

- [ ] **Step 1: Write the failing test**

Add to rt-config's tests:

```rust
    #[test]
    fn ctrl_shift_h_opens_clip_history_by_default() {
        let km = Keymap::default();
        let chord = keys::Chord::parse("<Shift><Control>h").expect("valid chord");
        assert_eq!(km.action_for(&chord), Some(Action::ClipHistory));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt-config ctrl_shift_h 2>&1 | tail -20`
Expected: FAIL — `Action::ClipHistory` does not exist (compile error).

- [ ] **Step 3: Implementation**

In the `Action` enum (after `Manual,`, before the closing `}`):

```rust
    /// Open/close the clipboard-history overlay (recent copies).
    ClipHistory,
    /// Empty the clipboard history.
    ClearClipHistory,
```

In the default-bindings table, right after the Copy/Paste rows:

```rust
            ("<Shift><Control>h", Action::ClipHistory),  // clipboard history
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt-config ctrl_shift_h 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rt-config/src/lib.rs
git commit -m "feat(rt-config): ClipHistory / ClearClipHistory actions + Ctrl+Shift+H"
```

---

### Task 4: Capture — ring state + record at the copy sites

**Files:**
- Modify: `crates/rt/src/main.rs` — the `Active` struct (near `clipboard:`, `main.rs:311`), its initializer (near `clipboard,`, `main.rs:1053`), a `record_clip` helper (near `do_copy`, `main.rs:2684`), and the three copy sites (`do_copy` `2684`, the drag-release `store_primary` `~2101`, `copy_selection_to_primary` `~3078`).

**Interfaces:**
- Consumes: `clip_history::ClipHistory`.
- Produces: `Active.clip_history: ClipHistory`, `fn record_clip(active: &mut Active, text: &str)`.

- [ ] **Step 1: Add the field + initializer**

In `Active`, beside `clipboard: Option<clipboard::Clipboard>,`:

```rust
    clip_history: clip_history::ClipHistory, // in-memory MRU ring of this session's copies
```

In the initializer, beside `clipboard,`:

```rust
            clip_history: clip_history::ClipHistory::new(),
```

- [ ] **Step 2: Add the record helper**

Near `do_copy` (`main.rs:2684`):

```rust
    /// Funnel every user copy through here so it enters the clipboard history.
    /// Called at the copy SITES (not inside clipboard.store), so promoting a
    /// clip picked from the history does not re-enter capture.
    fn record_clip(active: &mut Active, text: &str) {
        active.clip_history.record(text.to_string());
    }
```

- [ ] **Step 3: Call it at the three copy sites**

In `do_copy` (`main.rs:2684`), where it stores a non-empty selection:

```rust
    fn do_copy(active: &mut Active) {
        if let Some(text) = Self::selected_text(active) {
            if !text.is_empty() {
                if let Some(cb) = &active.clipboard {
                    cb.store(text.clone());
                    cb.store_primary(text.clone());
                }
                Self::record_clip(active, &text); // history
            }
        }
    }
```

(Note: `text.clone()` twice now, since `record_clip` needs `&text` after the moves — adjust the existing `cb.store_primary(text)` to `cb.store_primary(text.clone())` so `text` survives for `record_clip`.)

In `copy_selection_to_primary` (`main.rs:3078`) — this takes `&Active`, but `record_clip` needs `&mut Active`. Change its signature to `&mut Active` and update its call sites (double/triple-click select), then:

```rust
    fn copy_selection_to_primary(active: &mut Active) {
        if let Some(text) = Self::selected_text(active) {
            if let Some(cb) = &active.clipboard {
                cb.store_primary(text.clone());
            }
            Self::record_clip(active, &text);
        }
    }
```

In the drag-release PRIMARY store (`main.rs:~2101`, the release handler branch that does `cb.store_primary(text)` for a real selection): capture the text into a local, store it, and record:

```rust
                        } else if let Some(text) = Self::selected_text(active) {
                            if let Some(cb) = &active.clipboard {
                                cb.store_primary(text.clone()); // PRIMARY for middle-click paste
                            }
                            Self::record_clip(active, &text);
                        }
```

(Read the exact surrounding lines; the existing branch already computes `text` via `selected_text` — thread `record_clip` in after the store. Watch the borrow: finish the `&active.clipboard` borrow before the `&mut` call to `record_clip`, exactly as written above — the `if let Some(cb)` block ends before `record_clip`.)

Anchored-selection commit needs no new call: `compose_commit` calls `do_copy`, which now records.

- [ ] **Step 4: Build**

Run: `cargo build --release -p rt --features x11 2>&1 | grep -E "^error|warning:|Finished"`
Expected: `Finished`, no errors. (`ClipHistory` is now used; no dead-code warning. `get`/`iter`/`preview`/`badge` may warn as unused until Task 6 — acceptable.)

- [ ] **Step 5: Regression gate**

Run: `cargo test --workspace --release -- --test-threads=1 2>&1 | grep -E "test result:|error\["`
Expected: all pass, 0 failed.

- [ ] **Step 6: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): capture every rt copy into the clipboard history"
```

---

### Task 5: Overlay geometry (`chrome/clip_history.rs`)

A pure layout + hit-test module for the history overlay, mirroring `chrome/menu.rs`. Rows are the N clip previews followed by one **Clear history** row (last index).

**Files:**
- Create: `crates/rt/src/chrome/clip_history.rs`
- Modify: `crates/rt/src/chrome/mod.rs` (add `pub mod clip_history;`)
- Test: `crates/rt/src/chrome/clip_history.rs` (`tests`)

**Interfaces:**
- Consumes: `crate::chrome::{hit, Recti}`.
- Produces:
  - `pub struct Geom { pub panel: Recti, pub rows: Vec<Recti>, pub clear_row: usize }`
  - `pub fn layout(row_count: usize, anchor: (f32, f32), cell_w: f32, cell_h: f32, win_w: f32, win_h: f32, width_cols: usize) -> Geom` (`row_count` = number of clip rows; the Clear row is added internally at index `row_count`)
  - `pub fn hit_row(g: &Geom, p: (f32, f32)) -> Option<usize>` (0..=clear_row)

- [ ] **Step 1: Write the failing test**

Create `crates/rt/src/chrome/clip_history.rs`:

```rust
//! Native clipboard-history overlay: a list of recent-clip previews plus a
//! trailing "Clear history" row. Geometry + hit-testing are pure (this module);
//! `main.rs` supplies the preview strings and draws. Mirrors `chrome/menu.rs`.

use crate::backend::Backend;
use crate::chrome::{hit, Recti};
use crate::render::Color;

const PAD: f32 = 6.0;

/// Overlay geometry in window px. `rows` has one rect per clip row plus a final
/// rect for the Clear row (index `clear_row`), so indices line up with the
/// caller's clip list.
pub struct Geom {
    pub panel: Recti,
    pub rows: Vec<Recti>,
    pub clear_row: usize,
}

/// Lay the overlay out anchored at `anchor`, clamped fully on-screen. `row_count`
/// is the number of clip rows; a Clear row is appended at `clear_row`.
pub fn layout(row_count: usize, anchor: (f32, f32), cell_w: f32, cell_h: f32, win_w: f32, win_h: f32, width_cols: usize) -> Geom {
    let row_h = cell_h + 4.0;
    let total = row_count + 1; // + Clear row
    let w = width_cols as f32 * cell_w + PAD * 2.0;
    let h = total as f32 * row_h + PAD;
    let x = anchor.0.min(win_w - w).max(0.0);
    let y = anchor.1.min(win_h - h).max(0.0);
    let mut rows = Vec::with_capacity(total);
    let mut cy = y + PAD * 0.5;
    for _ in 0..total {
        rows.push(Recti { x, y: cy, w, h: row_h });
        cy += row_h;
    }
    Geom { panel: Recti { x, y, w, h }, rows, clear_row: row_count }
}

/// The row at `p` (a clip index `0..clear_row`, or `clear_row` for Clear), or
/// `None` outside the panel.
pub fn hit_row(g: &Geom, p: (f32, f32)) -> Option<usize> {
    hit(&g.rows, p)
}

/// Draw the panel, hover highlight, each preview + its size badge, and the Clear
/// row. `previews[i]`/`badges[i]` are the clip rows; `hover`/`selected` mark the
/// highlighted row (either may be the Clear row).
pub fn draw(
    be: &mut dyn Backend,
    g: &Geom,
    previews: &[String],
    badges: &[String],
    hover: Option<usize>,
    selected: usize,
    cell_w: f32,
    cell_h: f32,
) {
    let bg = Color::rgb(0x20, 0x22, 0x28);
    let fg = Color::rgb(0xd6, 0xde, 0xe8);
    let dim = Color::rgb(0x8b, 0x98, 0xa9);
    let hl = Color::rgb(0x33, 0x3a, 0x46);
    be.fill_rect(g.panel.x, g.panel.y, g.panel.w, g.panel.h, bg);
    for (i, r) in g.rows.iter().enumerate() {
        if hover == Some(i) || selected == i {
            be.fill_rect(r.x, r.y, r.w, r.h, hl);
        }
        let ty = r.y + 2.0;
        if i == g.clear_row {
            let label = "Clear history";
            for (c, ch) in label.chars().enumerate() {
                be.draw_char(r.x + PAD, ty, c, 0, ch, dim, false, false);
            }
        } else {
            let p = previews.get(i).map(String::as_str).unwrap_or("");
            for (c, ch) in p.chars().enumerate() {
                be.draw_char(r.x + PAD, ty, c, 0, ch, fg, false, false);
            }
            if let Some(b) = badges.get(i) {
                let bw = b.chars().count();
                let bx = r.x + r.w - PAD - bw as f32 * cell_w;
                for (c, ch) in b.chars().enumerate() {
                    be.draw_char(bx, ty, c, 0, ch, dim, false, false);
                }
            }
        }
    }
    let _ = cell_h;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_has_one_row_per_clip_plus_a_clear_row_on_screen() {
        let g = layout(3, (10.0, 10.0), 8.0, 16.0, 800.0, 600.0, 24);
        assert_eq!(g.rows.len(), 4); // 3 clips + Clear
        assert_eq!(g.clear_row, 3);
        // Fully on screen.
        assert!(g.panel.x >= 0.0 && g.panel.y >= 0.0);
        assert!(g.panel.x + g.panel.w <= 800.0);
        assert!(g.panel.y + g.panel.h <= 600.0);
    }

    #[test]
    fn hit_row_maps_points_to_clip_and_clear_rows() {
        let g = layout(2, (0.0, 0.0), 8.0, 16.0, 800.0, 600.0, 24);
        let mid = |r: &Recti| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(hit_row(&g, mid(&g.rows[0])), Some(0));
        assert_eq!(hit_row(&g, mid(&g.rows[1])), Some(1));
        assert_eq!(hit_row(&g, mid(&g.rows[2])), Some(2)); // Clear row
        assert_eq!(hit_row(&g, (5000.0, 5000.0)), None); // outside
    }

    #[test]
    fn anchor_clamps_so_the_panel_stays_visible() {
        let g = layout(5, (790.0, 590.0), 8.0, 16.0, 800.0, 600.0, 24);
        assert!(g.panel.x + g.panel.w <= 800.0);
        assert!(g.panel.y + g.panel.h <= 600.0);
    }
}
```

Add `pub mod clip_history;` to `crates/rt/src/chrome/mod.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rt chrome::clip_history 2>&1 | tail -20`
Expected: FAIL first (module not declared / functions absent), then after wiring the module, the 3 tests execute.

- [ ] **Step 3: Implementation** — the module above is complete; ensure `chrome/mod.rs` declares it and it compiles.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rt chrome::clip_history 2>&1 | tail -20`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rt/src/chrome/clip_history.rs crates/rt/src/chrome/mod.rs
git commit -m "feat(rt): clipboard-history overlay geometry + draw"
```

---

### Task 6: Open / navigate / pick the overlay + apply_action

Wire the overlay into the event loop: open via `Action::ClipHistory`, hover/click/keyboard, and pick → paste + promote + move-to-front + close. Also `Action::ClearClipHistory`.

**Files:**
- Modify: `crates/rt/src/main.rs` — `Active` (overlay state near the `menu:` field, `main.rs:308`), initializer, `apply_action` (`main.rs:2545`; arms for `ClipHistory`/`ClearClipHistory`), an event-routing block modelled on the context-menu block (`main.rs:~1476`), and the redraw path (draw the overlay when open).

**Interfaces:**
- Consumes: `clip_history::{preview, badge}`, `chrome::clip_history::{layout, hit_row, draw}`, `Session::feed_paste`, `clipboard.store`/`store_primary`.
- Produces: `Active.clip_overlay: Option<usize>` (Some(selected_row) when open), `fn open_clip_history(active)`, `fn pick_clip(active, row)`.

- [ ] **Step 1: State + initializer**

In `Active`, near `menu: Option<(f32, f32)>,`:

```rust
    clip_overlay: Option<usize>, // clipboard-history overlay open, carrying the selected row
```

In the initializer, near `menu: None,`:

```rust
            clip_overlay: None,
```

- [ ] **Step 2: open + pick helpers**

Near `do_copy` / `record_clip`:

```rust
    /// Open the clipboard-history overlay (no-op if the history is empty),
    /// selecting the newest clip.
    fn open_clip_history(active: &mut Active) {
        if active.clip_history.is_empty() {
            return;
        }
        active.clip_overlay = Some(0);
        active.force_full = true;
        active.window.request_redraw();
    }

    /// Act on a picked overlay row: the Clear row empties the history; a clip row
    /// pastes that clip into the focused pane, promotes it to CLIPBOARD+PRIMARY,
    /// and moves it to the front. Always closes the overlay.
    fn pick_clip(active: &mut Active, row: usize) {
        let n = active.clip_history.len();
        if row >= n {
            // the Clear row
            active.clip_history.clear();
        } else if let Some(text) = active.clip_history.get(row).map(str::to_string) {
            active.session.feed_paste(text.as_bytes()); // per-pane bracketed paste
            if let Some(cb) = &active.clipboard {
                cb.store(text.clone());
                cb.store_primary(text.clone());
            }
            active.clip_history.record(text); // move-to-front (does NOT go through record_clip)
        }
        active.clip_overlay = None;
        active.force_full = true;
        active.window.request_redraw();
    }
```

- [ ] **Step 3: apply_action arms**

In `apply_action` (`main.rs:2545`), add arms (use the same style as `Action::Search`):

```rust
            Action::ClipHistory => {
                if active.clip_overlay.is_some() {
                    active.clip_overlay = None; // toggle closed
                    active.force_full = true;
                    active.window.request_redraw();
                } else {
                    Self::open_clip_history(active);
                }
            }
            Action::ClearClipHistory => {
                active.clip_history.clear();
                active.clip_overlay = None;
                active.force_full = true;
                active.window.request_redraw();
            }
```

- [ ] **Step 4: Event routing block**

Model this on the context-menu block (`main.rs:~1476`, `if let Some(pos) = active.menu { ... }`). Add a sibling block that runs when `active.clip_overlay.is_some()`, BEFORE the normal key/mouse handling, and `return`s after handling so events don't leak. Anchor the overlay under the focused pane's titlebar (reuse `pane_content_rect(active, focus)` for an x/y near its top-left; if unavailable, `(40.0, 40.0)`). The block must handle:

```rust
        // Clipboard-history overlay: modal like the context menu. Arrow keys move
        // the selection, Enter/click picks, Esc/click-outside closes.
        if let Some(sel) = active.clip_overlay {
            let size = active.window.inner_size();
            let (cw, ch) = active.backend.cell_size();
            let n = active.clip_history.len();
            let anchor = Self::pane_content_rect(active, active.session.focus())
                .map(|r| (r.x, r.y))
                .unwrap_or((40.0, 40.0));
            let g = chrome::clip_history::layout(n, anchor, cw, ch, size.width as f32, size.height as f32, CLIP_PREVIEW_COLS);
            match &event {
                WindowEvent::KeyboardInput { event: ke, .. } if ke.state == ElementState::Pressed => {
                    match &ke.logical_key {
                        Key::Named(NamedKey::Escape) => { active.clip_overlay = None; active.force_full = true; active.window.request_redraw(); }
                        Key::Named(NamedKey::Enter) => Self::pick_clip(active, sel),
                        Key::Named(NamedKey::ArrowDown) => { active.clip_overlay = Some((sel + 1).min(g.clear_row)); active.force_full = true; active.window.request_redraw(); }
                        Key::Named(NamedKey::ArrowUp) => { active.clip_overlay = Some(sel.saturating_sub(1)); active.force_full = true; active.window.request_redraw(); }
                        _ => {}
                    }
                    return;
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let p = (position.x as f32, position.y as f32);
                    if let Some(i) = chrome::clip_history::hit_row(&g, p) { active.clip_overlay = Some(i); active.force_full = true; active.window.request_redraw(); }
                    return;
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                    let p = (active.mouse.0, active.mouse.1);
                    match chrome::clip_history::hit_row(&g, p) {
                        Some(i) => Self::pick_clip(active, i),
                        None => { active.clip_overlay = None; active.force_full = true; active.window.request_redraw(); } // click outside
                    }
                    return;
                }
                _ => {}
            }
        }
```

Add a module-level `const CLIP_PREVIEW_COLS: usize = 44;` (overlay width in cells) near the other consts. Read the existing menu block for the exact `event` variable name and match arms in scope (adapt field names — e.g. how the menu block reads keyboard vs `active.mouse`).

- [ ] **Step 5: Draw the overlay when open**

In the redraw path, where the context menu is drawn (search for `chrome::menu::draw`), add — after the menu draw, so the overlay sits on top:

```rust
        if let Some(sel) = active.clip_overlay {
            let (cw, ch) = active.backend.cell_size();
            let size = active.window.inner_size();
            let n = active.clip_history.len();
            let anchor = Self::pane_content_rect(active, active.session.focus()).map(|r| (r.x, r.y)).unwrap_or((40.0, 40.0));
            let previews: Vec<String> = active.clip_history.iter().map(|c| clip_history::preview(c, CLIP_PREVIEW_COLS - 8)).collect();
            let badges: Vec<String> = active.clip_history.iter().map(clip_history::badge).collect();
            let g = chrome::clip_history::layout(n, anchor, cw, ch, size.width as f32, size.height as f32, CLIP_PREVIEW_COLS);
            chrome::clip_history::draw(active.backend.as_mut(), &g, &previews, &badges, Some(sel), sel, cw, ch);
        }
```

(Read the surrounding redraw code for the exact backend accessor — e.g. `active.backend.as_mut()` vs a `be` local — and match it.)

- [ ] **Step 6: Build**

Run: `cargo build --release -p rt --features x11 2>&1 | grep -E "^error|warning:|Finished"`
Expected: `Finished`, no errors, no new warnings.

- [ ] **Step 7: Regression gate**

Run: `cargo test --workspace --release -- --test-threads=1 2>&1 | grep -E "test result:|error\["`
Expected: all pass, 0 failed.

- [ ] **Step 8: Manual verification (dop651)**

After `cargo install --path crates/rt --force`, run `rt`:
1. Copy a few different things (drag-select, Ctrl+Shift+C). Press **Ctrl+Shift+H** → the overlay lists them, newest first, one-line previews with a size badge; re-copying an existing one moves it to the top (no dupe).
2. **↑/↓** move the selection; **Enter** on a clip → it pastes into the focused pane and **Ctrl+Shift+V** repeats it.
3. Click a clip → same paste+promote. Click outside / **Esc** → closes.
4. A multi-line clip shows `↵` in the preview but pastes with real newlines.
5. The **Clear history** row empties it; reopening shows nothing (overlay won't open when empty).

- [ ] **Step 9: Commit**

```bash
git add crates/rt/src/main.rs
git commit -m "feat(rt): clipboard-history overlay — open, navigate, pick, paste+promote"
```

---

### Task 7: Titlebar affordance + Clear-history menu item

**Files:**
- Modify: `crates/rt/src/main.rs` — the focused-pane titlebar draw (the right-anchored fields near the size/meter, `main.rs:~3606`), and the click hit-testing on the titlebar; `crates/rt/src/menu.rs` — add a Clear-history item.

**Interfaces:**
- Consumes: `Active.clip_history`, `open_clip_history`.
- Produces: a clickable titlebar clip affordance; a menu row for `Action::ClearClipHistory`.

- [ ] **Step 1: Draw the affordance (focused pane only, non-empty history)**

In the titlebar builder, among the right-anchored fields for the FOCUSED pane, when `!active.clip_history.is_empty()`, draw a small glyph + count (e.g. `⎘ 7`) and remember its rect for hit-testing. Place it to the LEFT of the size string (advance `left_of` like the meter does):

```rust
                // Clipboard-history affordance: on the focused pane's titlebar, a
                // clip glyph + count that opens the history overlay on click.
                if focused && !active.clip_history.is_empty() {
                    let label = format!("⎘ {}", active.clip_history.len());
                    let lw = label.chars().count() as f32 * cell_w;
                    let lx = (left_of - 2.0 * cell_w - lw).max(left_x);
                    for (i, ch) in label.chars().enumerate() {
                        active.backend.draw_char(lx, text_top, i, 0, ch, Color::rgb(0x8b, 0x98, 0xa9), false, false);
                    }
                    active.clip_affordance = Some(Recti { x: lx, y: full.y, w: lw, h: bar_h });
                    left_of = lx;
                }
```

Add `clip_affordance: Option<Recti>` to `Active` (reset to `None` at the top of each titlebar-draw pass so a stale rect never lingers when the pane is unfocused or history empties), and initialise it `None`. Read the exact titlebar variable names (`left_of`, `left_x`, `text_top`, `full`, `bar_h`, `focused`, `cell_w`) in scope and match them.

- [ ] **Step 2: Click opens the overlay**

In the left-press handling for the titlebar (where titlebar clicks are already hit-tested — read the existing titlebar click code), before the normal titlebar handling, test the affordance rect:

```rust
                    if let Some(r) = active.clip_affordance {
                        if r.contains(active.mouse.0, active.mouse.1) {
                            Self::open_clip_history(active);
                            return;
                        }
                    }
```

(Use the existing point-in-rect helper — match whatever the titlebar hit-testing already uses; `Recti` may already have a `contains`. If not, compare bounds inline.)

- [ ] **Step 3: Clear-history menu item**

In `crates/rt/src/menu.rs` `rows(...)`, add an item near the other actions (in the `items()` list or as an explicit row) for Clear history, gated to when there is history — but `rows` has no history handle. Simplest: always show it and let `ClearClipHistory` be a no-op when empty. Add to the `items()` table:

```rust
        Item::Action("Clear Clipboard History", Action::ClearClipHistory),
```

- [ ] **Step 4: Build + regression gate**

Run: `cargo build --release -p rt --features x11 2>&1 | grep -E "^error|warning:|Finished"` → `Finished`, no errors.
Run: `cargo test --workspace --release -- --test-threads=1 2>&1 | grep -E "test result:"` → all pass.

- [ ] **Step 5: Manual verification (dop651)**

1. With per-pane titlebars on and something copied, the focused pane's titlebar shows `⎘ N`; clicking it opens the overlay. Unfocused panes don't show it; it disappears when history is cleared.
2. Right-click menu → **Clear Clipboard History** empties it.

- [ ] **Step 6: Commit**

```bash
git add crates/rt/src/main.rs crates/rt/src/menu.rs
git commit -m "feat(rt): clipboard-history titlebar affordance + Clear menu item"
```

---

## Self-Review

**Spec coverage** (against `docs/superpowers/specs/2026-07-24-clipboard-history-design.md`):
- MRU ring (cap 20, dedup, skip blank, in-memory) → Task 1.
- Preview (`↵`, truncation) + badge → Task 2.
- Capture at the three copy sites, not inside `store` → Task 4 (anchored commit covered via `do_copy`).
- Titlebar affordance + Ctrl+Shift+H → Tasks 3, 6, 7.
- Overlay list, previews, Clear row, keyboard + click → Tasks 5, 6.
- Pick → paste (`feed_paste`) + promote (`store`+`store_primary`) + move-to-front + close → Task 6.
- Privacy: in-memory only (no serialization anywhere), truncated previews, Clear action (overlay row + menu) → Tasks 1, 5, 6, 7.

**Placeholder scan:** no TBD/TODO; every code step shows complete code. The "read the surrounding lines / match the existing names" notes are explicit anchor checks (Tasks 4, 6, 7), not deferred work.

**Type consistency:** `ClipHistory`/`record`/`clear`/`len`/`is_empty`/`get`/`iter`, `preview`/`badge`, `layout`/`hit_row`/`draw`, `Geom{panel,rows,clear_row}`, `record_clip`/`open_clip_history`/`pick_clip`, `clip_overlay: Option<usize>`, `clip_affordance: Option<Recti>`, `Action::ClipHistory`/`ClearClipHistory`, `CLIP_PREVIEW_COLS`, `CLIP_HISTORY_MAX` are used consistently across tasks.

## Out of scope (own cycles)
- External OS-clipboard capture (polling) — not captured; rt's own copies only.
- On-disk persistence — deferred on privacy grounds (possible opt-in preference later).
- **C** — drag auto-scroll acceleration (separate spec).
