# Plan: an in-house VT parser and Term, verified against the vendored engine

Goal: replace the vendored `alacritty_terminal` + `vte` with our own `vt-parser`
and `vt-term`, at a conformance level we can *demonstrate*, without ever putting
the working rt at risk.

## Hard guarantees (non-negotiable)

1. **rt keeps working the whole time.** The vendored, patched `alacritty_terminal`
   + `vte` stay the DEFAULT engine and a permanent fallback. No phase below changes
   rt's runtime behaviour until we deliberately flip a switch — and the switch is
   reversible.
2. **The vendored engine is also the ORACLE.** We do not test our engine against a
   spec we wrote; we test it against the battle-tested implementation we already
   ship, in-process, across millions of inputs.
3. **Nothing is big-bang.** Every phase delivers something independently useful and
   independently tested. If the program stops halfway, what shipped still has value.

## Guiding principles

- **Clean-room.** Study alacritty and foot for *design* (grid, damage, reflow);
  let *behaviour diffs*, never copied source, tie us to them. This keeps provenance
  clean (relevant given the upstream AI-patch friction) and the implementation ours.
- **Preserve the wins we already have.** The parser hot path is already solved in
  our fork — `memchr` scan-to-control in `advance_ground` + batched
  `print_str`/`input_run`. Our parser MUST keep both; they are the speed.
- **Verify the tractable piece in isolation.** The parser is a finite state machine
  emitting an action stream. Test the action stream against `vte` before any Term
  semantics enter the picture.
- **Reflow is the wall — isolate it.** It is the one genuinely hard part. Ship an
  interim "no rewrap on resize" and carry reflow tests on a documented divergence
  ledger until they pass. Never let reflow block the rest.

## Target workspace layout

```
crates/rt-engine        # THE SEAM: engine-agnostic trait + types; selects an impl
  engine/vendored.rs     #   impl over vendored alacritty_terminal (default + fallback)
  engine/own.rs          #   impl over vt-term (feature/env gated)
crates/vt-parser        # our clean-room VTE state machine (own vte)
crates/vt-term          # our Term: grid, scrollback, semantics, damage, reflow
crates/vt-conformance   # DEV-ONLY: oracle interface, differential fuzzer, esctest
                        #   runner, replay corpora, property tests, divergence ledger
vendor/vte              # kept: parser oracle + current default
vendor/alacritty_terminal # kept: Term oracle + current default
xtask (or a test bin)   # `verify` entrypoint that runs the whole battery
```

Both rt AND rt-mux consume `rt-engine`, so the seam serves both for free.

## Phase 0 — Establish the seam (enabling refactor; ships to current rt)

Today `rt-engine` re-exports alacritty types (`TermMode`, `cell::Flags`,
`grid::Scroll`, `index::*`, `Cell`). Swapping engines is impossible while rt speaks
alacritty's vocabulary. So:

- Define `rt-engine`'s OWN public vocabulary (extend the existing `Snapshot`/cell
  types into a full `Mode`, `Scroll`, coordinate, damage vocabulary).
- Make rt and rt-mux talk ONLY to `rt-engine` types. Confine every `alacritty_*`
  path to one file (`engine/vendored.rs`).
- **Exit criteria:** `rg alacritty crates/rt/src crates/rt-mux/src` is empty; rt and
  rt-mux run byte-for-byte as before; all existing tests pass; deploy to the three
  machines unchanged. This is worth doing even if the rest never happens — it
  decouples rt from a dependency's API.

## Phase 1 — Conformance & oracle harness (built BEFORE our engine)

The `vt-conformance` crate, with the "system under test" slot filled by *alacritty*
so it is green from day one and ready to accept our engine later:

- **Oracle interface:** "drive these bytes at rows×cols, read back grid + cursor +
  scrollback + damage." Implemented first for vendored alacritty.
- **Differential fuzzer:** random bytes + a structured escape-sequence generator →
  two engines → diff observable state. Bounded iterations per CI run; a long nightly
  soak.
- **Codified spec:** an esctest/vttest runner (these encode xterm — the ground truth
  alacritty AND foot both chase; higher-leverage than any single oracle).
- **Property tests** (proptest): cursor always in bounds; scrollback ≤ limit; reflow
  preserves total character content; resize-then-back is identity for unwrapped text.
- **Replay corpora:** captured raw byte streams from vim, tmux, emacs, htop, git,
  cargo; replayed and diffed. Stored as fixtures.
- **(Optional) foot as an out-of-process tiebreaker:** drive real foot over a PTY,
  scrape state, diff only ambiguous sequences. When alacritty and foot *agree* and we
  differ, we are wrong; when they *disagree*, we have found an under-specified corner
  to decide deliberately (log it).
- **Exit criteria:** the full battery runs green with alacritty in the SUT slot; a
  one-line change swaps in a candidate engine.

## Phase 2 — Own VTE parser (`vt-parser`)

The bounded, tractable piece; done first for a fast, low-risk win.

- Clean-room Williams/DEC state machine (ground/esc/csi/osc/dcs) + UTF-8, keeping the
  `memchr` fast path and batched printable runs.
- **Verify the ACTION STREAM, not pixels:** feed bytes to `vt-parser` and to `vte`;
  assert identical sequences of dispatched actions. Finite-state ⇒ transitions are
  near-exhaustively testable.
- Ship as a drop-in for the parse layer while STILL using alacritty's Term, so it can
  ride in real rt (behind the switch) with zero Term risk.
- **Exit criteria:** action-stream parity with `vte` across fuzzer + corpora; rt on
  `vt-parser` + alacritty-Term is indistinguishable in real use.

## Phase 3 — Own Term (`vt-term`)

The open-ended piece, grown strictly under the harness:

1. Grid + cursor + printing + SGR → match the oracle on simple cases.
2. Modes (DECSET/DECRST), scroll regions, tab stops, charsets, alt screen, OSC.
3. Scrollback + damage tracking (rt's renderer needs damage).
4. Reflow LAST. Interim: clear/redraw on resize (no rewrap); reflow tests sit on the
   divergence ledger until implemented.

- Every step gated by differential fuzz + esctest against the oracle. The
  **divergence ledger** (`docs/engine-divergence.md`) is the honest, shrinking record
  of "where we don't yet match, and why."
- **Exit criteria:** the battery passes except the documented ledger; ledger is empty
  or only *intentional* divergences remain.

## Phase 4 — Wire into rt behind a switch; test rt itself

- `rt-engine` exposes both impls behind its trait, selected by a build feature and/or
  `RT_ENGINE={vendored|own}` (default **vendored**).
- Test rt AT THE APP LEVEL on the own engine: real vim/tmux/emacs/htop sessions, rt's
  own test suite, visual/interaction passes on all three machines.
- Flip the default only when the ledger is acceptable and real-app use is clean. Keep
  `RT_ENGINE=vendored` as a permanent escape hatch.

## Phase 5 — Automation

- `cargo xtask verify` (and CI): unit + property + bounded differential-fuzz +
  esctest + corpus replay. **Fails on any NEW divergence** (ledger is the allowlist).
- Nightly long-soak fuzz with a seed corpus that grows from any crash/divergence found
  (coverage-guided if practical).
- Corpus + ledger tracked in-repo so the conformance state is always visible.

## Decisions to confirm

- **Engine selection:** build-feature, runtime env, or both? (Recommend both; default
  vendored.)
- **Parser + Term as two crates** (recommended — isolates the tractable parser tests
  from the hard Term tests) vs one.
- **foot integration** now or deferred (recommend deferred; it is a tiebreaker/design
  reference, not on the critical path).

## Risk register

- **Reflow** — the hard wall. Mitigation: isolate, ship interim no-rewrap, ledger.
- **Oracle inherits bugs** — matching alacritty adopts alacritty's quirks; esctest +
  foot tiebreak catch cases where alacritty itself is wrong.
- **Oracle goes silent where we want to differ** — any place we intend to be *better*
  than alacritty has no oracle; those cases fall back to real-app judgement and must
  be flagged as intentional divergences, not bugs.
- **Scope creep** — Phase 0 and Phase 2 each stand alone; ship and benefit even if the
  program pauses.

## The through-line

The vendored engine is default, oracle, and fallback at every step. rt is never at
risk; we always have a reference; and "done" is defined as *demonstrable non-divergence
from the implementations the whole ecosystem trusts* — not the unreachable "provably
correct".
