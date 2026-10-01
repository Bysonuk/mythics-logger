//! The mythics.gg logger's engine, without any UI.
//!
//! It only ever reads World of Warcraft's combat log, a text file the game
//! writes for the player. It never opens the game's process, reads its memory,
//! injects anything or automates play (the owner's decision on #322).
//!
//! Privacy: nothing in this crate logs a raw log line or a name from one. Log
//! messages carry counts, byte offsets, ids and file names only.

pub mod addon;
pub mod api;
pub mod archive;
pub mod auth;
pub mod backlog;
pub mod bosshp;
pub mod chunker;
pub mod header;
pub mod line;
pub mod plan;
pub mod priority;
pub mod queue;
pub mod session;
pub mod splitter;
pub mod tailer;
pub mod throttle;
pub mod timestamp;
pub mod uploader;
pub mod wowdir;

/// Sent with every upload, so the server can tell app versions apart.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where uploads go unless a developer overrides it in Settings.
pub const DEFAULT_ORIGIN: &str = "https://mythics.gg";
