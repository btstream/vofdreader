//! Direct DisplayList rasterizer: no SVG serialization and no usvg parse.
//!
//! A page is prepared once into flat draw operations (group transforms are
//! composed up front, only translucent groups stay nested as layers). Every
//! operation carries page-space bounds, so a tile only paints what it hits.
//! Paint semantics deliberately mirror the SVG/resvg path this replaces:
//! same tiny-skia rasterizer, opacity rounding, dash rules, image filtering
//! and usvg's system-font fallback order.

use std::collections::HashMap;
use std::sync::Arc;

use resvg::usvg::fontdb;

mod build;
mod fonts;
mod images;
pub(super) mod ops;
mod paint;
mod path;

pub(crate) use build::prepare;
pub(crate) use fonts::*;
pub(crate) use images::*;
pub(super) use ops::Op;
pub(crate) use ops::Prepared;

/// Glyph outlines and font choices shared by all pages of one renderer.
#[derive(Default)]
pub(crate) struct FontCache {
    /// (family, generic, weight, italic) → base face, as usvg's query.
    base: HashMap<(String, &'static str, u16, bool), Option<fontdb::ID>>,
    /// (base face, char) → face actually used, as usvg's fallback selector.
    fallback: HashMap<(fontdb::ID, char), Option<fontdb::ID>>,
    /// (face, char) → outline in font units plus units per em.
    system_glyphs: HashMap<(fontdb::ID, char), SystemGlyph>,
    /// Embedded glyph outlines by (font key, path command identity).
    embedded: HashMap<(usize, u64), Arc<resvg::tiny_skia::Path>>,
}

pub(super) const IMAGE_CACHE_BYTES: usize = 256 * 1024 * 1024;
