//! wgpu renderer for plot scenes: lines, fills, heatmaps, grids, text.
//!
//! Scenes arrive as plain data ([`scene`]) with every coordinate already mapped to logical
//! pixels; this crate chooses pixel placement (scaling, snapping, anti-aliasing, clipping)
//! and never computes a displayed value.
//!
//! Interactive use (e.g. an egui-wgpu paint callback): build one [`RenderShared`] per target
//! format and a [`Renderer`] per plot from it, call [`Renderer::prepare`] with the frame's
//! [`FrameTarget`] before the pass and [`Renderer::paint`] inside it. Tests and screenshots
//! use [`offscreen`].
#![deny(unsafe_code)]

pub mod colormap;
mod geometry;
mod heatmap;
pub mod offscreen;
mod renderer;
pub mod scene;
mod text;

pub use offscreen::{Gpu, GpuError, Offscreen, Pixels, RenderError, render_to_image};
pub use renderer::{FrameTarget, PrepareError, RenderShared, Renderer};
pub use scene::*;
pub use text::{FONT_DATA, FONT_FAMILY};
/// The wgpu this crate is built against; hosts must hand it devices of the same version.
pub use wgpu;
