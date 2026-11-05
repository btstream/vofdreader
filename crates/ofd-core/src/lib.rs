//! OFD (GB/T 33190-2016) parser and rendering primitives.

pub mod content;
pub mod document;
pub mod error;
mod font;
pub mod loc;
pub mod model;
pub mod package;
pub mod render;
pub mod xml;

pub use document::OfdDocument;
pub use error::{OfdError, Result};
pub use model::*;
pub use render::{
    FontRenderReport, PageRenderer, PageTextChar, PageTextLayout, RasterBackend, RasterPage,
    RasterRegion, TextMatchRect, preload_system_fonts,
};
