# vt-parser design

> Status: **implemented** (Phase 2). Verified byte-identical to vendored `vte` across
> 8000 structured-fuzz cases (whole + chunked) plus the replay corpus
> (`vt-conformance`'s `parser.rs`). Code: `crates/vt-parser/src/lib.rs`. This document
> and the code stay in lockstep.

`vt-parser` turns a raw byte stream into a sequence of `Perform` actions and assigns
them no meaning — that is the Term's job. Its one hard requirement: **produce the same
action stream as `vte` for every input**, so it is a true drop-in for the parse layer.

## 1. Scope & the action vocabulary

The parser is the byte→action half; `vt-term` is the action→grid half. The action
vocabulary is the `Perform` trait, mirroring `vte`:

| action | meaning |
|---|---|
| `print(char)` | one printable character (post-UTF-8-decode) |
| `print_str(&str)` | a **run** of consecutive printables — the batched fast path |
| `execute(u8)` | a C0/C1 control (`\n`, `\r`, `\t`, BEL, …) |
| `csi_dispatch(params, intermediates, ignore, final)` | a CSI sequence `ESC [ … X` |
| `esc_dispatch(intermediates, ignore, byte)` | a plain ESC sequence `ESC … X` |
| `osc_dispatch(&[&[u8]], bell_terminated)` | an OSC string `ESC ] … (BEL|ST)` |
| `hook` / `put` / `unhook` | a DCS string `ESC P … ST` (begin / data byte / end) |

Every method has an empty default, so a consumer overrides only what it uses.

## 2. The state machine

A faithful implementation of [Paul Williams' ANSI parser](https://vt100.net/emu/dec_ansi_parser).
States: `Ground`, `Escape`, `EscapeIntermediate`, `CsiEntry`, `CsiParam`,
`CsiIntermediate`, `CsiIgnore`, `DcsEntry`, `DcsParam`, `DcsIntermediate`,
`DcsPassthrough`, `DcsIgnore`, `OscString`, `SosPmApcString`.

Each non-ground state is one `advance_*` method mapping the current byte to an action
and/or a state transition, exactly per the spec's transition table. The C0/C1
"anywhere" rules (`0x18`/`0x1A` → cancel to Ground; `0x1B` → restart into Escape) are
shared via `anywhere()`. Byte-range arms match the spec (e.g. CSI final bytes are
`0x40..=0x7E`; intermediates `0x20..=0x2F`; params `0x30..=0x39`; `:` `0x3A`; `;`
`0x3B`; private markers `0x3C..=0x3F`).

## 3. The ground fast path (the speed)

`Ground` is special: it can be left ONLY by `ESC` (`0x1B`). So `advance_ground` uses a
`memchr` SIMD scan to find the next `ESC` and hands the entire printable run in front of
it to `ground_dispatch` in one shot — long runs of text never touch the per-byte state
machine. This is the single most important parser optimisation and the reason a naive
hand-written parser would be slower; keeping it is non-negotiable (see the performance
mandate in `own-engine-plan.md`).

`ground_dispatch` then splits the UTF-8 run at any embedded control character
(`\x00..=\x1f` and C1 `\u{80}..=\u{9f}`), `execute`-ing each and batching the printable
stretches via `flush_run`: a stretch ≥ `BATCH_MIN` (**4** bytes, matched to vte) goes
out as one `print_str`; shorter stretches stay one `print` per char (so control-heavy
output pays no batching overhead). This preserves the exact `print`/`execute` ordering.

## 4. UTF-8

The ground run is validated with `str::from_utf8`. On an error we dispatch the valid
prefix, then:
- **invalid sequence** (`error_len = Some`): a lone byte ≤ `0x9F` is a C1 control →
  `execute`; anything else → `print('\u{FFFD}')`.
- **truncated by an ESC** mid-codepoint → `print('\u{FFFD}')` and take the ESC.
- **truncated by the buffer end** → stash the partial bytes in `partial_utf8`; the next
  `advance` call resumes via `advance_partial_utf8`, so a codepoint split across reads
  (or across a 1-byte chunk boundary) decodes correctly. This is exactly what the
  chunked differential test hammers.

## 5. Parameters & sub-parameters

CSI/DCS parameters are `;`-separated, each optionally carrying `:`-separated
sub-parameters (e.g. `38:2:255:0:0`). `Params` stores one `Vec<u16>` per parameter and
exposes `iter()` yielding `&[u16]` — matching vte's iteration so a `csi_dispatch`
handler sees the identical nested structure. `;` finalises a parameter (`push`), `:`
extends the current one (`extend`), digits accumulate with saturating arithmetic, and a
33rd value sets the `ignore` flag (cap = `MAX_PARAMS = 32`), all matching vte.

## 6. OSC & DCS

OSC bytes accumulate in a raw buffer; `;` records parameter boundaries (up to 16); BEL
(`0x07`) **or** ST (`ESC \`) terminates and dispatches the `;`-split slices, with a flag
recording which terminator was used. DCS `hook`s on its final byte, streams data bytes
via `put`, and `unhook`s on cancel/ST.

## 6a. Synchronized updates (DECSET 2026)

Modern TUIs wrap each frame in `\x1b[?2026h` … `\x1b[?2026l` (BSU/ESU) so the terminal
applies it atomically, without tearing. The vendored `vte` buffers every byte in between
and replays it at the ESU (or a 2 MiB cap); `vt-term` must match, or a stream captured
mid-frame diverges — which is exactly how `spiral_stress.bytes` ends, and how this gap was
found (the fuzz never emits 2026).

To keep the hot path untouched, this lives in a thin layer *above* the state machine:

- [`advance`](Parser::advance) is the **raw, sync-unaware** machine — the path the bench
  measures and the parser-vs-`vte` differential tests (`vte`'s low-level parser has no
  sync either, so they must agree byte-for-byte).
- [`feed`](Parser::feed) is what a terminal drives: it runs `advance_until_sync` (the same
  machine, stopping the instant a live BSU is dispatched), then buffers until it finds the
  ESU — scanning the buffer for the fixed escapes with a 7-byte overlap so a split escape
  is still caught — and replays the buffer through the machine (`flushing` guards against a
  buffered BSU re-entering sync). A later BSU *extends* the update: everything before it is
  applied, its tail stays buffered. A stream ending mid-update leaves the tail unapplied,
  matching the oracle. This mirrors `vte`'s `Processor`/`advance_sync` structure exactly.

## 7. Verification & performance

- **Correctness:** `vt-conformance/tests/parser.rs` records both engines' action
  streams into one neutral `Action` enum and asserts equality across a rich fuzzer
  (multibyte UTF-8, C1 bytes, random bytes, CSI with subparams/intermediates/private
  markers, ESC, OSC BEL/ST, DCS) — whole-buffer AND arbitrarily chunked — plus the
  replay corpus. 8000+ cases, green.
- **Performance:** benchmarked against vte on x86_64 AND riscv64 (milkv) via
  `examples/parser_bench.rs`, driven from `ci/verify.sh` (one command, both arches).
  After tuning, `vt-parser` **beats vte** — geomean own/vte **1.17× (x86_64)** and
  **1.09× (riscv64)**, at parity-or-better on every workload on the stable milkv board
  (plain 1.44×, unicode 1.05×, sgr 0.99×, control 1.01×, mixed 1.00×).

  Three tunings got it there, each measured on both arches:
  1. **Allocation-free `Params`** (flat `[u16;32]` + `[u8;32]` subparam counts instead
     of `Vec<Vec<u16>>`): the big one — CSI-heavy workloads went from ~0.53× to ~0.99×
     on milkv, where the per-sequence heap traffic hurt most.
  2. **Byte-scan `ground_dispatch`**: printable-ASCII bytes advance with a plain
     compare instead of constructing a `char`, only decoding on `<0x20`/`>=0x80` —
     ~doubled plain-text throughput (1.44× vs vte on milkv).
  3. **Stack-array OSC dispatch** + `#[inline]` on the byte-by-byte state functions.

  All three preserved the byte-identical action stream (the differential test is the
  guardrail: optimise internals freely, the 8000-case diff proves behaviour unchanged).
