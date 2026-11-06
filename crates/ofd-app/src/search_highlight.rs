use ofd_core::{SizeMm, TextMatchRect};

/// Highlight rectangle normalized to the rendered page area.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Converts page-millimetre rectangles into 0..1 page-relative rectangles.
pub fn normalize(size: SizeMm, rects: &[TextMatchRect]) -> Vec<Highlight> {
    if size.width <= 0.0 || size.height <= 0.0 {
        return Vec::new();
    }
    rects
        .iter()
        .filter_map(|rect| {
            let left = (rect.x / size.width).clamp(0.0, 1.0) as f32;
            let top = (rect.y / size.height).clamp(0.0, 1.0) as f32;
            let right = ((rect.x + rect.width) / size.width).clamp(0.0, 1.0) as f32;
            let bottom = ((rect.y + rect.height) / size.height).clamp(0.0, 1.0) as f32;
            let width = right - left;
            let height = bottom - top;
            if width <= 0.0 || height <= 0.0 {
                return None;
            }
            Some(Highlight {
                x: left,
                y: top,
                width,
                height,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
