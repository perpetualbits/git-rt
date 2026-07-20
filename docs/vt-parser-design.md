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

## 7. Verification & performance

- **Correctness:** `vt-conformance/tests/parser.rs` records both engines' action
  streams into one neutral `Action` enum and asserts equality across a rich fuzzer
  (multibyte UTF-8, C1 bytes, random bytes, CSI with subparams/intermediates/private
  markers, ESC, OSC BEL/ST, DCS) — whole-buffer AND arbitrarily chunked — plus the
  replay corpus. 8000+ cases, green.
- **Performance:** benchmarked against vte on x86_64 (dop651/apollo) AND riscv64
  (milkv) — the slow board magnifies regressions a fast host hides. Every speed trick
  is commented with its measurement. (Benchmarks land alongside Phase-2 tuning.)
