# Wishlist plan — what to pick up, what not

Triage of `docs/wishlist`, read against rt's development history — especially the
long road to X11 support and what `ssh -X` taught us
(`docs/remote-rendering-lessons.md`). Each item is judged by one test: **does it
ship language, not pixels, and repaint only what changed?** That is the invariant
the whole X11 arc was fought to protect.

## The finding that reframes the list

rt runs **two chrome toolkits in parallel**, chosen per frame by
`Backend::supports_egui()`:

| Chrome surface        | Wayland / local-GL      | `ssh -X` / XRender        |
|-----------------------|-------------------------|---------------------------|
| Preferences           | native (`chrome::prefs`)| native — **unified** ✅    |
| Instruments, search   | native                  | native — **unified** ✅    |
| Context menu          | **egui** (`menu.rs`)    | native (`chrome::menu`)   |
| Manual                | **egui** (`manual.rs`)  | native (`chrome::manual`) |

egui was adopted for chrome (ADR-0004), then found too heavy over `ssh -X` — it
re-renders full frames and ships pixels, exactly the cost
`remote-rendering-lessons.md` §0–1 dissects. It has been retired surface by
surface ever since; the convergence is **~80 % done**. Prefs already renders as
commands on *both* backends. Only the **context menu** and the **manual** still
take the egui path on GL — and a native equivalent of each already ships on the
XRender path.

So wishlist #1's musing — "maybe two different toolkits needed for Wayland and
X11" — points the wrong way. The uniform-look problem *is* the two-toolkit split.
The fix is not a second toolkit; it is **finishing the convergence and deleting
egui**.

## Guardrails (non-negotiable, from the lessons doc)

Every item below must hold these, or it does not ship:

1. **`PutImage == 0`** stays true — chrome draws as XRender commands, never blits
   (§0). There is a test behind this invariant.
2. **Repaint only what changed.** No full-window composite per keystroke; a
   chrome element that repaints must scissor to its own rect and use a partial
   present (§1, §3).
3. **A visual change with no cell-damage needs one explicit full frame**, keyed
   on the central `force_full` rule — never a per-site flag (§4).
4. **Measure the server, not just rt.** Any item that makes the X server draw
   gets a gate that samples the server's CPU across a keystroke burst (§1).
5. **No continuously-animating chrome over the wire.** Interaction-driven
   repaint only.

## Items — pick up

### Slice A — polish batch (small, independent, ship anytime)

These four are self-contained, low-risk, and do not depend on the toolkit
decision. Each is a candidate for its own short spec, or bundled.

- **#3 rt version in the menu.** `env!("CARGO_PKG_VERSION")` in a disabled row
  (or a small "About"). Pure text — cheap on both backends. Add to the native
  menu; if the egui menu still exists when this lands, add to both, else it comes
  free once Slice B removes the egui menu.

- **#4 column separators more visible.** A colour tweak on the separator
  `fill_rect`. The wishlist suggests the RGB mean of fg and bg; note that on a
  light-on-dark scheme a pure mean lands on muddy mid-grey. Blend biased ~60 %
  toward fg and eyeball it against several schemes. One-line change, one visual
  check.

- **#5 jack ports — draw order and size.** The "little vertical lines" are the
  pane edge / divider painted *over* the jack, because jacks are not drawn last
  (`remote-rendering-lessons.md` §4 is the same draw-order family). Fix: draw
  jacks **after** edges/dividers, and bump the radius a notch. Bounded; verify
  the jack sits proud of the edge and no seam crosses it.

- **#6 scrollbar search hit-markers.** Chrome/Firefox-style ticks on the
  scrollbar track marking where scrollback hits fall, clustering when too dense
  to separate. The search already computes hit lines for highlighting; map each
  to a normalised track position and draw a short `fill_rect`. Painted **only
  while search is active**, and only when the hit set changes — no per-frame
  cost. Bounded feature, high user value.

### Slice B — finish the toolkit convergence, delete egui (the backbone)

Resolves #1 and clears the ground for #2. Mostly plumbing, not a rewrite:

1. Route the GL path's context menu and manual at the existing native
   `chrome::menu` / `chrome::manual` units that already run on XRender. They need
   only `fill_rect` + text (both already implemented on `GlBackend`), not the AA
   circle primitives.
2. Delete egui: drop `egui`, `egui-winit`, `egui_glow` from `crates/rt`, delete
   `menu.rs` and `manual.rs` (the egui versions), and drop the egui value types
   used as convenient geometry (`Color32`, `Pos2`, `Rect`) in favour of rt's own
   `Color` / `Recti`.
3. `supports_egui()` collapses to a single native chrome path; keep the method
   only if another backend distinction still needs it, else remove it.

Outcome: one chrome look everywhere, a lighter binary, and a single place to add
any future widget. Verify the existing chrome tests (menu render, prefs settle)
still pass on both backends, and that `PutImage == 0` holds.

### Slice C — native colour picker (built once, on the converged chrome)

Depends on Slice B so it is built exactly once. Decision taken: **native on both
backends, not greyed out on X11.** The heaviness that scared the wishlist was
egui's per-frame repaint, not the picker concept.

- Draw a hue strip + saturation/value square as a few gradient-filled rects plus
  a position cursor, as XRender commands.
- Repaint **only while dragging** (pointer-driven), scissored to the picker's
  rect with a partial present — cheap even over `ssh -X`.
- Wire the chosen colour into the same config path the prefs rows already use
  (fg/bg/cursor + the 16-colour palette), so there is no parallel state.
- Gate it: a server-CPU sample across a drag burst must stay flat (guardrail 4),
  and `PutImage == 0` must hold through a full pick.

## Items — do NOT pick up

- **A second, heavier toolkit for X11**, or deepening egui. The history is
  unambiguous: that is the cost the whole arc removed.
- **Greying the colour picker out on X11** (the wishlist's fallback idea) — a
  command-drawn picker that repaints only on drag is cheap; a capability gap is
  not warranted.
- **Any chrome that animates continuously over the wire.** Interaction-driven
  repaint only.

## Sequencing

1. **Slice A** first — four quick, shippable wins, each independent, none blocked
   on the toolkit decision.
2. **Slice B** — finish convergence, delete egui. Unblocks a uniform look and a
   single widget home.
3. **Slice C** — colour picker, built once on the converged chrome.

Each slice gets its own design spec + implementation plan when it is picked up;
this document is the triage and the order, not the per-slice design.
