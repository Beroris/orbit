//! Platform-neutral shortcut model and the HyperKey state machine.
//!
//! Nothing in this crate touches the OS. A host feeds key events in and performs
//! the effects that come out, so the semantics in `docs/SHORTCUT_SEMANTICS.md`
//! can be tested exhaustively without a real keyboard.

pub mod chord;
pub mod hyper;

pub use chord::{Chord, Mods};
pub use hyper::{Decision, Effect, HyperConfig, HyperMachine, HyperMode, LEADER_WINDOW_MS, TapBehavior};
