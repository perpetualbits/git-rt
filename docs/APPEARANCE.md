# Appearance: translucency and background blur

Terminator has a Profiles → Background page with "Transparent background" + a
transparency slider (and, on some setups, compositor blur). This documents where
rt stands on the same ground, and — importantly — the hard Wayland constraint
around *blur*.

## Translucency (done, native)

A Wayland client **can** make its own background translucent: render with an
alpha channel < 1 on an alpha-capable surface, and the compositor blends the
window over whatever is behind it. rt does this:

- The EGL surface is created with an alpha channel (`with_alpha_size(8)`).
- The renderer uses **premultiplied-alpha** compositing (`glBlendFunc(ONE,
  ONE_MINUS_SRC_ALPHA)`, fragment outputs `rgb·a, a`) — what compositors expect.
  For fully-opaque content this is identical to straight blending, so normal
  rendering is unchanged (verified).
- The background clear carries the opacity in its alpha; **glyphs and chrome
  stay fully opaque**, so text is always crisp regardless of background opacity.

Controls:
- `Ctrl+Alt+Up` / `Ctrl+Alt+Down` — nudge opacity ±5% (range `0.05..=1.0`).
- `RT_OPACITY=0.8` env var — seed the opacity at startup (demos/screenshots).
- Future: a preferences panel with a real slider persists this to a config file.

## Background blur — the Wayland reality

**A Wayland client cannot blur the content behind its own window.** Wayland's
security model forbids a client from reading or processing the pixels of other
windows / the framebuffer behind it. So true "blur what's underneath" is
**exclusively the compositor's job**. Concretely:

| Compositor | Blur-behind | Client control |
|---|---|---|
| KDE KWin | yes (dual-Kawase, cheap) | `org_kde_kwin_blur` protocol: client can request blur **on/off + region**. **Strength is a KWin global**, not client-settable. |
| Hyprland | yes | Configured by compositor window rules; **no client protocol**. User enables it in `hyprland.conf`. |
| wlroots/sway | no built-in blur | — |
| Mutter/GNOME | no window blur | — |

Consequences for rt:
1. rt can *request* blur where a protocol exists (KDE), as an on/off toggle.
2. A **client-controlled blur-strength slider is not achievable** on Wayland —
   there is no standard protocol for it, and KWin's strength is a global.
3. On compositors without a blur protocol, the user enables blur in the
   compositor's own settings; rt cannot do it for them.

### The portable alternative that *does* give a slider

Since we can't blur what's behind, but the stated goal is *"the window below is
visible but its text is not too legible,"* a client-side **scrim** achieves that
goal everywhere with a real strength slider: over the translucent background,
draw a semi-opaque wash that reduces the contrast (and thus legibility) of
whatever shows through, without hiding its shapes/motion. It is not Gaussian
blur, but it is cheap, portable, and fully rt-controllable.

### Status / decision pending
Translucency is implemented. The blur approach is a user decision (see the
porting log): compositor-blur request (KDE on/off) vs. a portable scrim slider
vs. both. To be resolved before implementing.
