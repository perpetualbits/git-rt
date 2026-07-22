//! Native-damage oracle: vt-term's per-frame damage must let a renderer reproduce the true
//! grid. Two shapes:
//!   - `Full`/`Spans`: every cell that changed since the last frame must fall in a span.
//!   - `Scroll { lines, spans }`: the renderer blits the visible grid UP by `lines` (the
//!     scroll-blit) and then repaints `spans`; so after shifting the previous grid up by
//!     `lines`, every cell that still differs must fall in a span.
//! The brute-force before/after grid diff is the oracle; native damage must be a *superset*.
//! A mutation site that forgets to mark damage — or a wrong scroll count — fails this, over
//! thousands of fuzz scripts fed in random chunks. Cursor-only damage is extra (a superset),
//! so comparing grid cells is the right, tight property.

use vt_conformance::{gen_script, split};
use vt_term::{Damage, Term};

fn grid_of(t: &Term) -> Vec<Vec<vt_term::Cell>> {
    (0..t.rows()).map(|r| (0..t.cols()).map(|c| t.cell(r, c)).collect()).collect()
}

fn covers(d: &Damage, line: usize, col: usize) -> bool {
    let spans = match d {
        Damage::Full => return true,
        Damage::Spans(s) => s,
        Damage::Scroll { spans, .. } => spans,
    };
    spans.iter().any(|s| s.line == line && col >= s.left && col <= s.right)
}

/// The reference cell a renderer would show at `(r, c)` BEFORE repainting spans: for a plain
/// frame it's `before[r][c]`; for a scroll it's `before[r+lines][c]` (the blit), or — for the
/// newly-exposed bottom rows — nothing, so the cell must be repainted.
fn reference<'a>(d: &Damage, before: &'a [Vec<vt_term::Cell>], r: usize, c: usize) -> Option<&'a vt_term::Cell> {
    match d {
        Damage::Scroll { lines, .. } => before.get(r + lines).map(|row| &row[c]),
        _ => Some(&before[r][c]),
    }
}

#[test]
fn native_damage_covers_every_cell_change() {
    const N: u64 = 4000;
    for seed in 0..N {
        let script = gen_script(seed, 150);
        let chunks = split(&script, seed);
        let mut t = Term::new(24, 8);
        let _ = t.take_damage(); // drain the initial (full) state
        let mut before = grid_of(&t);
        for chunk in &chunks {
            t.feed(chunk);
            let dmg = t.take_damage();
            let after = grid_of(&t);
            for r in 0..after.len() {
                for c in 0..after[r].len() {
                    // A cell "changed" if it differs from what the renderer would show after
                    // the blit (if any) but before repainting spans.
                    let changed = reference(&dmg, &before, r, c) != Some(&after[r][c]);
                    if changed {
                        assert!(
                            covers(&dmg, r, c),
                            "seed {seed}: cell (row {r}, col {c}) = {:?} not reproduced by {dmg:?}",
                            after[r][c],
                        );
                    }
                }
            }
            before = after;
        }
    }
}

#[test]
fn full_screen_scroll_reports_scroll_damage() {
    let mut t = Term::new(10, 4);
    let _ = t.take_damage();
    t.feed(b"a\r\nb\r\nc\r\nd"); // fill 4 rows; cursor at bottom row
    let _ = t.take_damage();
    t.feed(b"\r\ne"); // LF at the bottom scrolls the whole screen up one, then prints
    match t.take_damage() {
        Damage::Scroll { lines, spans } => {
            assert_eq!(lines, 1, "one-line scroll");
            // the exposed bottom row (now "e") must be in the repaint spans
            assert!(spans.iter().any(|s| s.line == 3), "exposed row 3 dirty: {spans:?}");
        }
        other => panic!("expected Scroll, got {other:?}"),
    }
}

#[test]
fn subregion_scroll_falls_back_to_full() {
    let mut t = Term::new(10, 6);
    let _ = t.take_damage();
    t.feed(b"\x1b[1;3r"); // DECSTBM: scroll region rows 1..=3 (a sub-region)
    t.feed(b"a\r\nb\r\nc"); // fill the region; cursor at its bottom
    let _ = t.take_damage();
    t.feed(b"\r\nd"); // LF at the region bottom scrolls only rows 1..3 — not a full-grid blit
    assert_eq!(t.take_damage(), Damage::Full, "sub-region scroll must fall back to Full");
}
