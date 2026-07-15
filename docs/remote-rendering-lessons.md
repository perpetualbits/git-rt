# What making rt fast over `ssh -X` taught us

Notes from making rt usable on a StarFive JH7110 (riscv64, 4x U74) over `ssh -X`.
Almost none of it is new. X11 programmers knew all of it; the point of writing it
down is that a GPU-era codebase rediscovers it the hard way, one regression at a
time, and the same mistakes keep arriving wearing different clothes.

Every number here was measured on real hardware, not reasoned about.

## 0. The premise: ship language, not pixels

rt's original remote cost was 2.51 MB and 38 `PutImage` blits to draw "hello".
Terminator drew the same thing in 48 KB of drawing *commands*. That gap is the
whole thesis: X11 is a protocol for describing drawing, and using it as a pixel
pipe throws away the one thing it was designed to do. `PutImage == 0` is now a
falsifiable invariant with a test behind it (`xrender_commands.rs`).

The corollary bites harder than the premise: once you are sending commands, the
cost moves to the *other end of the wire*, where none of your instruments point.

## 1. Server-side work is invisible to client-side profiling

The instrument layer was composited full-window on every frame — every keystroke.
It ships no pixels and costs rt nothing, so rt's CPU, the xtrace byte counts, and
`PutImage == 0` all looked perfect. The X server was doing a full-window alpha
blend per keystroke:

| ~30 keystrokes, 1180x780 | X server CPU |
|---|---|
| instruments on | 110 ms |
| instruments off | 50 ms |

Scaling with **window area**. The design doc had justified this:

> "A full-window composite costs the same as a partial one over the wire (it is
> server-side), so `present()` stays always-full-window."

True over the wire. Generalised, wrongly, to "costs the same". Server CPU is
proportional to composited area, and `present()` runs every frame.

**Rule: if your backend's job is to make the server draw, at least one gate must
measure the SERVER's CPU.** Sampling `/proc/<Xorg>/stat` across a scripted
keystroke burst is enough. Every gate in that design watched rt or the wire, and
the cost was neither, so it shipped.

Fix: clip the composite to the pixels instruments actually occupy (thin border
bands). Overhead above baseline: +120% -> 0-30%.

## 2. Cost scales with what you didn't think to vary

The window froze for 5-10 s on an interactive resize, then repainted at once.
Six reproduction attempts failed. Each failure killed a plausible theory — link
saturation, SIGWINCH storms, compositor blur, window opacity,
`_NET_WM_SYNC_REQUEST` — and each harness was wrong in the same way: **near-empty
panes**.

`relayout` -> `Term::resize` per pane reflows the grid AND the entire scrollback
(10k lines default), synchronously on the event loop:

| panes | relayout per resize event |
|---|---|
| `sleep` shell, empty scrollback | 0.0-0.2 ms |
| flooding `ls -alR /`, full 10k scrollback | 400-1100 ms (676 ms median) |

One 20-step drag = 16 reflowing events x 676 ms = **10.6 s of blocked event
loop**. Not slow — *blocked*: no repaint, no input, until the backlog drains.

Reflow cost scales with **scrollback depth**. Not window size, not client speed:

| | rt CPU (3 flooding panes) | freeze |
|---|---|---|
| milkv (riscv64 U74) | 57% | none reproduced |
| apollo (Ryzen 7 7730U) | 58% | none reproduced |

A ~20x faster client, identical. **That rules the machine out rather than the
code** — which is the useful thing a negative result buys you.

## 3. Discard work nobody will see

A drag delivers ~20 configure events; 19 of those sizes are superseded within
50 ms and never observed by a human. Paying a full reflow for each is pure waste.
Do the cheap part now (surface, viewport: ~0.1 ms), defer the expensive part
until the size holds still (`RESIZE_SETTLE` = 120 ms).

The same trap has more than one door. Debouncing `WindowEvent::Resized` left the
divider drag reaching `relayout` through `set_split_ratio` — one reflow per
pointer sample, ~1 s steps. When something is too expensive to do per event,
**find every caller**, not just the one in the bug report.

Terminator only redraws at the resting size. It has always done this.

## 4. A visual change with no cell-damage needs an explicit full frame

The most-repeated bug of the whole arc, in four costumes:

- the focus border moved -> no cell damage -> stale/half-drawn border
- the bell stripe appeared -> no cell damage -> only one stripe drawn
- a tab switched / a pane split / panes rotated -> the persistent instrument
  layer still showed the PREVIOUS layout, composited over current content
- a pane's new jacks never appeared at all

Damage-based rendering only knows what the *engine* knows. Anything rt draws
itself — chrome, borders, overlays, instruments — is invisible to it.

Both times the durable fix was **one central rule**, not a flag at each site:

```rust
// focus/bell: engine has no damage for these
active.session.focus() != active.last_focus  -> mark_full()
// layout moved: the instrument layer is stale
let chrome_moved = active.force_full || focus_changed;
if chrome_moved { active.instr_layer_drawn = false; }
```

Per-site flags are where this bug goes to hide: someone adds a fifth costume.

**And watch what you key the rule on.** `chrome_moved` is keyed on `force_full`,
NOT on "damage is full" — the engine reports Full damage on any clear/scroll,
which under an output flood is most frames, and that would silently put
instrument geometry back onto content frames: the exact coupling the decoupling
guard exists to catch.

## 5. Tests that measure nothing pass loudly

The XRender gates reported green while measuring **zeros**. Every assertion here
is satisfied by a run that never rendered:

```
assert_eq!(put_image, 0);              // 0 == 0
assert!(flood_triangles <= 3*silent + 50);  // 0 <= 50
```

Three independent causes, all in the harness:

- **Liveness**: waited for the socket FILE to appear. A killed Xvfb leaves its
  socket behind — existence is not liveness. rt launched at a dead display, its
  connect refused, and the run measured nothing. `connect()` is the real check.
- **Name collisions**: fixed display guesses (`71 + pid%20`) landed on the
  socket/lock litter that killed runs leave behind (~200 stale entries locally).
- **Fixed sleeps**: `timeout 3` and a 2200 ms screenshot delay were calibrated to
  a ~1.5 s idle cold start. A debug build (what `cargo test` runs) under load
  takes ~3.5 s, so rt was killed *before drawing*.

None of these tests would have caught any regression in this document.

**Rules:** upper-bound assertions need a companion that fails on an empty run
(`assert!(rendered)`); wait for the *condition*, never a duration; and measure a
window that STARTS at first paint rather than bounding the whole process — a 4 s
bound around a 3.5 s cold start leaves 0.5 s of measurement, or none.

Also: a bound that divides by a noisy baseline is not a bound. The decoupling
guard compared against `silent_triangles`, which is bimodal (~280-336 when the
heat path bootstraps, exactly 56 when it doesn't), so it swung between 218 and
1058 and failed on a healthy tree. The invariant — "instrument geometry is paced
by the tick clock, not by content volume" — is absolute, so the bound should be
too.

## 6. Trust rustc, not the analyser; measure the machine, not the story

- rust-analyzer reported phantom errors (missing fields, undeclared types) four
  separate times this session on code `cargo build` compiled cleanly. Verify with
  the compiler.
- A stale binary produced a completely coherent, completely wrong measurement.
  `rt --version` prints the commit hash — check it before believing a number.
  (It also used to abort without a display, which hid on any `ssh -X` box.)
- `ssh` `ControlMaster auto` silently reuses an existing master, inheriting *its*
  X forwarding. An automated test nearly opened windows on the user's live
  desktop. Use `ssh -S none`.
- Under Xvfb, `env -u WAYLAND_DISPLAY` or winit opens on the host's real
  compositor instead.
- `xdotool windowsize` is not a drag: no WM, no compositor, no interactive resize
  path. A headless harness structurally cannot test what a real drag does. When
  the harness cannot reach the bug, **instrument the code and let the affected
  session report** — that is what finally found #2, in one run, after six failures.

## 7. The hardware was never the problem

The milkv is ~20x the CPU and 16x the RAM of the 1999 dual-CPU 512 MB Slackware
box that ran a physics department's mail, DNS and NFS home directories *while*
being the professor's fvwm X11 desktop.

fvwm was fast on that machine for the same reason Terminator is fast over
`ssh -X` today: it only ever repainted what changed. "Redraw everything every
frame" is a GPU-era habit, and on a remote or software path it is the whole bill.
rt was reflowing 30,000 lines of history to draw an intermediate frame that
existed for 50 ms and nobody ever saw.

"Slow hardware" is a comfortable place to hide a design flaw. Every measurement
in this document says the board was fine.

## Open

- **A single reflow is still ~676 ms** on the milkv (3 panes x 10k lines). We
  stopped rt doing it 16 times; we did not make it cheap. That means not
  reflowing off-screen scrollback, down in `Term::resize`.
- **Panes overlap while shrinking**: during a drag panes draw their old, larger
  grid into a smaller rect, and nothing clips a pane's draw to its own rect.
- **An unexplained frame artifact** ("orange rectangle"): measured at
  `(0, 0, W-16, H-16)` while `content_bounds` is `(8, 8, W-16, H-16)` — the right
  size at the wrong origin. A controlled capture shows the latency frame
  correctly at (8,8), so this is likely a ghost of an earlier draw, but it is not
  reproduced and not explained.
- **A single unreproduced input freeze**: after making tabs, typing was accepted
  nowhere — not even in other windows — and the queued text appeared in the
  launching terminal only once rt was closed. Suspected: rt saturating Xwayland
  with command volume under a 3-pane flood, stalling every X client on that
  server. Unconfirmed.
