use crate::*;

impl ReaderApp {
    pub(crate) fn save_position(&mut self) {
        if let Some(path) = &self.active.path {
            self.settings.general.last_file = Some(path.to_string_lossy().into_owned());
            self.settings.remember_position(path, self.page);
        }
    }

    /// Stores the active reader state in its tab before the document is released.
    pub(crate) fn snapshot_active_tab(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        if index >= self.tabs.len() {
            return;
        }
        let reading = self.reading_snapshot();
        let search = self.search.snapshot();
        let tab = &mut self.tabs[index];
        tab.search = search;
        if !self.active.is_open() {
            return;
        }
        tab.reading = reading;
        tab.file_name = self.active.file_name.clone();
        if let Some(path) = &self.active.path {
            tab.path = path.clone();
        }
    }

    /// Fraction of the current page that is scrolled above the viewport.
    pub(crate) fn current_page_offset(&self) -> f32 {
        if self.view_mode != ViewMode::Continuous || self.active.page_count() == 0 {
            return 0.0;
        }
        let viewport = self.viewport;
        let (top, height) = self.with_continuous_layout(viewport, |layout| {
            (
                layout.top(self.page),
                layout.sizes.get(self.page).map_or(0.0, |size| size.1),
            )
        });
        if height <= 0.0 {
            return 0.0;
        }
        let scroll = -f32::from(self.continuous_scroll.offset().y);
        ((scroll - top) / height).clamp(0.0, 1.0)
    }

    pub(crate) fn activate_tab(
        &mut self,
        index: usize,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        if self.active_tab == Some(index) && self.active.is_open() {
            return;
        }
        if self.active_tab != Some(index) {
            self.snapshot_active_tab();
        }
        self.load_tab(index, window, cx);
    }

    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = self.active_tab == Some(index);
        if was_active {
            self.tabs.remove(index);
            self.active_tab = None;
            if self.tabs.is_empty() {
                self.clear_active_document(cx);
                self.active.path = None;
                self.active.file_name.clear();
                self.active.file_size = 0;
                self.page = 0;
                self.pending_restore_page = None;
                self.pending_restore_scroll = None;
                self.settings.general.last_file = None;
                self.settings.save();
                self.set_search_input_value(window, "", cx);
                window.focus(&self.focus_handle, cx);
                cx.notify();
                return;
            }
            let next = index.min(self.tabs.len() - 1);
            self.activate_tab(next, Some(window), cx);
        } else {
            self.tabs.remove(index);
            if let Some(active) = self.active_tab
                && index < active
            {
                self.active_tab = Some(active - 1);
            }
            let active = self.active_tab.and_then(|active| self.tabs.get(active));
            self.settings.general.last_file =
                active.map(|tab| tab.path.to_string_lossy().into_owned());
            self.settings.save();
            cx.notify();
        }
    }
}
