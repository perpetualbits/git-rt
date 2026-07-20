//! Spec-case conformance: a declarative table of `(input, expected observable state)`
//! cases that encode xterm/ECMA-48 behaviour, run against any [`VtEngine`].
//!
//! This is the codified-spec layer of the plan (the esctest/vttest role). It reads the
//! engine's state DIRECTLY via [`observe`](crate::VtEngine::observe) rather than through
//! the terminal's own report sequences the way the Python esctest does — cleaner, and
//! it needs no query/report support in the engine under test. (Driving the actual
//! Python esctest, which relies on DSR/DECRQCRA replies, is a separate future path that
//! would additionally exercise the report code — a good Phase-3 forcing function.)
//!
//! Every case is authored against the vendored oracle's *actual* behaviour, so the
//! oracle validates the cases too; a wrong expectation fails loudly and gets fixed.

use crate::{feed_whole, NColor, ScreenState, VtEngine};

/// One assertion about the observed state after feeding a case's input.
#[derive(Clone, Debug)]
pub enum Check {
    /// Cursor is shown at `(col, line)`.
    Cursor(usize, usize),
    /// No cursor is shown (hidden or scrolled back).
    NoCursor,
    /// The character at `(row, col)`.
    Char(usize, usize, char),
    /// The trailing-trimmed text of `row`.
    Row(usize, &'static str),
    /// The cell at `(row, col)` has ALL of these attribute bits set.
    Attr(usize, usize, u16),
    /// The cell at `(row, col)` has NONE of these attribute bits set.
    NoAttr(usize, usize, u16),
    /// The foreground colour at `(row, col)`.
    Fg(usize, usize, NColor),
    /// The background colour at `(row, col)`.
    Bg(usize, usize, NColor),
    /// The alternate-screen flag.
    AltScreen(bool),
    /// The application-cursor-keys flag.
    AppCursor(bool),
}

/// A named conformance case: feed `input` into a fresh `cols`×`rows` terminal, then
/// every check in `checks` must hold.
pub struct SpecCase {
    pub name: &'static str,
    pub cols: usize,
    pub rows: usize,
    pub input: Vec<u8>,
    pub checks: Vec<Check>,
}

fn case(name: &'static str, cols: usize, rows: usize, input: &[u8], checks: Vec<Check>) -> SpecCase {
    SpecCase { name, cols, rows, input: input.to_vec(), checks }
}

fn row_text(s: &ScreenState, row: usize) -> String {
    s.grid[row].iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
}

/// Evaluate one check against a state; `Some(msg)` on failure.
fn eval(s: &ScreenState, chk: &Check) -> Option<String> {
    match *chk {
        Check::Cursor(col, line) => match s.cursor {
            Some(c) if c.col == col && c.line == line && c.visible => None,
            other => Some(format!("expected cursor at ({col},{line}), got {other:?}")),
        },
        Check::NoCursor => match s.cursor {
            None => None,
            other => Some(format!("expected no cursor, got {other:?}")),
        },
        Check::Char(row, col, ch) => {
            let got = s.grid[row][col].ch;
            (got != ch).then(|| format!("char ({row},{col}) = {got:?}, expected {ch:?}"))
        }
        Check::Row(row, text) => {
            let got = row_text(s, row);
            (got != text).then(|| format!("row {row} = {got:?}, expected {text:?}"))
        }
        Check::Attr(row, col, mask) => {
            let a = s.grid[row][col].attrs;
            (a & mask != mask).then(|| format!("attrs ({row},{col}) = {a:#06b}, missing {mask:#06b}"))
        }
        Check::NoAttr(row, col, mask) => {
            let a = s.grid[row][col].attrs;
            (a & mask != 0).then(|| format!("attrs ({row},{col}) = {a:#06b}, should lack {mask:#06b}"))
        }
        Check::Fg(row, col, want) => {
            let got = s.grid[row][col].fg;
            (got != want).then(|| format!("fg ({row},{col}) = {got:?}, expected {want:?}"))
        }
        Check::Bg(row, col, want) => {
            let got = s.grid[row][col].bg;
            (got != want).then(|| format!("bg ({row},{col}) = {got:?}, expected {want:?}"))
        }
        Check::AltScreen(want) => {
            (s.alt_screen != want).then(|| format!("alt_screen = {}, expected {want}", s.alt_screen))
        }
        Check::AppCursor(want) => {
            (s.app_cursor != want).then(|| format!("app_cursor = {}, expected {want}", s.app_cursor))
        }
    }
}

/// Run every case against engine `E`, returning a list of readable failure messages
/// (empty = all pass). The same cases run against the vendored oracle today and
/// against `vt-term` later.
pub fn run_spec_cases<E: VtEngine>(cases: &[SpecCase]) -> Vec<String> {
    let mut fails = Vec::new();
    for c in cases {
        let st = feed_whole::<E>(c.cols, c.rows, &c.input);
        for (i, chk) in c.checks.iter().enumerate() {
            if let Some(msg) = eval(&st, chk) {
                fails.push(format!("[{}] check #{i}: {msg}", c.name));
            }
        }
    }
    fails
}

use crate::attr;
use Check::*;

/// The seeded conformance set — a first batch across the important sequence families
/// (cursor motion, erase, SGR, wrap, screens, save/restore, index, tabs, insert/delete,
/// scroll region). Grows over time (ported esctest cases welcome here).
pub fn cases() -> Vec<SpecCase> {
    vec![
        // ── Cursor motion ──────────────────────────────────────────────────────
        case("CUP absolute", 80, 24, b"\x1b[3;5H", vec![Cursor(4, 2)]),
        case("CUU up", 80, 24, b"\x1b[5;5H\x1b[2A", vec![Cursor(4, 2)]),
        case("CUD down", 80, 24, b"\x1b[1;5H\x1b[3B", vec![Cursor(4, 3)]),
        case("CUF forward", 80, 24, b"\x1b[1;1H\x1b[4C", vec![Cursor(4, 0)]),
        case("CUB back", 80, 24, b"\x1b[1;9H\x1b[3D", vec![Cursor(5, 0)]),
        case("CHA column", 80, 24, b"\x1b[2;2H\x1b[10G", vec![Cursor(9, 1)]),
        case("VPA row", 80, 24, b"\x1b[1;3H\x1b[7d", vec![Cursor(2, 6)]),
        // ── Erase ──────────────────────────────────────────────────────────────
        case("ED 2 blanks screen", 80, 24, b"hi\r\nthere\x1b[2J", vec![Row(0, ""), Row(1, "")]),
        case("EL 0 to end of line", 80, 24, b"ABCDE\x1b[1;3H\x1b[0K", vec![Row(0, "AB")]),
        case("EL 1 to start of line", 80, 24, b"ABCDE\x1b[1;3H\x1b[1K", vec![Row(0, "   DE")]),
        case("EL 2 whole line", 80, 24, b"ABCDE\x1b[2K", vec![Row(0, "")]),
        // ── SGR ────────────────────────────────────────────────────────────────
        case("SGR bold then reset", 80, 24, b"\x1b[1mA\x1b[0mB",
             vec![Attr(0, 0, attr::BOLD), Char(0, 0, 'A'), NoAttr(0, 1, attr::BOLD), Char(0, 1, 'B')]),
        case("SGR underline+italic", 80, 24, b"\x1b[3;4mZ",
             vec![Attr(0, 0, attr::ITALIC | attr::UNDERLINE)]),
        case("SGR inverse", 80, 24, b"\x1b[7mI", vec![Attr(0, 0, attr::INVERSE)]),
        case("SGR truecolor fg", 80, 24, b"\x1b[38;2;10;20;30mA", vec![Fg(0, 0, NColor::Rgb(10, 20, 30))]),
        case("SGR indexed fg", 80, 24, b"\x1b[38;5;42mA", vec![Fg(0, 0, NColor::Indexed(42))]),
        case("SGR truecolor bg", 80, 24, b"\x1b[48;2;1;2;3mA", vec![Bg(0, 0, NColor::Rgb(1, 2, 3))]),
        // ── Autowrap ───────────────────────────────────────────────────────────
        case("autowrap on", 5, 24, b"ABCDEFG", vec![Row(0, "ABCDE"), Row(1, "FG")]),
        case("autowrap off (DECAWM)", 5, 24, b"\x1b[?7lABCDEFG", vec![Row(0, "ABCDG"), Row(1, "")]),
        // ── Screens / save-restore ───────────────────────────────────────────────
        case("alt screen enter", 80, 24, b"\x1b[?1049h", vec![AltScreen(true)]),
        case("alt screen leave", 80, 24, b"\x1b[?1049h\x1b[?1049l", vec![AltScreen(false)]),
        case("DECCKM app cursor", 80, 24, b"\x1b[?1h", vec![AppCursor(true)]),
        case("DECSC/DECRC save+restore cursor", 80, 24, b"\x1b[5;5H\x1b7\x1b[10;10H\x1b8", vec![Cursor(4, 4)]),
        // ── Index / newline ──────────────────────────────────────────────────────
        case("IND index down", 80, 24, b"\x1b[1;1H\x1bD", vec![Cursor(0, 1)]),
        case("RI reverse index up", 80, 24, b"\x1b[3;1H\x1bM", vec![Cursor(0, 1)]),
        case("NEL next line", 80, 24, b"\x1b[3;5H\x1bE", vec![Cursor(0, 3)]),
        // ── Tabs ─────────────────────────────────────────────────────────────────
        case("HT default tab stop", 80, 24, b"A\tB", vec![Char(0, 0, 'A'), Char(0, 8, 'B')]),
        // ── Insert / delete ──────────────────────────────────────────────────────
        case("ICH insert chars", 80, 24, b"ABCDE\x1b[1;2H\x1b[3@", vec![Row(0, "A   BCDE")]),
        case("DCH delete chars", 80, 24, b"ABCDE\x1b[1;2H\x1b[2P", vec![Row(0, "ADE")]),
        case("IL insert line", 80, 24, b"A\r\nB\r\nC\x1b[1;1H\x1b[L", vec![Row(0, ""), Row(1, "A"), Row(2, "B")]),
        case("DL delete line", 80, 24, b"A\r\nB\r\nC\x1b[1;1H\x1b[M", vec![Row(0, "B"), Row(1, "C")]),
        // ── Scroll region (DECSTBM) ──────────────────────────────────────────────
        // Region = lines 1..=3 (1-based 2;4). Fill 4 rows, home into region, feed a
        // line feed at the region bottom → only the region scrolls.
        case("DECSTBM confines scroll", 80, 24, b"\x1b[2;4r\x1b[1;1Ha\r\nb\r\nc\r\nd\x1b[4;1H\n",
             vec![Row(0, "a"), Row(1, "c"), Row(2, "d")]),
    ]
}
