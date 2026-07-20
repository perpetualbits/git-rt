//! Parser-layer differential test: feed identical bytes to the in-house `vt-parser`
//! and the vendored `vte`, and assert they emit the SAME action stream — the Phase-2
//! correctness contract. Covers whole-buffer and arbitrarily-chunked feeds (so the
//! parser must resume across read boundaries, incl. split multibyte UTF-8), a rich
//! structured fuzzer, and the real-world replay corpus.

use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;

use vt_conformance::{split, Rng};

/// One recorded action, in engine-neutral form so vte's and vt-parser's callbacks
/// record into the same representation and compare directly.
#[derive(Debug, PartialEq, Clone)]
enum Action {
    Print(char),
    PrintStr(String),
    Execute(u8),
    Csi { params: Vec<Vec<u16>>, inter: Vec<u8>, ignore: bool, action: char },
    Esc { inter: Vec<u8>, ignore: bool, byte: u8 },
    Osc { params: Vec<Vec<u8>>, bell: bool },
    Hook { params: Vec<Vec<u16>>, inter: Vec<u8>, ignore: bool, action: char },
    Put(u8),
    Unhook,
}

#[derive(Default)]
struct Rec(Vec<Action>);

impl vte::Perform for Rec {
    fn print(&mut self, c: char) {
        self.0.push(Action::Print(c));
    }
    fn print_str(&mut self, s: &str) {
        self.0.push(Action::PrintStr(s.to_string()));
    }
    fn execute(&mut self, b: u8) {
        self.0.push(Action::Execute(b));
    }
    fn hook(&mut self, params: &vte::Params, inter: &[u8], ignore: bool, action: char) {
        self.0.push(Action::Hook {
            params: params.iter().map(|p| p.to_vec()).collect(),
            inter: inter.to_vec(),
            ignore,
            action,
        });
    }
    fn put(&mut self, b: u8) {
        self.0.push(Action::Put(b));
    }
    fn unhook(&mut self) {
        self.0.push(Action::Unhook);
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], bell: bool) {
        self.0.push(Action::Osc { params: params.iter().map(|p| p.to_vec()).collect(), bell });
    }
    fn csi_dispatch(&mut self, params: &vte::Params, inter: &[u8], ignore: bool, action: char) {
        self.0.push(Action::Csi {
            params: params.iter().map(|p| p.to_vec()).collect(),
            inter: inter.to_vec(),
            ignore,
            action,
        });
    }
    fn esc_dispatch(&mut self, inter: &[u8], ignore: bool, byte: u8) {
        self.0.push(Action::Esc { inter: inter.to_vec(), ignore, byte });
    }
}

impl vt_parser::Perform for Rec {
    fn print(&mut self, c: char) {
        self.0.push(Action::Print(c));
    }
    fn print_str(&mut self, s: &str) {
        self.0.push(Action::PrintStr(s.to_string()));
    }
    fn execute(&mut self, b: u8) {
        self.0.push(Action::Execute(b));
    }
    fn hook(&mut self, params: &vt_parser::Params, inter: &[u8], ignore: bool, action: char) {
        self.0.push(Action::Hook {
            params: params.iter().map(|p| p.to_vec()).collect(),
            inter: inter.to_vec(),
            ignore,
            action,
        });
    }
    fn put(&mut self, b: u8) {
        self.0.push(Action::Put(b));
    }
    fn unhook(&mut self) {
        self.0.push(Action::Unhook);
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], bell: bool) {
        self.0.push(Action::Osc { params: params.iter().map(|p| p.to_vec()).collect(), bell });
    }
    fn csi_dispatch(&mut self, params: &vt_parser::Params, inter: &[u8], ignore: bool, action: char) {
        self.0.push(Action::Csi {
            params: params.iter().map(|p| p.to_vec()).collect(),
            inter: inter.to_vec(),
            ignore,
            action,
        });
    }
    fn esc_dispatch(&mut self, inter: &[u8], ignore: bool, byte: u8) {
        self.0.push(Action::Esc { inter: inter.to_vec(), ignore, byte });
    }
}

fn vte_actions(chunks: &[&[u8]]) -> Vec<Action> {
    let mut p = vte::Parser::new();
    let mut r = Rec::default();
    for c in chunks {
        p.advance(&mut r, c);
    }
    r.0
}

fn own_actions(chunks: &[&[u8]]) -> Vec<Action> {
    let mut p = vt_parser::Parser::new();
    let mut r = Rec::default();
    for c in chunks {
        p.advance(&mut r, c);
    }
    r.0
}

fn assert_same(label: impl Debug, chunks: &[&[u8]]) {
    let a = vte_actions(chunks);
    let b = own_actions(chunks);
    if a != b {
        let flat: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        panic!(
            "action-stream mismatch [{label:?}]\ninput: {:?}\n  vte: {a:?}\n  own: {b:?}",
            String::from_utf8_lossy(&flat)
        );
    }
}

/// A structured generator that reaches every sequence family and the nasty edges:
/// multibyte UTF-8, C1 bytes, invalid UTF-8, CSI with subparams/intermediates/private
/// markers, ESC, OSC (BEL- and ST-terminated), and DCS.
fn gen(seed: u64, tokens: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let mut o = Vec::new();
    let utf8 = ["é", "你", "🦀", "ß", "™"];
    for _ in 0..tokens {
        match r.below(16) {
            0..=3 => {
                for _ in 0..1 + r.below(6) {
                    o.push(b'a' + r.below(26) as u8);
                }
            }
            4 => o.extend_from_slice(utf8[r.below(utf8.len() as u32) as usize].as_bytes()),
            5 => o.push(match r.below(6) {
                0 => b'\n',
                1 => b'\r',
                2 => b'\t',
                3 => 0x08,
                4 => 0x07,
                _ => 0x0c,
            }),
            6 => o.push(0x80 + r.below(0x20) as u8), // a C1 byte (lone high byte)
            7 => o.push(r.below(256) as u8),         // any byte (hits error paths)
            8 => {
                // CSI: optional private marker, params w/ subparams, intermediates, final
                o.extend_from_slice(b"\x1b[");
                if r.below(3) == 0 {
                    o.push(b"<=>?"[r.below(4) as usize]);
                }
                let nparams = r.below(4);
                for i in 0..nparams {
                    if i > 0 {
                        o.push(b';');
                    }
                    o.extend_from_slice(format!("{}", r.below(300)).as_bytes());
                    if r.below(4) == 0 {
                        o.push(b':');
                        o.extend_from_slice(format!("{}", r.below(300)).as_bytes());
                    }
                }
                if r.below(4) == 0 {
                    o.push(b" !#$%"[r.below(5) as usize]);
                }
                o.push(0x40 + r.below(0x3f) as u8); // a final byte in 0x40..=0x7e
            }
            9 => o.extend_from_slice(b"\x1b[38;2;10;20;30m"),
            10 => o.extend_from_slice(b"\x1b[?1049h"),
            11 => {
                // ESC sequence (incl. intermediates)
                o.push(0x1b);
                if r.below(2) == 0 {
                    o.push(b"()*+"[r.below(4) as usize]);
                }
                o.push(0x30 + r.below(0x40) as u8);
            }
            12 => {
                // OSC, BEL- or ST-terminated
                o.extend_from_slice(b"\x1b]");
                o.extend_from_slice(format!("{}", r.below(20)).as_bytes());
                o.push(b';');
                for _ in 0..r.below(6) {
                    o.push(b'A' + r.below(26) as u8);
                }
                if r.below(2) == 0 {
                    o.push(0x07);
                } else {
                    o.extend_from_slice(b"\x1b\\");
                }
            }
            13 => {
                // DCS ... ST
                o.extend_from_slice(b"\x1bP");
                o.extend_from_slice(format!("{}", r.below(10)).as_bytes());
                o.push(b'|');
                for _ in 0..r.below(6) {
                    o.push(b'A' + r.below(26) as u8);
                }
                o.extend_from_slice(b"\x1b\\");
            }
            _ => o.push(0x1b), // a lone ESC (restart / truncation edge)
        }
    }
    o
}

#[test]
fn parser_matches_vte_whole() {
    for seed in 0..4_000u64 {
        let s = gen(seed, 120);
        assert_same(seed, &[&s]);
    }
}

#[test]
fn parser_matches_vte_chunked() {
    for seed in 0..4_000u64 {
        let s = gen(seed, 120);
        let chunks = split(&s, seed ^ 0x1234);
        assert_same(("chunked", seed), &chunks);
    }
}

#[test]
fn parser_matches_vte_on_corpus() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus");
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map_or(false, |e| e == "bytes") {
            let bytes = fs::read(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert_same((name.clone(), "whole"), &[&bytes]);
            for seed in 0..6u64 {
                assert_same((name.clone(), "chunked", seed), &split(&bytes, seed));
            }
            n += 1;
        }
    }
    assert!(n >= 3);
}
