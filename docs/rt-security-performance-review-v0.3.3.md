# rt v0.3.3 — security, correctness, and performance review

> **Maintainer response (addressed in v0.3.5).** Every load-bearing finding was
> re-verified against the source and confirmed accurate — no false positives. Status:
>
> | Finding | Status in v0.3.5 |
> |---|---|
> | RT-SEC-001 unbounded OSC | **Fixed** — 64 KiB cap in vt-parser *and* the vendored vte (std mode was also unbounded); overflow dropped until terminator. Test + differential still 0/10000. |
> | RT-SEC-002 patch-bay drain | **Fixed** — 512 KiB per-tick budget in `pump_wires` (rt) and `pump` (rt-mux); yields and re-pumps next frame. |
> | RT-SEC-003 unbounded input queue | **Fixed** — writer channel capped at 8 MiB (`WRITE_QUEUE_MAX`), drop-past-cap (keystrokes never approach it). |
> | RT-IO-001 partial FIFO write loss | **Fixed** — `write` (not `write_all`) + count only bytes delivered, in rt and rt-mux. Full per-wire pending-buffer (lossless) left as a follow-up. |
> | RT-CONF-001 unvalidated settings | **Fixed** — `Settings::normalize()` clamps opacity/font/scrollback + rejects non-finite after load; `save()` is now atomic (temp+rename). Test added. |
> | RT-TERM-001 RIS resets policy | **Fixed** — scrollback line/byte caps preserved across RIS. Test added. |
> | RT-TERM-002 ED 3 stale accounting | **Fixed** — `history_bytes`/`display_offset` reset with the history. Test added. |
> | RT-PERF-001 reflow amplification | **Open** — real; needs a move-not-clone reflow refactor + peak-RSS benchmarking. |
> | RT-PERF-002 snapshot copies | **Open** — documented tradeoff (vt-term has no native damage tracking yet). |
> | RT-PRIV-001 full URLs logged | **Fixed** — logs redacted scheme+host only. |
>
> Thanks to the reviewer — an unusually accurate static review.


**Repository:** https://github.com/perpetualbits/rt  
**Audited revision:** `a944484ffee87c50aba40064c7590a3f6166cbba` (`v0.3.3`, 2026-07-22)  
**Review date:** 2026-07-22  
**Method:** manual source review of all first-party crates, targeted review of unsafe and process/I/O boundaries, manifests and tests. Dynamic Rust tests, Clippy, Miri, sanitizers, and dependency-advisory scanning could not be run because this review environment did not contain a Rust toolchain. Findings below therefore distinguish confirmed source defects from hardening recommendations.

## Executive assessment

rt is unusually ambitious and unusually credible for a young terminal project. It is not merely a GUI around an existing widget: it contains a parser, terminal state machine, PTY integration, layout/session model, GL and XRender rendering paths, a text-mode multiplexer, and a differential-conformance framework. The architectural separation into small crates is good, the comments explain invariants rather than merely restating code, and the testing strategy—especially differential execution under different input chunkings—is excellent engineering.

The project is nevertheless **not ready to be described as security-hardened**. I found three high-priority availability/backpressure problems reachable through ordinary terminal or clipboard input, plus several medium-priority correctness and resource-accounting bugs. The most serious is an unbounded OSC buffer: a child process or remote stream can make rt allocate memory until exhaustion by beginning an OSC string and never terminating it.

No direct arbitrary-code-execution, shell-injection, cross-user FIFO hijack, or unsafe-memory exploit was established in this static review. The FIFO setup is, in fact, notably defensive: it uses private directories, refuses pre-existing endpoints, checks ownership and permissions, and fails closed. That is worth preserving.

### Priorities

| Priority | Finding | Impact |
|---|---|---|
| P0 | Unbounded OSC accumulation | Untrusted terminal output can exhaust process/system memory |
| P0 | Patch-bay drains each FIFO without a per-tick budget | A producer can indefinitely starve the GUI/event loop |
| P1 | Unbounded PTY input queues | Large/malicious clipboard input or a stalled child can exhaust memory |
| P1 | Nonblocking FIFO fan-out loses partial writes | Silent data corruption under normal backpressure |
| P1 | Settings loaded from TOML are not validated | Corrupt/extreme values can cause OOM, pathological allocation, or invalid rendering state |
| P2 | RIS reset discards configured scrollback limits and byte budget | Behavioral inconsistency; memory policy silently changes |
| P2 | ED 3 clears history without clearing byte accounting | New history can be evicted incorrectly after clearing scrollback |
| P2 | Reflow duplicates the complete scrollback several times | Large transient memory spikes and long stalls on resize |
| P3 | Full-grid snapshots allocate/copy repeatedly | Avoidable frame-time and memory-bandwidth cost |
| P3 | Full clicked URLs, including secrets, are logged | Privacy leak into logs |

## What deserves acclaim

1. **The differential test design is first-rate.** Comparing parser and terminal state cell-for-cell with a mature oracle, while varying chunk boundaries, specifically attacks the state-resumption bugs that ordinary golden tests miss. The separate parser, spec, replay, reflow, fuzz-style, and real-corpus suites show serious attention to correctness.

2. **The module boundaries are sensible.** `rt-core` is dependency-free model logic; `rt-session` owns coordinated mutation of layout and panes; engine selection is behind a seam; renderer and platform code live at the edge. This makes the important logic testable without a display or PTY.

3. **Security thinking is already visible.** The patch-bay code explicitly handles the predictable-PID/world-writable-directory threat, uses `lstat`-style metadata, checks effective ownership and permissions, refuses existing FIFOs, and disables the feature on uncertainty. The in-house engine ignores OSC clipboard operations and hyperlinks. Bracketed paste strips the normal end marker. These are good instincts.

4. **Remote performance is treated as a systems problem.** The XRender-specific damage path, remote animation throttling, and on-device RISC-V benchmarking show that the project measures the actual bottleneck rather than assuming workstation behavior generalizes.

5. **Failure containment is designed in.** Parser/render panic isolation per pane and graceful refusal of additional PTYs under resource exhaustion are materially better than letting one bad pane destroy all terminal sessions.

## Detailed findings

### RT-SEC-001 — Unbounded OSC strings permit memory-exhaustion DoS (High, confirmed)

**Evidence:** [`vt-parser/src/lib.rs` lines 187, 625–644](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/vt-parser/src/lib.rs#L187-L188) stores OSC bytes in `osc_raw: Vec<u8>` and pushes every accepted byte. The buffer is cleared only when an OSC terminator is processed at lines 732–743. Unlike synchronized updates, which have a 2 MiB cap, OSC has no cap.

**Exploit sketch:** a process prints `ESC ] 2 ;` followed by an endless byte stream without BEL or ST. An SSH session, `cat` of a hostile file, build tool, or local child can do this without special privileges. Memory grows with the stream until allocation failure or system pressure kills rt or other processes.

**Recommendation:** impose a small explicit cap (for example 64 KiB, with a much smaller title cap such as 4 KiB). Once exceeded, set an `osc_ignoring` flag and discard bytes until BEL/ST/CAN/SUB. Do not repeatedly clear and reallocate; preserve bounded capacity. Add tests for termination split across chunks, overflow, recovery after overflow, and continuous multi-megabyte input. Apply equivalent limits to every string-like control sequence, including future DCS/APC implementations.

### RT-SEC-002 — Patch-bay flood can monopolize the event thread (High, confirmed)

**Evidence:** [`rt/src/main.rs` lines 3979–4018](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt/src/main.rs#L3979-L4018) loops on a source FIFO until a nonblocking read returns `WouldBlock`. A producer that continuously keeps the pipe readable can prevent that loop from ending. `pump_wires` runs from the main application/event path. The same unbounded-drain design appears in `rt-mux`.

**Impact:** a pane that writes continuously to `$RT_OUT` or `$RT_ERR` can freeze input, rendering, close actions, and every other pane. This is especially easy because the advertised API invites programs to write those FIFOs.

**Recommendation:** use a per-frame global byte/time budget and a smaller per-source fairness quota (for example 256 KiB globally, 32–64 KiB per jack, or ≤1 ms). Round-robin the starting source between ticks. When budget remains, request another immediate poll/redraw rather than draining forever. Add a test with a producer that never stops and assert bounded event-loop latency.

### RT-SEC-003 — Unbounded input queues allow memory exhaustion (High/Medium, confirmed)

**Evidence:** [`rt-engine/src/vtpane.rs` lines 63–69, 151–178, 280–294](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt-engine/src/vtpane.rs#L63-L69) deliberately uses `std::sync::mpsc::channel`, an unbounded queue, and copies every input chunk. If a child stops reading, the writer blocks while the queue continues growing. Clipboard paste passes the entire clipboard text into this path, and broadcast duplicates it across panes. The vendored engine's channel should also be verified for boundedness.

**Threat model:** another desktop client can own a very large clipboard; the user needs only invoke paste. A stalled or malicious pane then turns that into persistent queued memory. Repeated paste or broadcast multiplies the cost.

**Recommendation:** replace the unbounded channel with a bounded byte queue, not merely a bounded message count. Chunk large pastes, apply backpressure, expose “paste queued/cancel” state, and cap a single clipboard import. Keystrokes can receive a reserved small priority queue so they are not dropped behind a paste. Avoid copying the same broadcast payload per pane by sharing immutable chunks (`Arc<[u8]>`).

### RT-IO-001 — Nonblocking patch-bay writes silently truncate streams (Medium, confirmed)

**Evidence:** [`rt/src/main.rs` lines 4007–4014](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt/src/main.rs#L4007-L4014) calls `write_all` on an `O_NONBLOCK` FIFO and ignores the result. `write_all` may write a prefix and then return `WouldBlock`; the unwritten suffix is lost, yet the meter accounts the full `n`. A destination that reads slowly makes this routine, not exceptional. `rt-mux` follows the same best-effort pattern.

**Recommendation:** maintain a bounded pending buffer per wire/destination and advance it with partial `write` results. Poll writable endpoints, define overflow behavior explicitly (backpressure source, disconnect, or drop with a visible counter), and count only bytes actually written. Because multiple wires may target one FIFO, serialize per destination to preserve ordering.

### RT-CONF-001 — Persisted settings bypass their documented bounds (Medium, confirmed)

**Evidence:** configuration is deserialized directly in [`rt-config/src/lib.rs` lines 324–361](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt-config/src/lib.rs#L324-L361), and consumed at [`rt/src/main.rs` lines 690–725](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt/src/main.rs#L690-L725). The UI constrains opacity, font size, and scrollback, but `Config::load` does not. Thus hand-edited/corrupt TOML can provide extreme `scrollback`, negative/NaN/huge floats, or enormous font sizes. The vendored backend does not have the in-house 1 GiB history budget.

**Recommendation:** add `Settings::validate_and_normalize()` and call it after deserialization and after all environment/CLI overrides. Reject non-finite floats. Clamp font size, opacity, and scrollback; consider a lower default byte budget (1 GiB *per pane* is itself high). Report corrected fields to stderr rather than silently accepting them. Save atomically using temp-file + `fsync`/rename and create the directory/file with private permissions independent of the caller's umask.

### RT-TERM-001 — RIS resets memory-policy configuration (Medium, confirmed)

**Evidence:** [`vt-term/src/lib.rs` lines 1142–1145](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/vt-term/src/lib.rs#L1142-L1145) implements RIS (`ESC c`) by replacing the complete terminal with `Term::new(cols, rows)`. This resets `scrollback_lines` to the library default and `scrollback_bytes` to `usize::MAX`, discarding the host's configured line and byte caps.

**Impact:** terminal output can silently alter resource policy. In current code the line cap falls to 10,000, limiting the worst case, but the byte cap is still lost and user behavior changes.

**Recommendation:** preserve host policy across reset: save `scrollback_lines` and `scrollback_bytes`, create/reset state, then restore them. Add a test that sends RIS after `set_scrollback` and verifies both limits remain unchanged.

### RT-TERM-002 — ED 3 leaves scrollback byte accounting stale (Medium, confirmed)

**Evidence:** [`vt-term/src/lib.rs` line 939](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/vt-term/src/lib.rs#L939) clears `history` for `CSI 3 J` but does not reset `history_bytes` or normalize `display_offset`.

**Impact:** subsequent history pushes are evaluated against bytes that no longer exist and can be evicted prematurely; a scrolled viewport can also retain an offset beyond the now-empty history until another operation clamps it.

**Recommendation:** centralize history clearing in a method that clears/deallocates history as intended, sets `history_bytes = 0`, resets/clamps `display_offset`, and handles the recycle pool consistently. Add invariant assertions in tests: `history_bytes == history.iter().map(byte_size).sum()` after every mutating escape and resize.

### RT-PERF-001 — Reflow has very high transient memory amplification (Medium)

**Evidence:** [`vt-term/src/lib.rs` lines 441–491](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/vt-term/src/lib.rs#L441-L491) clones every visible and historical row into `rows`, creates `nb`, clones it again into grid/history, then converts those vectors into `Line`s. With a history near the advertised 1 GiB per-pane budget, resize can require multiple additional gigabytes and hold the terminal mutex for the entire operation.

**Recommendation:** reflow in bounded chunks or move rows instead of cloning them. Enforce budgets during, not after, reconstruction. Consider a smaller practical history byte cap and a per-session aggregate cap. Benchmark peak RSS and resize latency with long, wide history—not only steady-state throughput.

### RT-PERF-002 — In-house rendering copies full grids more than necessary (Low/Medium)

**Evidence:** `VtPane::capture` builds a new `Vec<Vec<SnapCell>>`; `render_snapshot` then compares it with `last_render` and finally clones the new rows again into `last_render` ([`vtpane.rs` lines 330–380](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt-engine/src/vtpane.rs#L330-L380)). This makes precise damage detection O(rows × columns) and adds another full-grid copy whenever a pane is rendered.

**Recommendation:** add native damage tracking to `vt-term`, or swap the new grid into `last_render` and return/share an immutable snapshot without the second deep clone. Longer term, expose changed spans directly from parser/grid mutation. Measure the tradeoff: for small terminal grids this is acceptable; for 4K windows, many panes, or fast output it becomes material.

### RT-PRIV-001 — Complete URLs are written to logs (Low)

**Evidence:** [`rt/src/main.rs` lines 2905–2912](https://github.com/perpetualbits/rt/blob/a944484ffee87c50aba40064c7590a3f6166cbba/crates/rt/src/main.rs#L2905-L2912) logs the full URL after opening it. URLs commonly contain password-reset tokens, signed object URLs, query credentials, and private paths.

**Recommendation:** log only scheme + redacted host/path at debug level, or omit successful-open logging. Consider requiring confirmation for `file://` and non-HTTP schemes originating in untrusted terminal text. Passing the URL as a separate `Command` argument avoids shell injection; that part is correct.

## Additional hardening recommendations

1. **Turn parser limits into one documented policy.** Bound OSC bytes, title length, synchronized-update bytes, selection extraction, search query length, clipboard import, pending PTY input, patch-bay queues, history per pane, and history per session. A terminal is an untrusted-stream parser and every retained byte needs a ceiling.

2. **Add invariant/property tests independent of the oracle.** Differential agreement can reproduce oracle quirks and cannot establish resource bounds. Generate arbitrary escape streams while asserting maximum allocation proxies, valid cursor/grid bounds, history-accounting equality, and recovery after ignored oversized sequences.

3. **Run continuous security tooling:** `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test` under debug and release, `cargo audit` or `cargo deny`, `cargo machete`, fuzz targets under `cargo-fuzz`, and Miri for pure unsafe-adjacent units. Use AddressSanitizer/LeakSanitizer on Linux integration tests where supported.

4. **Fuzz both parsers and host integration.** Targets should cover arbitrary chunking, OSC/DCS cancellation, 8-bit C1 forms, UTF-8 splits, huge numeric params, resize interleaving, repeated alt-screen/reset operations, and synchronized-update overflow. Include a resource oracle that rejects superlinear time or unbounded retained capacity.

5. **Audit all unsafe blocks with explicit invariants.** Most reviewed unsafe use is conventional FFI/GL handling, but raw display/surface pointers and duplicated PTY file descriptors deserve focused Miri/sanitizer/manual review. Add `#![deny(unsafe_op_in_unsafe_fn)]` and consider `#![warn(clippy::undocumented_unsafe_blocks)]` workspace-wide.

6. **Cap panes and aggregate resources.** Each pane creates a shell/PTY, descriptors, terminal grid, channels, and threads. Graceful spawn failure is good, but a configurable pane cap and a session memory/queue budget produce a better failure mode than exhausting OS limits.

7. **Make backpressure visible.** Patch-bay semantics should say whether it is lossless. If loss is permitted, show dropped bytes on the wire; if lossless, queue within a bound and pause reads. Silent best-effort behavior is dangerous for a feature advertised as literal byte wiring.

8. **Use atomic config persistence.** A crash during `std::fs::write` can leave malformed TOML and reset all preferences on next launch. Write privately to a sibling temporary file, sync if desired, and rename.

## Suggested remediation order

1. Cap and ignore oversized OSC strings; add regression/fuzz tests.
2. Add fair byte/time budgets to both patch-bay pumps.
3. Implement partial-write queues and explicit overflow policy.
4. Bound clipboard/paste and PTY input queues.
5. Normalize all loaded settings and add aggregate resource limits.
6. Fix RIS policy preservation and ED 3 accounting; add invariants.
7. Rework reflow and snapshot ownership to eliminate bulk cloning.
8. Add automated advisory, fuzz, sanitizer, and Miri jobs.

## Bottom line

rt deserves real praise: the project combines originality with a quality of architectural explanation and differential testing that many mature terminal projects lack. Its best idea is not any single feature; it is the insistence that terminal behavior be measurable, comparable, and testable across architectures and rendering backends.

The fine-comb review also finds a consistent blind spot: **correctness is strongly tested at the cell-state level, while resource correctness and backpressure are not yet equally specified**. Closing that gap—by bounding every retained input path and making loss/backpressure explicit—would materially raise rt from an impressive fast-moving project to a defensible systems tool.
