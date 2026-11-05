use std::path::Path;

use serde::{Deserialize, Serialize};

/// Output format for the current page, selected from the save dialog's file name.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Png,
    Svg,
}

impl ExportFormat {
    /// Anything without an explicit `.svg` extension exports as PNG, matching
    /// the suggested default name.
    pub fn for_path(path: &Path) -> Self {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("svg") => Self::Svg,
            _ => Self::Png,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Svg => "svg",
        }
    }
}

/// Suggested export name for a page, keeping the document stem and page number.
pub fn suggested_name_for(stem: &str, page: usize, format: ExportFormat) -> String {
    let stem = stem.trim();
    let stem = if stem.is_empty() { "ofd" } else { stem };
    format!("{stem}-第{}页.{}", page + 1, format.extension())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_extension_selects_svg_case_insensitively() {
        assert_eq!(
            ExportFormat::for_path(Path::new("a.SVG")),
            ExportFormat::Svg
        );
        assert_eq!(
            ExportFormat::for_path(Path::new("a.svg")),
            ExportFormat::Svg
        );
    }

    #[test]
    fn other_or_missing_extensions_fall_back_to_png() {
        assert_eq!(
            ExportFormat::for_path(Path::new("a.png")),
            ExportFormat::Png
        );
        assert_eq!(ExportFormat::for_path(Path::new("a")), ExportFormat::Png);
        assert_eq!(
            ExportFormat::for_path(Path::new("a.pdf")),
            ExportFormat::Png
        );
    }

    #[test]
    fn suggested_name_includes_one_based_page_and_keeps_stem() {
        assert_eq!(
            suggested_name_for("发票", 0, ExportFormat::Png),
            "发票-第1页.png"
        );
        assert_eq!(
            suggested_name_for(" invoice ", 4, ExportFormat::Svg),
            "invoice-第5页.svg"
        );
        assert_eq!(
            suggested_name_for("", 2, ExportFormat::Png),
            "ofd-第3页.png"
        );
    }
}
