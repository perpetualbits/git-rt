//! Native-damage oracle: vt-term's per-frame damage must cover EVERY cell that actually
//! changed. The brute-force grid diff (comparing the whole grid before/after) is the oracle;
//! native damage is required to be a *superset* of it. A mutation site that forgets to mark
//! damage makes a changed cell fall outside every reported span → this test fails, pointing
//! at the seed. Cursor-only damage is extra (a superset), so comparing grid cells is the
//! right, tight property.

use vt_conformance::{gen_script, split};
use vt_term::{Damage, Term};

fn grid_of(t: &Term) -> Vec<Vec<vt_term::Cell>> {
    (0..t.rows()).map(|r| (0..t.cols()).map(|c| t.cell(r, c)).collect()).collect()
}

fn covers(d: &Damage, line: usize, col: usize) -> bool {
    match d {
        Damage::Full => true,
        Damage::Spans(spans) => {
            spans.iter().any(|s| s.line == line && col >= s.left && col <= s.right)
        }
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
                    if before[r][c] != after[r][c] {
                        assert!(
                            covers(&dmg, r, c),
                            "seed {seed}: cell (row {r}, col {c}) changed \
                             {:?} -> {:?} but is not covered by damage {dmg:?}",
                            before[r][c], after[r][c],
                        );
                    }
                }
            }
            before = after;
        }
    }
}
