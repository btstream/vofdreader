use crate::*;

impl ReaderApp {
    /// Drops every per-document raster product and invalidates in-flight jobs.
    ///
    /// This does not touch `ActiveDocument` metadata or the GPU registry: the
    /// caller owns those because dropping atlas entries needs a `Window`.
    pub(crate) fn reset_raster_state(&mut self) {
        self.raster.reset();
        self.continuous_last_scroll = None;
        self.continuous_settle_pending = false;
        self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
        self.continuous_layout = RefCell::new(continuous::LayoutCache::default());
    }

    /// Releases every rendering resource owned by the active tab, then clears
    /// the document state. Inactive tabs keep only their `DocumentTab` snapshot.
    pub(crate) fn clear_active_document(&mut self, cx: &mut Context<Self>) {
        let gpu = self.gpu.clone();
        gpu.borrow_mut()
            .release_all(|image| cx.drop_image(image, None));
        self.reset_raster_state();
        self.active.close();
        self.selection.layout = None;
        self.page_bounds.set(None);
        self.clear_selection();
        self.search.close();
        self.pending_restore_page = None;
        self.pending_restore_scroll = None;
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
        let reading = ReadingSnapshot {
            page,
            view_mode: match self.settings.reading.default_view {
                settings::ViewChoice::Single => ViewMode::Single,
                settings::ViewChoice::Continuous => ViewMode::Continuous,
            },
            ..ReadingSnapshot::default()
        };
        self.tabs.push(DocumentTab {
            path,
            file_name,
            reading,
            search: SearchSnapshot::default(),
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
        self.apply_reading(&tab.reading);
        self.search.apply_snapshot(&tab.search);
        self.load_document(tab.path, tab.reading.page, window, cx);
    }

    pub(crate) fn load_document(
        &mut self,
        path: PathBuf,
        target_page: usize,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let file_size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        // Keep the attempted path visible in the title bar even when opening
        // fails; `adopt` replaces it with the canonical identity on success.
        self.active.path = Some(path.clone());
        self.active.file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        match OfdDocument::open(&path) {
            Ok(doc) => {
                let doc = Arc::new(doc);
                self.active.adopt(doc.clone(), path.clone(), file_size);
                let recent_limit = self.settings.files.recent_limit;
                recent::push_recent(&mut self.recent, &path, recent_limit);
                recent::save_recent(&self.recent, recent_limit);
                self.page = target_page.min(self.active.page_count().saturating_sub(1));
                self.pending_restore_page = (self.active.page_count() > 0
                    && (self.page > 0 || self.pending_restore_scroll.is_some()))
                .then_some(self.page);
                // Do not scan every page on the UI thread. The visible page is
                // resolved synchronously; the rest are filled in the background.
                if let Ok(Some(size)) = doc.page_size_only(self.page)
                    && self.page < self.active.page_sizes.len()
                {
                    self.active.page_sizes[self.page] = size;
                }
                self.reset_raster_state();
                self.search.clear_results();
                self.clear_selection();
                self.grid_open = false;
                self.info_dialog_open = false;
                self.notice = None;
                self.wheel_accum = 0.0;
                self.settings.general.last_file = Some(path.to_string_lossy().into_owned());
                self.settings.save();
                self.restore_tab_search(window.as_deref_mut(), cx);
                if let Some(window) = window {
                    if self.search.open {
                        if let Some(input) = &self.search.input {
                            input.update(cx, |state, cx| state.focus(window, cx));
                        }
                    } else {
                        window.focus(&self.focus_handle, cx);
                    }
                }

                // Font database construction is expensive; keep the UI thread
                // free so the window chrome and empty state paint immediately.
                let generation = self.active.generation;
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
                        if this.active.generation != generation {
                            return;
                        }
                        this.active.renderer = Some(Arc::new(renderer));
                        this.request_page_render(cx);
                        cx.notify();
                    });
                })
                .detach();

                let sizes_doc = doc.clone();
                let generation = self.active.generation;
                cx.spawn(async move |this, cx| {
                    let sizes = cx
                        .background_spawn(async move {
                            (0..sizes_doc.page_count())
                                .map(|index| sizes_doc.page_size_only(index).ok().flatten())
                                .collect::<Vec<_>>()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.active.generation != generation {
                            return;
                        }
                        let mut changed = false;
                        for (index, size) in sizes.into_iter().enumerate() {
                            if let Some(size) = size
                                && index < this.active.page_sizes.len()
                                && this.active.page_sizes[index] != size
                            {
                                this.active.page_sizes[index] = size;
                                changed = true;
                            }
                        }
                        if changed {
                            this.active.page_sizes_revision =
                                this.active.page_sizes_revision.wrapping_add(1);
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
                self.reset_raster_state();
                self.active.fail(format!(
                    "无法打开 {}：\n{error}",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                self.search.clear_results();
                self.clear_selection();
                self.bar_visible = false;
                self.bar_hovered = false;
            }
        }
    }
}
