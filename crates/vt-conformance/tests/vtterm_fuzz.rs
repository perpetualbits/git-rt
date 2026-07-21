//! Random-fuzz differential: the in-house `vt_term::Term` vs the vendored oracle over
//! thousands of generated scripts. Compares the FULL observable state — grid, cursor, modes, AND scrollback history.

use vt_conformance::{feed_whole, gen_script, vendored::Vendored};

#[test]
fn vt_term_matches_oracle_under_fuzz() {
    let mut fails = 0;
    let mut first = None;
    for seed in 0..8000u64 {
        let s = gen_script(seed, 150);
        // A small grid maximises wrap/scroll edge coverage.
        let a = feed_whole::<Vendored>(24, 8, &s);
        let b = feed_whole::<vt_term::Term>(24, 8, &s);
        if let Some(d) = a.diff(&b) {
            fails += 1;
            if first.is_none() {
                first = Some(format!("seed {seed}: {d}"));
            }
        }
    }
    assert_eq!(fails, 0, "vt-term divergences vs oracle: {fails}/8000\nfirst: {first:?}");
}
