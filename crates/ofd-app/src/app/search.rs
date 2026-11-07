use super::search_state::SearchHit;
use crate::*;

impl ReaderApp {
    pub(crate) fn on_toggle_search(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_search(window, cx);
    }

    pub(crate) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.open {
            self.close_search(window, cx);
            return;
        }
        self.ensure_search_input(window, cx);
        self.search.open = true;
        // Results live in the sidebar, so make sure it is visible when a
        // search starts.
        if self.active.is_open() {
            self.sidebar_open = true;
        }
        if let Some(input) = &self.search.input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        self.run_search(cx);
    }

    pub(crate) fn ensure_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.search.input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索文档内容"));
        cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Change => {
                if !this.search.suppress_change {
                    this.run_search(cx);
                }
            }
            InputEvent::PressEnter { shift, .. } => {
                this.search_step(if *shift { -1 } else { 1 }, cx);
            }
            _ => {}
        })
        .detach();
        self.search.input = Some(input.clone());
        input
    }

    pub(crate) fn set_search_input_value(
        &mut self,
        window: &mut Window,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        if value.is_empty() && self.search.input.is_none() {
            return;
        }
        let input = self.ensure_search_input(window, cx);
        self.search.suppress_change = true;
        input.update(cx, |state, cx| state.set_value(value, window, cx));
        self.search.suppress_change = false;
    }

    /// Restores the tab's search query and selected hit after the document has
    /// been reopened. The input widget is only synced when a window is around.
    pub(crate) fn restore_tab_search(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.search.pending_index = None;
        self.search.pending_input = None;
        let saved_index = self.search.index;
        if !self.search.open {
            if let Some(window) = window {
                self.set_search_input_value(window, "", cx);
            } else {
                self.search.pending_input = Some(String::new());
            }
            return;
        }
        let query = self.search.query.clone();
        if let Some(window) = window {
            self.set_search_input_value(window, &query, cx);
        } else {
            self.search.pending_input = Some(query.clone());
        }
        self.apply_search_query(query.clone(), cx);
        if !self.search.hits.is_empty() {
            self.search.index = saved_index.min(self.search.hits.len() - 1);
            self.sync_search_active_highlight();
            self.search.pending_index = Some((query, self.search.index));
        }
    }

    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.open = false;
        self.search.query.clear();
        self.search.hits.clear();
        self.search.index = 0;
        self.search.highlights.clear();
        self.search.active_highlight = None;
        self.search.pending_index = None;
        self.search.pending_input = None;
        if let Some(input) = &self.search.input {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Case-sensitive substring search over the extracted text of every page.
    /// One hit is produced per occurrence so Enter walks through them in order.
    pub(crate) fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self
            .search
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.apply_search_query(query, cx);
    }

    pub(crate) fn apply_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search.query = query;
        self.search.hits.clear();
        self.search.index = 0;
        self.search
            .results_scroll
            .set_offset(point(px(0.0), px(0.0)));

        let needle = self.search.query.trim();
        if needle.is_empty() {
            self.search.pending_index = None;
            self.search.highlights.clear();
            self.search.active_highlight = None;
            cx.notify();
            return;
        }
        let Some(doc) = &self.active.doc else {
            self.search.pending_index = None;
            self.search.highlights.clear();
            self.search.active_highlight = None;
            cx.notify();
            return;
        };
        if self.active.page_text_cache.is_none() {
            self.active.page_text_cache = Some(
                (0..self.active.page_count())
                    .map(|page| doc.page_display_text(page).unwrap_or_default())
                    .collect(),
            );
        }
        let pages = self.active.page_text_cache.as_ref().expect("just built");

        'pages: for (page, texts) in pages.iter().enumerate() {
            let mut page_match = 0;
            for text in texts {
                let text = text.replace(['\n', '\r'], " ");
                let mut from = 0;
                while let Some(offset) = text[from..].find(needle) {
                    let start = from + offset;
                    let end = start + needle.len();
                    self.search.hits.push(SearchHit {
                        page,
                        match_index: page_match,
                        snippet: search_snippet(&text, start, end),
                    });
                    page_match += 1;
                    if self.search.hits.len() >= SEARCH_LIMIT {
                        break 'pages;
                    }
                    from = end;
                }
            }
        }
        if let Some((pending_query, pending_index)) = self.search.pending_index.take()
            && pending_query == self.search.query
        {
            self.search.index = pending_index.min(self.search.hits.len().saturating_sub(1));
        }
        self.refresh_search_highlights();
        cx.notify();
    }

    /// Recomputes on-page highlight rectangles for the current page and query.
    pub(crate) fn refresh_search_highlights(&mut self) {
        self.search.highlights.clear();
        self.search.active_highlight = None;
        if !self.search.open {
            return;
        }
        let query = self.search.query.trim();
        if query.is_empty() {
            return;
        }
        let Some(doc) = self.active.doc.as_ref() else {
            return;
        };
        let size = self
            .active
            .page_sizes
            .get(self.page)
            .copied()
            .unwrap_or(SizeMm::new(210.0, 297.0));
        if let Ok(rects) = doc.page_search_rects(self.page, query) {
            self.search.highlights =
                rotate::highlights(search_highlight::normalize(size, &rects), self.rotation);
            self.sync_search_active_highlight();
        }
    }

    /// Points the distinct on-page colour at the result selected in the list.
    pub(crate) fn sync_search_active_highlight(&mut self) {
        self.search.active_highlight = self
            .search
            .hits
            .get(self.search.index)
            .filter(|hit| hit.page == self.page)
            .and_then(|hit| self.search.highlights.get(hit.match_index).cloned());
    }

    /// Reveals the selected hit: switches page when needed and centres the
    /// match in the continuous view.
    pub(crate) fn reveal_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.settings.reading.search_auto_scroll {
            cx.notify();
            return;
        }
        let Some(hit) = self.search.hits.get(index) else {
            return;
        };
        let (page, match_index) = (hit.page, hit.match_index);
        if page != self.page {
            self.go_to_page(page, cx);
        }
        if self.view_mode == ViewMode::Continuous {
            // Scrolling clears the rects until it settles; selecting a result
            // must restore them even if the pointer arrives mid-scroll.
            if self.search.highlights.is_empty() {
                self.refresh_search_highlights();
            }
            let viewport = self.viewport;
            self.scroll_to_search_match(viewport, page, match_index, cx);
        }
        cx.notify();
    }

    pub(crate) fn search_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.search.hits.is_empty() {
            return;
        }
        let len = self.search.hits.len() as isize;
        self.search.index = (self.search.index as isize + delta).rem_euclid(len) as usize;
        self.search.results_scroll.scroll_to_item(self.search.index);
        self.sync_search_active_highlight();
        self.reveal_search_hit(self.search.index, cx);
    }

    pub(crate) fn select_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.search.hits.len() {
            return;
        }
        self.search.index = index;
        self.search.results_scroll.scroll_to_item(index);
        self.sync_search_active_highlight();
        self.reveal_search_hit(index, cx);
    }
}
