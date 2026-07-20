//! Replay real-world byte streams (`corpus/*.bytes`) through the engine. Today this
//! asserts chunk-invariance — the SAME captured bytes fed in any framing must produce
//! identical state, which stresses control-sequence AND multibyte-UTF-8 resumption
//! across read boundaries (see `corpus/tui_frame.bytes`'s box-drawing). When `vt-term`
//! lands, the same corpus is diffed vendored-vs-own — the highest-signal test there is,
//! because it is exactly the traffic real programs emit.

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
