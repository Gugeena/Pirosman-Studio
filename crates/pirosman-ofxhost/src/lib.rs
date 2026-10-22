//! Pirosman's OpenFX host, run as its own process (`pirosman-ofxhost`) so a
//! plugin that crashes takes down only that process, never the editor.
//!
//! The editor talks to it through [`protocol`]: `scan` describes the plugins
//! in one `.ofx` binary, `render` streams frames through an effect chain.

pub mod host;
pub mod props;
pub mod protocol;
pub mod stream;
pub mod sys;
