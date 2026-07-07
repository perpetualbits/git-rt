# rt-mux — a text-mode terminal multiplexer (mullion + rt-engine)

`rt-mux` is rt's **text-mode sibling**: a tmux-style multiplexer that runs *inside
any terminal* and draws with characters, while reusing the exact same terminal
engine as the Wayland/GL `rt` binary. It is the working prototype that validates
the "mullion hosts terminals" idea — at the correct scope (text cells, not GPU
surfaces; see `docs/mullion-terminal-host-prompt.md` for the corrected brief).

## The idea in one line

**Cells into cells.** [mullion](../../mullion) is a ratatui-shaped TUI tiling
engine; [`rt-engine`](../crates/rt-engine) is a PTY + `alacritty_terminal` grid
exposing a per-cell `Snapshot`. rt-mux hosts one engine pane per mullion tile and
blits the pane's snapshot (`char + fg/bg + attrs`) straight into mullion's
`Buffer` each frame. mullion then diffs and flushes only what changed.

## What each side owns

| Concern | Owner |
|---|---|
| PTY, shell, ANSI/VTE parsing, scrollback grid | `rt-engine` (→ `alacritty_terminal`) |
| Tiling tree, split/focus/zoom, borders + junctions, double-buffer diff, input thread | `mullion` |
| The seam: snapshot → `Buffer` cells, key encoding, split/close tree edits | `rt-mux` (this crate, ~600 lines) |

The engine is **the same one** `rt` uses, so a text-mode mux and a Wayland GPU
terminal share one battle-tested core with zero duplicated terminal logic.

## The seam (the whole adapter)

- **Paint:** `render_shared(buf, tree.effective_root_mut(), area, &style, &focus_override(...))`
  draws shared single-cell seams (with correct `┬ ┤ ┼` junctions and a thickened
  focused border) and returns each tile's content `Rect`. For each rect:
  `pane.resize(w, h); for cell in snapshot { buf.set_char(x, y, cell.c, style_of(cell)) }`.
- **Colour/attrs:** `SnapCell.fg/bg: Rgb → Color::Rgb`, `CellAttrs → Modifier`
  (bold/italic/underline; strikeout has no mullion Modifier and is dropped).
- **Cursor:** reverse-video the focused pane's cursor cell.
- **Input:** a threaded `EventReader`; non-prefix keys are encoded to PTY bytes
  (`encode_key`, CSI/SS3 per DECCKM) and written to the focused pane.
- **Structure:** splits/closes edit mullion's `Node` tree directly (`split_tile` /
  `remove_tile` with single-child collapse), mirroring `rt-core`'s layout ops.

## Keys (tmux-style, prefix = `Ctrl-a`)

| Key | Action |
|---|---|
| `Ctrl-a %` / `Ctrl-a v` | split side-by-side |
| `Ctrl-a "` / `Ctrl-a s` | split stacked |
| `Ctrl-a o` / `Ctrl-a Tab` | focus next |
| `Ctrl-a ←↑↓→` / `h j k l` | directional focus |
| `Ctrl-a z` | zoom (maximise) / restore |
| `Ctrl-a r` | rotate (flip the parent split H↔V) |
| `Ctrl-a x` | close the focused pane |
| `Ctrl-a q` | quit |
| `Ctrl-a Ctrl-a` | send a literal `Ctrl-a` to the shell |

## Run

```sh
cargo run -p rt-mux
```

## Border instruments — the borders *mean* something

Every pane border is a live readout, not decoration (the aerie `rim-latency`
philosophy: motion is measurement). Three instruments share the geometry because
they measure different physics:

- **Output (green flow).** Packets march around each pane's ring; brightness and
  speed ∝ that pane's output rate (`PaneEvent::Wakeup`). Idle = frozen dim dots,
  busy = bright marching green. You see which shells are working across the screen.
- **Heat (planck/blackbody tint).** The border's *base* colour is the pane's CPU
  temperature — dim deep-red idle → orange → yellow → white-hot → blue-white —
  summed over the pane's session (shell + children) via a ~2 Hz `/proc` scan. The
  green flow rides over this base, so one ring shows compute *and* output at once.
- **Latency (violet window frame).** The outer frame calmly undulates
  purple-blue-violet and flares when the render loop misses its deadline (a CPU
  hogger stealing the frame). The mux's own pulse.

## Patch-bay — wire shells' fds together

The novel feature: terminals connect their fds to each other. Each pane exposes
side-channel pipe jacks separate from the tty — `$RT_OUT` (a program writes,
rt-mux reads) and `$RT_IN` (rt-mux writes, a program reads). `C-a w` arms a wire
from the focused pane's output jack; move focus; `C-a w` again connects
`src.out → dst.in`. rt-mux pumps the bytes and the **wire is drawn** — routed
between the floating panes with mullion's `socket`/`route`/`junction`, with a
green flow whose packets are the literal bytes on the pipe.

```sh
# in pane A:            in pane B (wired A.out → B.in):
producer > $RT_OUT      consumer < $RT_IN
```

## Status

Working proof-of-concept, all verified inside tmux via `capture-pane`:
single/multi-pane splits with light/heavy junctions, focus/zoom/close, live PTY
I/O; the three border instruments (output green, heat planck, latency violet);
and the patch-bay (data crosses A→B, drawn as a routed animated wire).

Not yet ported from the GPU `rt`: scrollback search overlay, newspaper columns,
mouse (selection/URL, and mouse-drag wiring), groups/broadcast. Wire polish:
stderr as a distinct red jack, fan-out/merge, true cross-pane pipelines.
