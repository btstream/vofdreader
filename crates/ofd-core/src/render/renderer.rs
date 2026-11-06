use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use resvg::usvg::{self, fontdb};

use crate::document::OfdDocument;
use crate::error::{OfdError, Result};
use crate::font::select_face;
use crate::loc::resolve_loc;
use crate::xml::{child, child_text, children, parse_xml};

use super::TREE_CACHE_LIMIT;
use super::fonts::{GlyphFont, fallback_family, system_fonts};
use super::parse::*;
use super::raster;
use super::svg::*;
use super::text::*;
use super::types::*;

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

    pub(crate) fn append_template(
        &self,
        id: &str,
        items: &mut Vec<Item>,
        warnings: &mut Vec<String>,
    ) {
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

    pub(crate) fn append_signature_seals(&self, page_id: &str, items: &mut Vec<Item>) {
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

    pub(crate) fn append_annotations(
        &self,
        page_id: &str,
        items: &mut Vec<Item>,
        warnings: &mut Vec<String>,
    ) {
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
        collect_text_matches(&list.items, query, Matrix::IDENTITY, &mut rects);
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

    pub(crate) fn page_svg_impl(
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

/// Sampling used when page images are scaled (direct backend). Bicubic is
/// the smoothest and slowest: on pages with large upscaled images it costs
/// 2–3× the whole render compared with bilinear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageFilter {
    /// Nearest neighbour: the image's own pixels, no smoothing.
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
}

impl ImageFilter {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::Nearest => 0,
            Self::Bilinear => 1,
            Self::Bicubic => 2,
        }
    }

    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Nearest,
            1 => Self::Bilinear,
            _ => Self::Bicubic,
        }
    }
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
pub(crate) enum CachedPage {
    Tree(Arc<usvg::Tree>),
    Prepared(Arc<raster::Prepared>),
}

impl CachedPage {
    /// Allocation identity, for cache tests.
    #[cfg(test)]
    pub(crate) fn id(&self) -> usize {
        match self {
            Self::Tree(tree) => Arc::as_ptr(tree) as usize,
            Self::Prepared(page) => Arc::as_ptr(page) as usize,
        }
    }
}

pub struct PageRenderer {
    pub(crate) fontdb: Arc<fontdb::Database>,
    pub(crate) font_aliases: HashMap<String, String>,
    pub(crate) glyph_fonts: HashMap<String, GlyphFont>,
    pub(crate) font_reports: Mutex<HashMap<usize, Arc<FontRenderReport>>>,
    pub(crate) backend: RasterBackend,
    /// Prepared pages (or parsed SVG trees), most recently used last.
    /// Re-rasterizing at a new DPI or revisiting a page skips parsing.
    pub(crate) tree_cache: Mutex<VecDeque<(usize, CachedPage)>>,
    pub(crate) tree_cache_limit: usize,
    /// Per-page build gates so concurrent requests share one build.
    pub(crate) building: Mutex<HashMap<usize, Arc<Mutex<()>>>>,
    pub(crate) font_cache: Mutex<raster::FontCache>,
    pub(crate) image_cache: Mutex<raster::ImageCache>,
    /// `ImageFilter` as u8: applied at raster time, so changing it needs no
    /// re-parse of cached pages (callers drop their old bitmaps).
    pub(crate) image_filter: std::sync::atomic::AtomicU8,
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
            image_filter: std::sync::atomic::AtomicU8::new(ImageFilter::default().to_u8()),
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

    /// Image sampling for the direct backend; takes effect on the next
    /// rasterization. The SVG backend keeps resvg's own (bicubic) choice.
    pub fn set_image_filter(&self, filter: ImageFilter) {
        self.image_filter
            .store(filter.to_u8(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn with_image_filter(self, filter: ImageFilter) -> Self {
        self.set_image_filter(filter);
        self
    }

    pub fn image_filter(&self) -> ImageFilter {
        ImageFilter::from_u8(self.image_filter.load(std::sync::atomic::Ordering::Relaxed))
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
    pub(crate) fn cached_page(
        &self,
        doc: &OfdDocument,
        index: usize,
        retain: bool,
    ) -> Result<CachedPage> {
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

    pub(crate) fn cache_lookup(&self, index: usize) -> Option<CachedPage> {
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

    pub(crate) fn build_page(&self, doc: &OfdDocument, index: usize) -> Result<CachedPage> {
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

    pub(crate) fn rasterize_pixmap(
        &self,
        doc: &OfdDocument,
        index: usize,
        dpi: f32,
    ) -> Result<(resvg::tiny_skia::Pixmap, usize, usize)> {
        self.rasterize_pixmap_with(doc, index, dpi, true)
    }

    pub(crate) fn rasterize_pixmap_with(
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
                    self.image_filter(),
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
                    self.image_filter(),
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

pub(crate) fn pixmap_to_raster(pixmap: resvg::tiny_skia::Pixmap) -> RasterPage {
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
