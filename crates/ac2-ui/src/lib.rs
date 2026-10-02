//! ac2 desktop UI: egui chrome, `ac2-plot` plots in egui-wgpu paint callbacks, every scene
//! from `ac2-scene`, the daemon through `ac2-client`.
//!
//! - [`state`]: the pure reducer (keys, commands, link events → state + requests).
//! - [`keys`]: the one scoped binding table, `keys.toml` overrides; [`palette`]: fuzzy
//!   command search.
//! - [`conn`]: the link thread (client, data drain, stimulus lease).
//! - [`scenes`]: frames + state → `ac2-scene` builders; [`plot`]: scene → pixels inside
//!   egui's pass.
//! - [`app`] / `view`: the eframe app and its drawing.
//!
//! This crate has no DSP and computes no displayed value; `ac2-scene` does display math.
#![forbid(unsafe_code)]

pub mod anim;
pub mod app;
pub mod conn;
pub mod connect;
pub mod embedded;
pub mod forms;
pub mod keys;
pub mod palette;
pub mod plot;
pub mod prefs;
pub mod scenes;
pub mod session_dialog;
pub mod state;
pub mod theme;
mod view;

pub use app::{App, AppOptions};
