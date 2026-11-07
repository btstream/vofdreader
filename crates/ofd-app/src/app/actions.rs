use crate::*;

impl ReaderApp {
    pub(crate) fn on_open(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.prompt_open(cx);
    }

    pub(crate) fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("打开 OFD 文件".into()),
        });

        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        if path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("ofd"))
                        {
                            this.open_path(path, None, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn on_toggle_theme(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dark = !self.dark;
        self.settings.appearance.theme = if self.dark {
            settings::ThemeChoice::Dark
        } else {
            settings::ThemeChoice::Light
        };
        self.settings.save();
        apply_component_theme(self.dark, &self.ui_font, cx);
        cx.notify();
    }

    /// Shows a transient status-bar message and clears it after a few seconds.
    pub(crate) fn set_notice(
        &mut self,
        text: impl Into<String>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.notice_generation = self.notice_generation.wrapping_add(1);
        let generation = self.notice_generation;
        self.notice = Some((text.into(), error));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice_generation == generation {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn on_export(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(doc) = self.active.doc.clone() else {
            self.set_notice("请先打开 OFD 文档", true, cx);
            cx.notify();
            return;
        };
        let Some(renderer) = self.active.renderer.clone() else {
            self.set_notice("文档渲染器尚未就绪", true, cx);
            cx.notify();
            return;
        };

        let page = self.page;
        let directory = self
            .active
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let stem = self
            .active
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ofd".to_string());

        let default_format = self.settings.files.export_format;
        let export_dpi = self.settings.files.export_dpi;
        let receiver = cx.prompt_for_new_path(
            &directory,
            Some(&suggested_name_for(&stem, page, default_format)),
        );
        self.exporting = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let target = match receiver.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        cx.notify();
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let message = format!("无法打开保存对话框：{error}");
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        this.set_notice(message, true, cx);
                        cx.notify();
                    });
                    return;
                }
                Err(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.exporting = false;
                        cx.notify();
                    });
                    return;
                }
            };

            let format = ExportFormat::for_path(&target);
            let result = cx
                .background_spawn(async move {
                    if format == ExportFormat::Svg {
                        let svg = doc.page_svg(page).map_err(|error| error.to_string())?;
                        std::fs::write(&target, svg).map_err(|error| error.to_string())?;
                    } else {
                        let rendered = renderer
                            .render_png(&doc, page, export_dpi)
                            .map_err(|error| error.to_string())?;
                        std::fs::write(&target, rendered.png).map_err(|error| error.to_string())?;
                    }
                    Ok::<PathBuf, String>(target)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.exporting = false;
                match result {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        this.set_notice(format!("已导出 {name}"), false, cx);
                    }
                    Err(error) => this.set_notice(format!("导出失败：{error}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn on_open_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(cx);
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.info_dialog_open = false;
        self.settings_open = true;
        self.settings_reset_armed = false;
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.settings_reset_armed = false;
        self.settings.save();
        cx.notify();
    }

    pub(crate) fn on_noop(&mut self, _: &ClickEvent, _: &mut Window, _: &mut Context<Self>) {}

    pub(crate) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        if self.settings_open {
            if key == "escape" {
                self.close_settings(cx);
            }
            return;
        }
        if self.info_dialog_open {
            if key == "escape" {
                self.info_dialog_open = false;
                cx.notify();
            }
            return;
        }
        if key == "," && (modifiers.control || modifiers.platform) {
            self.open_settings(cx);
            return;
        }
        if key == "f" && (modifiers.control || modifiers.platform) {
            self.toggle_search(window, cx);
            return;
        }
        if key == "o" && (modifiers.control || modifiers.platform) {
            self.prompt_open(cx);
            return;
        }
        if key == "w" && (modifiers.control || modifiers.platform) {
            if let Some(index) = self.active_tab {
                self.close_tab(index, window, cx);
            }
            return;
        }
        if key == "tab" && (modifiers.control || modifiers.platform) {
            let len = self.tabs.len();
            if len > 1 {
                let current = self.active_tab.unwrap_or(0);
                let next = if modifiers.shift {
                    (current + len - 1) % len
                } else {
                    (current + 1) % len
                };
                self.activate_tab(next, Some(window), cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.search.open {
            match key {
                "escape" => self.close_search(window, cx),
                "up" => self.search_step(-1, cx),
                "down" => self.search_step(1, cx),
                _ => {}
            }
            return;
        }
        if self.grid_open {
            if key == "escape" {
                self.grid_open = false;
                cx.notify();
            } else if key == "down" || key == "j" {
                self.go_to_page(
                    (self.page + 1).min(self.active.page_count().saturating_sub(1)),
                    cx,
                );
            } else if key == "up" || key == "k" {
                self.go_to_page(self.page.saturating_sub(1), cx);
            }
            return;
        }
        if key == "c" && (modifiers.control || modifiers.platform) {
            self.copy_selection(cx);
            return;
        }
        if !modifiers.control && !modifiers.platform && !modifiers.alt && self.active.is_open() {
            match key {
                "r" => {
                    self.rotate_view(cx);
                    return;
                }
                "g" => {
                    self.toggle_grid(cx);
                    return;
                }
                _ => {}
            }
        }
        if key == "escape" && !self.selection.is_empty() {
            self.clear_selection();
            cx.notify();
            return;
        }
        if self.view_mode == ViewMode::Continuous {
            let viewport = window.viewport_size();
            let page_step = (f32::from(viewport.height) - 180.0).max(160.0);
            match key {
                "down" | "j" => self.scroll_continuous(120.0, cx),
                "up" | "k" => self.scroll_continuous(-120.0, cx),
                "pagedown" | "space" if !modifiers.shift => self.scroll_continuous(page_step, cx),
                "pageup" | "space" => self.scroll_continuous(-page_step, cx),
                "left" => self.prev_page(cx),
                "right" => self.next_page(cx),
                "home" => {
                    self.continuous_scroll.set_offset(point(px(0.0), px(0.0)));
                    self.go_to_page(0, cx);
                }
                "end" => {
                    self.continuous_scroll.scroll_to_bottom();
                    self.go_to_page(self.active.page_count().saturating_sub(1), cx);
                }
                "=" | "+" => self.set_zoom(self.zoom + 0.25, cx),
                "-" | "_" => self.set_zoom(self.zoom - 0.25, cx),
                "0" => self.set_zoom(1.0, cx),
                _ => return,
            }
            return;
        }
        match key {
            "pagedown" | "right" | "down" | "j" | "space" if !modifiers.shift => self.next_page(cx),
            "pageup" | "left" | "up" | "k" | "space" => self.prev_page(cx),
            "home" => self.go_to_page(0, cx),
            "end" => self.go_to_page(self.active.page_count().saturating_sub(1), cx),
            "=" | "+" => self.set_zoom(self.zoom + 0.25, cx),
            "-" | "_" => self.set_zoom(self.zoom - 0.25, cx),
            "0" => self.set_zoom(1.0, cx),
            _ => return,
        }
        cx.notify();
    }

    /// Pointer moves caused by hover/hitbox changes are synthetically dispatched
    /// by GPUI; only a real position change is activity, otherwise hiding the
    /// bar would immediately show it again. Tiny per-frame jitter is ignored so
    /// a stationary pointer can time out.
    pub(crate) fn on_pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection.dragging && event.pressed_button == Some(MouseButton::Left) {
            self.update_selection_drag(event.position, cx);
        }
        if self
            .bar_pointer
            .is_some_and(|last| !pointer_moved(last, event.position))
        {
            return;
        }
        self.bar_pointer = Some(event.position);
        if self.active.is_open()
            && self.settings.reading.floating_toolbar
            && self
                .reader_bounds
                .get()
                .is_some_and(|bounds| near_floating_bar(bounds, event.position))
        {
            self.note_bar_activity(cx);
        }
    }

    /// Only proximity to the toolbar reveals it; unrelated pointer movement
    /// must not keep resetting its hide timer.
    pub(crate) fn note_bar_activity(&mut self, cx: &mut Context<Self>) {
        self.bar_last_activity = Instant::now();
        if !self.bar_visible {
            self.bar_visible = true;
            cx.notify();
        }
        if self.bar_watchdog || !self.active.is_open() {
            return;
        }

        self.bar_watchdog = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let keep_running = this.update(cx, |this, cx| {
                    if !this.active.is_open() || !this.bar_visible {
                        this.bar_visible = false;
                        this.bar_hovered = false;
                        this.bar_watchdog = false;
                        return false;
                    }
                    if !this.bar_hovered && this.bar_last_activity.elapsed() >= BAR_HIDE_DELAY {
                        this.bar_visible = false;
                        this.bar_watchdog = false;
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(keep_running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}
