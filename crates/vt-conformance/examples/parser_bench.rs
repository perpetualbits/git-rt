//! Throughput benchmark: the in-house `vt-parser` vs vendored `vte`, on representative
//! terminal workloads. Both are driven through the SAME counting no-op sink, so we
//! measure the parser (state machine + fast path + params handling), not a Term.
//!
//! Run (RELEASE is essential):
//!   cargo run --release --example parser_bench -p vt-conformance
//!
//! Reports MB/s for each engine and the ratio (own ÷ vte; >1.0 = we are faster). Run on
//! x86_64 AND riscv64 (milkv) — the slow board magnifies differences (perf mandate).

use std::hint::black_box;
use std::time::{Duration, Instant};

/// A minimal sink that does just enough observable work (a wrapping counter) that the
/// optimiser cannot elide the parse, without adding real Term cost.
#[derive(Default)]
struct Sink {
    n: u64,
}

impl vte::Perform for Sink {
    fn print(&mut self, _c: char) {
        self.n = self.n.wrapping_add(1);
    }
    fn print_str(&mut self, s: &str) {
        self.n = self.n.wrapping_add(s.len() as u64);
    }
    fn execute(&mut self, _b: u8) {
        self.n = self.n.wrapping_add(1);
    }
    fn csi_dispatch(&mut self, _p: &vte::Params, _i: &[u8], _ig: bool, _a: char) {
        self.n = self.n.wrapping_add(1);
    }
    fn esc_dispatch(&mut self, _i: &[u8], _ig: bool, _b: u8) {
        self.n = self.n.wrapping_add(1);
    }
    fn osc_dispatch(&mut self, _p: &[&[u8]], _bell: bool) {
        self.n = self.n.wrapping_add(1);
    }
}

impl vt_parser::Perform for Sink {
    fn print(&mut self, _c: char) {
        self.n = self.n.wrapping_add(1);
    }
    fn print_str(&mut self, s: &str) {
        self.n = self.n.wrapping_add(s.len() as u64);
    }
    fn execute(&mut self, _b: u8) {
        self.n = self.n.wrapping_add(1);
    }
    fn csi_dispatch(&mut self, _p: &vt_parser::Params, _i: &[u8], _ig: bool, _a: char) {
        self.n = self.n.wrapping_add(1);
    }
    fn esc_dispatch(&mut self, _i: &[u8], _ig: bool, _b: u8) {
        self.n = self.n.wrapping_add(1);
    }
    fn osc_dispatch(&mut self, _p: &[&[u8]], _bell: bool) {
        self.n = self.n.wrapping_add(1);
    }
}

fn parse_vte(buf: &[u8]) -> u64 {
    let mut p = vte::Parser::new();
    let mut s = Sink::default();
    p.advance(&mut s, buf);
    s.n
}

fn parse_own(buf: &[u8]) -> u64 {
    let mut p = vt_parser::Parser::new();
    let mut s = Sink::default();
    p.advance(&mut s, buf);
    s.n
}

/// Time-budgeted throughput in MB/s: run until ~`budget` elapsed, count bytes processed.
/// Self-adapts to the host, so it works on both a fast x86 and the slow milkv.
fn mbps(buf: &[u8], mut parse: impl FnMut(&[u8]) -> u64) -> f64 {
    black_box(parse(buf)); // warm up caches / branch predictor
    let budget = Duration::from_millis(700);
    let start = Instant::now();
    let mut bytes = 0u64;
    while start.elapsed() < budget {
        black_box(parse(black_box(buf)));
        bytes += buf.len() as u64;
    }
    bytes as f64 / start.elapsed().as_secs_f64() / 1e6
}

/// Repeat `pattern` until the buffer is at least `target` bytes.
fn grow(pattern: &[u8], target: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(target + pattern.len());
    while v.len() < target {
        v.extend_from_slice(pattern);
    }
    v
}

fn main() {
    const SIZE: usize = 4 << 20; // 4 MiB per workload
    let mut workloads: Vec<(&str, Vec<u8>)> = vec![
        ("plain ascii", grow(b"The quick brown fox jumps over the lazy dog. 0123456789\n", SIZE)),
        ("unicode text", grow("héllo 你好世界 🦀 wörld café ™ ελληνικά — dash\n".as_bytes(), SIZE)),
        ("sgr heavy", grow(b"\x1b[38;5;196mERR\x1b[0m \x1b[1;32mOK\x1b[0m \x1b[3;4;33mwarn\x1b[0m \x1b[38;2;10;20;30mrgb\x1b[0m\n", SIZE)),
        ("control heavy", grow(b"\x1b[2;5Hx\x1b[3;1H\r\ty\x1b[H\x1b[Kz\x08\x08w\n", SIZE)),
        ("mixed tui", grow(b"\x1b[?1049h\x1b[2J\x1b[1;1H\x1b[44;37m top \x1b[0m\x1b[3;3H\xe2\x94\x8c\xe2\x94\x80\xe2\x94\x90 data \x1b[10;1Hmore text here 123\n", SIZE)),
    ];
    // Real captured output of aerie's `spiral_stress` demo — a full-screen animated TUI
    // (truecolor SGR, box drawing, full clears): the most representative workload there
    // is. Grown to SIZE by repetition.
    let spiral = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/spiral_stress.bytes");
    if let Ok(bytes) = std::fs::read(&spiral) {
        workloads.push(("spiral (real)", grow(&bytes, SIZE)));
    }

    let arch = std::env::consts::ARCH;
    println!("parser throughput — {arch}  (4 MiB workloads, higher MB/s is better)\n");
    println!("{:<16} {:>12} {:>12} {:>10}", "workload", "vte MB/s", "own MB/s", "own/vte");
    println!("{}", "-".repeat(52));
    let mut ratios = Vec::new();
    for (name, buf) in &workloads {
        let v = mbps(buf, parse_vte);
        let o = mbps(buf, parse_own);
        let r = o / v;
        ratios.push(r);
        println!("{name:<16} {v:>12.0} {o:>12.0} {r:>9.2}x");
    }
    let geo = ratios.iter().map(|r| r.ln()).sum::<f64>() / ratios.len() as f64;
    println!("{}", "-".repeat(52));
    println!("geomean own/vte: {:.2}x", geo.exp());
}
