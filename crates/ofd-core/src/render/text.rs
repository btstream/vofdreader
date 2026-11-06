use super::types::*;

impl PageTextLayout {
    pub(crate) fn from_items(items: &[Item]) -> Self {
        let mut chars = Vec::new();
        let mut run = 0u32;
        let mut line = 0u32;
        let mut previous: Option<(f64, f64)> = None;

        for item in items {
            let Item::Text {
                transform,
                chars: positioned,
                style,
                ..
            } = item
            else {
                continue;
            };
            for index in 0..positioned.len() {
                let Some(rect) =
                    text_run_rect(transform, positioned, index, index + 1, style.font_size)
                else {
                    continue;
                };
                let (_, baseline) =
                    transform.transform_point(positioned[index].x, positioned[index].y);
                if let Some((previous_baseline, previous_x)) = previous
                    && ((baseline - previous_baseline).abs() > style.font_size * 0.6
                        || rect.x + style.font_size * 0.1 < previous_x)
                {
                    line += 1;
                }
                previous = Some((baseline, rect.x));
                chars.push(PageTextChar {
                    ch: positioned[index].ch,
                    rect,
                    line,
                    run,
                });
            }
            run = run.wrapping_add(1);
        }

        Self { chars }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Index of the character under the point, or the nearest one within
    /// `max_distance` millimetres of its bounding box.
    pub fn index_at(&self, x: f64, y: f64, max_distance: f64) -> Option<usize> {
        let mut nearest: Option<(usize, f64)> = None;
        for (index, character) in self.chars.iter().enumerate() {
            let rect = character.rect;
            if x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height {
                return Some(index);
            }
            let center_x = rect.x + rect.width / 2.0;
            let center_y = rect.y + rect.height / 2.0;
            let distance = (x - center_x).powi(2) + (y - center_y).powi(2);
            if nearest.is_none_or(|(_, best)| distance < best) {
                nearest = Some((index, distance));
            }
        }
        nearest
            .filter(|(_, distance)| *distance <= max_distance * max_distance)
            .map(|(index, _)| index)
    }

    pub fn text_range(&self, start: usize, end: usize) -> String {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len());
        if start >= end {
            return String::new();
        }
        let mut text = String::new();
        for index in start..end {
            let character = &self.chars[index];
            if index > start {
                let previous = &self.chars[index - 1];
                if character.line != previous.line {
                    text.push('\n');
                } else if character.run != previous.run && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
            }
            text.push(character.ch);
        }
        text
    }

    pub fn rects(&self, start: usize, end: usize) -> Vec<TextMatchRect> {
        let start = start.min(self.chars.len());
        let end = end.min(self.chars.len());
        self.chars[start..end]
            .iter()
            .map(|char| char.rect)
            .collect()
    }
}

pub(crate) fn collect_text_matches(
    items: &[Item],
    query: &str,
    parent: Matrix,
    out: &mut Vec<TextMatchRect>,
) {
    for item in items {
        match item {
            Item::Text {
                transform,
                chars,
                style,
                ..
            } => {
                let transform = parent.compose(*transform);
                let text: String = chars.iter().map(|char| char.ch).collect();
                let mut from = 0;
                while let Some(offset) = text[from..].find(query) {
                    let start = from + offset;
                    let end = start + query.len();
                    let char_start = text[..start].chars().count();
                    let char_end = text[..end].chars().count();
                    if char_start < char_end
                        && char_end <= chars.len()
                        && let Some(rect) =
                            text_run_rect(&transform, chars, char_start, char_end, style.font_size)
                    {
                        out.push(rect);
                    }
                    from = end;
                }
            }
            Item::Group {
                transform, items, ..
            } => collect_text_matches(items, query, parent.compose(*transform), out),
            Item::Clip { items, .. } => collect_text_matches(items, query, parent, out),
            _ => {}
        }
    }
}

pub(crate) fn text_run_rect(
    transform: &Matrix,
    chars: &[PositionedChar],
    start: usize,
    end: usize,
    font_size: f64,
) -> Option<TextMatchRect> {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for index in start..end {
        let positioned = &chars[index];
        let advance = chars
            .get(index + 1)
            .map(|next| next.x - positioned.x)
            .filter(|advance| *advance > 0.0)
            .unwrap_or_else(|| default_advance(positioned.ch, font_size));
        let top = positioned.y - font_size * 0.88;
        let bottom = positioned.y + font_size * 0.12;
        for (x, y) in [
            (positioned.x, top),
            (positioned.x + advance, top),
            (positioned.x + advance, bottom),
            (positioned.x, bottom),
        ] {
            let (page_x, page_y) = transform.transform_point(x, y);
            min_x = min_x.min(page_x);
            min_y = min_y.min(page_y);
            max_x = max_x.max(page_x);
            max_y = max_y.max(page_y);
        }
    }

    let width = max_x - min_x;
    let height = max_y - min_y;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(TextMatchRect {
        x: min_x,
        y: min_y,
        width,
        height,
    })
}

pub(crate) fn default_advance(ch: char, size: f64) -> f64 {
    if ch == ' ' {
        size * 0.5
    } else if ch.is_ascii() {
        size * 0.55
    } else {
        size
    }
}
