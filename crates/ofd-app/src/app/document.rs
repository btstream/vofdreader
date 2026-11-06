use crate::*;

impl ReaderApp {
    /// Releases every rendering resource owned by the active tab, then clears
    /// the document state. Inactive tabs keep only their `DocumentTab` snapshot.
    pub(crate) fn clear_active_document(&mut self, cx: &mut Context<Self>) {
        self.doc_generation = self.doc_generation.wrapping_add(1);
        let gpu = self.gpu.clone();
        gpu.borrow_mut()
            .release_all(|image| cx.drop_image(image, None));
        self.doc = None;
        self.renderer = None;
        self.page_image = None;
        self.image_dpi = 0;
        self.page_cache.clear();
        self.page_cache_order.clear();
        self.page_cache_bytes = 0;
        self.clear_tiles();
        self.tile_order.clear();
        self.tile_bytes = 0;
        self.font_reports.clear();
        self.continuous_pending.clear();
        self.continuous_failed.clear();
        self.continuous_last_scroll = None;
        self.continuous_settle_pending = false;
        self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
        self.continuous_layout = RefCell::new(continuous::LayoutCache::default());
        self.page_sizes_revision = self.page_sizes_revision.wrapping_add(1);
        self.render_seq = self.render_seq.wrapping_add(1);
        self.latest_render = None;
        self.thumbs.clear();
        self.thumbs_started = false;
        self.page_text_cache = None;
        self.text_layout = None;
        self.page_bounds.set(None);
        self.clear_selection();
        self.search_highlights.clear();
        self.search_active_highlight = None;
        self.search_hits.clear();
        self.search_index = 0;
        self.search_open = false;
        self.search_query.clear();
        self.search_pending_index = None;
        self.search_pending_input = None;
        self.pending_restore_page = None;
        self.pending_restore_scroll = None;
        self.page_count = 0;
        self.page_sizes.clear();
        self.loading = false;
        self.error = None;
        self.notice = None;
        self.exporting = false;
        self.grid_open = false;
        self.info_dialog_open = false;
        self.bar_visible = false;
        self.bar_hovered = false;
    }

    pub(crate) fn open_path(
        &mut self,
        path: PathBuf,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.save_position();
        self.settings.save();
        // Recent files and the export directory should not depend on the
        // directory the application happened to start in.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate_tab(index, window, cx);
            return;
        }
        self.snapshot_active_tab();
        let page = if self.settings.reading.restore_position {
            self.settings.position(&path).unwrap_or(0)
        } else {
            0
        };
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.tabs.push(DocumentTab {
            path,
            file_name,
            page,
            zoom: 1.0,
            rotation: 0,
            view_mode: match self.settings.reading.default_view {
                settings::ViewChoice::Single => ViewMode::Single,
                settings::ViewChoice::Continuous => ViewMode::Continuous,
            },
            search_open: false,
            search_query: String::new(),
            search_index: 0,
            page_offset: 0.0,
        });
        let index = self.tabs.len() - 1;
        self.load_tab(index, window, cx);
    }

    /// Releases the previous document, restores the tab's reading state and
    /// opens the file again through the ordinary single-document path.
    pub(crate) fn load_tab(
        &mut self,
        index: usize,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index).cloned() else {
            return;
        };
        self.clear_active_document(cx);
        self.active_tab = Some(index);
        self.file_path = Some(tab.path.clone());
        self.file_name = tab.file_name;
        self.zoom = tab.zoom.max(0.1);
        self.rotation = tab.rotation % 4;
        self.view_mode = tab.view_mode;
        self.search_open = tab.search_open;
        self.search_query = tab.search_query.clone();
        self.search_index = tab.search_index;
        self.pending_restore_scroll = (tab.page_offset > 0.0).then_some(tab.page_offset);
        self.load_document(tab.path, tab.page, window, cx);
    }

    pub(crate) fn load_document(
        &mut self,
        path: PathBuf,
        target_page: usize,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match OfdDocument::open(&path) {
            Ok(doc) => {
                let doc = Arc::new(doc);
                self.file_path = Some(path.clone());
                self.file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                let recent_limit = self.settings.files.recent_limit;
                recent::push_recent(&mut self.recent, &path, recent_limit);
                recent::save_recent(&self.recent, recent_limit);
                self.page_count = doc.page_count();
                self.page = target_page.min(self.page_count.saturating_sub(1));
                self.pending_restore_page = (self.page_count > 0
                    && (self.page > 0 || self.pending_restore_scroll.is_some()))
                .then_some(self.page);
                let fallback = doc.meta.page_area.unwrap_or(SizeMm::new(210.0, 297.0));
                // Do not scan every page on the UI thread. The visible page is
                // resolved synchronously; the rest are filled in the background.
                self.page_sizes = vec![fallback; self.page_count];
                self.page_sizes_revision = self.page_sizes_revision.wrapping_add(1);
                if let Ok(Some(size)) = doc.page_size_only(self.page)
                    && self.page < self.page_sizes.len()
                {
                    self.page_sizes[self.page] = size;
                }
                self.renderer = None;
                self.doc = Some(doc.clone());
                self.doc_generation = self.doc_generation.wrapping_add(1);
                self.page_image = None;
                self.image_dpi = 0;
                self.page_cache.clear();
                self.clear_tiles();
                self.font_reports.clear();
                self.continuous_pending.clear();
                self.continuous_failed.clear();
                self.continuous_last_scroll = None;
                self.continuous_settle_pending = false;
                self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                self.page_cache_order.clear();
                self.page_cache_bytes = 0;
                self.render_seq = self.render_seq.wrapping_add(1);
                self.latest_render = None;
                self.thumbs = vec![None; self.page_count];
                self.thumbs_started = false;
                self.page_text_cache = None;
                self.search_highlights.clear();
                self.search_active_highlight = None;
                self.clear_selection();
                self.grid_open = false;
                self.info_dialog_open = false;
                self.error = None;
                self.notice = None;
                self.loading = true;
                self.wheel_accum = 0.0;
                self.settings.general.last_file = Some(path.to_string_lossy().into_owned());
                self.settings.save();
                self.restore_tab_search(window.as_deref_mut(), cx);
                if let Some(window) = window {
                    if self.search_open {
                        if let Some(input) = &self.search_input {
                            input.update(cx, |state, cx| state.focus(window, cx));
                        }
                    } else {
                        window.focus(&self.focus_handle, cx);
                    }
                }

                // Font database construction is expensive; keep the UI thread
                // free so the window chrome and empty state paint immediately.
                let generation = self.doc_generation;
                let renderer_doc = doc.clone();
                let parsed_page_limit = self.cache_config.parsed_page_limit();
                let image_filter = self.settings.performance.image_sampling.filter();
                cx.spawn(async move |this, cx| {
                    let renderer = cx
                        .background_spawn(async move {
                            PageRenderer::with_tree_cache_limit(&renderer_doc, parsed_page_limit)
                                .with_image_filter(image_filter)
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation {
                            return;
                        }
                        this.renderer = Some(Arc::new(renderer));
                        this.request_page_render(cx);
                        cx.notify();
                    });
                })
                .detach();

                let sizes_doc = doc.clone();
                let generation = self.doc_generation;
                cx.spawn(async move |this, cx| {
                    let sizes = cx
                        .background_spawn(async move {
                            (0..sizes_doc.page_count())
                                .map(|index| sizes_doc.page_size_only(index).ok().flatten())
                                .collect::<Vec<_>>()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.doc_generation != generation {
                            return;
                        }
                        let mut changed = false;
                        for (index, size) in sizes.into_iter().enumerate() {
                            if let Some(size) = size
                                && index < this.page_sizes.len()
                                && this.page_sizes[index] != size
                            {
                                this.page_sizes[index] = size;
                                changed = true;
                            }
                        }
                        if changed {
                            this.page_sizes_revision = this.page_sizes_revision.wrapping_add(1);
                            if this.view_mode == ViewMode::Continuous {
                                this.ensure_continuous_pages(this.viewport, cx);
                            }
                            cx.notify();
                        }
                        if let Some(page) = this.pending_restore_page.take()
                            && this.view_mode == ViewMode::Continuous
                        {
                            let offset = this.pending_restore_scroll.take().unwrap_or(0.0);
                            this.scroll_to_continuous_page_offset(this.viewport, page, offset);
                            this.sync_continuous_page(this.viewport, cx);
                            cx.notify();
                        }
                    });
                })
                .detach();
                self.bar_visible = false;
                self.bar_hovered = false;
            }
            Err(error) => {
                self.bar_visible = false;
                self.bar_hovered = false;
                self.continuous_last_scroll = None;
                self.continuous_settle_pending = false;
                self.error = Some(format!(
                    "无法打开 {}：\n{error}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                self.doc = None;
                self.renderer = None;
                self.page_image = None;
                self.image_dpi = 0;
                self.page_cache.clear();
                self.clear_tiles();
                self.font_reports.clear();
                self.continuous_pending.clear();
                self.continuous_failed.clear();
                self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                self.page_cache_order.clear();
                self.page_cache_bytes = 0;
                self.render_seq = self.render_seq.wrapping_add(1);
                self.latest_render = None;
                self.thumbs.clear();
                self.thumbs_started = false;
                self.page_text_cache = None;
                self.search_highlights.clear();
                self.search_active_highlight = None;
                self.clear_selection();
                self.page_count = 0;
                self.page_sizes.clear();
                self.file_size = 0;
                self.loading = false;
            }
        }
    }
}
