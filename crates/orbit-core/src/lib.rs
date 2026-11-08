//! Orbit's shared domain logic. No OS calls, no C code, no background work.

pub mod action;
pub mod bindings_text;
pub mod calc;
pub mod currency;
pub mod emoji;
pub mod hotkeys;
pub mod links;
pub mod search;
pub mod settings;
pub mod snippet;
pub mod text;
pub mod vim;
pub mod speedtest;
pub mod weather;
pub mod timer;
pub mod colour;

pub use action::{Action, Builtin};
pub use search::{Hit, MatchClass, QueryScratch, SearchIndex, SearchItem};
pub use settings::Settings;
