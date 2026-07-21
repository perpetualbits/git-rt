//! Replay real-world byte streams (`corpus/*.bytes`) through the engine. Two properties:
//! (1) **chunk-invariance** — the SAME captured bytes fed in any framing must produce
//! identical state, which stresses control-sequence AND multibyte-UTF-8 resumption across
//! read boundaries (see `corpus/tui_frame.bytes`'s box-drawing); and (2) **vendored ==
//! vt-term** — the in-house engine reproduces the oracle's state on exactly the traffic
//! real programs emit. Property (2) is the highest-signal test here: it found the missing
//! synchronized-update support (DECSET 2026) that 10 000 fuzz seeds never reached, because
//! `spiral_stress.bytes` (a real TUI capture) ends mid-synchronized-update.

use std::fs;
use std::path::PathBuf;

use vt_conformance::vendored::Vendored;
use vt_conformance::{feed_chunks, feed_whole, split};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

#[test]
fn replay_corpus_is_chunk_invariant() {
    let mut fixtures = 0;
    for entry in fs::read_dir(corpus_dir()).expect("corpus dir readable") {
        let path = entry.unwrap().path();
        if path.extension().map_or(false, |e| e == "bytes") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = fs::read(&path).unwrap();

            let whole = feed_whole::<Vendored>(80, 24, &bytes);
            // Several framings, including 1-byte chunks that split every multibyte char.
            for seed in 0..8u64 {
                let chunked = feed_chunks::<Vendored>(80, 24, &split(&bytes, seed));
                if let Some(d) = whole.diff(&chunked) {
                    panic!("{name}: chunk-invariance broke (split seed {seed}): {d}");
                }
            }
            // Sanity: the stream neither panicked nor produced out-of-bounds state.
            assert_eq!((whole.cols, whole.rows), (80, 24), "{name} dims");
            if let Some(c) = whole.cursor {
                assert!(c.col < 80 && c.line < 24, "{name} cursor out of bounds: {c:?}");
            }
            fixtures += 1;
        }
    }
    assert!(fixtures >= 3, "expected ≥3 corpus fixtures, found {fixtures}");
}

/// vt-term must reproduce the oracle's state on every corpus fixture — whole-feed and
/// under chunked framing (which splits synchronized-update escapes and multibyte chars
/// across reads). This is the differential that surfaced the DECSET-2026 gap.
#[test]
fn replay_corpus_matches_oracle() {
    let mut fixtures = 0;
    for entry in fs::read_dir(corpus_dir()).expect("corpus dir readable") {
        let path = entry.unwrap().path();
        if path.extension().map_or(false, |e| e == "bytes") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = fs::read(&path).unwrap();

            let oracle = feed_whole::<Vendored>(80, 24, &bytes);
            let ours = feed_whole::<vt_term::Term>(80, 24, &bytes);
            if let Some(d) = oracle.diff(&ours) {
                panic!("{name}: vt-term diverges from oracle (whole feed): {d}");
            }
            for seed in 0..4u64 {
                let chunked = feed_chunks::<vt_term::Term>(80, 24, &split(&bytes, seed));
                if let Some(d) = oracle.diff(&chunked) {
                    panic!("{name}: vt-term diverges from oracle (split seed {seed}): {d}");
                }
            }
            fixtures += 1;
        }
    }
    assert!(fixtures >= 3, "expected ≥3 corpus fixtures, found {fixtures}");
}
