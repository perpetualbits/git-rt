# Engine divergence ledger

Where the in-house `vt-term` does NOT yet match the vendored `alacritty_terminal`
oracle. The Phase-3 process (see `docs/own-engine-plan.md`) is to drive this list to
empty (or to *intentional*, documented differences) under the `vt-conformance` harness.
Each entry: what diverges, the measured impact, and the plan.

Status snapshot (2026-07-21):
- Spec cases (`spec.rs`, 32 cases): **PASS** against vt-term.
- Curated differential (`vtterm_diff.rs`, 16 scripts): **PASS**.
- Random-fuzz grid divergence, scrollback ignored (`vtterm_fuzz.rs`, 5000 scripts):
  **0** — the grid, cursor, and modes match the oracle exactly. Locked in as a test.
- Remaining full divergence is entirely the deferred scrollback counter (below).

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

## Open divergences

### 1. Scrollback / history (deferred feature) — the ONLY remaining divergence
vt-term has no scrollback: content scrolled off the top is dropped, so `history` stays
0 and `display_offset` stays 0 while the oracle accumulates them. With those counters
neutralised, grid divergence is **0/5000**. It is a *missing feature*, not a bug.
**Plan (next milestone):** a scrollback ring buffer, which also unlocks
`snapshot_lines`/scroll for the eventual rt wiring, and lets the full differential
(history included) go green.

## Known not-yet-implemented (will diverge when exercised)

- **Reflow on resize** — vt-term truncates/extends; the oracle rewraps. THE hard part,
  isolated to the end by design.
- **Wide characters** (CJK/emoji): treated as width 1; the oracle places a spacer cell.
- **Colon sub-parameter SGR** beyond the extended-colour case.
- **OSC / DCS semantics** (title, clipboard, hyperlinks): parsed but not applied.
- **Charsets** (G0–G3 designations): ignored (ASCII assumed).
- **Origin mode** edge interactions, DECSCUSR cursor shape, LNM newline mode.

## Reconciliations already done

- **Neutral colour model** unified to `Default`/`Indexed`/`Rgb`: alacritty named
  colours 0–15 → `Indexed`, Foreground/Background → the `Named(256)` default sentinel,
  matching vt-term's `Color::Default`.
- **ED(Above)** matched to alacritty's `cursor.line > 1` quirk.
- **Tab** matched to alacritty's write-`\t`-glyph-into-the-blank-start-cell behaviour.
