mod display;
#[cfg(test)]
pub(crate) use display::seal_appearance;
mod fonts;
mod parse;
mod raster;
mod renderer;
mod svg;
mod text;
mod types;

pub use fonts::preload_system_fonts;
pub use renderer::{FontRenderReport, ImageFilter, PageRenderer, RasterBackend};
pub use types::*;

/// Reusable page rasterizer. Loading system fonts is expensive, so the desktop
/// app creates one renderer per document and shares it between background jobs.
/// Parsed pages kept per document. Each tree is small compared with a bitmap.
pub(crate) const TREE_CACHE_LIMIT: usize = 4;

// Font matrices often contain 1/2048 or 1/1000 and are subsequently scaled by
// the object's CTM. Four decimal places introduces percent-level glyph-size
// errors, or rounds small glyph transforms to zero entirely.

#[cfg(test)]
mod tests;
