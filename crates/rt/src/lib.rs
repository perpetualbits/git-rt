//! `rt_app` — the testable core of the rt binary.
//!
//! The GL rendering and the winit run-loop live in `main.rs` (they need a
//! display and cannot be unit-tested here). What *can* be tested — and is the
//! subtlest, most bug-prone part of a keyboard-driven app — is the translation
//! from a physical winit key event into rt's semantic [`Action`], plus the
//! encoding of ordinary typed keys into the bytes a PTY expects. Both live here
//! as pure functions with unit tests.

pub mod input; // winit key/modifiers -> Chord, and typed-key -> PTY bytes

pub use input::{chord_from_winit, encode_key};
