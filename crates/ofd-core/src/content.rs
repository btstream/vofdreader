use std::collections::BTreeSet;

use crate::error::Result;
use crate::model::{PageSummary, SizeMm, parse_physical_box};
use crate::xml::{child, child_text, parse_xml};

/// Parse a page `Content.xml` into a lightweight summary.
///
/// This is the first step of the rendering pipeline: it validates the structure
/// and extracts text. A full `DisplayList` builder will replace the counting
/// walk in M2 while keeping the same traversal.
pub fn parse_page_content(
    file: &str,
    bytes: &[u8],
    index: usize,
    page_id: &str,
    fallback_size: Option<SizeMm>,
) -> Result<PageSummary> {
    let document = parse_xml(bytes, file)?;
    let root = document.root_element();
    let mut summary = PageSummary {
        index,
        id: page_id.to_string(),
        ..Default::default()
    };

    summary.size = child(root, "Area")
        .and_then(|area| child_text(area, "PhysicalBox"))
        .and_then(parse_physical_box)
        .or(fallback_size);

    if summary.size.is_none() {
        summary
            .warnings
            .push("页面缺少有效的 PhysicalBox，尺寸未知".to_string());
    }

    for template in root
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "Template")
    {
        let id = template.attribute("TemplateID").unwrap_or("?");
        let order = template.attribute("ZOrder").unwrap_or("Background");
        summary.templates.push(format!("{id}({order})"));
    }

    if let Some(content) = child(root, "Content") {
        let mut fonts = BTreeSet::new();
        walk_block(content, &mut summary, &mut fonts);
        summary.used_fonts = fonts.into_iter().collect();
    } else {
        summary.warnings.push("页面缺少 Content 节点".to_string());
    }

    summary.text.retain(|line| !line.trim().is_empty());
    Ok(summary)
}

fn walk_block(
    node: roxmltree::Node<'_, '_>,
    summary: &mut PageSummary,
    fonts: &mut BTreeSet<String>,
) {
    for child in node.children().filter(|node| node.is_element()) {
        match child.tag_name().name() {
            "Layer" => walk_block(child, summary, fonts),
            "PageBlock" => {
                summary.page_blocks += 1;
                walk_block(child, summary, fonts);
            }
            "TextObject" => {
                summary.text_objects += 1;
                if let Some(font) = child.attribute("Font") {
                    fonts.insert(font.to_string());
                }
                read_text_object(child, summary);
            }
            "PathObject" => summary.path_objects += 1,
            "ImageObject" => summary.image_objects += 1,
            "CompositeObject" => summary.composite_objects += 1,
            _ => {}
        }
    }
}

fn read_text_object(node: roxmltree::Node<'_, '_>, summary: &mut PageSummary) {
    if let Some(code) = child(node, "TextCode")
        && let Some(text) = code.text()
    {
        let text = text.trim();
        if !text.is_empty() {
            summary.text.push(text.to_string());
        }
    }

    let Some(cg) = child(node, "CGTransform") else {
        return;
    };
    summary.cg_transforms += 1;

    let code_count = cg
        .attribute("CodeCount")
        .and_then(|v| v.parse::<usize>().ok());
    let glyph_count = cg
        .attribute("GlyphCount")
        .and_then(|v| v.parse::<usize>().ok());
    let glyphs = cg
        .attribute("Glyphs")
        .map(|value| value.split_whitespace().count())
        .or_else(|| child_text(cg, "Glyphs").map(|value| value.split_whitespace().count()));

    match (code_count, glyph_count, glyphs) {
        (Some(code), Some(glyph), Some(list)) if code == glyph && glyph == list => {}
        (Some(code), Some(glyph), Some(list)) => summary.warnings.push(format!(
            "CGTransform 数量不一致：CodeCount={code}, GlyphCount={glyph}, Glyphs={list}"
        )),
        _ => summary
            .warnings
            .push("CGTransform 缺少计数或 Glyphs 列表".to_string()),
    }
}
