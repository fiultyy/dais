//! This crate contains generic utilities and helpers available for use across all internal warp
//! crates.
//!
//! Generally, if a given function/abstraction is useful outside of a single warp-internal crate
//! but isn't large/complex enough to warrant its own crate, it belongs here.
pub mod ascii;
pub use ascii::{AsciiDebug, parse_ascii_u32};
pub mod assets;
pub mod content_version;
pub mod extensions;
pub use extensions::TrimStringExt;
pub mod file;
pub mod file_type;
pub mod on_cancel;
pub mod path;
pub mod standardized_path;
pub mod user_input;
pub mod worktree_names;

#[cfg(windows)]
pub mod windows;
