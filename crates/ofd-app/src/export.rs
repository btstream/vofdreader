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
mod tests;
