# Native damage tracking for vt-term (RT-PERF-002 follow-up)

Status: **implemented** 2026-07-22 (design approved same day). Goal: eliminate the per-frame
O(rows×cols) work in the in-house `VtPane` render path by tracking damage where mutations
happen, instead of diffing whole grids.

## Problem

`VtPane::render_snapshot` (in-house engine) currently does **two** O(rows×cols) passes every
frame: `capture()` rebuilds the whole resolved-colour `SnapCell` grid, then it is diffed
against `last_render` to derive per-line damage. The vendored `AlacPane` avoids this — it uses
alacritty's *native* damage (`term.damage()` → per-line `left/right` bounds). vt-term has no
such tracking, so it reconstructs damage by brute force. Material for 4K grids / many panes /
fast output (measured ~130 µs/pane/frame of grid work at 4K).

## Design

Full-incremental: vt-term authors damage at mutation time; `VtPane` keeps a persistent
resolved grid and refreshes only damaged cells. The old full-diff is retained as a **test
oracle**, not on the hot path.

### vt-term
- Field `damage`: `{ full: bool, dirty: Vec<Option<(usize, usize)>> }`, one entry per visible
  row (`Some(left,right)` = dirty column span), `full` for whole-screen invalidations.
- Mark helpers `damage_cell(r,c)` / `damage_span(r,l,r2)` / `damage_full()`, called from every
  mutating site: `write_cell`/`write_spacer` and the neighbour cells `clear_wide_left` edits;
  the `print_str` fast-path span; `erase_in_line`/`erase_in_display`; `erase_chars`,
  `insert_chars`, `delete_chars`; `insert_lines`/`delete_lines`. Structural changes call
  `damage_full()`: `scroll_up`/`scroll_down`, `line_feed` that scrolls, `resize`, alt-screen
  switch, RIS/`reset`, and any `display_offset` change (scrollback view).
- `pub fn take_damage(&mut self) -> Damage` drains and resets, returning a vt-term-local
  `Damage { Full | Spans(Vec<(line, left, right)>) }`. `VtPane` maps it to the `rt_engine`
  `Damage`/`CellDamage` shape.

### Cursor damage (a correctness gain)
The current diff silently misses pure cursor moves (no cell change), masked in practice by the
post-keystroke blink window. vt-term remembers the last-drained cursor cell; `take_damage`
adds the old **and** new cursor cells when the cursor moved. One place, all motion paths.

### VtPane
Replace `last_render` with `resolved: Arc<Vec<Vec<SnapCell>>>`. Per frame: drain vt-term
damage; on `Full` or a dimension change rebuild the whole grid (today's `capture`); otherwise
`Arc::make_mut` and re-resolve only the damaged spans. Map the drained damage to the returned
`Damage`, share the `Arc`. `make_mut` is in-place because the previous frame's snapshot is
dropped before the next `render_snapshot` (verified in `rt`'s draw loop); it degrades to a
clone only if a snapshot is ever retained across frames.

## Verification

The safety net is a new conformance test asserting **native damage ⊇ actual cell changes**:
snapshot → reset damage → feed a chunk → snapshot → drain damage; every cell that changed must
lie in a reported span. A missed mutation site fails it. Run over the fuzz corpus and with
chunk splits. Existing `rt-engine` `damage_tests` (Full first frame, convergence to Lines,
reset each frame) stay green.

## Non-goals (follow-ups)
- Renderer-side scroll blitting (so a scroll isn't a ~full redraw) — touches GL/XRender.
- Damage for the vendored `AlacPane` is unchanged (already native).
