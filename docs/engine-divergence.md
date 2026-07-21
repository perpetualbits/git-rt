# Engine divergence ledger

Where the in-house `vt-term` does NOT yet match the vendored `alacritty_terminal`
oracle. The Phase-3 process (see `docs/own-engine-plan.md`) is to drive this list to
empty (or to *intentional*, documented differences) under the `vt-conformance` harness.
Each entry: what diverges, the measured impact, and the plan.

Status snapshot (2026-07-21):
- Spec cases (`spec.rs`, 32 cases): **PASS** against vt-term.
- Curated differential (`vtterm_diff.rs`, 16 scripts): **PASS**.
- Random-fuzz FULL differential (`vtterm_fuzz.rs`, 8000 scripts) — grid, cursor, modes,
  AND scrollback history: **0 divergences** (verified 0/10000 in a wider sweep). Locked
  in as a test, green on x86_64 and riscv64.

**vt-term now matches the vendored oracle exactly on every fuzzed input.** The open
items below are not-yet-exercised features (nothing in the fuzz reaches them yet).

### Fixed under the harness (2026-07-21)
Four alacritty behaviours the differential fuzz surfaced, each traced to a minimal
reproducer via delta-debugging and matched:
- **LF keeps `pending_wrap`** — linefeed/newline do NOT clear the deferred-wrap flag
  (they did in the first draft), so a char after a bare LF wraps one more line. This
  one fix took grid divergence 3.2%→0.36%.
- **EL-Right is a no-op while a wrap is pending** (`clear_line … if input_needs_wrap`).
- **Private-marker CSI** (`?…H` etc.) is ignored; only `?…h/l` (DECSET/DECRST) act.
- **`pending_wrap` is part of the cursor** — saved/restored by the alternate screen and
  DECSC/DECRC.

Scrollback (the ring buffer) reconciled to the oracle:
- **History grows only on a top-anchored scroll** (`scroll_up` when the region starts at
  row 0), never inside a DECSTBM region that starts below the top, never on the alt
  screen.
- **`\x1b[2J` scrolls the viewport into history** (alacritty's `clear_viewport`), not a
  plain blank. `positions` = last-non-empty-row + 1; on an all-empty screen it is 1 when
  history is empty (the scan stops at line 0) and 0 otherwise (it descends to line −1) —
  a genuine iterator edge, matched exactly.
- **The alt screen reports `history_size` 0** (no scrollback); the primary's history is
  preserved and returns on exit. `\x1b[3J` clears scrollback.

Wide characters (CJK/emoji) reconciled to the oracle:
- Wide glyph + trailing spacer placement, the right-edge leading spacer + wrap, and the
  WIDE flag (derived from char width) all match. A `spacer` flag distinguishes a real
  trailing spacer from an erase-left blank, so overwriting a spacer clears the glyph
  (alacritty's clear_wide) but overwriting an EL'd blank does not.
- `clear_viewport`'s emptiness scan treats a spacer as non-empty (matching alacritty's
  is_empty, which also ignores bold/dim/italic).
- DCH clamps its count to the FULL width (not cols−col), so a large count also clears
  cells left of the cursor.
- CNL (`ESC [ E`) / CPL (`ESC [ F`) added; private-marker CSI (`ESC [ ? … H`) ignored.

Charsets (DEC line drawing) reconciled to the oracle:
- G0–G3 designations (`ESC ( ) * + <final>`, `0` = Special) are part of the CURSOR:
  saved/restored by the alt screen and DECSC. The active charset `gl` (SI/SO) is
  Term-GLOBAL and is NOT swapped by the alt screen — matching alacritty exactly. The
  DEC special-graphics map matches `StandardCharset::map` character-for-character.

## Open divergences

- **Combining mark exactly at a pending-wrap boundary.** A zero-width mark arriving when
  the cursor is in the deferred-wrap state resolves the wrap in the oracle but not in
  vt-term (`pending_wrap` is not observable, so the harness only catches it via a later
  op). Obscure — combining marks rarely land on the last column — and parked out of the
  fuzz generator. Everywhere else combining marks match (attach to the base, ignored).

The scrollback ring is implemented; the full differential (grid, cursor, modes, history,
wide chars AND charsets) is **0/10000**. `display_offset` is always observed at 0 (bottom of the
view); reading scrolled-back lines / viewport scrolling are future.

## Reflow on resize — implemented, common cases matched (2026-07-21)

vt-term now reflows (was: truncate/extend). Algorithm mirrors alacritty: **lines first**
(a pure row move — the cursor is kept in view by scrolling top rows into scrollback on the
primary screen, or discarding them on the alt screen), **then columns** (rejoin
`WRAPLINE`-marked soft-wrapped rows into logical lines, re-split at the new width with
leading spacers for wide glyphs at the boundary, re-lay-out bottom-anchored, track the
cursor). The alt screen does not reflow columns (truncate/extend + clamp).

Result: **0 divergence on the non-resize fuzz is unchanged (0/10000)**; a random-resize
sweep matches the oracle on **~92%** (≈240/3000 diverge), guarded by
`tests/vtterm_reflow.rs` (curated exact cases + a fuzz-rate ceiling). Down from ~65%
divergence (pure truncate/extend). Remaining divergences, to drive to zero:

- **Exact cursor position through reflow.** When the cursor's reflowed offset lands on a
  column boundary, alacritty's `Boundary::Cursor` math places it differently than our
  offset→(row,col) mapping (~2% of cases; grid is otherwise identical).
- **Wide-glyph reflow boundaries in reflowed history.** A one-column shift can appear
  around a wide glyph sitting at an old/new wrap boundary within reflowed scrollback.
- **Root cause for several:** alacritty tracks each row's *written* occupied length
  (`occ`); our fixed-width rows approximate it from content (trim trailing blanks), which
  differs for printed-then-cleared cells. Closing this likely needs real `occ` tracking.

### Synchronized updates (DECSET/DECRST 2026) — implemented (2026-07-21)

The vendored `vte` buffers all bytes between `\x1b[?2026h` and `\x1b[?2026l`, applying
them atomically at the end (or on a 2 MiB cap); vt-parser now does the same, in a layer
above the raw state machine (`Parser::feed`; see `docs/vt-parser-design.md` §6a). Only
*observable* when a feed ends mid-sync (the oracle holds the buffered tail unapplied) —
exactly how a captured stream ends. **Surfaced by the `spiral_stress` replay corpus**, not
the fuzz (the generator emits no 2026). All four corpus fixtures now match the oracle
whole-feed and chunk-split (`tests/replay.rs::replay_corpus_matches_oracle`). The raw
`Parser::advance` path is unchanged, so the parser-vs-`vte` differential and the throughput
bench are unaffected.

## Known not-yet-implemented (will diverge when exercised)

- **Colon sub-parameter SGR** beyond the extended-colour case.
- **OSC / DCS semantics** (title, clipboard, hyperlinks): parsed but not applied.
- **Origin mode** edge interactions, DECSCUSR cursor shape, LNM newline mode.

## Reconciliations already done

- **Neutral colour model** unified to `Default`/`Indexed`/`Rgb`: alacritty named
  colours 0–15 → `Indexed`, Foreground/Background → the `Named(256)` default sentinel,
  matching vt-term's `Color::Default`.
- **ED(Above)** matched to alacritty's `cursor.line > 1` quirk.
- **Tab** matched to alacritty's write-`\t`-glyph-into-the-blank-start-cell behaviour.
