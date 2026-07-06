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

## Status

Working proof-of-concept. Verified (hosted inside tmux via `capture-pane`):
single pane with title + focus border + status bar; live PTY I/O; side-by-side
and stacked splits with correct light/heavy seam junctions; focus tracking and
cycling; zoom/restore; close with split-collapse; clean quit.

Not yet ported from the GPU `rt`: scrollback search overlay, newspaper columns,
mouse selection/URL open, groups/broadcast, the animated "bitstream" rim-glow
border (mullion's `render_rim` / `Field::perimeter` — the obvious next flourish).
