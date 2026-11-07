//! Text-selection state. `app/selection.rs` holds the pointer-driven
//! behaviour; this type only owns the ranges and derived rectangles.

use crate::*;

/// Character range on the current page plus its derived highlight rectangles
/// and copied text. The layout is rebuilt whenever the page changes.
pub(crate) struct SelectionState {
    pub(crate) layout: Option<PageTextLayout>,
    pub(crate) anchor: Option<usize>,
    pub(crate) focus: Option<usize>,
    pub(crate) rects: Vec<search_highlight::Highlight>,
    pub(crate) text: String,
    pub(crate) dragging: bool,
    pub(crate) moved: bool,
}

impl SelectionState {
    pub(crate) fn new() -> Self {
        Self {
            layout: None,
            anchor: None,
            focus: None,
            rects: Vec::new(),
            text: String::new(),
            dragging: false,
            moved: false,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor.is_none() && self.focus.is_none()
    }

    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.rects.clear();
        self.text.clear();
        self.dragging = false;
        self.moved = false;
    }
}
