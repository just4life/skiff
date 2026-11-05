//! One file browser pane (local or remote). Purely a view: it returns the
//! actions the user requested and the app performs them.

use std::collections::BTreeSet;

use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::remote::RemoteEntry;
use crate::theme::{self, ACCENT, BG0, BG3, TEXT, TEXT_WEAK};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortCol {
    Name,
    Size,
    Modified,
    Perms,
}

#[derive(Clone, Debug)]
pub struct DragPayload {
    pub from: Side,
    pub names: Vec<String>,
}

pub enum PaneAction {
    Open(String),
    /// Open a local file or folder with the system's default program.
    OpenExternal(String),
    Up,
    Back,
    Home,
    Navigate(String),
    Refresh,
    Transfer(Vec<String>),
    Mkdir,
    Rename(String),
    Delete(Vec<String>),
    Dropped(Vec<String>),
    Focus,
}

pub struct Pane {
    pub side: Side,
    pub path: String,
    pub path_edit: String,
    pub entries: Vec<RemoteEntry>,
    pub selected: BTreeSet<String>,
    pub anchor: Option<usize>,
    pub sort: SortCol,
    pub asc: bool,
    pub filter: String,
    pub error: Option<String>,
    pub history: Vec<String>,
    pub loaded: bool,
    pub suppress_history: bool,
    /// Bumped whenever `entries` changes; keys the sorted view cache.
    generation: u64,
    /// Sorted and filtered rows, rebuilt only when something they depend on
    /// changes instead of on every frame (big folders made frames slow).
    view: std::sync::Arc<Vec<RemoteEntry>>,
    view_key: Option<(u64, SortCol, bool, String)>,
    /// (files, dirs, bytes) of the listing.
    totals: (usize, usize, u64),
}

impl Pane {
    pub fn new(side: Side) -> Self {
        Self {
            side,
            path: String::new(),
            path_edit: String::new(),
            entries: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            sort: SortCol::Name,
            asc: true,
            filter: String::new(),
            error: None,
            history: Vec::new(),
            loaded: false,
            suppress_history: false,
            generation: 0,
            view: Default::default(),
            view_key: None,
            totals: (0, 0, 0),
        }
    }

    /// Replace the listing; keeps selection if the directory did not change.
    pub fn set_listing(&mut self, path: String, entries: Vec<RemoteEntry>) {
        if path != self.path {
            if !self.path.is_empty() && !self.suppress_history {
                self.history.push(self.path.clone());
                if self.history.len() > 50 {
                    self.history.remove(0);
                }
            }
            self.selected.clear();
            self.anchor = None;
            self.filter.clear();
        } else {
            self.selected.retain(|n| entries.iter().any(|e| &e.name == n));
        }
        self.path_edit = path.clone();
        self.path = path;
        self.entries = entries;
        self.generation += 1;
        let files = self.entries.iter().filter(|e| !e.is_dir).count();
        let bytes = self.entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
        self.totals = (files, self.entries.len() - files, bytes);
        self.error = None;
        self.loaded = true;
        self.suppress_history = false;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.generation += 1;
        self.totals = (0, 0, 0);
        self.selected.clear();
        self.path.clear();
        self.path_edit.clear();
        self.history.clear();
        self.loaded = false;
        self.error = None;
    }

    pub fn entry(&self, name: &str) -> Option<&RemoteEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    fn visible_cached(&mut self) -> std::sync::Arc<Vec<RemoteEntry>> {
        let key = (self.generation, self.sort, self.asc, self.filter.clone());
        if self.view_key.as_ref() != Some(&key) {
            self.view = std::sync::Arc::new(self.visible().into_iter().cloned().collect());
            self.view_key = Some(key);
        }
        self.view.clone()
    }

    fn visible(&self) -> Vec<&RemoteEntry> {
        let f = self.filter.to_lowercase();
        let mut v: Vec<&RemoteEntry> = self
            .entries
            .iter()
            .filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f))
            .collect();
        v.sort_by(|a, b| {
            let dirs = b.is_dir.cmp(&a.is_dir);
            if dirs != std::cmp::Ordering::Equal {
                return dirs;
            }
            let ord = match self.sort {
                SortCol::Name => natord(&a.name, &b.name),
                SortCol::Size => a.size.cmp(&b.size).then_with(|| natord(&a.name, &b.name)),
                SortCol::Modified => a.modified.cmp(&b.modified),
                SortCol::Perms => a.perms.cmp(&b.perms),
            };
            if self.asc { ord } else { ord.reverse() }
        });
        v
    }

    pub fn selection(&self) -> Vec<String> {
        self.selected.iter().cloned().collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        subtitle: &str,
        enabled: bool,
        busy: bool,
        active: bool,
        roots: &[std::path::PathBuf],
    ) -> Vec<PaneAction> {
        let mut actions = Vec::new();
        let side = self.side;
        let frame = theme::card_frame().stroke(Stroke::new(
            1.0,
            if active { Color32::from_rgb(0x34, 0x3e, 0x5e) } else { theme::BORDER },
        ));
        let resp = frame.show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            // Table row highlights extend by half the item spacing; keep them
            // inside the card's border.
            let clip = ui.max_rect().shrink(1.0).intersect(ui.clip_rect());
            ui.set_clip_rect(clip);
            // ---- header ------------------------------------------------------
            egui::Frame::new()
                .inner_margin(Margin { left: 12, right: 10, top: 10, bottom: 6 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let icon = if side == Side::Local { "💻" } else { "🌐" };
                        ui.label(RichText::new(icon).size(16.0).color(ACCENT));
                        ui.label(theme::bold(title, 14.5).color(theme::TEXT_STRONG));
                        if !subtitle.is_empty() {
                            ui.label(RichText::new(subtitle).color(TEXT_WEAK).size(12.0));
                        }
                        if busy {
                            ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_enabled(
                                enabled,
                                egui::TextEdit::singleline(&mut self.filter)
                                    .hint_text("🔍 Фильтр")
                                    .desired_width(150.0),
                            );
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(enabled, |ui| {
                            let b = |ui: &mut egui::Ui, icon: &str, tip: &str| {
                                ui.add(egui::Button::new(RichText::new(icon).size(14.0)).min_size(egui::vec2(28.0, 26.0)))
                                    .on_hover_text(tip)
                            };
                            if b(ui, "⬅", "Назад").clicked() {
                                actions.push(PaneAction::Back);
                            }
                            if b(ui, "⬆", "Вверх (Backspace)").clicked() {
                                actions.push(PaneAction::Up);
                            }
                            if side == Side::Local {
                                ui.menu_button(RichText::new("🏠").size(14.0), |ui| {
                                    for r in roots {
                                        if ui.button(r.display().to_string()).clicked() {
                                            actions.push(PaneAction::Navigate(r.display().to_string()));
                                            ui.close();
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("Избранные места и диски");
                            } else if b(ui, "🏠", "Домашняя папка").clicked() {
                                actions.push(PaneAction::Home);
                            }
                            if b(ui, "🔃", "Обновить (F5)").clicked() {
                                actions.push(PaneAction::Refresh);
                            }
                            if b(ui, "➕", "Новая папка").clicked() {
                                actions.push(PaneAction::Mkdir);
                            }
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.path_edit)
                                    .desired_width(f32::INFINITY)
                                    .font(egui::TextStyle::Monospace)
                                    .margin(Margin::symmetric(8, 5)),
                            );
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                actions.push(PaneAction::Navigate(self.path_edit.trim().to_string()));
                            }
                        });
                    });
                });

            // ---- table -------------------------------------------------------
            let footer_h = 26.0;
            let table_h = (ui.available_height() - footer_h).max(60.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), table_h), |ui| {
                if !enabled {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Нет подключения\n\nВыберите сайт или используйте быстрое подключение")
                                .color(TEXT_WEAK),
                        );
                    });
                    return;
                }
                if let Some(err) = &self.error {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x3a, 0x1d, 0x25))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(10, 6))
                        .outer_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("⚠ {err}")).color(theme::RED));
                        });
                }
                self.table(ui, &mut actions);
            });

            // ---- footer ------------------------------------------------------
            egui::Frame::new()
                .fill(BG0.gamma_multiply(0.6))
                .inner_margin(Margin::symmetric(12, 4))
                .corner_radius(CornerRadius { nw: 0, ne: 0, sw: 10, se: 10 })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (files, dirs, total) = self.totals;
                    let mut text = format!("{files} файлов и {dirs} папок · {}", theme::human_size(total));
                    if !self.selected.is_empty() {
                        let sel: u64 = self
                            .entries
                            .iter()
                            .filter(|e| !e.is_dir && self.selected.contains(&e.name))
                            .map(|e| e.size)
                            .sum();
                        text = format!("Выбрано {} · {} — {text}", self.selected.len(), theme::human_size(sel));
                    }
                    ui.label(RichText::new(text).size(11.5).color(TEXT_WEAK));
                });
        });

        // ---- drag & drop target -------------------------------------------
        let rect = resp.response.rect;
        let ctx = ui.ctx().clone();
        if let Some(payload) = egui::DragAndDrop::payload::<DragPayload>(&ctx)
            && payload.from != side
            && enabled
        {
            let over = ctx.pointer_latest_pos().is_some_and(|p| rect.contains(p));
            if over {
                ui.painter().rect_stroke(
                    rect.shrink(1.0),
                    CornerRadius::same(10),
                    Stroke::new(2.0, ACCENT),
                    egui::StrokeKind::Inside,
                );
                if ctx.input(|i| i.pointer.any_released())
                    && let Some(p) = egui::DragAndDrop::take_payload::<DragPayload>(&ctx)
                {
                    actions.push(PaneAction::Dropped(p.names.clone()));
                }
            }
        }
        if resp.response.contains_pointer() && ctx.input(|i| i.pointer.any_pressed()) {
            actions.push(PaneAction::Focus);
        }
        actions
    }

    fn table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<PaneAction>) {
        let visible = self.visible_cached();
        let has_parent = !is_root(&self.path, self.side);
        let offset = usize::from(has_parent);
        let total = visible.len() + offset;
        let side = self.side;
        let show_perms = side == Side::Remote || cfg!(unix);
        let modifiers = ui.input(|i| i.modifiers);

        ui.style_mut().interaction.selectable_labels = false;
        ui.style_mut().visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;

        let mut header_click: Option<SortCol> = None;
        let mut table = TableBuilder::new(ui)
            .id_salt(("table", side == Side::Local))
            .striped(true)
            .resizable(false)
            .sense(Sense::click_and_drag())
            .cell_layout(Layout::left_to_right(Align::Center))
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::initial(84.0).at_least(60.0))
            .column(Column::initial(128.0).at_least(80.0).clip(true));
        if show_perms {
            table = table.column(Column::initial(86.0).at_least(50.0).clip(true));
        }

        let sort = self.sort;
        let asc = self.asc;
        let header_label = |ui: &mut egui::Ui, text: &str, col: SortCol, clicked: &mut Option<SortCol>| {
            let arrow = if sort == col { if asc { " ▲" } else { " ▼" } } else { "" };
            let r = ui.add(
                egui::Label::new(RichText::new(format!("{text}{arrow}")).size(12.0).color(TEXT_WEAK).strong())
                    .sense(Sense::click()),
            );
            if r.clicked() {
                *clicked = Some(col);
            }
        };

        table
            .header(24.0, |mut h| {
                h.col(|ui| header_label(ui, "  Имя", SortCol::Name, &mut header_click));
                h.col(|ui| header_label(ui, "Размер", SortCol::Size, &mut header_click));
                h.col(|ui| header_label(ui, "Изменён", SortCol::Modified, &mut header_click));
                if show_perms {
                    h.col(|ui| header_label(ui, "Права", SortCol::Perms, &mut header_click));
                }
            })
            .body(|body| {
                body.rows(24.0, total, |mut row| {
                    let idx = row.index();
                    if has_parent && idx == 0 {
                        row.col(|ui| {
                            ui.add_space(6.0);
                            theme::folder_icon(ui, false);
                            ui.label(RichText::new("..").color(TEXT));
                        });
                        row.col(|_| {});
                        row.col(|_| {});
                        if show_perms {
                            row.col(|_| {});
                        }
                        let r = row.response();
                        if r.double_clicked() {
                            actions.push(PaneAction::Up);
                        } else if r.clicked() {
                            self.selected.clear();
                        }
                        return;
                    }
                    let e = &visible[idx - offset];
                    let selected = self.selected.contains(&e.name);
                    row.set_selected(selected);
                    row.col(|ui| {
                        ui.add_space(6.0);
                        if e.is_dir {
                            theme::folder_icon(ui, e.is_link);
                        } else {
                            let (icon, color) = theme::file_icon(&e.name, e.is_dir, e.is_link);
                            ui.add_sized([18.0, 16.0], egui::Label::new(RichText::new(icon).color(color)));
                        }
                        let mut t = RichText::new(&e.name).color(if selected { theme::TEXT_STRONG } else { TEXT });
                        if e.is_dir {
                            t = t.strong();
                        }
                        ui.add(egui::Label::new(t).truncate());
                    });
                    row.col(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(8.0);
                            if !e.is_dir {
                                ui.label(RichText::new(theme::human_size(e.size)).color(TEXT_WEAK).size(12.5));
                            }
                        });
                    });
                    row.col(|ui| {
                        if let Some(m) = e.modified {
                            ui.label(RichText::new(m.format("%d.%m.%Y %H:%M").to_string()).color(TEXT_WEAK).size(12.5));
                        }
                    });
                    if show_perms {
                        row.col(|ui| {
                            ui.label(RichText::new(&e.perms).monospace().color(TEXT_WEAK).size(11.5));
                        });
                    }

                    let r = row.response();
                    if r.clicked() || r.secondary_clicked() && !selected {
                        if modifiers.command {
                            if !self.selected.remove(&e.name) {
                                self.selected.insert(e.name.clone());
                            }
                            self.anchor = Some(idx);
                        } else if modifiers.shift && self.anchor.is_some() {
                            let a = self.anchor.unwrap();
                            let (lo, hi) = if a <= idx { (a, idx) } else { (idx, a) };
                            self.selected.clear();
                            for i in lo..=hi {
                                if i >= offset {
                                    self.selected.insert(visible[i - offset].name.clone());
                                }
                            }
                        } else {
                            self.selected.clear();
                            self.selected.insert(e.name.clone());
                            self.anchor = Some(idx);
                        }
                    }
                    if r.double_clicked() {
                        actions.push(PaneAction::Open(e.name.clone()));
                    }
                    if r.drag_started() {
                        if !self.selected.contains(&e.name) {
                            self.selected.clear();
                            self.selected.insert(e.name.clone());
                        }
                        egui::DragAndDrop::set_payload(
                            &r.ctx,
                            DragPayload { from: side, names: self.selected.iter().cloned().collect() },
                        );
                    }
                    let selected_set = &self.selected;
                    // Built only while the menu is open: cloning a large
                    // selection for every row made each frame slow.
                    let sel = || -> Vec<String> {
                        if selected_set.contains(&e.name) {
                            selected_set.iter().cloned().collect()
                        } else {
                            vec![e.name.clone()]
                        }
                    };
                    r.context_menu(|ui| {
                        ui.set_min_width(190.0);
                        let transfer = if side == Side::Local { "⬆  Загрузить на сервер" } else { "⬇  Скачать" };
                        if ui.button(transfer).clicked() {
                            actions.push(PaneAction::Transfer(sel()));
                            ui.close();
                        }
                        if e.is_dir && ui.button("📂  Открыть").clicked() {
                            actions.push(PaneAction::Open(e.name.clone()));
                            ui.close();
                        }
                        if side == Side::Local {
                            let label = if e.is_dir { "↗  Открыть в проводнике" } else { "↗  Открыть" };
                            if ui.button(label).clicked() {
                                actions.push(PaneAction::OpenExternal(e.name.clone()));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("✏  Переименовать        F2").clicked() {
                            actions.push(PaneAction::Rename(e.name.clone()));
                            ui.close();
                        }
                        if ui.button("➕  Новая папка").clicked() {
                            actions.push(PaneAction::Mkdir);
                            ui.close();
                        }
                        if ui
                            .button(RichText::new("🗑  Удалить                  Del").color(theme::RED))
                            .clicked()
                        {
                            actions.push(PaneAction::Delete(sel()));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("🔃  Обновить               F5").clicked() {
                            actions.push(PaneAction::Refresh);
                            ui.close();
                        }
                    });
                });
            });

        if let Some(col) = header_click {
            if self.sort == col {
                self.asc = !self.asc;
            } else {
                self.sort = col;
                self.asc = true;
            }
        }

        // Empty area below the rows: click clears selection, right-click menu.
        let rest = ui.available_rect_before_wrap();
        if rest.height() > 4.0 {
            let r = ui.interact(rest, ui.scope_id().with(("empty", side == Side::Local)), Sense::click());
            if r.clicked() {
                self.selected.clear();
            }
            r.context_menu(|ui| {
                if ui.button("➕  Новая папка").clicked() {
                    actions.push(PaneAction::Mkdir);
                    ui.close();
                }
                if ui.button("🔃  Обновить").clicked() {
                    actions.push(PaneAction::Refresh);
                    ui.close();
                }
            });
        }
        let _ = BG3;
    }
}

pub fn is_root(path: &str, side: Side) -> bool {
    match side {
        Side::Remote => path.is_empty() || path == "/",
        Side::Local => std::path::Path::new(path).parent().is_none(),
    }
}

/// Natural ("file2" < "file10"), case-insensitive ordering.
pub fn natord(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().filter(|c| c.is_ascii_digit()) {
                    na.push(*c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().filter(|c| c.is_ascii_digit()) {
                    nb.push(*c);
                    bi.next();
                }
                let o = na
                    .trim_start_matches('0')
                    .len()
                    .cmp(&nb.trim_start_matches('0').len())
                    .then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let (x, y) = (x.to_lowercase().next(), y.to_lowercase().next());
                if x != y {
                    return x.cmp(&y);
                }
                ai.next();
                bi.next();
            }
        }
    }
}
