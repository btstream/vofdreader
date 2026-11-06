use std::sync::{Arc, OnceLock};

use resvg::usvg::fontdb;

use crate::model::FontResource;

#[derive(Clone)]
pub(crate) struct GlyphFont {
    pub(crate) data: Arc<Vec<u8>>,
    pub(crate) face_index: u32,
}

/// System font data is shared: scanning and parsing system fonts costs more
/// than a page render, and docs only add a handful of embedded faces on top.
pub(crate) fn system_fonts() -> fontdb::Database {
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

pub(crate) fn fallback_family(font: &FontResource) -> String {
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
pub(crate) fn system_font_alias(name: &str) -> Option<&'static str> {
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
