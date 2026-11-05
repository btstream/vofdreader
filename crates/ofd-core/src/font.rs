//! Embedded font access, independent of system font selection.
use crate::model::FontResource;
use crate::render::Matrix;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

// Short-lived per text run: keep borrowed parsers on the stack, avoiding a
// heap allocation for every SFNT run during rendering and auditing.
#[allow(clippy::large_enum_variant)]
pub(crate) enum EmbeddedFace<'a> {
    Sfnt(Face<'a>),
    Cff(ttf_parser::cff::Table<'a>),
}

impl<'a> EmbeddedFace<'a> {
    pub fn parse(data: &'a [u8], index: u32) -> Option<Self> {
        Face::parse(data, index).ok().map(Self::Sfnt).or_else(|| {
            (index == 0)
                .then(|| ttf_parser::cff::Table::parse(data))
                .flatten()
                .map(Self::Cff)
        })
    }

    /// Font coordinates to em coordinates, including a raw CFF FontMatrix.
    pub fn matrix(&self) -> Matrix {
        match self {
            Self::Sfnt(face) => {
                let scale = 1.0 / f64::from(face.units_per_em());
                Matrix {
                    a: scale,
                    d: scale,
                    ..Matrix::IDENTITY
                }
            }
            Self::Cff(face) => {
                let m = face.matrix();
                Matrix {
                    a: m.sx.into(),
                    b: m.ky.into(),
                    c: m.kx.into(),
                    d: m.sy.into(),
                    e: m.tx.into(),
                    f: m.ty.into(),
                }
            }
        }
    }

    pub fn glyph(&self, ch: char) -> Option<u16> {
        match self {
            Self::Sfnt(face) => face.glyph_index(ch).map(|id| id.0),
            // Raw CFF has no Unicode cmap. Glyph names are a safe Unicode
            // mapping for uniXXXX/uXXXXX and basic ASCII; encoding bytes are
            // deliberately not treated as Unicode (e.g. custom encodings).
            Self::Cff(face) => {
                let code = ch as u32;
                let name = if code <= 0xffff {
                    format!("uni{code:04X}")
                } else {
                    format!("u{code:X}")
                };
                face.glyph_index_by_name(&name)
                    .or_else(|| {
                        if ch.is_ascii_alphabetic() {
                            face.glyph_index_by_name(&ch.to_string())
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        let name = match ch {
                            ' ' => "space",
                            '0' => "zero",
                            '1' => "one",
                            '2' => "two",
                            '3' => "three",
                            '4' => "four",
                            '5' => "five",
                            '6' => "six",
                            '7' => "seven",
                            '8' => "eight",
                            '9' => "nine",
                            '.' => "period",
                            ',' => "comma",
                            ':' => "colon",
                            ';' => "semicolon",
                            '-' => "hyphen",
                            '(' => "parenleft",
                            ')' => "parenright",
                            '/' => "slash",
                            _ => return None,
                        };
                        face.glyph_index_by_name(name)
                    })
                    .map(|id| id.0)
            }
        }
    }

    pub fn outline(&self, id: u16, builder: &mut dyn OutlineBuilder) -> bool {
        if id == 0 {
            return false;
        }
        match self {
            Self::Sfnt(face) => face.outline_glyph(GlyphId(id), builder).is_some(),
            Self::Cff(face) => face.outline(GlyphId(id), builder).is_ok(),
        }
    }

    /// Standalone CID-keyed CFF subsets retain the original CIDs in OFD
    /// Glyphs while CharStrings are densely renumbered. SFNT glyph IDs are
    /// already indices and must not pass through this charset conversion.
    pub fn explicit_glyph(&self, id: u16) -> Option<u16> {
        if let Self::Cff(face) = self
            && face.glyph_cid(GlyphId(0)).is_some()
        {
            return (0..face.number_of_glyphs())
                .find(|index| face.glyph_cid(GlyphId(*index)) == Some(id));
        }
        Some(id)
    }

    pub fn advance(&self, id: u16) -> f64 {
        match self {
            Self::Sfnt(face) => {
                f64::from(
                    face.glyph_hor_advance(GlyphId(id))
                        .unwrap_or(face.units_per_em()),
                ) * self.matrix().a
            }
            Self::Cff(face) => {
                f64::from(face.glyph_width(GlyphId(id)).unwrap_or(1000)) * self.matrix().a
            }
        }
    }

    pub fn weight(&self) -> u16 {
        match self {
            Self::Sfnt(face) => face.weight().to_number(),
            Self::Cff(_) => 400,
        }
    }

    pub fn italic(&self) -> bool {
        matches!(self, Self::Sfnt(face) if face.is_italic() || face.is_oblique())
    }
}

fn normalized_name(name: &str) -> String {
    let name = name
        .split_once('+')
        .filter(|(prefix, _)| prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()))
        .map_or(name, |(_, name)| name);
    name.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn select_face(data: &[u8], font: &FontResource) -> u32 {
    let Some(count) = ttf_parser::fonts_in_collection(data) else {
        return 0;
    };
    let wanted = [normalized_name(&font.name), normalized_name(&font.family)];
    let mut best = None;
    for index in 0..count {
        let Ok(face) = Face::parse(data, index) else {
            continue;
        };
        let mut score = 0;
        for name in face.names() {
            if !matches!(name.name_id, 1 | 4 | 6 | 16) {
                continue;
            }
            let Some(value) = name.to_string() else {
                continue;
            };
            let value = normalized_name(&value);
            if !value.is_empty() && value == wanted[0] {
                score = score.max(if matches!(name.name_id, 4 | 6) { 4 } else { 3 });
            }
            if !value.is_empty() && value == wanted[1] {
                score = score.max(2);
            }
        }
        let style_score =
            u16::from(face.is_bold() == font.bold) + u16::from(face.is_italic() == font.italic);
        let score = score * 10 + style_score;
        if best.is_none_or(|(previous, _)| score > previous) {
            best = Some((score, index));
        }
    }
    best.map_or(0, |(_, index)| index)
}
