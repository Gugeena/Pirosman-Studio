//! Pirosman Studio core: everything that does not need a window.
//!
//! The Tauri app (`src-tauri`) is a thin command layer over this crate, so the
//! project model, editing, rendering, and version control are all testable
//! with `cargo test -p pirosman-core`.

pub mod bundles;
pub mod collab;
pub mod diagnostics;
pub mod diff;
pub mod error;
pub mod history;
pub mod interchange;
pub mod media;
pub mod merge;
pub mod model;
pub mod ofx;
pub mod otio;
pub mod project;
pub mod render;
pub mod resolve_re;
pub mod revisions;
pub mod scopes;
pub mod time;
pub mod timeline;

pub use error::{CoreError, CoreResult};
