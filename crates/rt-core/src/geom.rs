//! Minimal rectangle geometry shared by the layout tree and the renderer.
//!
//! We use `f32` logical pixels throughout. Integer cell math happens later, in
//! the renderer, once a pane's pixel rectangle and the font cell size are both
//! known; keeping geometry in floats here avoids rounding bias when we divide a
//! parent rectangle among weighted children.

/// An axis-aligned rectangle in logical pixels.
///
/// `x`/`y` are the top-left corner; `w`/`h` are width/height. The coordinate
/// system is the usual GUI one: +x goes right, +y goes down. All layout output
/// is expressed as `Rect`s so the renderer can blit each pane independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32, // left edge
    pub y: f32, // top edge
    pub w: f32, // width  (>= 0)
    pub h: f32, // height (>= 0)
}

impl Rect {
    /// Construct a rectangle from its top-left corner and size.
    ///
    /// Negative sizes are clamped to zero so downstream code never has to
    /// reason about "inside-out" rectangles (a defensive choice: a zero-area
    /// pane is harmless, a negative-area one corrupts hit-testing).
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect {
            x,
            y,
            w: w.max(0.0), // clamp: never allow negative width
            h: h.max(0.0), // clamp: never allow negative height
        }
    }

    /// The x coordinate of the right edge (`x + w`).
    pub fn right(&self) -> f32 {
        self.x + self.w // right edge = left + width
    }

    /// The y coordinate of the bottom edge (`y + h`).
    pub fn bottom(&self) -> f32 {
        self.y + self.h // bottom edge = top + height
    }

    /// The geometric centre point `(cx, cy)` of the rectangle.
    ///
    /// Used by directional focus navigation to decide which neighbouring pane
    /// lies "to the left/right/up/down" of the current one.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5) // midpoint on each axis
    }

    /// Whether a point lies inside the rectangle (half-open on the far edges).
    ///
    /// Half-open (`< right`, not `<= right`) so two panes sharing a border do
    /// not both claim a click on that border — the lower/right pane wins by not
    /// claiming it, and the divider gutter absorbs it anyway.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}
