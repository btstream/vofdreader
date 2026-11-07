use std::sync::Arc;

use resvg::tiny_skia as sk;
use resvg::usvg::fontdb;

/// A system-font outline in font units, with that face's units per em.
pub(crate) type SystemGlyph = Option<(Arc<sk::Path>, f32)>;

fn face_has_char(db: &fontdb::Database, id: fontdb::ID, ch: char) -> bool {
    db.with_face_data(id, |data, index| {
        ttf_parser::Face::parse(data, index)
            .ok()
            .and_then(|face| face.glyph_index(ch))
            .is_some_and(|glyph| glyph.0 != 0)
    })
    .unwrap_or(false)
}

/// usvg's default fallback: base face if it maps the char, otherwise the
/// first face in database order with a compatible style that does.
pub(crate) fn select_face(db: &fontdb::Database, base: fontdb::ID, ch: char) -> Option<fontdb::ID> {
    if face_has_char(db, base, ch) {
        return Some(base);
    }
    let base_face = db.face(base)?;
    for face in db.faces() {
        if face.id == base {
            continue;
        }
        if base_face.style != face.style
            && base_face.weight != face.weight
            && base_face.stretch != face.stretch
        {
            continue;
        }
        if face_has_char(db, face.id, ch) {
            return Some(face.id);
        }
    }
    // usvg keeps the base face and draws its .notdef box; nothing visible
    // is lost by skipping an outline-less missing glyph.
    Some(base)
}

pub(crate) fn system_outline(db: &fontdb::Database, id: fontdb::ID, ch: char) -> SystemGlyph {
    db.with_face_data(id, |data, index| {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let glyph = face.glyph_index(ch).unwrap_or(ttf_parser::GlyphId(0));
        let mut builder = SkOutline(sk::PathBuilder::new());
        face.outline_glyph(glyph, &mut builder)?;
        Some((
            Arc::new(builder.0.finish()?),
            f32::from(face.units_per_em()),
        ))
    })
    .flatten()
}

pub(crate) struct SkOutline(sk::PathBuilder);

impl ttf_parser::OutlineBuilder for SkOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}
