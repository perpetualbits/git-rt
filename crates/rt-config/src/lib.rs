//! `rt-config` — keybindings and configuration for rt.
//!
//! This is the port of Terminator's `keybindings` section of `config.py`. We
//! keep Terminator's exact accelerator syntax (`<Shift><Control>o`) so a user's
//! muscle memory — and eventually their config file — carries over. Parsing
//! that string into a normalised [`Chord`] and mapping it to a semantic
//! [`Action`] is pure logic, unit-tested without any GUI.
//!
//! The GUI front-end converts a physical winit key event into a [`Chord`] and
//! calls [`Keymap::action_for`]; the returned [`Action`] is then handed to the
//! session controller (`rt-session`).

pub mod keys; // Chord / Key / Mods normalisation and parsing

pub use keys::{Chord, Key, Mods};

/// A semantic editor action, decoupled from the physical keys that trigger it.
///
/// This is the subset of Terminator's action list that rt implements (or will
/// implement imminently). Naming follows Terminator's config keys so the
/// mapping is one-to-one and auditable against `config.py`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// `split_horiz` — split with a horizontal divider (panes stacked
    /// top/bottom). Maps to `Orientation::TopBottom` in rt-core.
    SplitHoriz,
    /// `split_vert` — split with a vertical divider (panes side by side).
    /// Maps to `Orientation::LeftRight`.
    SplitVert,
    /// `close_term` — close the focused pane.
    CloseTerm,
    /// `new_tab` — open a new tab beside the focused pane.
    NewTab,
    /// `next_tab` / `prev_tab` — cycle the active tab.
    NextTab,
    PrevTab,
    /// `go_up`/`down`/`left`/`right` — move focus spatially between panes.
    GoUp,
    GoDown,
    GoLeft,
    GoRight,
    /// `copy` / `paste` — clipboard integration (wired at the GUI layer).
    Copy,
    Paste,
    /// `broadcast_off` / `broadcast_group` / `broadcast_all` — set how typed
    /// input fans out to other panes (rt's port of Terminator grouping).
    BroadcastOff,
    BroadcastGroup,
    BroadcastAll,
    /// `close_window` — close the whole window.
    CloseWindow,
    /// rt-specific: add one newspaper column to the focused pane (1 = normal).
    ColumnsMore,
    /// rt-specific: remove one newspaper column (clamped at 1 = normal).
    ColumnsFewer,
    /// rt-specific: make the window background more opaque.
    OpacityUp,
    /// rt-specific: make the window background more translucent (see-through).
    OpacityDown,
}

/// Window-level appearance settings (Terminator's "Profiles → Background" in
/// spirit). Kept minimal for now; a future preferences panel edits these and a
/// config file persists them.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Background opacity, `0.05..=1.0`. `1.0` is fully opaque; lower values let
    /// the window(s) behind show through (compositor permitting). Clamped away
    /// from 0 so the window can never become completely invisible.
    pub background_opacity: f32,
}

impl Default for Settings {
    /// Sensible defaults: fully opaque, matching a normal terminal.
    fn default() -> Self {
        Settings { background_opacity: 1.0 } // opaque until the user dials it down
    }
}

impl Settings {
    /// The smallest opacity we allow, so the window never vanishes entirely.
    pub const MIN_OPACITY: f32 = 0.05;

    /// Nudge the opacity by `delta`, clamped to `[MIN_OPACITY, 1.0]`. Returns
    /// the new value. Used by the `OpacityUp`/`OpacityDown` actions.
    pub fn adjust_opacity(&mut self, delta: f32) -> f32 {
        // Clamp so we stay in a usable, always-visible range.
        self.background_opacity = (self.background_opacity + delta).clamp(Self::MIN_OPACITY, 1.0);
        self.background_opacity
    }
}

/// The keymap: an ordered list of `(chord, action)` bindings.
///
/// A `Vec` (not a `HashMap`) because the list is short (a few dozen entries),
/// lookup is O(n) but trivially fast, and a `Vec` preserves the ability to have
/// later user bindings override earlier defaults simply by being searched
/// first. `action_for` returns the first match.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<(Chord, Action)>, // searched front-to-back; user entries go first
}

impl Keymap {
    /// Build the keymap pre-populated with Terminator's default bindings.
    ///
    /// Only the actions rt currently implements are included; the rest of
    /// Terminator's map is intentionally omitted until the matching feature
    /// exists, so no key silently does nothing-but-looks-bound.
    pub fn defaults() -> Self {
        // (Terminator accelerator string, action) pairs, transcribed from
        // reference/terminator/terminatorlib/config.py:126-210.
        let defaults: &[(&str, Action)] = &[
            ("<Shift><Control>o", Action::SplitHoriz),   // split_horiz
            ("<Shift><Control>e", Action::SplitVert),    // split_vert
            ("<Shift><Control>w", Action::CloseTerm),    // close_term
            ("<Shift><Control>t", Action::NewTab),       // new_tab
            ("<Control>Page_Down", Action::NextTab),     // next_tab
            ("<Control>Page_Up", Action::PrevTab),       // prev_tab
            ("<Alt>Up", Action::GoUp),                   // go_up
            ("<Alt>Down", Action::GoDown),               // go_down
            ("<Alt>Left", Action::GoLeft),               // go_left
            ("<Alt>Right", Action::GoRight),             // go_right
            ("<Shift><Control>c", Action::Copy),         // copy
            ("<Shift><Control>v", Action::Paste),        // paste
            ("<Shift><Control>q", Action::CloseWindow),  // close_window
            // rt-specific newspaper-column controls. Deliberately Ctrl+symbol
            // (no Shift) so winit's shifted-symbol remapping can't break them.
            ("<Control>period", Action::ColumnsMore),    // Ctrl+.  -> more columns
            ("<Control>comma", Action::ColumnsFewer),    // Ctrl+,  -> fewer columns
            // Live background-opacity nudges (also settable in preferences).
            ("<Control><Alt>Up", Action::OpacityUp),     // more opaque
            ("<Control><Alt>Down", Action::OpacityDown), // more see-through
        ];
        let mut map = Keymap::default(); // empty binding list
        for (accel, action) in defaults {
            // Parse each default; a malformed default is a programming error, so
            // we skip it rather than panic (keeps `defaults()` infallible).
            if let Some(chord) = Chord::parse(accel) {
                map.bindings.push((chord, *action)); // register the binding
            }
        }
        map
    }

    /// Register (or override) a binding. Inserted at the *front* so it shadows
    /// any earlier binding for the same chord — this is how user config
    /// overrides defaults.
    pub fn bind(&mut self, chord: Chord, action: Action) {
        self.bindings.insert(0, (chord, action)); // front insert = highest priority
    }

    /// Look up the action bound to `chord`, if any. Returns the first match in
    /// priority order (user overrides before defaults).
    pub fn action_for(&self, chord: &Chord) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(c, _)| c == chord) // first chord that matches exactly
            .map(|(_, a)| *a) // hand back just the action
    }
}
