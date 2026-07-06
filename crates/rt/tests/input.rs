//! Tests for winit-key → Chord and typed-key → PTY-bytes translation. These are
//! pure and need no display, so they guard the fiddliest part of the app.

use rt_app::{chord_from_winit, encode_key};
use rt_config::{Action, Chord, Key as RtKey, Keymap, Mods};
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

/// Helper: a winit character key from a `&str`.
fn ch(s: &str) -> Key {
    Key::Character(SmolStr::new(s)) // winit stores chars as small strings
}

#[test]
fn ctrl_shift_o_maps_to_split_horiz() {
    // Ctrl+Shift+O should resolve, through the default keymap, to SplitHoriz.
    let mods = ModifiersState::CONTROL | ModifiersState::SHIFT; // held modifiers
    let chord = chord_from_winit(&ch("o"), mods).expect("maps to a chord");
    // The chord equals the parsed Terminator accelerator.
    assert_eq!(chord, Chord::parse("<Shift><Control>o").unwrap());
    // And the default keymap turns it into the split action.
    assert_eq!(Keymap::defaults().action_for(&chord), Some(Action::SplitHoriz));
}

#[test]
fn alt_arrows_map_to_focus_moves() {
    // Alt+Left is go_left in the default map.
    let chord = chord_from_winit(&Key::Named(NamedKey::ArrowLeft), ModifiersState::ALT).unwrap();
    assert_eq!(chord.key, RtKey::Left);
    assert!(chord.mods.contains(Mods::ALT));
    assert_eq!(Keymap::defaults().action_for(&chord), Some(Action::GoLeft));
}

#[test]
fn plain_char_has_no_binding_but_encodes_to_bytes() {
    // A bare 'a' is not a binding (no modifiers) — the keymap misses...
    let chord = chord_from_winit(&ch("a"), ModifiersState::empty()).unwrap();
    assert_eq!(Keymap::defaults().action_for(&chord), None);
    // ...so it falls through to encoding: 'a' -> the byte 'a'.
    assert_eq!(encode_key(&ch("a"), ModifiersState::empty()), Some(b"a".to_vec()));
}

#[test]
fn control_letter_encodes_c0_control_code() {
    // Ctrl-C must send 0x03 (ETX), the interrupt control code.
    assert_eq!(encode_key(&ch("c"), ModifiersState::CONTROL), Some(vec![0x03]));
    // Ctrl-A sends 0x01.
    assert_eq!(encode_key(&ch("a"), ModifiersState::CONTROL), Some(vec![0x01]));
}

#[test]
fn special_keys_encode_ansi_sequences() {
    // Enter -> CR; arrows -> CSI sequences; Backspace -> DEL.
    assert_eq!(encode_key(&Key::Named(NamedKey::Enter), ModifiersState::empty()), Some(b"\r".to_vec()));
    assert_eq!(encode_key(&Key::Named(NamedKey::ArrowUp), ModifiersState::empty()), Some(b"\x1b[A".to_vec()));
    assert_eq!(encode_key(&Key::Named(NamedKey::Backspace), ModifiersState::empty()), Some(vec![0x7f]));
}
