use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use resvg::usvg::{self, fontdb};

use crate::document::OfdDocument;
use crate::error::{OfdError, Result};
use crate::font::{EmbeddedFace, select_face};
use crate::loc::resolve_loc;
use crate::model::{Color, DrawParam, FontResource, SizeMm};
use crate::xml::{child, child_text, children, parse_xml};

mod raster;

/// 3×2 affine matrix in SVG order: `a b c d e f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f64, y: f64) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut values = value.split_whitespace();
        let a = values.next()?.parse().ok()?;
        let b = values.next()?.parse().ok()?;
        let c = values.next()?.parse().ok()?;
        let d = values.next()?.parse().ok()?;
        let e = values.next()?.parse().ok()?;
        let f = values.next()?.parse().ok()?;
        Some(Self { a, b, c, d, e, f })
    }

    /// Compose `self * other` (apply `other` first, then `self`).
    pub fn compose(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    pub fn invert(&self) -> Option<Self> {
        let determinant = self.a * self.d - self.b * self.c;
        if determinant.abs() < 1e-12 {
            return None;
        }
        Some(Self {
            a: self.d / determinant,
            b: -self.b / determinant,
            c: -self.c / determinant,
            d: self.a / determinant,
            e: (self.c * self.f - self.d * self.e) / determinant,
            f: (self.b * self.e - self.a * self.f) / determinant,
        })
    }

    /// Applies this affine transform to a point.
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn svg(&self) -> String {
        format!(
            "matrix({} {} {} {} {} {})",
            matrix_num(self.a),
            matrix_num(self.b),
            matrix_num(self.c),
            matrix_num(self.d),
            matrix_num(self.e),
            matrix_num(self.f)
        )
    }
}

#[derive(Debug, Clone)]
pub struct DisplayList {
    pub size: SizeMm,
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Path {
        transform: Matrix,
        cmds: Vec<PathCmd>,
        style: PathStyle,
    },
    Text {
        transform: Matrix,
        chars: Vec<PositionedChar>,
        glyphs: Vec<u16>,
        style: TextStyle,
    },
    Image {
        transform: Matrix,
        width: f64,
        height: f64,
        mime: &'static str,
        data: Vec<u8>,
        blend: Option<String>,
        /// Optional clip rectangle in image-local millimetres.
        clip: Option<(f64, f64, f64, f64)>,
    },
    Group {
        transform: Matrix,
        items: Vec<Item>,
        opacity: f64,
    },
    /// Object-level `Clips`: `items` are visible only where every region
    /// overlaps (regions intersect; the shapes inside one region unite).
    Clip {
        regions: Vec<Vec<ClipShape>>,
        items: Vec<Item>,
    },
}

/// One clip outline, in the same coordinate space as the clipped items'
/// parent (the object's Boundary origin and the Area CTM are folded in).
#[derive(Debug, Clone)]
pub struct ClipShape {
    pub transform: Matrix,
    pub cmds: Vec<PathCmd>,
    pub even_odd: bool,
}

/// `AxialShd` / `RadialShd` gradient in the path's own coordinates.
#[derive(Debug, Clone)]
pub struct Shading {
    pub kind: ShadingKind,
    /// Raw `Segment` stops, positions in 0..=1, ascending.
    pub stops: Vec<(f64, Color)>,
    pub extend_start: bool,
    pub extend_end: bool,
    pub spread: ShadingSpread,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadingKind {
    Axial {
        start: (f64, f64),
        end: (f64, f64),
    },
    /// Focal point `start` (radius `start_radius`) to circle `end`/`radius`.
    Radial {
        start: (f64, f64),
        start_radius: f64,
        end: (f64, f64),
        radius: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadingSpread {
    Pad,
    Repeat,
    Reflect,
}

impl Shading {
    /// Stops as both backends draw them. A non-zero start radius maps onto
    /// the start_radius/radius..1 part of the gradient (both backends only
    /// support a zero focal radius), and an end that must not extend gets a
    /// transparent guard stop so padding leaves it unpainted.
    pub fn resolved_stops(&self) -> Vec<(f64, Color)> {
        const GUARD: f64 = 1e-4;
        let offset = match self.kind {
            ShadingKind::Radial {
                start_radius,
                radius,
                ..
            } if radius > 0.0 && start_radius > 0.0 => (start_radius / radius).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let mut stops: Vec<(f64, Color)> = self
            .stops
            .iter()
            .map(|(position, color)| (offset + position.clamp(0.0, 1.0) * (1.0 - offset), *color))
            .collect();
        if self.spread != ShadingSpread::Pad || stops.is_empty() {
            return stops;
        }
        let clear = |color: Color| Color { a: 0, ..color };
        if !self.extend_start {
            let first = stops[0].1;
            for stop in &mut stops {
                stop.0 = stop.0.max(offset + GUARD);
            }
            stops.insert(0, (offset, clear(first)));
            if offset > 0.0 {
                stops.insert(0, (0.0, clear(first)));
            }
        } else if offset > 0.0 {
            stops.insert(0, (0.0, stops[0].1));
        }
        if !self.extend_end {
            let last = stops[stops.len() - 1].1;
            for stop in &mut stops {
                stop.0 = stop.0.min(1.0 - GUARD);
            }
            stops.push((1.0, clear(last)));
        }
        stops
    }
}

#[derive(Debug, Clone)]
pub enum PathCmd {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Arc {
        rx: f64,
        ry: f64,
        rotation: f64,
        large: bool,
        sweep: bool,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Debug, Clone)]
pub struct PathStyle {
    pub fill: bool,
    pub stroke: bool,
    pub fill_color: Option<Color>,
    pub stroke_color: Option<Color>,
    pub line_width: f64,
    pub cap: String,
    pub join: String,
    pub dash: Vec<f64>,
    pub opacity: f64,
    pub even_odd: bool,
    /// Gradient that replaces `fill_color` / `stroke_color` when present.
    pub fill_shading: Option<Shading>,
    pub stroke_shading: Option<Shading>,
}

impl Default for PathStyle {
    fn default() -> Self {
        Self {
            fill: true,
            stroke: true,
            fill_color: None,
            stroke_color: Some(Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            line_width: 0.25,
            cap: "Butt".to_string(),
            join: "Miter".to_string(),
            dash: Vec::new(),
            opacity: 1.0,
            even_odd: false,
            fill_shading: None,
            stroke_shading: None,
        }
    }
}

impl PathStyle {
    fn apply(&mut self, param: &DrawParam) {
        if let Some(width) = param.line_width {
            self.line_width = width;
        }
        if let Some(color) = param.stroke_color {
            self.stroke_color = Some(color);
        }
        if let Some(color) = param.fill_color {
            self.fill_color = Some(color);
        }
        if let Some(cap) = &param.cap {
            self.cap = cap.clone();
        }
        if let Some(join) = &param.join {
            self.join = join.clone();
        }
        if !param.dash_pattern.is_empty() {
            self.dash = param.dash_pattern.clone();
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextStyle {
    /// Resource key (`scope:id`) used to resolve embedded faces.
    pub font_key: String,
    pub font_family: String,
    pub font_size: f64,
    pub color: Color,
    pub weight: u16,
    pub italic: bool,
    pub opacity: f64,
    pub h_scale: f64,
    pub char_direction: u16,
    pub read_direction: u16,
    pub paint: PathStyle,
    pub clusters: Vec<GlyphCluster>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlyphCluster {
    pub start: usize,
    pub end: usize,
    pub glyphs: Vec<u16>,
    pub positions: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct PositionedChar {
    pub ch: char,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterized page ready for a GPU texture. GPUI's `RenderImage` expects
/// straight-alpha BGRA, so no PNG encode/decode round trip is needed.
#[derive(Debug, Clone)]
pub struct RasterPage {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A crop in page-device pixels at the requested DPI. A caller can include a
/// small gutter and clip it when compositing adjacent tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RasterRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Bounding box of a text match in page millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMatchRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One selectable character with its page-space bounds.
#[derive(Debug, Clone)]
pub struct PageTextChar {
    pub ch: char,
    pub rect: TextMatchRect,
    /// Visual line index; selection inserts a newline between lines.
    pub line: u32,
    /// Source text object; same-line runs are joined with a space.
    pub run: u32,
}

/// Flattened selectable text of a page, in reading order of the display list.
/// Annotation groups are excluded so stamps and watermarks do not pollute a
/// selection.
#[derive(Debug, Clone, Default)]
pub struct PageTextLayout {
    pub chars: Vec<PageTextChar>,
}

impl PageTextLayout {
    fn from_items(items: &[Item]) -> Self {
        let mut chars = Vec::new();
        let mut run = 0u32;
        let mut line = 0u32;
        let mut previous: Option<(f64, f64)> = None;

        for item in items {
            let Item::Text {
                transform,
                chars: positioned,
                style,
                ..
            } = item
            else {
                continue;
            };
            for index in 0..positioned.len() {
                let Some(rect) =
                    text_run_rect(transform, positioned, index, index + 1, style.font_size)
                else {
                    continue;
                };
                let (_, baseline) =
                    transform.transform_point(positioned[index].x, positioned[index].y);
                if let Some((previous_baseline, previous_x)) = previous
                    && ((baseline - previous_baseline).abs() > style.font_size * 0.6
                        || rect.x + style.font_size * 0.1 < previous_x)
                {
                    line += 1;
                }
                previous = Some((baseline, rect.x));
                chars.push(PageTextChar {
                    ch: positioned[index].ch,
                    rect,
                    line,
                    run,
                });
            }
            run = run.wrapping_add(1);
        }

        Self { chars }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Index of the character under the point, or the nearest one within
    /// `max_distance` millimetres of its bounding box.
    pub fn index_at(&self, x: f64, y: f64, max_distance: f64) -> Option<usize> {
        let mut nearest: Option<(usize, f64)> = None;
        for (index, character) in self.chars.iter().enumerate() {
            let rect = character.rect;
            if x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height {
                return Some(index);
            }
            let center_x = rect.x + rect.width / 2.0;
            let center_y = rect.y + rect.height / 2.0;
            let distance = (x - center_x).powi(2) + (y - center_y).powi(2);
            if nearest.is_none_or(|(_, best)| distance < best) {
                nearest = Some((index, distance));
            }
        }
        nearest
            .filter(|(_, distance)| *distance <= max_distance * max_distance)
            .map(|(index, _)| index)
    }

    pub fn text_range(&self, start: usize, end: usize) -> String {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len());
        if start >= end {
            return String::new();
        }
        let mut text = String::new();
        for index in start..end {
            let character = &self.chars[index];
            if index > start {
                let previous = &self.chars[index - 1];
                if character.line != previous.line {
                    text.push('\n');
                } else if character.run != previous.run && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
            }
            text.push(character.ch);
        }
        text
    }

    pub fn rects(&self, start: usize, end: usize) -> Vec<TextMatchRect> {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len());
        self.chars[start..end]
            .iter()
            .map(|char| char.rect)
            .collect()
    }
}

impl OfdDocument {
    /// Parse one page (template + content + annotations) into a display list.
    pub fn display_list(&self, index: usize) -> Result<DisplayList> {
        let page = self.page(index)?;
        let loc = resolve_loc(&self.document_file, &page.base_loc)?;
        let bytes = self.package.read_shared(&loc)?;
        let document = parse_xml(&bytes, &loc)?;
        let root = document.root_element();

        let size = root
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "Area")
            .and_then(|area| child_text(area, "PhysicalBox"))
            .and_then(crate::model::parse_physical_box)
            .or(self.meta.page_area)
            .ok_or_else(|| OfdError::MissingElement("PhysicalBox".to_string()))?;

        let mut items = Vec::new();
        let mut warnings = self.warnings.clone();

        let mut template_uses = Vec::new();
        for node in root
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "Template")
        {
            template_uses.push((
                node.attribute("TemplateID").unwrap_or("").to_string(),
                node.attribute("ZOrder").unwrap_or("Background").to_string(),
            ));
        }

        for (id, order) in &template_uses {
            if order.eq_ignore_ascii_case("Background") {
                self.append_template(id, &mut items, &mut warnings);
            }
        }

        if let Some(content) = child(root, "Content") {
            walk_block(
                self,
                content,
                &mut items,
                &PathStyle::default(),
                &mut warnings,
            );
        }

        for (id, order) in &template_uses {
            if order.eq_ignore_ascii_case("Foreground") {
                self.append_template(id, &mut items, &mut warnings);
            }
        }

        self.append_annotations(&page.id, &mut items, &mut warnings);
        self.append_signature_seals(&page.id, &mut items);

        Ok(DisplayList {
            size,
            items,
            warnings,
        })
    }

    fn append_template(&self, id: &str, items: &mut Vec<Item>, warnings: &mut Vec<String>) {
        let Some(template) = self
            .meta
            .template_pages
            .iter()
            .find(|template| template.id == id)
        else {
            warnings.push(format!("模板 {id} 未在 Document.xml 中声明"));
            return;
        };

        let loc = match resolve_loc(&self.document_file, &template.base_loc) {
            Ok(loc) => loc,
            Err(error) => {
                warnings.push(format!("模板 {id} 路径非法：{error}"));
                return;
            }
        };
        let bytes = match self.package.read_shared(&loc) {
            Ok(bytes) => bytes,
            Err(error) => {
                warnings.push(format!("模板 {id} 读取失败：{error}"));
                return;
            }
        };
        let document = match parse_xml(&bytes, &loc) {
            Ok(document) => document,
            Err(error) => {
                warnings.push(format!("模板 {id} 解析失败：{error}"));
                return;
            }
        };

        if let Some(content) = child(document.root_element(), "Content") {
            walk_block(self, content, items, &PathStyle::default(), warnings);
        }
    }

    fn append_signature_seals(&self, page_id: &str, items: &mut Vec<Item>) {
        for seal in self.signature_seals() {
            if seal.page_id != page_id {
                continue;
            }
            items.push(Item::Image {
                transform: Matrix::translate(seal.x, seal.y),
                width: seal.width,
                height: seal.height,
                mime: seal.mime,
                data: seal.data.as_ref().clone(),
                // Many stamp images are opaque (white paper around red ink).
                // Multiply keeps the ink and lets the page show through the
                // white, like a real stamp; transparent stamps are unchanged.
                blend: Some("Multiply".to_string()),
                clip: seal.clip,
            });
        }
    }

    fn append_annotations(&self, page_id: &str, items: &mut Vec<Item>, warnings: &mut Vec<String>) {
        let Some(annotation_page) = self.annotations_for_page(page_id) else {
            return;
        };
        let bytes = match self.package.read_shared(&annotation_page.file) {
            Ok(bytes) => bytes,
            Err(error) => {
                warnings.push(format!("注释页读取失败：{error}"));
                return;
            }
        };
        let document = match parse_xml(&bytes, &annotation_page.file) {
            Ok(document) => document,
            Err(error) => {
                warnings.push(format!("注释页解析失败：{error}"));
                return;
            }
        };

        for annot in children(document.root_element(), "Annot") {
            let Some(appearance) = child(annot, "Appearance") else {
                continue;
            };
            let Some(boundary) = appearance.attribute("Boundary") else {
                continue;
            };
            let Some((x, y, _, _)) = crate::model::parse_box(boundary) else {
                continue;
            };

            let mut children_items = Vec::new();
            walk_block(
                self,
                appearance,
                &mut children_items,
                &PathStyle::default(),
                warnings,
            );
            if !children_items.is_empty() {
                items.push(Item::Group {
                    transform: Matrix::translate(x, y),
                    items: children_items,
                    opacity: 1.0,
                });
            }
        }
    }

    /// Bounding boxes for every occurrence of `query` on a page, in page
    /// millimetres. Rotated text is represented by its axis-aligned bounds.
    pub fn page_search_rects(&self, index: usize, query: &str) -> Result<Vec<TextMatchRect>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let list = self.display_list(index)?;
        let mut rects = Vec::new();
        collect_text_matches(&list.items, query, &mut rects);
        Ok(rects)
    }

    /// Selectable text geometry for a page. Annotation groups are skipped.
    pub fn page_text_layout(&self, index: usize) -> Result<PageTextLayout> {
        let list = self.display_list(index)?;
        Ok(PageTextLayout::from_items(&list.items))
    }

    pub fn page_svg(&self, index: usize) -> Result<String> {
        PageRenderer::new(self).export_svg(self, index)
    }

    /// SVG for rasterization, with the same embedded glyph outlines and
    /// fallback aliases as the reusable renderer, but no font Base64 rules.
    pub fn page_svg_for_render(&self, index: usize) -> Result<String> {
        PageRenderer::new(self).render_svg(self, index)
    }

    fn page_svg_impl(
        &self,
        index: usize,
        families: Option<&HashMap<String, String>>,
        glyph_fonts: Option<&HashMap<String, GlyphFont>>,
        paper_background: bool,
    ) -> Result<String> {
        let list = self.display_list(index)?;
        let mut out = String::new();

        let _ = writeln!(
            out,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="0 0 {w} {h}">
"#,
            w = num(list.size.width),
            h = num(list.size.height)
        );

        // Paper background.
        if paper_background {
            let _ = writeln!(
                out,
                r##"<rect x="0" y="0" width="{w}" height="{h}" fill="#FFFFFF"/>"##,
                w = num(list.size.width),
                h = num(list.size.height)
            );
        }

        for item in &list.items {
            write_item_with_fonts(&mut out, item, families, glyph_fonts);
        }

        out.push_str("</svg>\n");
        Ok(out)
    }

    pub fn render_page_png(&self, index: usize, dpi: f32) -> Result<RenderedPage> {
        PageRenderer::new(self).render_png(self, index, dpi)
    }
}

/// Reusable page rasterizer. Loading system fonts is expensive, so the desktop
/// app creates one renderer per document and shares it between background jobs.
/// Parsed pages kept per document. Each tree is small compared with a bitmap.
const TREE_CACHE_LIMIT: usize = 4;

#[derive(Clone)]
struct GlyphFont {
    data: Arc<Vec<u8>>,
    face_index: u32,
}

/// How pages become pixels. Both share DisplayList parsing and fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterBackend {
    /// DisplayList drawn straight with tiny-skia (default).
    Direct,
    /// Former path: DisplayList → SVG text → usvg → resvg. Kept for A/B
    /// comparison; select with `OFD_RENDER_BACKEND=svg`.
    Svg,
}

impl RasterBackend {
    pub fn from_env() -> Self {
        match std::env::var("OFD_RENDER_BACKEND") {
            Ok(value) if value.eq_ignore_ascii_case("svg") => Self::Svg,
            _ => Self::Direct,
        }
    }
}

#[derive(Clone)]
enum CachedPage {
    Tree(Arc<usvg::Tree>),
    Prepared(Arc<raster::Prepared>),
}

impl CachedPage {
    /// Allocation identity, for cache tests.
    #[cfg(test)]
    fn id(&self) -> usize {
        match self {
            Self::Tree(tree) => Arc::as_ptr(tree) as usize,
            Self::Prepared(page) => Arc::as_ptr(page) as usize,
        }
    }
}

pub struct PageRenderer {
    fontdb: Arc<fontdb::Database>,
    font_aliases: HashMap<String, String>,
    glyph_fonts: HashMap<String, GlyphFont>,
    font_reports: Mutex<HashMap<usize, Arc<FontRenderReport>>>,
    backend: RasterBackend,
    /// Prepared pages (or parsed SVG trees), most recently used last.
    /// Re-rasterizing at a new DPI or revisiting a page skips parsing.
    tree_cache: Mutex<VecDeque<(usize, CachedPage)>>,
    tree_cache_limit: usize,
    /// Per-page build gates so concurrent requests share one build.
    building: Mutex<HashMap<usize, Arc<Mutex<()>>>>,
    font_cache: Mutex<raster::FontCache>,
    image_cache: Mutex<raster::ImageCache>,
}

#[derive(Debug, Clone, Default)]
pub struct FontRenderReport {
    pub embedded_glyphs: usize,
    /// Non-whitespace source characters replaced because an embedded font
    /// or its requested outlines could not be used. System-only fonts excluded.
    pub fallback_chars: usize,
    pub replaced_fonts: Vec<String>,
    pub fallback_codepoints: std::collections::BTreeMap<String, std::collections::BTreeSet<u32>>,
}

/// System font data is shared: scanning and parsing system fonts costs more
/// than a page render, and docs only add a handful of embedded faces on top.
fn system_fonts() -> fontdb::Database {
    static SYSTEM_FONTS: OnceLock<fontdb::Database> = OnceLock::new();
    SYSTEM_FONTS
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            database
        })
        .clone()
}

/// Starts scanning system fonts on a worker thread so window construction and
/// the first document open do not pay for it on the UI thread.
pub fn preload_system_fonts() {
    std::thread::spawn(|| {
        let _ = system_fonts();
    });
}

fn fallback_family(font: &FontResource) -> String {
    for name in [font.css_family(), font.name.as_str(), font.family.as_str()] {
        if let Some(alias) = system_font_alias(name) {
            return alias.to_string();
        }
    }
    font.css_family().to_string()
}

/// Map common Chinese font names from documents to families installed on the
/// system. This keeps text visible when an embedded subset has no outlines and
/// preserves the intended bold/宋体/仿宋 look where possible.
fn system_font_alias(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase().replace(' ', "");
    if lower.contains("fzxbsjw") || name.contains("小标宋") {
        Some("方正小标宋_GBK")
    } else if lower.contains("fzfsjw") || name.contains("仿宋") {
        Some("仿宋")
    } else if lower.contains("stsongti") || lower.contains("stsong") {
        Some("华文宋体")
    } else if lower.contains("stfangsong") {
        Some("华文仿宋")
    } else if lower.contains("simsun") || name.contains("宋体") {
        Some("宋体")
    } else if lower.contains("kaiti") || name.contains("楷体") {
        Some("楷体")
    } else if lower.contains("simhei") || name.contains("黑体") {
        Some("黑体")
    } else if lower.contains("fangsong") {
        Some("仿宋")
    } else {
        None
    }
}

impl PageRenderer {
    pub fn new(doc: &OfdDocument) -> Self {
        Self::with_tree_cache_limit(doc, TREE_CACHE_LIMIT)
    }

    /// Bound parsed-page LRU retention independently from raster output caches.
    /// Zero disables retention; the cap protects callers using user settings.
    pub fn with_tree_cache_limit(doc: &OfdDocument, limit: usize) -> Self {
        // System fonts stay in fontdb; embedded faces are rendered directly
        // from their outlines. Registering every embedded subset in fontdb
        // makes identical FamilyName entries (for example 宋体) race with each
        // other, so the publisher's font can be replaced by the wrong subset.
        let database = system_fonts();
        let mut glyph_fonts = HashMap::new();
        let mut font_aliases = HashMap::new();
        let mut seen = std::collections::HashSet::new();

        for font in &doc.resources.fonts {
            if !seen.insert(font.id.clone()) {
                continue;
            }
            let Some(selected) = doc.resources.font(&font.id) else {
                continue;
            };
            let key = selected.key();
            let fallback = fallback_family(selected);
            font_aliases.insert(key.clone(), fallback);

            let Some(file) = &selected.file else {
                continue;
            };
            let Ok(data) = doc.package.read(file) else {
                continue;
            };
            let data = Arc::new(data);
            let face_index = select_face(&data, selected);
            let glyph_font = GlyphFont { data, face_index };
            glyph_fonts.insert(key, glyph_font);
        }

        Self {
            fontdb: Arc::new(database),
            font_aliases,
            glyph_fonts,
            font_reports: Mutex::new(HashMap::new()),
            backend: RasterBackend::from_env(),
            tree_cache: Mutex::new(VecDeque::new()),
            tree_cache_limit: limit.min(256),
            building: Mutex::new(HashMap::new()),
            font_cache: Mutex::new(raster::FontCache::default()),
            image_cache: Mutex::new(raster::ImageCache::default()),
        }
    }

    /// Override the backend chosen from `OFD_RENDER_BACKEND`.
    pub fn with_backend(mut self, backend: RasterBackend) -> Self {
        self.backend = backend;
        self.tree_cache
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self
    }

    pub fn backend(&self) -> RasterBackend {
        self.backend
    }

    pub fn export_svg(&self, doc: &OfdDocument, index: usize) -> Result<String> {
        // Use exactly the same glyph selection as the rasterizer, including
        // explicit CGTransform IDs even when the font also has a Unicode cmap.
        self.render_svg(doc, index)
    }

    pub fn render_svg(&self, doc: &OfdDocument, index: usize) -> Result<String> {
        doc.page_svg_impl(
            index,
            Some(&self.font_aliases),
            Some(&self.glyph_fonts),
            true,
        )
    }

    pub(crate) fn render_seal_svg(&self, doc: &OfdDocument) -> Result<String> {
        doc.page_svg_impl(0, Some(&self.font_aliases), Some(&self.glyph_fonts), false)
    }

    /// Cached, deterministic outline audit. This reports embedded-font
    /// replacements, not just whether font files were present in the ZIP.
    pub fn font_report(&self, doc: &OfdDocument, index: usize) -> Result<Arc<FontRenderReport>> {
        if let Some(report) = self
            .font_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&index)
        {
            return Ok(report.clone());
        }
        fn visit(
            renderer: &PageRenderer,
            doc: &OfdDocument,
            items: &[Item],
            report: &mut FontRenderReport,
        ) {
            for item in items {
                match item {
                    Item::Group { items, .. } | Item::Clip { items, .. } => {
                        visit(renderer, doc, items, report)
                    }
                    Item::Text { chars, style, .. } => {
                        let embedded = doc
                            .resources
                            .fonts
                            .iter()
                            .any(|font| font.key() == style.font_key && font.embedded());
                        if !embedded {
                            continue;
                        }
                        let fallback_items = if let Some(converted) =
                            text_glyph_items(item, &renderer.glyph_fonts)
                        {
                            report.embedded_glyphs += converted
                                .iter()
                                .filter(|item| matches!(item, Item::Path { .. }))
                                .count();
                            converted
                                .into_iter()
                                .filter_map(|item| match item {
                                    Item::Text { chars, .. } => Some(chars),
                                    _ => None,
                                })
                                .flatten()
                                .collect::<Vec<_>>()
                        } else {
                            chars.clone()
                        };
                        let fallback = fallback_items
                            .iter()
                            .filter(|ch| !ch.ch.is_whitespace())
                            .count();
                        if fallback > 0 {
                            let codepoints = report
                                .fallback_codepoints
                                .entry(style.font_key.clone())
                                .or_default();
                            codepoints.extend(
                                fallback_items
                                    .iter()
                                    .filter(|ch| !ch.ch.is_whitespace())
                                    .map(|ch| ch.ch as u32),
                            );
                        }
                        report.fallback_chars += fallback;
                        if fallback > 0 && !report.replaced_fonts.contains(&style.font_key) {
                            report.replaced_fonts.push(style.font_key.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut report = FontRenderReport::default();
        visit(self, doc, &doc.display_list(index)?.items, &mut report);
        report.replaced_fonts.sort();
        let report = Arc::new(report);
        self.font_reports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index, report.clone());
        Ok(report)
    }

    /// Cached prepared page or SVG tree. The cache lock is not held while
    /// parsing, so jobs for other pages never wait; concurrent jobs for the
    /// same page (parallel tiles) wait for one build instead of repeating it.
    /// With `retain == false` a miss is built but not cached (thumbnails must
    /// not evict the pages being read).
    fn cached_page(&self, doc: &OfdDocument, index: usize, retain: bool) -> Result<CachedPage> {
        if let Some(page) = self.cache_lookup(index) {
            return Ok(page);
        }
        let gate = self
            .building
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(index)
            .or_default()
            .clone();
        let _building = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(page) = self.cache_lookup(index) {
            return Ok(page);
        }
        let page = self.build_page(doc, index);
        if retain && let Ok(page) = &page {
            let mut cache = self
                .tree_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !cache.iter().any(|(cached, _)| *cached == index) {
                cache.push_back((index, page.clone()));
            }
            while cache.len() > self.tree_cache_limit {
                cache.pop_front();
            }
        }
        self.building
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&index);
        page
    }

    fn cache_lookup(&self, index: usize) -> Option<CachedPage> {
        let mut cache = self
            .tree_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let position = cache.iter().position(|(cached, _)| *cached == index)?;
        let entry = cache.remove(position).expect("position came from iter");
        let page = entry.1.clone();
        cache.push_back(entry);
        Some(page)
    }

    fn build_page(&self, doc: &OfdDocument, index: usize) -> Result<CachedPage> {
        Ok(match self.backend {
            RasterBackend::Direct => {
                let list = doc.display_list(index)?;
                CachedPage::Prepared(Arc::new(raster::prepare(self, list.size, &list.items)))
            }
            RasterBackend::Svg => {
                let svg = self.render_svg(doc, index)?;
                let options = usvg::Options {
                    fontdb: self.fontdb.clone(),
                    font_size: 12.0,
                    ..Default::default()
                };
                CachedPage::Tree(Arc::new(
                    usvg::Tree::from_str(&svg, &options)
                        .map_err(|error| OfdError::Render(format!("usvg: {error}")))?,
                ))
            }
        })
    }

    fn rasterize_pixmap(
        &self,
        doc: &OfdDocument,
        index: usize,
        dpi: f32,
    ) -> Result<(resvg::tiny_skia::Pixmap, usize, usize)> {
        self.rasterize_pixmap_with(doc, index, dpi, true)
    }

    fn rasterize_pixmap_with(
        &self,
        doc: &OfdDocument,
        index: usize,
        dpi: f32,
        retain: bool,
    ) -> Result<(resvg::tiny_skia::Pixmap, usize, usize)> {
        if !dpi.is_finite() || dpi <= 0.0 || dpi > 2400.0 {
            return Err(OfdError::Render("无效的渲染密度".into()));
        }
        let page = self.cached_page(doc, index, retain)?;
        let scale = dpi / 96.0;
        let (width, height) = match &page {
            CachedPage::Tree(tree) => (
                (tree.size().width() * scale).ceil().max(1.0) as u32,
                (tree.size().height() * scale).ceil().max(1.0) as u32,
            ),
            CachedPage::Prepared(prepared) => prepared.pixel_size(dpi),
        };
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| OfdError::Render("无法创建像素缓冲区".to_string()))?;
        match &page {
            CachedPage::Tree(tree) => resvg::render(
                tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            ),
            CachedPage::Prepared(prepared) => {
                let mm = dpi / 25.4;
                prepared.render(
                    resvg::tiny_skia::Transform::from_scale(mm, mm),
                    &mut pixmap.as_mut(),
                );
            }
        }
        Ok((pixmap, width as usize, height as usize))
    }

    /// Rasterizes straight into BGRA for direct display, skipping PNG.
    pub fn rasterize(&self, doc: &OfdDocument, index: usize, dpi: f32) -> Result<RasterPage> {
        let (pixmap, _, _) = self.rasterize_pixmap(doc, index, dpi)?;
        Ok(pixmap_to_raster(pixmap))
    }

    /// Like [`Self::rasterize`], for background previews: reuses a cached
    /// page, but a page built here is dropped afterwards instead of
    /// evicting the recently read pages from the cache.
    pub fn rasterize_preview(
        &self,
        doc: &OfdDocument,
        index: usize,
        dpi: f32,
    ) -> Result<RasterPage> {
        let (pixmap, _, _) = self.rasterize_pixmap_with(doc, index, dpi, false)?;
        Ok(pixmap_to_raster(pixmap))
    }

    /// Render only a device-pixel region, without allocating a whole-page
    /// pixmap. The same cached page and global pixel origin are used, so
    /// glyphs, patterns and images retain their page coordinates. The direct
    /// backend only paints operations whose bounds reach the region.
    pub fn rasterize_region(
        &self,
        doc: &OfdDocument,
        index: usize,
        dpi: f32,
        region: RasterRegion,
    ) -> Result<RasterPage> {
        if !dpi.is_finite()
            || dpi <= 0.0
            || dpi > 2400.0
            || region.width == 0
            || region.height == 0
            || region.width > 4096
            || region.height > 4096
            || region.x.checked_add(region.width).is_none()
            || region.y.checked_add(region.height).is_none()
        {
            return Err(OfdError::Render("无效或过大的分块渲染区域".into()));
        }
        let page = self.cached_page(doc, index, true)?;
        // Keep rasterizer clipping away from the returned pixels. This is
        // separate from the small texture sampling gutter used by the UI.
        let pad = 16;
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(region.width + pad * 2, region.height + pad * 2)
                .ok_or_else(|| OfdError::Render("无法创建分块像素缓冲区".into()))?;
        let dx = pad as f32 - region.x as f32;
        let dy = pad as f32 - region.y as f32;
        match &page {
            CachedPage::Tree(tree) => {
                let scale = dpi / 96.0;
                resvg::render(
                    tree,
                    resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
                    &mut pixmap.as_mut(),
                );
            }
            CachedPage::Prepared(prepared) => {
                let mm = dpi / 25.4;
                prepared.render(
                    resvg::tiny_skia::Transform::from_row(mm, 0.0, 0.0, mm, dx, dy),
                    &mut pixmap.as_mut(),
                );
            }
        }
        let rect = resvg::tiny_skia::IntRect::from_xywh(
            pad as i32,
            pad as i32,
            region.width,
            region.height,
        )
        .unwrap();
        let crop = pixmap
            .clone_rect(rect)
            .ok_or_else(|| OfdError::Render("无法裁剪分块缓冲区".into()))?;
        Ok(pixmap_to_raster(crop))
    }

    pub fn render_png(&self, doc: &OfdDocument, index: usize, dpi: f32) -> Result<RenderedPage> {
        let (pixmap, width, height) = self.rasterize_pixmap(doc, index, dpi)?;
        let png = pixmap
            .encode_png()
            .map_err(|error| OfdError::Render(format!("PNG 编码失败：{error}")))?;
        Ok(RenderedPage {
            png,
            width: width as u32,
            height: height as u32,
        })
    }
}

fn pixmap_to_raster(pixmap: resvg::tiny_skia::Pixmap) -> RasterPage {
    let (width, height) = (pixmap.width(), pixmap.height());
    let mut bgra = pixmap.take();
    for pixel in bgra.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha == 255 {
            // Page pixels are almost all opaque; skip the division.
            pixel.swap(0, 2);
        } else if alpha == 0 {
            pixel.copy_from_slice(&[0, 0, 0, 0]);
        } else {
            pixel.swap(0, 2);
            let round = alpha / 2;
            pixel[0] = ((pixel[0] as u32 * 255 + round) / alpha) as u8;
            pixel[1] = ((pixel[1] as u32 * 255 + round) / alpha) as u8;
            pixel[2] = ((pixel[2] as u32 * 255 + round) / alpha) as u8;
        }
    }
    RasterPage {
        bgra,
        width,
        height,
    }
}

fn walk_block(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    items: &mut Vec<Item>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) {
    for child_node in node.children().filter(|node| node.is_element()) {
        let name = child_node.tag_name().name();
        let mut style = inherited.clone();
        if let Some(id) = child_node.attribute("DrawParam")
            && let Some(param) = doc.resources.draw_param(id)
        {
            style.apply(&param);
        }

        let start = items.len();
        match name {
            "Layer" | "PageBlock" => {
                walk_block(doc, child_node, items, &style, warnings);
            }
            "TextObject" => {
                if let Some(item) = parse_text_object(doc, child_node, &style) {
                    items.push(item);
                }
            }
            "PathObject" => {
                if let Some(expanded) = expand_pattern_fill(doc, child_node, &style, warnings) {
                    items.extend(expanded);
                } else if let Some(item) = parse_path_object(doc, child_node, &style, warnings) {
                    items.push(item);
                }
            }
            "ImageObject" => {
                if let Some(item) = parse_image_object(doc, child_node, warnings) {
                    items.push(item);
                }
            }
            "CompositeObject" => {
                append_composite_unit(doc, child_node, &style, items, warnings);
            }
            _ => {}
        }

        if name != "Layer"
            && name != "PageBlock"
            && items.len() > start
            && let Some(regions) = parse_clips(child_node, warnings)
        {
            let clipped = items.split_off(start);
            items.push(Item::Clip {
                regions,
                items: clipped,
            });
        }
    }
}

/// Object `Clips` as parent-space regions.
///
/// Clip geometry lives in the object's Boundary space:
/// `T(Boundary) · Area.CTM · T(Path.Boundary) · Path.CTM`. With
/// `TransFlag="true"` it is also transformed by the object CTM (producers
/// then write e.g. a unit square for an image). Each `Clip` is one region
/// (its `Area`s unite); regions intersect. Text areas are not supported and
/// leave their region out, which never hides content.
fn parse_clips(
    node: roxmltree::Node<'_, '_>,
    warnings: &mut Vec<String>,
) -> Option<Vec<Vec<ClipShape>>> {
    let clips = child(node, "Clips")?;
    let (x, y) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .map(|(x, y, _, _)| (x, y))
        .unwrap_or((0.0, 0.0));
    let mut origin = Matrix::translate(x, y);
    if clips
        .attribute("TransFlag")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        && let Some(ctm) = node.attribute("CTM").and_then(Matrix::parse)
    {
        origin = origin.compose(ctm);
    }
    let mut regions = Vec::new();
    for clip in children(clips, "Clip") {
        let mut region = Vec::new();
        let mut complete = true;
        for area in children(clip, "Area") {
            let area_ctm = area
                .attribute("CTM")
                .and_then(Matrix::parse)
                .unwrap_or(Matrix::IDENTITY);
            let Some(path) = child(area, "Path") else {
                complete = false;
                continue;
            };
            let Some(cmds) = child_text(path, "AbbreviatedData")
                .and_then(|data| parse_path_data(data, warnings))
                .filter(|cmds| !cmds.is_empty())
            else {
                complete = false;
                continue;
            };
            let (path_x, path_y) = path
                .attribute("Boundary")
                .and_then(crate::model::parse_box)
                .map(|(x, y, _, _)| (x, y))
                .unwrap_or((0.0, 0.0));
            let path_ctm = path
                .attribute("CTM")
                .and_then(Matrix::parse)
                .unwrap_or(Matrix::IDENTITY);
            region.push(ClipShape {
                transform: origin
                    .compose(area_ctm)
                    .compose(Matrix::translate(path_x, path_y))
                    .compose(path_ctm),
                cmds,
                even_odd: path
                    .attribute("Rule")
                    .is_some_and(|value| value.eq_ignore_ascii_case("Even-Odd")),
            });
        }
        if !complete {
            warnings.push("裁剪区含不支持的文字或无效路径，已忽略该裁剪区".to_string());
        } else if !region.is_empty() {
            regions.push(region);
        }
    }
    (!regions.is_empty()).then_some(regions)
}

/// `AxialShd` / `RadialShd` inside a `FillColor` or `StrokeColor`.
fn parse_shading(color: roxmltree::Node<'_, '_>) -> Option<Shading> {
    let node = child(color, "AxialShd").or_else(|| child(color, "RadialShd"))?;
    let point = |name: &str| -> Option<(f64, f64)> {
        let mut values = node.attribute(name)?.split_whitespace();
        Some((values.next()?.parse().ok()?, values.next()?.parse().ok()?))
    };
    let kind = if node.tag_name().name() == "AxialShd" {
        ShadingKind::Axial {
            start: point("StartPoint")?,
            end: point("EndPoint")?,
        }
    } else {
        ShadingKind::Radial {
            start: point("StartPoint")?,
            start_radius: attribute_f64(node, "StartRadius").unwrap_or(0.0).max(0.0),
            end: point("EndPoint")?,
            radius: attribute_f64(node, "EndRadius")?,
        }
    };
    let segments: Vec<_> = children(node, "Segment").collect();
    let count = segments.len();
    let mut stops = Vec::with_capacity(count);
    for (index, segment) in segments.into_iter().enumerate() {
        let Some(color) = child(segment, "Color") else {
            continue;
        };
        let Some(value) = color.attribute("Value").and_then(|value| {
            Color::parse(
                value,
                color
                    .attribute("Alpha")
                    .and_then(|alpha| alpha.parse::<u8>().ok())
                    .unwrap_or(255),
            )
        }) else {
            continue;
        };
        let default = if count > 1 {
            index as f64 / (count - 1) as f64
        } else {
            0.0
        };
        let position = attribute_f64(segment, "Position")
            .filter(|position| position.is_finite())
            .unwrap_or(default)
            .clamp(0.0, 1.0);
        stops.push((position, value));
    }
    if stops.is_empty() {
        return None;
    }
    // Keep document order for equal positions (hard colour steps).
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let extend = node
        .attribute("Extend")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let spread = match node.attribute("MapType") {
        Some(value) if value.eq_ignore_ascii_case("Repeat") => ShadingSpread::Repeat,
        Some(value) if value.eq_ignore_ascii_case("Reflect") => ShadingSpread::Reflect,
        _ => ShadingSpread::Pad,
    };
    Some(Shading {
        kind,
        stops,
        extend_start: extend & 1 != 0,
        extend_end: extend & 2 != 0,
        spread,
    })
}

/// `CompositeObject` references a reusable `CompositeGraphicUnit` resource.
/// The unit content is stored as a standalone XML fragment; parse it here and
/// wrap the resulting items in the object's Boundary + CTM.
fn append_composite_unit(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    items: &mut Vec<Item>,
    warnings: &mut Vec<String>,
) {
    let Some(resource_id) = node.attribute("ResourceID") else {
        return;
    };
    let Some(unit) = doc.resources.composite_unit(resource_id) else {
        warnings.push(format!("复合图形资源 {resource_id} 不存在"));
        return;
    };
    let document = match parse_xml(unit.content.as_bytes(), "CompositeGraphicUnit") {
        Ok(document) => document,
        Err(error) => {
            warnings.push(format!("复合图形资源 {} 解析失败：{error}", unit.id));
            return;
        }
    };
    let root = document.root_element();
    let unit_root = if root.tag_name().name() == "CompositeGraphicUnit" {
        root
    } else {
        root.descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "CompositeGraphicUnit")
            .unwrap_or(root)
    };

    let mut unit_items = Vec::new();
    let mut found_content = false;
    for content in unit_root
        .children()
        .filter(|child| child.is_element() && child.tag_name().name() == "Content")
    {
        found_content = true;
        walk_block(doc, content, &mut unit_items, inherited, warnings);
    }
    if !found_content {
        walk_block(doc, unit_root, &mut unit_items, inherited, warnings);
    }
    if !unit_items.is_empty() {
        let opacity = node
            .attribute("Alpha")
            .and_then(|value| value.parse::<u16>().ok())
            .map(|alpha| alpha.min(255) as f64 / 255.0)
            .unwrap_or(1.0);
        items.push(Item::Group {
            transform: object_transform(node),
            items: unit_items,
            opacity,
        });
    }
}

fn object_transform(node: roxmltree::Node<'_, '_>) -> Matrix {
    let (x, y) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .map(|(x, y, _, _)| (x, y))
        .unwrap_or((0.0, 0.0));
    let ctm = node
        .attribute("CTM")
        .and_then(Matrix::parse)
        .unwrap_or(Matrix::IDENTITY);
    Matrix::translate(x, y).compose(ctm)
}

/// `CGTransform` maps text-code positions to glyph IDs. A `TextObject` may
/// contain several transforms, and positions count UTF-16 code units, so a
/// surrogate pair occupies two positions but produces one glyph. Build a list
/// aligned with our Rust `char` vector; an incomplete mapping falls back to the
/// Unicode cmap (or to the system font).
fn positioned_glyphs(node: roxmltree::Node<'_, '_>, chars: &[PositionedChar]) -> Vec<u16> {
    let clusters = glyph_clusters(node, chars);
    let mut result = vec![None; chars.len()];
    for cluster in clusters {
        if cluster.end - cluster.start != cluster.glyphs.len() {
            return Vec::new();
        }
        for (slot, id) in result[cluster.start..cluster.end]
            .iter_mut()
            .zip(cluster.glyphs)
        {
            *slot = Some(id);
        }
    }
    result
        .into_iter()
        .zip(chars)
        .map(|(id, ch)| id.or_else(|| ch.ch.is_whitespace().then_some(0)))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

fn glyph_clusters(node: roxmltree::Node<'_, '_>, chars: &[PositionedChar]) -> Vec<GlyphCluster> {
    let transforms: Vec<_> = children(node, "CGTransform").collect();
    if transforms.is_empty() {
        return Vec::new();
    }

    let mut unit_starts = Vec::with_capacity(chars.len());
    let mut unit_count = 0usize;
    for positioned in chars {
        unit_starts.push(unit_count);
        unit_count += positioned.ch.len_utf16();
    }
    let mut occupied = vec![false; chars.len()];
    let mut clusters = Vec::new();
    let mut cursor = 0usize;

    for transform in transforms {
        let glyphs: Vec<u16> = transform
            .attribute("Glyphs")
            .map(str::to_string)
            .or_else(|| child_text(transform, "Glyphs").map(str::to_string))
            .unwrap_or_default()
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<Vec<u16>, _>>()
            .unwrap_or_default();
        if glyphs.is_empty() {
            continue;
        }
        let code_position = transform
            .attribute("CodePosition")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(cursor);
        let code_count = transform
            .attribute("CodeCount")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let glyph_count = transform
            .attribute("GlyphCount")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let Some(code_end) = code_position.checked_add(code_count) else {
            return Vec::new();
        };
        if code_count == 0
            || glyph_count == 0
            || glyph_count != glyphs.len()
            || code_end > unit_count
        {
            continue;
        }
        let Ok(start_char) = unit_starts.binary_search(&code_position) else {
            return Vec::new();
        };
        let end_char = if code_end == unit_count {
            chars.len()
        } else if let Ok(index) = unit_starts.binary_search(&code_end) {
            index
        } else {
            return Vec::new();
        };
        if occupied[start_char..end_char].iter().any(|value| *value) {
            continue;
        }
        occupied[start_char..end_char].fill(true);
        if end_char - start_char == glyphs.len() {
            for (index, glyph) in (start_char..end_char).zip(glyphs) {
                clusters.push(GlyphCluster {
                    start: index,
                    end: index + 1,
                    glyphs: vec![glyph],
                    positions: Vec::new(),
                });
            }
        } else {
            clusters.push(GlyphCluster {
                start: start_char,
                end: end_char,
                glyphs,
                positions: Vec::new(),
            });
        }
        cursor = code_end;
    }

    clusters.sort_by_key(|cluster| cluster.start);
    clusters
}

fn text_direction(value: Option<&str>) -> u16 {
    match value.and_then(|v| v.parse().ok()) {
        Some(90) => 90,
        Some(180) => 180,
        Some(270) => 270,
        _ => 0,
    }
}

/// OFD permits UTF-16 code units escaped as a backslash plus four hex digits.
fn decode_text_code(text: &str) -> String {
    let mut units = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let mut lookahead = chars.clone();
            let digits: String = lookahead.by_ref().take(4).collect();
            if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                units.push(u16::from_str_radix(&digits, 16).expect("four hex digits"));
                chars = lookahead;
                continue;
            }
        }
        units.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
    }
    String::from_utf16_lossy(&units)
}

fn parse_text_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
) -> Option<Item> {
    // Text inherits FillColor/Alpha from its layer or PageBlock DrawParam, just
    // like paths. Object-level DrawParam still wins over the inherited style.
    let mut inherited_style = inherited.clone();
    if let Some(id) = node.attribute("DrawParam")
        && let Some(param) = doc.resources.draw_param(id)
    {
        inherited_style.apply(&param);
    }

    let size = node
        .attribute("Size")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(3.175);
    let font_id = node.attribute("Font").unwrap_or("");
    let font = doc.resources.font(font_id);
    let font_key = font.map(|font| font.key()).unwrap_or_default();
    let family = font
        .map(|font| font.css_family().to_string())
        .unwrap_or_else(|| "sans-serif".to_string());
    let color = child(node, "FillColor")
        .and_then(|color| {
            let value = color.attribute("Value")?;
            let alpha = color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255);
            Color::parse(value, alpha)
        })
        .or(inherited_style.fill_color)
        .unwrap_or(Color {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        });

    let object_opacity = node
        .attribute("Alpha")
        .and_then(|value| value.parse::<u16>().ok())
        .map(|alpha| alpha.min(255) as f64 / 255.0)
        .unwrap_or(1.0);

    let weight = node
        .attribute("Weight")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(if font.is_some_and(|font| font.bold) {
            700
        } else {
            400
        });
    let italic = node
        .attribute("Italic")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(font.is_some_and(|font| font.italic));

    let mut paint = inherited_style.clone();
    paint.fill = node
        .attribute("Fill")
        .is_none_or(|v| v != "false" && v != "0");
    paint.stroke = node
        .attribute("Stroke")
        .is_some_and(|v| v == "true" || v == "1");
    paint.fill_color = Some(color);
    if let Some(stroke) = child(node, "StrokeColor") {
        paint.stroke_color = stroke.attribute("Value").and_then(|value| {
            Color::parse(
                value,
                stroke
                    .attribute("Alpha")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(255),
            )
        });
    }
    if let Some(width) = node
        .attribute("LineWidth")
        .and_then(|v| v.parse::<f64>().ok())
    {
        paint.line_width = width;
    }
    paint.opacity = object_opacity * inherited_style.opacity;
    let mut style = TextStyle {
        font_key,
        font_family: family,
        font_size: size,
        color,
        weight,
        italic,
        opacity: object_opacity * inherited_style.opacity,
        h_scale: node
            .attribute("HScale")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0),
        char_direction: text_direction(node.attribute("CharDirection")),
        read_direction: text_direction(node.attribute("ReadDirection")),
        paint,
        clusters: Vec::new(),
    };

    let mut chars = Vec::new();
    let mut previous_start = (0.0, 0.0);
    for code in children(node, "TextCode") {
        let text = decode_text_code(code.text().unwrap_or(""));
        let start_x = code
            .attribute("X")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(previous_start.0);
        let baseline = code
            .attribute("Y")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(previous_start.1);
        previous_start = (start_x, baseline);
        let deltas = code
            .attribute("DeltaX")
            .map(parse_delta_list)
            .unwrap_or_default();
        let delta_y = code
            .attribute("DeltaY")
            .map(parse_delta_list)
            .unwrap_or_default();
        let has_delta_x = code.attribute("DeltaX").is_some();
        let vertical = !delta_y.is_empty() && !has_delta_x;

        let mut x = start_x;
        let mut y = baseline;
        for (index, ch) in text.chars().enumerate() {
            if index > 0 {
                let advance = default_advance(ch, size) * style.h_scale;
                let (default_x, default_y) = match style.read_direction {
                    90 => (0.0, advance),
                    180 => (-advance, 0.0),
                    270 => (0.0, -advance),
                    _ if vertical => (0.0, 0.0),
                    _ => (advance, 0.0),
                };
                let advance_x = deltas.get(index - 1).copied().unwrap_or(default_x);
                x += advance_x;
                y += delta_y.get(index - 1).copied().unwrap_or(default_y);
            }
            chars.push(PositionedChar { ch, x, y });
        }
    }

    if chars.is_empty() {
        return None;
    }

    let glyphs = positioned_glyphs(node, &chars);
    style.clusters = glyph_clusters(node, &chars);
    position_glyph_clusters(doc, font, node, &mut chars, &mut style);

    Some(Item::Text {
        transform: object_transform(node),
        chars,
        glyphs,
        style,
    })
}

/// DeltaX/DeltaY address the rendered glyph stream, which may be longer or
/// shorter than the source character stream after CGTransform. Preserve the
/// source characters for copying while positioning the glyphs separately.
fn position_glyph_clusters(
    doc: &OfdDocument,
    font: Option<&FontResource>,
    node: roxmltree::Node<'_, '_>,
    chars: &mut [PositionedChar],
    style: &mut TextStyle,
) {
    let data = font
        .and_then(|font| font.file.as_ref())
        .and_then(|file| doc.package.read_shared(file).ok());
    let face = data
        .as_ref()
        .and_then(|data| EmbeddedFace::parse(data, font.map_or(0, |font| select_face(data, font))));
    let mut source_start = 0;
    let mut previous_start = (0.0, 0.0);
    for code in children(node, "TextCode") {
        let count = decode_text_code(code.text().unwrap_or("")).chars().count();
        let source_end = (source_start + count).min(chars.len());
        let start = (
            code.attribute("X")
                .and_then(|v| v.parse().ok())
                .unwrap_or(previous_start.0),
            code.attribute("Y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(previous_start.1),
        );
        previous_start = start;
        let dx = code
            .attribute("DeltaX")
            .map(parse_delta_list)
            .unwrap_or_default();
        let dy = code
            .attribute("DeltaY")
            .map(parse_delta_list)
            .unwrap_or_default();
        let vertical = !dy.is_empty() && code.attribute("DeltaX").is_none();
        let mut position = start;
        let mut glyph_index = 0usize;
        let mut previous_advance = 0.0;
        let mut index = source_start;
        while index < source_end {
            let cluster = style
                .clusters
                .iter_mut()
                .find(|cluster| cluster.start == index && cluster.end <= source_end);
            let end = cluster.as_ref().map_or(index + 1, |cluster| cluster.end);
            let ids = cluster.as_ref().map_or_else(
                || vec![face.as_ref().and_then(|face| face.glyph(chars[index].ch))],
                |cluster| {
                    cluster
                        .glyphs
                        .iter()
                        .map(|id| face.as_ref().and_then(|face| face.explicit_glyph(*id)))
                        .collect()
                },
            );
            let mut positions = Vec::with_capacity(ids.len());
            for id in ids {
                if glyph_index > 0 {
                    let default = match style.read_direction {
                        90 => (0.0, previous_advance),
                        180 => (-previous_advance, 0.0),
                        270 => (0.0, -previous_advance),
                        _ if vertical => (0.0, 0.0),
                        _ => (previous_advance, 0.0),
                    };
                    position.0 += dx.get(glyph_index - 1).copied().unwrap_or(default.0);
                    position.1 += dy.get(glyph_index - 1).copied().unwrap_or(default.1);
                }
                positions.push(position);
                previous_advance = if vertical || matches!(style.read_direction, 90 | 270) {
                    style.font_size
                } else {
                    id.and_then(|id| {
                        face.as_ref()
                            .map(|face| face.advance(id) * style.font_size * style.h_scale)
                    })
                    .filter(|advance| advance.is_finite() && *advance > 0.0)
                    .unwrap_or_else(|| {
                        default_advance(chars[index].ch, style.font_size) * style.h_scale
                    })
                };
                glyph_index += 1;
            }
            for (offset, ch) in chars[index..end].iter_mut().enumerate() {
                let pos = positions.get(offset).copied().unwrap_or(positions[0]);
                ch.x = pos.0;
                ch.y = pos.1;
            }
            if let Some(cluster) = cluster {
                cluster.positions = positions;
            }
            index = end;
        }
        source_start = source_end;
    }
}

fn collect_text_matches(items: &[Item], query: &str, out: &mut Vec<TextMatchRect>) {
    for item in items {
        match item {
            Item::Text {
                transform,
                chars,
                style,
                ..
            } => {
                let text: String = chars.iter().map(|char| char.ch).collect();
                let mut from = 0;
                while let Some(offset) = text[from..].find(query) {
                    let start = from + offset;
                    let end = start + query.len();
                    let char_start = text[..start].chars().count();
                    let char_end = text[..end].chars().count();
                    if char_start < char_end
                        && char_end <= chars.len()
                        && let Some(rect) =
                            text_run_rect(transform, chars, char_start, char_end, style.font_size)
                    {
                        out.push(rect);
                    }
                    from = end;
                }
            }
            Item::Group { items, .. } | Item::Clip { items, .. } => {
                collect_text_matches(items, query, out)
            }
            _ => {}
        }
    }
}

fn text_run_rect(
    transform: &Matrix,
    chars: &[PositionedChar],
    start: usize,
    end: usize,
    font_size: f64,
) -> Option<TextMatchRect> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for index in start..end {
        let positioned = &chars[index];
        let advance = chars
            .get(index + 1)
            .map(|next| next.x - positioned.x)
            .filter(|advance| *advance > 0.0)
            .unwrap_or_else(|| default_advance(positioned.ch, font_size));
        let top = positioned.y - font_size * 0.88;
        let bottom = positioned.y + font_size * 0.12;
        for (x, y) in [
            (positioned.x, top),
            (positioned.x + advance, top),
            (positioned.x + advance, bottom),
            (positioned.x, bottom),
        ] {
            let (page_x, page_y) = transform.transform_point(x, y);
            min_x = min_x.min(page_x);
            min_y = min_y.min(page_y);
            max_x = max_x.max(page_x);
            max_y = max_y.max(page_y);
        }
    }

    let width = max_x - min_x;
    let height = max_y - min_y;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(TextMatchRect {
        x: min_x,
        y: min_y,
        width,
        height,
    })
}

/// OFD allows `g N X` to repeat the advance `X` for `N` following glyphs.
/// Producers use it heavily for long runs of fixed-pitch text.
fn parse_delta_list(value: &str) -> Vec<f64> {
    let mut tokens = value.split_whitespace().peekable();
    let mut deltas = Vec::new();
    while let Some(token) = tokens.next() {
        if token == "g" {
            let Some(count) = tokens.next().and_then(|value| value.parse::<usize>().ok()) else {
                continue;
            };
            let Some(advance) = tokens.next().and_then(|value| value.parse::<f64>().ok()) else {
                continue;
            };
            deltas.extend(std::iter::repeat_n(advance, count.min(10_000)));
            continue;
        }
        if let Ok(advance) = token.trim_start_matches('g').parse::<f64>() {
            deltas.push(advance);
        }
    }
    deltas
}

fn default_advance(ch: char, size: f64) -> f64 {
    if ch == ' ' {
        size * 0.5
    } else if ch.is_ascii() {
        size * 0.55
    } else {
        size
    }
}

/// OFD paints a pattern-filled path by tiling `CellContent`. A solid fill
/// fallback would paint the whole page black (the watermark test does exactly
/// that), so expand the tiles into groups instead.
fn expand_pattern_fill(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) -> Option<Vec<Item>> {
    let fill = child(node, "FillColor")?;
    let pattern = child(fill, "Pattern")?;
    let cell = child(pattern, "CellContent")?;
    let width = attribute_f64(pattern, "Width")?;
    let height = attribute_f64(pattern, "Height")?;
    let x_step = attribute_f64(pattern, "XStep").unwrap_or(width);
    let y_step = attribute_f64(pattern, "YStep").unwrap_or(height);
    if width <= 0.0 || height <= 0.0 || x_step <= 1e-6 || y_step <= 1e-6 {
        return None;
    }
    let ctm = pattern
        .attribute("CTM")
        .and_then(Matrix::parse)
        .unwrap_or(Matrix::IDENTITY);
    let inverse = ctm.invert()?;

    let (x, y, box_width, box_height) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .unwrap_or((0.0, 0.0, 210.0, 297.0));

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (corner_x, corner_y) in [
        (x, y),
        (x + box_width, y),
        (x + box_width, y + box_height),
        (x, y + box_height),
    ] {
        let (pattern_x, pattern_y) = inverse.transform_point(corner_x, corner_y);
        min_x = min_x.min(pattern_x);
        min_y = min_y.min(pattern_y);
        max_x = max_x.max(pattern_x);
        max_y = max_y.max(pattern_y);
    }

    let mut cell_items = Vec::new();
    walk_block(doc, cell, &mut cell_items, inherited, warnings);
    if cell_items.is_empty() {
        return None;
    }

    let start_column = (min_x / x_step).floor() as i64 - 1;
    let end_column = (max_x / x_step).ceil() as i64 + 1;
    let start_row = (min_y / y_step).floor() as i64 - 1;
    let end_row = (max_y / y_step).ceil() as i64 + 1;
    let columns = (end_column - start_column + 1).max(0) as usize;
    let rows = (end_row - start_row + 1).max(0) as usize;
    if columns.saturating_mul(rows) > 4096 {
        warnings.push("图案平铺数量过多，已跳过".to_string());
        return None;
    }

    let mut items = Vec::with_capacity(columns * rows);
    for row in start_row..=end_row {
        for column in start_column..=end_column {
            let tile = ctm.compose(Matrix::translate(
                column as f64 * x_step,
                row as f64 * y_step,
            ));
            items.push(Item::Group {
                transform: tile,
                items: cell_items.clone(),
                opacity: 1.0,
            });
        }
    }
    Some(items)
}

fn attribute_f64(node: roxmltree::Node<'_, '_>, name: &str) -> Option<f64> {
    node.attribute(name).and_then(|value| value.parse().ok())
}

fn parse_path_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    inherited: &PathStyle,
    warnings: &mut Vec<String>,
) -> Option<Item> {
    let mut style = inherited.clone();
    if let Some(id) = node.attribute("DrawParam")
        && let Some(param) = doc.resources.draw_param(id)
    {
        style.apply(&param);
    }

    // OFD 8.2 表 41: a path is stroked by default and is **not** filled.
    // FillColor alone (including a colour inherited from a layer DrawParam)
    // must not turn on filling; text objects have the opposite defaults.
    style.fill = node
        .attribute("Fill")
        .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
    style.stroke = node
        .attribute("Stroke")
        .is_none_or(|value| !(value.eq_ignore_ascii_case("false") || value == "0"));
    if let Some(width) = node
        .attribute("LineWidth")
        .and_then(|value| value.parse::<f64>().ok())
    {
        style.line_width = width;
    }
    if let Some(cap) = node.attribute("Cap") {
        style.cap = cap.to_string();
    }
    if let Some(join) = node.attribute("Join") {
        style.join = join.to_string();
    }
    if let Some(dash) = node.attribute("DashPattern") {
        style.dash = dash
            .split_whitespace()
            .filter_map(|part| part.parse().ok())
            .collect();
    }
    if node
        .attribute("Rule")
        .is_some_and(|value| value.eq_ignore_ascii_case("Even-Odd"))
    {
        style.even_odd = true;
    }
    if let Some(color) = child(node, "FillColor")
        && let Some(value) = color.attribute("Value")
    {
        style.fill_color = Color::parse(
            value,
            color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255),
        );
    }
    style.fill_shading = child(node, "FillColor").and_then(parse_shading);
    style.stroke_shading = child(node, "StrokeColor").and_then(parse_shading);
    if let Some(color) = child(node, "StrokeColor")
        && let Some(value) = color.attribute("Value")
    {
        style.stroke_color = Color::parse(
            value,
            color
                .attribute("Alpha")
                .and_then(|value| value.parse::<u8>().ok())
                .unwrap_or(255),
        );
    }
    if let Some(alpha) = node
        .attribute("Alpha")
        .and_then(|value| value.parse::<u16>().ok())
    {
        style.opacity = alpha.min(255) as f64 / 255.0;
    }

    let data = child_text(node, "AbbreviatedData")?;
    let cmds = parse_path_data(data, warnings)?;
    if cmds.is_empty() {
        return None;
    }

    Some(Item::Path {
        transform: object_transform(node),
        cmds,
        style,
    })
}

fn parse_path_data(data: &str, warnings: &mut Vec<String>) -> Option<Vec<PathCmd>> {
    let mut tokens = data.split_whitespace();
    let mut cmds = Vec::new();

    while let Some(token) = tokens.next() {
        let mut number = |what: &str| -> Option<f64> {
            let value = tokens.next()?;
            match value.parse::<f64>() {
                Ok(value) => Some(value),
                Err(_) => {
                    warnings.push(format!("路径 {what} 参数非法：{value}"));
                    None
                }
            }
        };

        match token {
            "S" | "M" => {
                let x = number("M")?;
                let y = number("M")?;
                cmds.push(PathCmd::Move(x, y));
            }
            "L" => {
                let x = number("L")?;
                let y = number("L")?;
                cmds.push(PathCmd::Line(x, y));
            }
            "Q" => {
                let x1 = number("Q")?;
                let y1 = number("Q")?;
                let x2 = number("Q")?;
                let y2 = number("Q")?;
                cmds.push(PathCmd::Quad(x1, y1, x2, y2));
            }
            "B" => {
                let x1 = number("B")?;
                let y1 = number("B")?;
                let x2 = number("B")?;
                let y2 = number("B")?;
                let x3 = number("B")?;
                let y3 = number("B")?;
                cmds.push(PathCmd::Cubic(x1, y1, x2, y2, x3, y3));
            }
            "A" => {
                let rx = number("A")?;
                let ry = number("A")?;
                let rotation = number("A")?;
                let large = number("A")? != 0.0;
                let sweep = number("A")? != 0.0;
                let x = number("A")?;
                let y = number("A")?;
                cmds.push(PathCmd::Arc {
                    rx,
                    ry,
                    rotation,
                    large,
                    sweep,
                    x,
                    y,
                });
            }
            "C" => cmds.push(PathCmd::Close),
            other => {
                warnings.push(format!("不支持的路径操作符：{other}"));
                return None;
            }
        }
    }

    Some(cmds)
}

fn parse_image_object(
    doc: &OfdDocument,
    node: roxmltree::Node<'_, '_>,
    warnings: &mut Vec<String>,
) -> Option<Item> {
    let resource_id = node.attribute("ResourceID")?;
    let Some(image) = doc.resources.image(resource_id) else {
        warnings.push(format!("图像资源未声明：{resource_id}"));
        return None;
    };
    let Some(file) = &image.file else {
        return None;
    };
    let data = match doc.package.read(file) {
        Ok(data) => data,
        Err(error) => {
            warnings.push(format!("图像读取失败 {file}：{error}"));
            return None;
        }
    };

    let (_, _, width, height) = node
        .attribute("Boundary")
        .and_then(crate::model::parse_box)
        .unwrap_or((0.0, 0.0, 1.0, 1.0));
    let ctm = node.attribute("CTM").and_then(Matrix::parse);
    let has_ctm = ctm.is_some_and(|matrix| !matrix.is_identity());
    let (local_width, local_height) = if has_ctm { (1.0, 1.0) } else { (width, height) };

    let mime = match image.format.as_deref() {
        Some(format) if format.eq_ignore_ascii_case("JPEG") => "image/jpeg",
        Some(format) if format.eq_ignore_ascii_case("PNG") => "image/png",
        _ => detect_image_mime(&data),
    };

    Some(Item::Image {
        transform: object_transform(node),
        width: local_width,
        height: local_height,
        mime,
        data,
        blend: node.attribute("BlendMode").map(str::to_string),
        clip: None,
    })
}

fn blend_mode(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "darken" => Some("darken"),
        "multiply" => Some("multiply"),
        "screen" => Some("screen"),
        "overlay" => Some("overlay"),
        "lighten" => Some("lighten"),
        _ => None,
    }
}

fn detect_image_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"BM") {
        "image/bmp"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        "image/tiff"
    } else {
        "image/png"
    }
}

#[derive(Default)]
struct GlyphPathBuilder {
    cmds: Vec<PathCmd>,
}

impl ttf_parser::OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::Move(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(PathCmd::Line(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.cmds
            .push(PathCmd::Quad(x1 as f64, y1 as f64, x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cmds.push(PathCmd::Cubic(
            x1 as f64, y1 as f64, x2 as f64, y2 as f64, x as f64, y as f64,
        ));
    }
    fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }
}

/// Draw embedded outlines using explicit GIDs first, otherwise Unicode cmap.
/// Missing outlines fall back per character, not for the entire text object.
fn text_glyph_items(item: &Item, fonts: &HashMap<String, GlyphFont>) -> Option<Vec<Item>> {
    let Item::Text {
        transform,
        chars,
        glyphs,
        style,
    } = item
    else {
        return None;
    };
    if chars.is_empty() {
        return None;
    }
    let font = fonts.get(&style.font_key)?;
    let face = EmbeddedFace::parse(&font.data, font.face_index)?;

    let mut items = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let cluster = style.clusters.iter().find(|cluster| cluster.start == index);
        let end = cluster.map_or(index + 1, |cluster| cluster.end);
        let ids: Vec<Option<u16>> = if let Some(cluster) = cluster {
            cluster
                .glyphs
                .iter()
                .map(|id| face.explicit_glyph(*id))
                .collect()
        } else if glyphs.len() == chars.len() {
            vec![face.explicit_glyph(glyphs[index])]
        } else {
            vec![face.glyph(chars[index].ch)]
        };
        let mut cluster_items = Vec::new();
        let mut advance = 0.0;
        let mut failed = false;
        for (glyph_index, glyph_id) in ids.iter().enumerate() {
            let positioned = if ids.len() == end - index {
                &chars[index + glyph_index]
            } else {
                &chars[index]
            };
            let mut builder = GlyphPathBuilder::default();
            let outlined = glyph_id
                .filter(|id| *id != 0)
                .is_some_and(|id| face.outline(id, &mut builder));
            if !outlined || builder.cmds.is_empty() {
                if end == index + 1 && positioned.ch.is_whitespace() {
                    continue;
                }
                failed = true;
                break;
            }
            let (dx, dy) = if ids.len() == end - index {
                (0.0, 0.0)
            } else {
                match style.read_direction {
                    90 => (0.0, advance),
                    180 => (-advance, 0.0),
                    270 => (0.0, -advance),
                    _ => (advance, 0.0),
                }
            };
            let position = cluster
                .and_then(|cluster| cluster.positions.get(glyph_index).copied())
                .unwrap_or((positioned.x + dx, positioned.y + dy));
            let scale = style.font_size * face.matrix().a.abs();
            let mut path_style = style.paint.clone();
            path_style.line_width /= scale.max(f64::EPSILON);
            if style.weight >= 600
                && style.weight > face.weight().saturating_add(100)
                && path_style.fill
                && !path_style.stroke
            {
                path_style.stroke = true;
                path_style.stroke_color = path_style.fill_color;
                path_style.join = "Round".into();
                path_style.line_width = (f64::from(style.weight - face.weight()) / 10000.0)
                    .min(0.06)
                    / face.matrix().a.abs().max(f64::EPSILON);
            }
            let italic = if style.italic && !face.italic() {
                12_f64.to_radians().tan()
            } else {
                0.0
            };
            let glyph_transform = transform
                .compose(Matrix::translate(position.0, position.1))
                .compose(glyph_orientation(style))
                .compose(Matrix {
                    a: style.font_size,
                    d: -style.font_size,
                    ..Matrix::IDENTITY
                })
                .compose(Matrix {
                    c: italic,
                    ..Matrix::IDENTITY
                })
                .compose(face.matrix());
            cluster_items.push(Item::Path {
                transform: glyph_transform,
                cmds: builder.cmds,
                style: path_style,
            });
            advance +=
                glyph_id.map_or(0.0, |id| face.advance(id)) * style.font_size * style.h_scale;
        }
        if failed {
            let mut fallback_style = style.clone();
            fallback_style.clusters.clear();
            items.push(Item::Text {
                transform: *transform,
                chars: chars[index..end].to_vec(),
                glyphs: Vec::new(),
                style: fallback_style,
            });
        } else {
            items.extend(cluster_items);
        }
        index = end;
    }
    if items.is_empty() { None } else { Some(items) }
}

fn glyph_orientation(style: &TextStyle) -> Matrix {
    let angle = f64::from(style.char_direction).to_radians();
    let (sin, cos) = angle.sin_cos();
    Matrix {
        a: cos * style.h_scale,
        b: sin * style.h_scale,
        c: -sin,
        d: cos,
        e: 0.0,
        f: 0.0,
    }
}

fn write_item(out: &mut String, item: &Item, families: Option<&HashMap<String, String>>) {
    write_item_with_fonts(out, item, families, None);
}

fn write_item_with_fonts(
    out: &mut String,
    item: &Item,
    families: Option<&HashMap<String, String>>,
    glyph_fonts: Option<&HashMap<String, GlyphFont>>,
) {
    if let Some(fonts) = glyph_fonts
        && let Some(paths) = text_glyph_items(item, fonts)
    {
        for path in &paths {
            write_item(out, path, families);
        }
        return;
    }
    match item {
        Item::Path {
            transform,
            cmds,
            style,
        } => {
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let fill_gradient = style
                .fill
                .then_some(style.fill_shading.as_ref())
                .flatten()
                .map(|shading| write_gradient(out, shading));
            let stroke_gradient = style
                .stroke
                .then_some(style.stroke_shading.as_ref())
                .flatten()
                .map(|shading| write_gradient(out, shading));
            let _ = write!(out, r#"<path d="{}""#, path_data(cmds));
            if let Some(id) = &fill_gradient {
                let _ = write!(
                    out,
                    r#" fill="url(#{id})" fill-opacity="{}""#,
                    num(style.opacity)
                );
            } else if style.fill {
                if let Some(color) = style.fill_color {
                    let _ = write!(
                        out,
                        r#" fill="{}" fill-opacity="{}""#,
                        color.hex(),
                        num(color.opacity() * style.opacity)
                    );
                } else {
                    let _ = write!(out, r#" fill="none""#);
                }
            } else {
                let _ = write!(out, r#" fill="none""#);
            }
            if style.stroke {
                let paint = match (&stroke_gradient, style.stroke_color) {
                    (Some(id), _) => Some((format!("url(#{id})"), style.opacity)),
                    (None, Some(color)) => Some((color.hex(), color.opacity() * style.opacity)),
                    (None, None) => None,
                };
                if let Some((paint, opacity)) = paint {
                    let _ = write!(
                        out,
                        r#" stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
                        paint,
                        num(opacity),
                        num(style.line_width)
                    );
                    if !style.dash.is_empty() {
                        let dash = style
                            .dash
                            .iter()
                            .map(|value| num(*value))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let _ = write!(out, r#" stroke-dasharray="{dash}""#);
                    }
                    let _ = write!(
                        out,
                        r#" stroke-linecap="{}" stroke-linejoin="{}""#,
                        cap(&style.cap),
                        join(&style.join)
                    );
                }
            } else {
                let _ = write!(out, r#" stroke="none""#);
            }
            let _ = writeln!(
                out,
                r#" fill-rule="{}"/></g>"#,
                if style.even_odd { "evenodd" } else { "nonzero" }
            );
        }
        Item::Text {
            transform,
            chars,
            style,
            ..
        } => {
            if style.char_direction != 0 || (style.h_scale - 1.0).abs() > 1e-9 {
                let mut plain_style = style.clone();
                plain_style.char_direction = 0;
                plain_style.h_scale = 1.0;
                for positioned in chars {
                    let plain = Item::Text {
                        transform: transform
                            .compose(Matrix::translate(positioned.x, positioned.y))
                            .compose(glyph_orientation(style)),
                        chars: vec![PositionedChar {
                            ch: positioned.ch,
                            x: 0.0,
                            y: 0.0,
                        }],
                        glyphs: Vec::new(),
                        style: plain_style.clone(),
                    };
                    write_item(out, &plain, families);
                }
                return;
            }
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let color = style.color;
            let family_name = families
                .and_then(|map| map.get(&style.font_key))
                .map(String::as_str)
                .unwrap_or(&style.font_family);
            let family = css_escape(family_name);
            let generic = if style.font_family.contains("Courier") {
                "monospace"
            } else {
                "serif"
            };
            let mut start = 0;
            while start < chars.len() {
                let baseline = chars[start].y;
                let mut end = start + 1;
                while end < chars.len() && (chars[end].y - baseline).abs() < 1e-9 {
                    end += 1;
                }
                let run = &chars[start..end];
                let x_list = run
                    .iter()
                    .map(|positioned| num(positioned.x))
                    .collect::<Vec<_>>()
                    .join(" ");
                let text: String = run.iter().map(|positioned| positioned.ch).collect();
                let _ = write!(
                    out,
                    r#"<text x="{}" y="{}" font-family="'{}', 'Source Han Serif CN', 'Noto Serif CJK SC', {}" font-size="{}" fill="{}" fill-opacity="{}""#,
                    x_list,
                    num(baseline),
                    family,
                    generic,
                    num(style.font_size),
                    if style.paint.fill {
                        color.hex()
                    } else {
                        "none".into()
                    },
                    num(color.opacity() * style.opacity),
                );
                if style.paint.stroke
                    && let Some(stroke) = style.paint.stroke_color
                {
                    let _ = write!(
                        out,
                        r#" stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
                        stroke.hex(),
                        num(stroke.opacity() * style.opacity),
                        num(style.paint.line_width)
                    );
                }
                if style.weight != 400 {
                    let _ = write!(out, r#" font-weight="{}""#, style.weight.clamp(100, 900));
                }
                if style.italic {
                    let _ = write!(out, r#" font-style="italic""#);
                }
                let _ = writeln!(out, r#" xml:space="preserve">{}</text>"#, escape_xml(&text));
                start = end;
            }
            out.push_str("</g>");
        }
        Item::Image {
            transform,
            width,
            height,
            mime,
            data,
            blend,
            clip,
        } => {
            let encoded = STANDARD.encode(data);
            let blend_style = blend
                .as_deref()
                .and_then(blend_mode)
                .map(|mode| format!(r#" style="mix-blend-mode:{mode}""#))
                .unwrap_or_default();
            let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            let clip_attr = if let Some((clip_x, clip_y, clip_width, clip_height)) = clip {
                static SEAL_CLIP_ID: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let id = SEAL_CLIP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _ = write!(
                    out,
                    r#"<defs><clipPath id="seal-clip-{id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath></defs>"#,
                    num(*clip_x),
                    num(*clip_y),
                    num(*clip_width),
                    num(*clip_height),
                );
                format!(r#" clip-path="url(#seal-clip-{id})""#)
            } else {
                String::new()
            };
            let _ = write!(
                out,
                r#"<image x="0" y="0" width="{}" height="{}" preserveAspectRatio="none"{}{} href="data:{};base64,{}"/></g>"#,
                num(*width),
                num(*height),
                blend_style,
                clip_attr,
                mime,
                encoded
            );
        }
        Item::Group {
            transform,
            items,
            opacity,
        } => {
            if *opacity < 1.0 {
                let _ = write!(
                    out,
                    r#"<g transform="{}" opacity="{}">"#,
                    transform.svg(),
                    num(*opacity)
                );
            } else {
                let _ = write!(out, r#"<g transform="{}">"#, transform.svg());
            }
            for item in items {
                write_item_with_fonts(out, item, families, glyph_fonts);
            }
            out.push_str("</g>");
        }
        Item::Clip { regions, items } => {
            // One nested group per region: nested clip-paths intersect,
            // children of one clipPath unite.
            for region in regions {
                let id = next_svg_id("clip");
                let _ = write!(
                    out,
                    r#"<defs><clipPath id="{id}" clipPathUnits="userSpaceOnUse">"#
                );
                for shape in region {
                    let _ = write!(
                        out,
                        r#"<path transform="{}" d="{}" clip-rule="{}"/>"#,
                        shape.transform.svg(),
                        path_data(&shape.cmds),
                        if shape.even_odd { "evenodd" } else { "nonzero" }
                    );
                }
                let _ = write!(out, r#"</clipPath></defs><g clip-path="url(#{id})">"#);
            }
            for item in items {
                write_item_with_fonts(out, item, families, glyph_fonts);
            }
            for _ in regions {
                out.push_str("</g>");
            }
        }
    }
}

/// Document-unique ids for inline `<defs>`.
fn next_svg_id(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}-{id}")
}

/// Writes a `userSpaceOnUse` gradient (path coordinates) and returns its id.
fn write_gradient(out: &mut String, shading: &Shading) -> String {
    let id = next_svg_id("shd");
    let spread = match shading.spread {
        ShadingSpread::Pad => "pad",
        ShadingSpread::Repeat => "repeat",
        ShadingSpread::Reflect => "reflect",
    };
    out.push_str("<defs>");
    match shading.kind {
        ShadingKind::Axial { start, end } => {
            let _ = write!(
                out,
                r#"<linearGradient id="{id}" gradientUnits="userSpaceOnUse" spreadMethod="{spread}" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                num(start.0),
                num(start.1),
                num(end.0),
                num(end.1)
            );
        }
        ShadingKind::Radial {
            start, end, radius, ..
        } => {
            let _ = write!(
                out,
                r#"<radialGradient id="{id}" gradientUnits="userSpaceOnUse" spreadMethod="{spread}" cx="{}" cy="{}" r="{}" fx="{}" fy="{}">"#,
                num(end.0),
                num(end.1),
                num(radius),
                num(start.0),
                num(start.1)
            );
        }
    }
    for (offset, color) in shading.resolved_stops() {
        let _ = write!(
            out,
            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
            num(offset),
            color.hex(),
            num(color.opacity())
        );
    }
    out.push_str(match shading.kind {
        ShadingKind::Axial { .. } => "</linearGradient></defs>",
        ShadingKind::Radial { .. } => "</radialGradient></defs>",
    });
    id
}

fn path_data(cmds: &[PathCmd]) -> String {
    let mut out = String::new();
    for cmd in cmds {
        let _ = match cmd {
            PathCmd::Move(x, y) => write!(out, "M {} {} ", num(*x), num(*y)),
            PathCmd::Line(x, y) => write!(out, "L {} {} ", num(*x), num(*y)),
            PathCmd::Quad(x1, y1, x2, y2) => write!(
                out,
                "Q {} {} {} {} ",
                num(*x1),
                num(*y1),
                num(*x2),
                num(*y2)
            ),
            PathCmd::Cubic(x1, y1, x2, y2, x3, y3) => write!(
                out,
                "C {} {} {} {} {} {} ",
                num(*x1),
                num(*y1),
                num(*x2),
                num(*y2),
                num(*x3),
                num(*y3)
            ),
            PathCmd::Arc {
                rx,
                ry,
                rotation,
                large,
                sweep,
                x,
                y,
            } => write!(
                out,
                "A {} {} {} {} {} {} {} ",
                num(*rx),
                num(*ry),
                num(*rotation),
                if *large { 1 } else { 0 },
                if *sweep { 1 } else { 0 },
                num(*x),
                num(*y)
            ),
            PathCmd::Close => write!(out, "Z "),
        };
    }
    out.trim_end().to_string()
}

fn num(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let formatted = format!("{value:.4}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

// Font matrices often contain 1/2048 or 1/1000 and are subsequently scaled by
// the object's CTM. Four decimal places introduces percent-level glyph-size
// errors, or rounds small glyph transforms to zero entirely.
fn matrix_num(value: f64) -> String {
    if !value.is_finite() || value.abs() < 0.000_000_000_05 {
        return "0".into();
    }
    format!("{value:.10}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn cap(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "round" => "round",
        "square" => "square",
        _ => "butt",
    }
}

fn join(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "round" => "round",
        "bevel" => "bevel",
        _ => "miter",
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn css_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    #[test]
    fn configurable_tree_cache_reuses_recent_pages_and_evicts_lru() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.ofd");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, content) in [
            (
                "OFD.xml",
                r#"<OFD><DocBody><DocRoot>Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Document.xml",
                r#"<Document><CommonData><PageArea><PhysicalBox>0 0 10 10</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/><Page ID="2" BaseLoc="Page.xml"/><Page ID="3" BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            ("Page.xml", "<Page><Content/></Page>"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let doc = OfdDocument::open(path).unwrap();
        let renderer = PageRenderer::with_tree_cache_limit(&doc, 2);
        let page =
            |renderer: &PageRenderer, index| renderer.cached_page(&doc, index, true).unwrap().id();
        let first = page(&renderer, 0);
        let second = page(&renderer, 1);
        assert_eq!(first, page(&renderer, 0));
        page(&renderer, 2);
        assert_eq!(renderer.tree_cache.lock().unwrap().len(), 2);
        assert_eq!(first, page(&renderer, 0));
        // An evicted page is rebuilt; compare while the old one is alive.
        let old = renderer.cached_page(&doc, 1, true).unwrap();
        let _ = second;
        page(&renderer, 0);
        page(&renderer, 2);
        assert_ne!(old.id(), page(&renderer, 1));
        let disabled = PageRenderer::with_tree_cache_limit(&doc, 0);
        let kept = disabled.cached_page(&doc, 0, true).unwrap();
        assert_ne!(kept.id(), page(&disabled, 0));
        assert!(disabled.tree_cache.lock().unwrap().is_empty());
        assert_eq!(
            PageRenderer::with_tree_cache_limit(&doc, usize::MAX).tree_cache_limit,
            256
        );

        // Previews reuse cached pages but never insert or evict.
        let preview = PageRenderer::with_tree_cache_limit(&doc, 2);
        let read = preview.cached_page(&doc, 0, true).unwrap().id();
        preview.rasterize_preview(&doc, 1, 48.0).unwrap();
        preview.rasterize_preview(&doc, 2, 48.0).unwrap();
        preview.rasterize_preview(&doc, 0, 48.0).unwrap();
        assert_eq!(preview.tree_cache.lock().unwrap().len(), 1);
        assert_eq!(read, page(&preview, 0));

        // Parallel tiles of one uncached page share one build and match
        // serial output exactly.
        let parallel = PageRenderer::with_tree_cache_limit(&doc, 4);
        let region = |x| crate::render::RasterRegion {
            x,
            y: 0,
            width: 64,
            height: 64,
        };
        let results: Vec<_> = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8)
                .map(|i| {
                    let parallel = &parallel;
                    let doc = &doc;
                    scope.spawn(move || {
                        parallel
                            .rasterize_region(doc, 1, 192.0, region(i * 64))
                            .unwrap()
                            .bgra
                    })
                })
                .collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect()
        });
        assert_eq!(parallel.tree_cache.lock().unwrap().len(), 1);
        assert!(parallel.building.lock().unwrap().is_empty());
        for (i, tile) in results.iter().enumerate() {
            let serial = renderer
                .rasterize_region(&doc, 1, 192.0, region(i as u32 * 64))
                .unwrap();
            assert_eq!(tile, &serial.bgra);
        }
    }

    use super::*;

    // Generated two-glyph TrueType fixture: a triangle mapped to U+0041.
    // No dependency on installed fonts or private corpus documents.
    fn audit_font() -> GlyphFont {
        let data = STANDARD.decode("AAEAAAAKAIAAAwAgT1MvMkE4QXkAAAEoAAAAYGNtYXAADACUAAABkAAAADRnbHlmCHsoEQAAAcwAAAAWaGVhZFlVpJwAAACsAAAANmhoZWEFFgEuAAAA5AAAACRobXR4AfQAAAAAAYgAAAAGbG9jYQALAAAAAAHEAAAABm1heHAABAAFAAABCAAAACBuYW1lLh01yQAAAeQAAABacG9zdAAoAAAAAAJAAAAAJgABAAAAAQAA9LIRg18PPPUAAwPoAAAAAHwlsIAAAAAAfCWwgAAAAAAB9AK8AAAAAwACAAAAAAAAAAEAAAMg/zgAAAH0AAAAAAH0AAEAAAAAAAAAAAAAAAAAAAABAAEAAAACAAMAAQAAAAAAAgAAAAAAAAAAAAAAAAAAAAAAAwH0AZAABQAEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAPz8/PwAAAEEAQQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAB9AAAAAAAAAAAAAIAAAADAAAAFAADAAEAAAAUAAQAIAAAAAQABAABAAAAQf//AAAAQf///8AAAQAAAAAAAAAAAAsAAAABAAAAAAH0ArwAAgAAMSEDAfT6ArwAAAAAAAQANgABAAAAAAABAAUAAAABAAAAAAACAAcABQADAAEECQABAAoADAADAAEECQACAA4AFkF1ZGl0UmVndWxhcgBBAHUAZABpAHQAUgBlAGcAdQBsAGEAcgAAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAACQAAA==").unwrap();
        GlyphFont {
            data: Arc::new(data),
            face_index: 0,
        }
    }

    fn audit_text(glyphs: Vec<u16>) -> Item {
        Item::Text {
            transform: Matrix::IDENTITY,
            chars: vec![PositionedChar {
                ch: 'A',
                x: 2.0,
                y: 3.0,
            }],
            glyphs,
            style: TextStyle {
                font_key: "Public:1".into(),
                font_family: "not-installed".into(),
                font_size: 10.0,
                color: Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                weight: 400,
                italic: false,
                opacity: 1.0,
                h_scale: 1.0,
                char_direction: 0,
                read_direction: 0,
                paint: PathStyle {
                    stroke: false,
                    fill_color: Some(Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 255,
                    }),
                    ..PathStyle::default()
                },
                clusters: Vec::new(),
            },
        }
    }

    #[test]
    fn embedded_cmap_and_gid_fonts_draw_inside_nested_groups() {
        for use_gid in [false, true] {
            let font = audit_font();
            let fonts = HashMap::from([("Public:1".into(), font)]);
            let text = audit_text(if use_gid { vec![1] } else { vec![] });
            let group = Item::Group {
                transform: Matrix::translate(10.0, 20.0),
                opacity: 0.4,
                items: vec![Item::Group {
                    transform: Matrix::IDENTITY,
                    opacity: 1.0,
                    items: vec![text],
                }],
            };
            let mut svg = String::new();
            write_item_with_fonts(&mut svg, &group, None, Some(&fonts));
            assert!(svg.contains("<path"), "{svg}");
            assert!(!svg.contains("<text"), "{svg}");
            assert!(svg.contains(r#"opacity="0.4""#), "{svg}");
            assert!(svg.contains("matrix(0.01 0 0 -0.01 2 3)"), "{svg}");
        }
    }

    #[test]
    fn invalid_embedded_outline_falls_back_without_blank_text() {
        let fonts = HashMap::from([("Public:1".into(), audit_font())]);
        let mut svg = String::new();
        write_item_with_fonts(&mut svg, &audit_text(vec![0]), None, Some(&fonts));
        assert!(svg.contains("<text"), "{svg}");
        assert!(svg.contains(">A</text>"), "{svg}");
    }

    #[test]
    fn missing_glyph_does_not_replace_the_rest_of_the_embedded_run() {
        let fonts = HashMap::from([("Public:1".into(), audit_font())]);
        let mut text = audit_text(vec![]);
        if let Item::Text { chars, .. } = &mut text {
            chars.push(PositionedChar {
                ch: 'B',
                x: 7.0,
                y: 3.0,
            });
        }
        let mut svg = String::new();
        write_item_with_fonts(&mut svg, &text, None, Some(&fonts));
        assert_eq!(svg.matches("<path ").count(), 1, "{svg}");
        assert_eq!(svg.matches("<text ").count(), 1, "{svg}");
        assert!(svg.contains(">B</text>"), "{svg}");
        assert!(!svg.contains(">AB</text>"), "{svg}");
    }

    #[test]
    fn raw_cff_uses_font_matrix_and_explicit_glyphs() {
        let font = GlyphFont {
            data: Arc::new(STANDARD.decode("AQAEAQABAQEJQXVkaXRDRkYAAQEBGPgbAvgbA/gYBIuL+Ij5UAXID4vhEssRAAEBAQpBdWRpdCBDRkYAAAAAIgACAQEEEfiIDviIixb4iAb7jvlQBQ4=").unwrap()),
            face_index: 0,
        };
        let fonts = HashMap::from([("Public:1".into(), font)]);
        let paths = text_glyph_items(&audit_text(vec![1]), &fonts).unwrap();
        assert!(matches!(&paths[0], Item::Path { .. }));
        let face = EmbeddedFace::parse(&fonts["Public:1"].data, 0).unwrap();
        assert!((face.matrix().a - 0.001).abs() < 1e-9);
        assert_eq!(face.glyph('A'), Some(1));
    }

    #[test]
    fn raw_cid_cff_maps_sparse_cids_to_dense_charstrings() {
        let data = STANDARD.decode("AQAEAQABAQEJQXVkaXRDSUQAAQEBIvgb+ByLDB6Li/iI+VAF9xAMIuAPi/cXEuMMJfcKDCTrEQADAQEGDhZBZG9iZUlkZW50aXR5QXVkaXRDSUQAAAAAewMAAQAAAAACAAIBAQQR+IgO+IiLFviIBvuO+VAFDgABAQEJ+B0MJov3FxI=").unwrap();
        let face = EmbeddedFace::parse(&data, 0).unwrap();
        assert_eq!(face.explicit_glyph(123), Some(1));
        assert_eq!(face.explicit_glyph(124), None);
        let fonts = HashMap::from([(
            "Public:1".into(),
            GlyphFont {
                data: Arc::new(data),
                face_index: 0,
            },
        )]);
        let paths = text_glyph_items(&audit_text(vec![123]), &fonts).unwrap();
        assert!(matches!(paths[0], Item::Path { .. }));
    }

    #[test]
    fn matrix_composition_agrees_with_successive_point_transforms() {
        let first = Matrix {
            a: 0.5,
            b: 1.0,
            c: -2.0,
            d: 0.3,
            e: 3.0,
            f: 4.0,
        };
        let second = Matrix {
            a: 2.0,
            b: -0.2,
            c: 0.7,
            d: 3.0,
            e: 6.0,
            f: -2.0,
        };
        let intermediate = second.transform_point(1.5, -7.0);
        let expected = first.transform_point(intermediate.0, intermediate.1);
        let actual = first.compose(second).transform_point(1.5, -7.0);
        assert!((actual.0 - expected.0).abs() < 1e-9);
        assert!((actual.1 - expected.1).abs() < 1e-9);
    }

    #[test]
    fn svg_font_matrix_preserves_fractional_em_scale() {
        let matrix = Matrix {
            a: 1.0 / 2048.0,
            d: -1.0 / 2048.0,
            ..Matrix::IDENTITY
        };
        let svg = matrix.svg();
        let roundtrip = Matrix::parse(
            svg.strip_prefix("matrix(")
                .unwrap()
                .strip_suffix(')')
                .unwrap(),
        )
        .unwrap();
        let (x, y) = roundtrip.transform_point(2048.0, 2048.0);
        assert!((x - 1.0).abs() < 1e-6, "{svg}");
        assert!((y + 1.0).abs() < 1e-6, "{svg}");
    }

    #[test]
    fn one_to_one_clusters_do_not_replace_a_run_containing_spaces() {
        let xml = parse_xml(br#"<TextObject><CGTransform CodePosition="0" CodeCount="3" GlyphCount="3"><Glyphs>1 0 1</Glyphs></CGTransform></TextObject>"#, "space.xml").unwrap();
        let mut text = audit_text(vec![]);
        if let Item::Text { chars, style, .. } = &mut text {
            *chars = "A A"
                .chars()
                .enumerate()
                .map(|(index, ch)| PositionedChar {
                    ch,
                    x: index as f64 * 5.0,
                    y: 0.0,
                })
                .collect();
            style.clusters = glyph_clusters(xml.root_element(), chars);
        }
        let fonts = HashMap::from([("Public:1".into(), audit_font())]);
        let converted = text_glyph_items(&text, &fonts).unwrap();
        assert_eq!(converted.len(), 2);
        assert!(
            converted
                .iter()
                .all(|item| matches!(item, Item::Path { .. }))
        );
    }

    #[test]
    fn font_audit_counts_actual_fallbacks_and_caches_page_results() {
        use std::io::Write as _;
        use zip::write::SimpleFileOptions;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("font-audit.ofd");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let entries = [
            (
                "OFD.xml",
                r#"<OFD><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document><CommonData><PublicRes>Res.xml</PublicRes><PageArea><PhysicalBox>0 0 100 50</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/><Page ID="2" BaseLoc="Cluster.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Res.xml",
                r#"<Res><Fonts><Font ID="1" FontName="Audit"><FontFile>font.ttf</FontFile></Font></Fonts></Res>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page><Content><Layer><TextObject Font="1" Size="10" Boundary="0 0 40 15"><TextCode X="0" Y="10" DeltaX="5">AB</TextCode></TextObject></Layer></Content></Page>"#,
            ),
            (
                "Doc/Cluster.xml",
                r#"<Page><Content><Layer><TextObject Font="1" Size="10" Boundary="0 0 40 15" HScale="0.5" CharDirection="90" Weight="700" Italic="true" Fill="false" Stroke="true" LineWidth="0.2"><StrokeColor Value="255 0 0"/><CGTransform CodePosition="0" CodeCount="2" GlyphCount="3"><Glyphs>1 1 1</Glyphs></CGTransform><TextCode X="3" Y="10" DeltaX="2 3">AA</TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ];
        for (name, content) in entries {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.start_file("Doc/font.ttf", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&audit_font().data).unwrap();
        zip.finish().unwrap();
        let doc = OfdDocument::open(path).unwrap();
        let renderer = PageRenderer::new(&doc);
        let report = renderer.font_report(&doc, 0).unwrap();
        assert_eq!(report.embedded_glyphs, 1);
        assert_eq!(report.fallback_chars, 1);
        assert!(report.fallback_codepoints["Public:1"].contains(&('B' as u32)));
        assert!(Arc::ptr_eq(
            &report,
            &renderer.font_report(&doc, 0).unwrap()
        ));
        assert_eq!(
            renderer.export_svg(&doc, 0).unwrap(),
            renderer.render_svg(&doc, 0).unwrap()
        );
        let page = doc.display_list(1).unwrap();
        let paths = text_glyph_items(&page.items[0], &renderer.glyph_fonts).unwrap();
        assert_eq!(paths.len(), 3);
        for (path, x) in paths.iter().zip([3.0, 5.0, 8.0]) {
            let Item::Path {
                transform, style, ..
            } = path
            else {
                panic!("expected glyph")
            };
            assert!((transform.e - x).abs() < 1e-9);
            assert!((transform.f - 10.0).abs() < 1e-9);
            assert!((transform.b - 0.005).abs() < 1e-9);
            assert!(!style.fill && style.stroke);
            assert_eq!(style.stroke_color.unwrap().hex(), "#FF0000");
        }
        let report = renderer.font_report(&doc, 1).unwrap();
        assert_eq!((report.embedded_glyphs, report.fallback_chars), (3, 0));
    }

    #[test]
    fn collection_selects_named_member_instead_of_first_face() {
        let first = (*audit_font().data).clone();
        let mut second = first.clone();
        for (from, to) in [
            (b"Audit".as_slice(), b"Other".as_slice()),
            (b"\0A\0u\0d\0i\0t".as_slice(), b"\0O\0t\0h\0e\0r".as_slice()),
        ] {
            for start in 0..=second.len() - from.len() {
                if &second[start..start + from.len()] == from {
                    second[start..start + from.len()].copy_from_slice(to);
                }
            }
        }
        let mut collection = b"ttcf\0\x01\0\0\0\0\0\x02".to_vec();
        collection.extend_from_slice(&20_u32.to_be_bytes());
        collection.extend_from_slice(&(20_u32 + first.len() as u32).to_be_bytes());
        for mut font in [first, second] {
            let base = collection.len() as u32;
            let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
            for index in 0..tables {
                let offset = 12 + index * 16 + 8;
                let original = u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
                font[offset..offset + 4].copy_from_slice(&(base + original).to_be_bytes());
            }
            collection.extend_from_slice(&font);
        }
        let resource = FontResource {
            id: "1".into(),
            scope: crate::model::ResScope::Public,
            name: "Other".into(),
            family: "".into(),
            file: Some("font.ttc".into()),
            bold: false,
            italic: false,
        };
        assert_eq!(select_face(&collection, &resource), 1);
        assert!(
            EmbeddedFace::parse(&collection, 1)
                .unwrap()
                .glyph('A')
                .is_some()
        );
    }

    #[test]
    fn clusters_handle_ligatures_expansion_and_unmapped_text() {
        let xml = r#"<TextObject><CGTransform CodePosition="0" CodeCount="2"><Glyphs>1</Glyphs></CGTransform><CGTransform CodePosition="2" GlyphCount="2"><Glyphs>1 1</Glyphs></CGTransform></TextObject>"#;
        let xml = parse_xml(xml.as_bytes(), "cluster.xml").unwrap();
        let mut text = audit_text(vec![]);
        if let Item::Text { chars, style, .. } = &mut text {
            *chars = "AAAA"
                .chars()
                .enumerate()
                .map(|(index, ch)| PositionedChar {
                    ch,
                    x: index as f64 * 10.0,
                    y: 0.0,
                })
                .collect();
            style.clusters = glyph_clusters(xml.root_element(), chars);
            assert_eq!(style.clusters.len(), 2);
        }
        let fonts = HashMap::from([("Public:1".into(), audit_font())]);
        let converted = text_glyph_items(&text, &fonts).unwrap();
        // AB -> one ligature, C -> two glyphs, D -> its own cmap glyph.
        assert_eq!(converted.len(), 4);
        assert!(
            converted
                .iter()
                .all(|item| matches!(item, Item::Path { .. }))
        );
        if let Item::Path { transform, .. } = &converted[2] {
            assert!((transform.e - 25.0).abs() < 1e-9);
        }
    }

    #[test]
    fn embedded_style_applies_scale_direction_bold_and_italic() {
        let mut text = audit_text(vec![]);
        if let Item::Text { style, .. } = &mut text {
            style.h_scale = 0.5;
            style.char_direction = 90;
            style.italic = true;
            style.weight = 700;
        }
        let fonts = HashMap::from([("Public:1".into(), audit_font())]);
        let converted = text_glyph_items(&text, &fonts).unwrap();
        let Item::Path {
            transform, style, ..
        } = &converted[0]
        else {
            panic!("expected outline")
        };
        assert!(transform.a.abs() < 1e-9);
        assert!((transform.b - 0.005).abs() < 1e-9);
        assert!(style.stroke && style.line_width > 0.0);
        assert_eq!((transform.e, transform.f), (2.0, 3.0));
        assert!(
            transform.d.abs() > 0.0,
            "italic shear should remain after rotation"
        );
    }

    #[test]
    fn text_code_decodes_utf16_escapes_without_losing_literal_backslashes() {
        assert_eq!(
            decode_text_code(r"A\0020\4E2D\D83D\DE00\path"),
            "A 中😀\\path"
        );
    }

    #[test]
    fn cgtransform_does_not_guess_ligature_or_overflow_mappings() {
        let chars = vec![
            PositionedChar {
                ch: 'A',
                x: 0.0,
                y: 0.0,
            },
            PositionedChar {
                ch: 'B',
                x: 1.0,
                y: 0.0,
            },
        ];
        for (position, count, glyph_count) in [(0, 2, 1), (usize::MAX, 2, 1)] {
            let xml = format!(
                r#"<TextObject><CGTransform CodePosition="{position}" CodeCount="{count}" GlyphCount="{glyph_count}"><Glyphs>1</Glyphs></CGTransform></TextObject>"#
            );
            let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
            assert!(positioned_glyphs(document.root_element(), &chars).is_empty());
        }
    }

    #[test]
    fn translate_then_scale_keeps_page_position() {
        let translate = Matrix::translate(106.0, 32.5);
        let scale = Matrix {
            a: 0.89,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        };
        let combined = translate.compose(scale);
        assert!((combined.a - 0.89).abs() < 1e-9);
        assert!((combined.e - 106.0).abs() < 1e-9);
        assert!((combined.f - 32.5).abs() < 1e-9);
    }

    fn test_char(ch: char, x: f64, line: u32, run: u32) -> PageTextChar {
        PageTextChar {
            ch,
            rect: TextMatchRect {
                x,
                y: 10.0,
                width: 1.0,
                height: 1.0,
            },
            line,
            run,
        }
    }

    #[test]
    fn selection_text_joins_runs_and_lines() {
        let layout = PageTextLayout {
            chars: vec![
                test_char('发', 1.0, 0, 0),
                test_char('票', 2.0, 0, 0),
                test_char('测', 3.0, 0, 1),
                test_char('试', 4.0, 1, 1),
            ],
        };
        assert_eq!(layout.text_range(0, 4), "发票 测\n试");
        assert_eq!(layout.text_range(1, 3), "票 测");
        assert_eq!(layout.rects(1, 3).len(), 2);
    }

    #[test]
    fn selection_hit_tests_rects_and_falls_back_to_distance() {
        let layout = PageTextLayout {
            chars: vec![test_char('发', 1.0, 0, 0), test_char('票', 3.0, 0, 0)],
        };
        assert_eq!(layout.index_at(1.5, 10.5, 0.0), Some(0));
        assert_eq!(layout.index_at(3.5, 10.5, 0.0), Some(1));
        assert_eq!(layout.index_at(2.8, 10.5, 1.0), Some(1));
        assert_eq!(layout.index_at(50.0, 50.0, 1.0), None);
    }

    #[test]
    fn expands_batched_delta_x_groups() {
        let deltas = parse_delta_list("g 3 26.04 25.94 g 2 26.04");
        assert_eq!(deltas, vec![26.04, 26.04, 26.04, 25.94, 26.04, 26.04]);
    }

    #[test]
    fn cgtransform_maps_utf16_positions_to_glyphs() {
        let xml = r#"<TextObject>
            <CGTransform CodePosition="0" CodeCount="1" GlyphCount="1"><Glyphs>7</Glyphs></CGTransform>
            <CGTransform CodePosition="1" CodeCount="2" GlyphCount="2"><Glyphs>8 9</Glyphs></CGTransform>
        </TextObject>"#;
        let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
        let chars = vec![
            PositionedChar {
                ch: '中',
                x: 0.0,
                y: 0.0,
            },
            PositionedChar {
                ch: '国',
                x: 1.0,
                y: 0.0,
            },
            PositionedChar {
                ch: 'A',
                x: 2.0,
                y: 0.0,
            },
        ];
        assert_eq!(
            positioned_glyphs(document.root_element(), &chars),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn cgtransform_handles_surrogate_pairs_and_missing_whitespace() {
        let xml = r#"<TextObject>
            <CGTransform CodePosition="0" CodeCount="2" GlyphCount="1"><Glyphs>11</Glyphs></CGTransform>
        </TextObject>"#;
        let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
        let chars = vec![
            PositionedChar {
                ch: '😀',
                x: 0.0,
                y: 0.0,
            },
            PositionedChar {
                ch: ' ',
                x: 1.0,
                y: 0.0,
            },
        ];
        assert_eq!(
            positioned_glyphs(document.root_element(), &chars),
            vec![11, 0]
        );
    }

    #[test]
    fn cgtransform_rejects_incomplete_mappings() {
        let xml = r#"<TextObject>
            <CGTransform CodePosition="0" CodeCount="1" GlyphCount="1"><Glyphs>7</Glyphs></CGTransform>
        </TextObject>"#;
        let document = parse_xml(xml.as_bytes(), "test.xml").unwrap();
        let chars = vec![
            PositionedChar {
                ch: '中',
                x: 0.0,
                y: 0.0,
            },
            PositionedChar {
                ch: '国',
                x: 1.0,
                y: 0.0,
            },
        ];
        assert!(positioned_glyphs(document.root_element(), &chars).is_empty());
    }

    #[test]
    fn parses_plain_delta_x_values() {
        assert_eq!(parse_delta_list("1.5 g 2 3"), vec![1.5, 3.0, 3.0]);
    }

    #[test]
    fn parses_ofd_path_commands() {
        let mut warnings = Vec::new();
        let cmds =
            parse_path_data("M 0 0 L 10 0 Q 12 0 12 2 B 12 4 10 6 8 6 C", &mut warnings).unwrap();
        assert_eq!(cmds.len(), 5);
        assert!(warnings.is_empty());
    }

    #[test]
    fn layer_fill_colour_does_not_turn_on_path_filling() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("path-fill.ofd");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, content) in [
            (
                "OFD.xml",
                r#"<OFD><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document><CommonData><PublicRes>Res.xml</PublicRes><PageArea><PhysicalBox>0 0 100 100</PhysicalBox></PageArea></CommonData><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Res.xml",
                r#"<Res><DrawParams><DrawParam ID="2"><FillColor Value="255 0 0"/></DrawParam></DrawParams></Res>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page><Content><Layer DrawParam="2"><PathObject Boundary="0 0 20 20"><StrokeColor Value="255 0 0"/><AbbreviatedData>M 0 0 L 20 0 L 20 20 L 0 20 C</AbbreviatedData></PathObject><TextObject Boundary="30 0 20 5" Size="3"><TextCode X="0" Y="3">A</TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let doc = OfdDocument::open(path).unwrap();
        let page = doc.display_list(0).unwrap();
        let Item::Path { style, .. } = &page.items[0] else {
            panic!("expected path")
        };
        // OFD 8.2 表 41: no Fill attribute means no fill, even when the layer
        // DrawParam supplies a fill colour.
        assert!(!style.fill);
        assert!(style.stroke);
        assert_eq!(style.stroke_color.unwrap().hex(), "#FF0000");

        // Text still inherits the layer colour (text is filled by default).
        let Item::Text { style, .. } = &page.items[1] else {
            panic!("expected text")
        };
        assert_eq!(style.color.hex(), "#FF0000");
    }
}
