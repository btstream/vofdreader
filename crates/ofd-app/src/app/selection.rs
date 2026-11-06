use crate::*;

impl ReaderApp {
    pub(crate) fn ensure_text_layout(&mut self) {
        if self.text_layout.is_some() {
            return;
        }
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        self.text_layout = doc.page_text_layout(self.page).ok();
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_focus = None;
        self.selection_rects.clear();
        self.selected_text.clear();
        self.selecting = false;
        self.selection_moved = false;
    }

    /// Converts a window position into page millimetres. `clamp` is used while
    /// dragging so the selection still follows the pointer outside the page.
    pub(crate) fn page_point(&self, position: Point<Pixels>, clamp: bool) -> Option<(f64, f64)> {
        let bounds = self.page_bounds.get()?;
        let size = self.page_sizes.get(self.page).copied()?;
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let mut x = (f32::from(position.x) - f32::from(bounds.origin.x)) / width;
        let mut y = (f32::from(position.y) - f32::from(bounds.origin.y)) / height;
        if !clamp && !(0.0..=1.0).contains(&x) {
            return None;
        }
        if !clamp && !(0.0..=1.0).contains(&y) {
            return None;
        }
        x = x.clamp(0.0, 1.0);
        y = y.clamp(0.0, 1.0);
        let (x, y) = rotate::inverse_point(x as f64, y as f64, self.rotation);
        Some((x * size.width, y * size.height))
    }

    pub(crate) fn update_selection(&mut self) {
        self.selection_rects.clear();
        self.selected_text.clear();
        let (Some(layout), Some(anchor), Some(focus)) = (
            &self.text_layout,
            self.selection_anchor,
            self.selection_focus,
        ) else {
            return;
        };
        let start = anchor.min(focus);
        let end = anchor.max(focus) + 1;
        let size = self
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        self.selected_text = layout.text_range(start, end);
        self.selection_rects = rotate::highlights(
            search_highlight::normalize(size, &layout.rects(start, end)),
            self.rotation,
        );
    }

    pub(crate) fn on_page_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        self.ensure_text_layout();
        self.selecting = false;
        self.selection_moved = false;
        let Some((x, y)) = self.page_point(event.position, false) else {
            self.clear_selection();
            cx.notify();
            return;
        };
        let index = self
            .text_layout
            .as_ref()
            .and_then(|layout| layout.index_at(x, y, 8.0));
        if let Some(index) = index {
            self.selection_anchor = Some(index);
            self.selection_focus = Some(index);
            self.selecting = true;
            self.update_selection();
        } else {
            self.clear_selection();
        }
        cx.notify();
    }

    pub(crate) fn update_selection_drag(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        let Some((x, y)) = self.page_point(position, true) else {
            return;
        };
        let Some(index) = self
            .text_layout
            .as_ref()
            .and_then(|layout| layout.index_at(x, y, f64::INFINITY))
        else {
            return;
        };
        if self.selection_focus != Some(index) {
            self.selection_focus = Some(index);
            self.selection_moved = true;
            self.update_selection();
            cx.notify();
        }
    }

    pub(crate) fn on_page_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        if !self.selection_moved {
            self.clear_selection();
        }
        cx.notify();
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text.clone()));
        let count = self.selected_text.chars().count();
        self.set_notice(format!("已复制 {count} 个字符"), false, cx);
        cx.notify();
    }
}
