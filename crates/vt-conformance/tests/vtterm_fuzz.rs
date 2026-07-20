//! Random-fuzz differential: the in-house `vt_term::Term` vs the vendored oracle over
//! thousands of generated scripts. Compares the grid, cursor, and modes; scrollback
//! (history / display_offset) is a deferred feature (see docs/engine-divergence.md), so
//! those counters are neutralised until the scrollback ring lands.

use vt_conformance::{feed_whole, gen_script, vendored::Vendored};

#[test]
fn vt_term_grid_matches_oracle_under_fuzz() {
    let mut fails = 0;
    let mut first = None;
    for seed in 0..5000u64 {
        let s = gen_script(seed, 150);
        // A small grid maximises wrap/scroll edge coverage.
        let mut a = feed_whole::<Vendored>(24, 8, &s);
        let mut b = feed_whole::<vt_term::Term>(24, 8, &s);
        a.history = 0;
        b.history = 0;
        a.display_offset = 0;
        b.display_offset = 0;
        if let Some(d) = a.diff(&b) {
            fails += 1;
            if first.is_none() {
                first = Some(format!("seed {seed}: {d}"));
            }
        }
    }
    assert_eq!(fails, 0, "vt-term grid divergences vs oracle: {fails}/5000\nfirst: {first:?}");
}
