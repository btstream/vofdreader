use std::fmt;

/// Physical size in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeMm {
    pub width: f64,
    pub height: f64,
}

impl SizeMm {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn is_valid(&self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

impl fmt::Display for SizeMm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1} × {:.1} mm", self.width, self.height)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DocInfo {
    pub doc_id: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub creator: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OfdInfo {
    pub version: String,
    pub doc_type: String,
    pub info: DocInfo,
    pub doc_root: String,
    pub signatures: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PageRef {
    pub id: String,
    pub base_loc: String,
}

#[derive(Debug, Clone)]
pub struct TemplateRef {
    pub id: String,
    pub base_loc: String,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentMeta {
    pub public_res: Option<String>,
    pub document_res: Option<String>,
    pub annotations: Option<String>,
    pub template_pages: Vec<TemplateRef>,
    pub pages: Vec<PageRef>,
    pub page_area: Option<SizeMm>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResScope {
    #[default]
    Public,
    Document,
}

impl fmt::Display for ResScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Public => f.write_str("Public"),
            Self::Document => f.write_str("Document"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FontResource {
    pub id: String,
    pub scope: ResScope,
    pub name: String,
    pub family: String,
    pub file: Option<String>,
    pub bold: bool,
    pub italic: bool,
}

impl FontResource {
    pub fn embedded(&self) -> bool {
        self.file.is_some()
    }

    pub fn css_family(&self) -> &str {
        if !self.family.trim().is_empty() {
            &self.family
        } else if !self.name.trim().is_empty() {
            &self.name
        } else {
            "sans-serif"
        }
    }

    /// Stable key for the font entry. Font IDs are only unique inside their
    /// scope, so public and document resources may reuse the same ID.
    pub fn key(&self) -> String {
        format!("{}:{}", self.scope, self.id)
    }

    /// Unique CSS family for an embedded face. Documents often use the same
    /// `FamilyName` (for example 宋体) for several different embedded subsets;
    /// without a unique name their `@font-face` rules would overwrite each
    /// other and text would silently switch to the wrong font.
    pub fn embedded_family(&self) -> String {
        let Some(file) = &self.file else {
            return self.css_family().to_string();
        };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in file.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("OFD-Embedded-{hash:016x}")
    }
}

#[derive(Debug, Clone)]
pub struct ImageResource {
    pub id: String,
    pub scope: ResScope,
    pub format: Option<String>,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub fn parse(value: &str, alpha: u8) -> Option<Self> {
        let value = value.trim();
        if let Some(hex) = value.strip_prefix('#') {
            let digits = hex.trim();
            let (r, g, b) = match digits.len() {
                6 => (
                    u8::from_str_radix(&digits[0..2], 16).ok()?,
                    u8::from_str_radix(&digits[2..4], 16).ok()?,
                    u8::from_str_radix(&digits[4..6], 16).ok()?,
                ),
                8 => (
                    u8::from_str_radix(&digits[2..4], 16).ok()?,
                    u8::from_str_radix(&digits[4..6], 16).ok()?,
                    u8::from_str_radix(&digits[6..8], 16).ok()?,
                ),
                _ => return None,
            };
            return Some(Self { r, g, b, a: alpha });
        }

        let mut parts = value.split_whitespace();
        let r = parts.next()?.parse().ok()?;
        let g = parts.next()?.parse().ok()?;
        let b = parts.next()?.parse().ok()?;
        Some(Self { r, g, b, a: alpha })
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn opacity(self) -> f64 {
        self.a as f64 / 255.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct DrawParam {
    pub id: String,
    pub scope: ResScope,
    pub relative: Option<String>,
    pub line_width: Option<f64>,
    pub stroke_color: Option<Color>,
    pub fill_color: Option<Color>,
    pub cap: Option<String>,
    pub join: Option<String>,
    pub dash_pattern: Vec<f64>,
}

impl DrawParam {
    pub fn merge_from(&mut self, base: &DrawParam) {
        if self.line_width.is_none() {
            self.line_width = base.line_width;
        }
        if self.stroke_color.is_none() {
            self.stroke_color = base.stroke_color;
        }
        if self.fill_color.is_none() {
            self.fill_color = base.fill_color;
        }
        if self.cap.is_none() {
            self.cap = base.cap.clone();
        }
        if self.join.is_none() {
            self.join = base.join.clone();
        }
        if self.dash_pattern.is_empty() {
            self.dash_pattern = base.dash_pattern.clone();
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResourceSet {
    pub fonts: Vec<FontResource>,
    pub images: Vec<ImageResource>,
    pub composite_units: Vec<CompositeUnit>,
    pub draw_params: Vec<DrawParam>,
    pub color_spaces: usize,
    /// Non-fatal issues discovered while loading resources.
    pub warnings: Vec<String>,
}

/// A reusable `CompositeGraphicUnit`. Its content is stored as a small
/// standalone XML fragment because OFD resources may be loaded long before a
/// page references the unit.
#[derive(Debug, Clone)]
pub struct CompositeUnit {
    pub id: String,
    pub scope: ResScope,
    pub width: f64,
    pub height: f64,
    pub content: String,
}

impl ResourceSet {
    pub fn font_ids(&self) -> impl Iterator<Item = &str> {
        self.fonts.iter().map(|font| font.id.as_str())
    }

    pub fn embedded_font_count(&self) -> usize {
        self.fonts.iter().filter(|font| font.embedded()).count()
    }

    pub fn font(&self, id: &str) -> Option<&FontResource> {
        self.fonts
            .iter()
            .find(|font| font.id == id && font.scope == ResScope::Document)
            .or_else(|| self.fonts.iter().find(|font| font.id == id))
    }

    pub fn composite_unit(&self, resource_id: &str) -> Option<&CompositeUnit> {
        let mut parts = resource_id.split_whitespace();
        let id = parts.next()?;
        let scope = parts.next().and_then(|kind| match kind {
            "0" => Some(ResScope::Public),
            "1" => Some(ResScope::Document),
            _ => None,
        });
        if let Some(scope) = scope {
            return self
                .composite_units
                .iter()
                .find(|unit| unit.id == id && unit.scope == scope);
        }
        self.composite_units
            .iter()
            .find(|unit| unit.id == id && unit.scope == ResScope::Document)
            .or_else(|| self.composite_units.iter().find(|unit| unit.id == id))
    }

    pub fn image(&self, resource_id: &str) -> Option<&ImageResource> {
        let mut parts = resource_id.split_whitespace();
        let id = parts.next()?;
        let scope = parts.next().and_then(|kind| match kind {
            "0" => Some(ResScope::Public),
            "1" => Some(ResScope::Document),
            _ => None,
        });

        if let Some(scope) = scope {
            return self
                .images
                .iter()
                .find(|image| image.id == id && image.scope == scope);
        }

        self.images
            .iter()
            .find(|image| image.id == id && image.scope == ResScope::Document)
            .or_else(|| self.images.iter().find(|image| image.id == id))
    }

    pub fn draw_param(&self, id: &str) -> Option<DrawParam> {
        let initial = self
            .draw_params
            .iter()
            .find(|param| param.id == id)?
            .clone();
        let mut resolved = initial.clone();
        let mut current = initial;
        for _ in 0..8 {
            let Some(relative) = current.relative.as_deref() else {
                break;
            };
            let Some(base) = self.draw_params.iter().find(|param| param.id == relative) else {
                break;
            };
            resolved.merge_from(base);
            current = base.clone();
        }
        Some(resolved)
    }
}

#[derive(Debug, Clone)]
pub struct AnnotInfo {
    pub id: String,
    pub kind: String,
    pub subtype: Option<String>,
    pub boundary: Option<String>,
    pub text: Vec<String>,
    pub image_resources: Vec<String>,
    pub path_objects: usize,
}

#[derive(Debug, Clone)]
pub struct AnnotationPage {
    pub page_id: String,
    pub file: String,
    pub annots: Vec<AnnotInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct PageSummary {
    pub index: usize,
    pub id: String,
    pub size: Option<SizeMm>,
    pub text_objects: usize,
    pub path_objects: usize,
    pub image_objects: usize,
    pub composite_objects: usize,
    pub page_blocks: usize,
    pub cg_transforms: usize,
    pub templates: Vec<String>,
    pub text: Vec<String>,
    pub used_fonts: Vec<String>,
    pub warnings: Vec<String>,
}

impl PageSummary {
    pub fn object_count(&self) -> usize {
        self.text_objects + self.path_objects + self.image_objects + self.composite_objects
    }
}

/// Parse an `ST_Box` value: `x y width height`.
pub fn parse_box(value: &str) -> Option<(f64, f64, f64, f64)> {
    let mut values = value.split_whitespace();
    let x = values.next()?.parse().ok()?;
    let y = values.next()?.parse().ok()?;
    let w = values.next()?.parse().ok()?;
    let h = values.next()?.parse().ok()?;
    Some((x, y, w, h))
}

/// `PhysicalBox` uses the same four-number syntax; only width/height matter here.
pub fn parse_physical_box(value: &str) -> Option<SizeMm> {
    let (_, _, width, height) = parse_box(value)?;
    let size = SizeMm::new(width, height);
    size.is_valid().then_some(size)
}

#[cfg(test)]
mod tests;
