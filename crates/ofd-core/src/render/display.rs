//! Page assembly: packaging resources, templates, annotations, signature
//! seal appearances and text geometry into a `DisplayList`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use crate::document::{OfdDocument, RenderedSeal, SealAppearance};
use crate::error::{OfdError, Result};
use crate::loc::resolve_loc;
use crate::xml::{child, child_text, children, parse_xml};

use super::fonts::GlyphFont;
use super::parse::*;
use super::renderer::PageRenderer;
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
            let Some(rendered) = seal_appearance(seal) else {
                continue;
            };
            items.push(Item::Image {
                transform: Matrix::translate(seal.x, seal.y),
                width: seal.width,
                height: seal.height,
                mime: rendered.mime,
                data: rendered.data.as_ref().clone(),
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

    /// Text of the rendered page, including background templates. Search uses
    /// this so invoice titles and table labels declared in the template are
    /// findable, not just the page's own content stream.
    pub fn page_display_text(&self, index: usize) -> Result<Vec<String>> {
        fn collect(items: &[Item], out: &mut Vec<String>) {
            for item in items {
                match item {
                    Item::Text { chars, .. } => {
                        let text: String = chars.iter().map(|char| char.ch).collect();
                        if !text.trim().is_empty() {
                            out.push(text);
                        }
                    }
                    Item::Group { items, .. } | Item::Clip { items, .. } => collect(items, out),
                    _ => {}
                }
            }
        }

        let list = self.display_list(index)?;
        let mut out = Vec::new();
        collect(&list.items, &mut out);
        Ok(out)
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

/// Converts a seal's extracted material into a drawable image once. Nested
/// OFD appearances are rendered here rather than during document parsing,
/// which keeps `document.rs` independent of the renderer.
pub(crate) fn seal_appearance(seal: &crate::document::SealStamp) -> Option<&RenderedSeal> {
    seal.rendered
        .get_or_init(|| match &seal.appearance {
            SealAppearance::Image { mime, data } => Some(RenderedSeal {
                mime,
                data: data.clone(),
            }),
            SealAppearance::Ofd(picture) => {
                let doc = OfdDocument::from_seal_bytes(picture.as_ref().clone()).ok()?;
                let svg = PageRenderer::new(&doc).render_seal_svg(&doc).ok()?;
                Some(RenderedSeal {
                    mime: "image/svg+xml",
                    data: Arc::new(svg.into_bytes()),
                })
            }
        })
        .as_ref()
}
