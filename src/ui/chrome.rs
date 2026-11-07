//! Toolbar, bottom panel (queue + log), dialogs and prompts.

use egui::{Align, Color32, Layout, Margin, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, BottomTab, ConnState, Dialog};
use crate::config::{ExistsAction, ProgressStyle, Protocol};
use crate::events::ExistsAnswer;
use crate::remote::LogLevel;
use crate::theme::{self, ACCENT, BG2, TEXT_WEAK};
use crate::transfer::{Direction, MAX_SEGMENTS, Part, Status};
use crate::trust::{TrustDecision, TrustKind};
use crate::ui::pane::Side;
use crate::ui::site_manager::SiteManager;

fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG2)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(14)
        .inner_margin(Margin::same(22))
        .shadow(egui::Shadow { offset: [0, 16], blur: 40, spread: 0, color: Color32::from_black_alpha(160) })
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(TEXT_WEAK));
}

impl App {
    pub fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // logo
            let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
            crate::icon::paint_logo(ui.painter(), rect);
            ui.label(theme::bold("Skiff", 17.0).color(theme::TEXT_STRONG));
            ui.add_space(10.0);

            // site manager
            let r = ui.add(egui::Button::new(RichText::new("🗄  Сайты").size(13.5)).min_size(egui::vec2(0.0, 30.0)));
            if r.clicked() {
                let sel = self.connected_site().map(|s| s.id);
                self.site_manager = Some(SiteManager::new(&self.sites, sel));
            }
            ui.menu_button(RichText::new("⏷").size(12.0), |ui| {
                ui.set_min_width(240.0);
                if self.sites.is_empty() {
                    ui.label(RichText::new("Сайтов пока нет").color(TEXT_WEAK));
                }
                let mut connect = None;
                for s in &self.sites {
                    let label = if s.group.is_empty() { s.name.clone() } else { format!("{} / {}", s.group, s.name) };
                    if ui.button(format!("{}   {}", label, s.protocol.short())).clicked() {
                        connect = Some(s.clone());
                        ui.close();
                    }
                }
                if let Some(s) = connect {
                    self.connect(s);
                }
            });
            ui.separator();

            // quickconnect
            let h = 30.0;
            egui::ComboBox::from_id_salt("quick-proto")
                .width(118.0)
                .selected_text(self.quick.protocol.short().to_uppercase())
                .show_ui(ui, |ui| {
                    for p in Protocol::ALL {
                        ui.selectable_value(&mut self.quick.protocol, p, p.label());
                    }
                });
            let mut enter = false;
            // Lay the row out from what is actually left, so nothing overlaps
            // in a narrow window: the right-hand buttons get reserved space,
            // fields shrink first and the buttons go icon-only last.
            let w = ui.available_width();
            let connected = self.conn != ConnState::Disconnected;
            let gap = ui.spacing().item_spacing.x;
            let compact = w < 760.0;
            let connect_w = if compact { 36.0 } else { 140.0 };
            let disconnect_w = match (connected, compact) {
                (false, _) => 0.0,
                (true, true) => 36.0 + gap,
                (true, false) => 130.0 + gap,
            };
            let reserve = 36.0 + gap + disconnect_w + 8.0;
            let fields = (w - reserve - 58.0 - connect_w - 5.0 * gap).max(150.0);
            let host_w = (fields * 0.44).clamp(70.0, 320.0);
            let cred_w = (fields * 0.28).clamp(40.0, 140.0);
            let r = ui.add_sized(
                [host_w, h],
                egui::TextEdit::singleline(&mut self.quick_host)
                    .hint_text("Хост  (например sftp://host)")
                    .margin(Margin::symmetric(8, 6)),
            );
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let r = ui.add_sized(
                [cred_w, h],
                egui::TextEdit::singleline(&mut self.quick.user).hint_text("Пользователь").margin(Margin::symmetric(8, 6)),
            );
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let r = ui.add_sized(
                [cred_w, h],
                egui::TextEdit::singleline(&mut self.quick.password)
                    .password(true)
                    .hint_text("Пароль")
                    .margin(Margin::symmetric(8, 6)),
            );
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let port_hint = self.quick.protocol.default_port().to_string();
            let r = ui.add_sized(
                [58.0, h],
                egui::TextEdit::singleline(&mut self.quick_port).hint_text(port_hint).margin(Margin::symmetric(8, 6)),
            );
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let go = ui
                .add(
                    egui::Button::new(
                        RichText::new(if compact { "⚡" } else { "⚡ Подключиться" }).color(Color32::WHITE).strong(),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(0.0, h)),
                )
                .on_hover_text("Подключиться (Enter)")
                .clicked();
            if go || enter {
                self.quick_connect();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(RichText::new("☰").size(16.0), |ui| {
                    ui.set_min_width(200.0);
                    if ui.button("⚙  Настройки").clicked() {
                        self.dialog = Some(Dialog::Settings);
                        ui.close();
                    }
                    if ui.button("📥  Импорт сайтов из FileZilla").clicked() {
                        let mut sm = SiteManager::new(&self.sites, None);
                        sm.import_requested = true;
                        self.site_manager = Some(sm);
                        ui.close();
                    }
                    if ui.button("ℹ  О программе").clicked() {
                        self.dialog = Some(Dialog::About);
                        ui.close();
                    }
                });
                if connected {
                    let r = ui
                        .add(
                            egui::Button::new(
                                RichText::new(if compact { "⏏" } else { "⏏ Отключиться" }).color(theme::RED),
                            )
                            .min_size(egui::vec2(0.0, h)),
                        )
                        .on_hover_text("Отключиться");
                    if r.clicked() {
                        self.disconnect(true);
                    }
                }
            });
        });
    }

    pub fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let (n_queue, n_done, n_failed) = {
            let q = self.transfers.queue.lock().unwrap();
            let files = q.items.iter().filter(|i| !i.is_dir);
            let mut a = (0, 0, 0);
            for i in files {
                match i.status {
                    Status::Queued | Status::Active => a.0 += 1,
                    Status::Done | Status::Skipped => a.1 += 1,
                    Status::Failed(_) | Status::Cancelled => a.2 += 1,
                }
            }
            a
        };
        ui.horizontal(|ui| {
            let tab = |ui: &mut egui::Ui, cur: &mut BottomTab, t: BottomTab, label: String| {
                let selected = *cur == t;
                let text = RichText::new(label).size(13.0).color(if selected { theme::TEXT_STRONG } else { TEXT_WEAK });
                let r = ui.add(egui::Button::new(text).frame(selected).min_size(egui::vec2(0.0, 26.0)));
                if r.clicked() {
                    *cur = t;
                }
            };
            tab(ui, &mut self.bottom_tab, BottomTab::Queue, format!("Очередь  {n_queue}"));
            tab(ui, &mut self.bottom_tab, BottomTab::Done, format!("Завершённые  {n_done}"));
            tab(
                ui,
                &mut self.bottom_tab,
                BottomTab::Failed,
                if n_failed > 0 { format!("Ошибки  {n_failed}") } else { "Ошибки".into() },
            );
            tab(ui, &mut self.bottom_tab, BottomTab::Log, "Журнал".into());

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match self.bottom_tab {
                BottomTab::Queue => {
                    let paused = self.transfers.queue.lock().unwrap().paused;
                    if ui.button("✖ Отменить все").clicked() {
                        self.transfers.cancel_all();
                    }
                    let label = if paused { "▶ Продолжить" } else { "⏸ Пауза" };
                    if ui.button(label).clicked() {
                        self.transfers.set_paused(!paused);
                    }
                    let mut n = self.settings.max_transfers;
                    if ui
                        .add(egui::DragValue::new(&mut n).range(1..=10).prefix("потоков: "))
                        .on_hover_text("Одновременных передач")
                        .changed()
                    {
                        self.settings.max_transfers = n;
                        self.transfers.queue.lock().unwrap().max_workers = n;
                        self.transfers.ensure_workers();
                        self.save_settings();
                    }
                }
                BottomTab::Done => {
                    if ui.button("🗑 Очистить").clicked() {
                        self.transfers.clear_finished(|s| matches!(s, Status::Done | Status::Skipped));
                    }
                }
                BottomTab::Failed => {
                    if ui.button("🗑 Очистить").clicked() {
                        self.transfers.clear_finished(|s| matches!(s, Status::Failed(_) | Status::Cancelled));
                    }
                    if ui.button("↻ Повторить все").clicked() {
                        let ids: Vec<u64> = self
                            .transfers
                            .queue
                            .lock()
                            .unwrap()
                            .items
                            .iter()
                            .filter(|i| matches!(i.status, Status::Failed(_) | Status::Cancelled))
                            .map(|i| i.id)
                            .collect();
                        for id in ids {
                            self.transfers.retry(id);
                        }
                    }
                }
                BottomTab::Log => {
                    if ui.button("🗑 Очистить").clicked() {
                        self.log.clear();
                    }
                }
            });
        });
        ui.add_space(2.0);
        // Fill exactly the remaining rect so the resizable panel keeps its size.
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let painter = child.painter().clone();
        painter.rect(rect, 10, BG2, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        let mut inner = child.new_child(egui::UiBuilder::new().max_rect(rect.shrink(1.0)));
        inner.set_clip_rect(rect.shrink(1.0));
        match self.bottom_tab {
            BottomTab::Log => self.log_view(&mut inner),
            t => self.queue_view(&mut inner, t),
        }
    }

    fn log_view(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.style_mut().spacing.item_spacing.y = 1.0;
                    ui.style_mut().spacing.interact_size.y = 16.0;
                    for l in &self.log {
                        let color = match l.level {
                            LogLevel::Info => theme::TEXT,
                            LogLevel::Command => theme::CYAN,
                            LogLevel::Reply => TEXT_WEAK,
                            LogLevel::Success => theme::GREEN,
                            LogLevel::Error => theme::RED,
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(l.time.format("%H:%M:%S").to_string()).monospace().size(11.5).color(TEXT_WEAK));
                            ui.add(egui::Label::new(RichText::new(&l.text).monospace().size(12.0).color(color)).wrap());
                        });
                    }
                });
            });
    }

    fn queue_view(&mut self, ui: &mut egui::Ui, tab: BottomTab) {
        let items: Vec<crate::transfer::Item> = {
            let q = self.transfers.queue.lock().unwrap();
            q.items
                .iter()
                .filter(|i| !i.is_dir || i.status == Status::Active || matches!(i.status, Status::Failed(_)))
                .filter(|i| match tab {
                    BottomTab::Queue => matches!(i.status, Status::Queued | Status::Active),
                    BottomTab::Done => matches!(i.status, Status::Done | Status::Skipped),
                    BottomTab::Failed => matches!(i.status, Status::Failed(_) | Status::Cancelled),
                    BottomTab::Log => false,
                })
                .cloned()
                .collect()
        };
        let style = self.settings.progress_style;
        if items.iter().any(|i| i.status == Status::Active) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        if items.is_empty() {
            ui.centered_and_justified(|ui| {
                let text = match tab {
                    BottomTab::Queue => "Очередь пуста — перетащите файлы между панелями или дважды щёлкните по файлу",
                    BottomTab::Done => "Здесь появятся завершённые передачи",
                    _ => "Ошибок нет",
                };
                ui.label(RichText::new(text).color(TEXT_WEAK));
            });
            return;
        }
        let mut cancel = None;
        let mut retry = None;
        ui.style_mut().interaction.selectable_labels = false;
        TableBuilder::new(ui)
            .id_salt(("queue", tab as u8))
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink([false, false])
            .column(Column::exact(34.0))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::initial(80.0))
            .column(Column::initial(300.0).at_least(140.0).clip(true))
            .column(Column::exact(40.0))
            .header(22.0, |mut h| {
                h.col(|_| {});
                h.col(|ui| {
                    ui.label(RichText::new("Файл").size(12.0).color(TEXT_WEAK).strong());
                });
                h.col(|ui| {
                    ui.label(RichText::new("Размер").size(12.0).color(TEXT_WEAK).strong());
                });
                h.col(|ui| {
                    ui.label(RichText::new("Состояние").size(12.0).color(TEXT_WEAK).strong());
                });
                h.col(|_| {});
            })
            .body(|body| {
                body.rows(34.0, items.len(), |mut row| {
                    let it = &items[row.index()];
                    row.col(|ui| {
                        ui.add_space(8.0);
                        let (icon, color) = match it.direction {
                            Direction::Upload => ("⬆", theme::GREEN),
                            Direction::Download => ("⬇", theme::CYAN),
                        };
                        ui.label(RichText::new(icon).color(color).size(15.0));
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let (src, dst) = match it.direction {
                                Direction::Upload => (it.local.display().to_string(), it.remote.clone()),
                                Direction::Download => (it.remote.clone(), it.local.display().to_string()),
                            };
                            ui.add(egui::Label::new(RichText::new(&src).size(12.5).color(theme::TEXT)).truncate());
                            ui.add(
                                egui::Label::new(RichText::new(format!("→ {dst}  ·  {}", it.site.host)).size(11.0).color(TEXT_WEAK))
                                    .truncate(),
                            );
                        });
                    });
                    row.col(|ui| {
                        if !it.is_dir {
                            ui.label(RichText::new(theme::human_size(it.size)).size(12.5).color(TEXT_WEAK));
                        }
                    });
                    row.col(|ui| match &it.status {
                        Status::Active if !it.is_dir => progress_cell(ui, it, style),
                        Status::Active => {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(RichText::new("чтение каталога…").size(12.0).color(TEXT_WEAK));
                        }
                        Status::Queued => {
                            ui.label(RichText::new("в очереди").size(12.0).color(TEXT_WEAK));
                        }
                        Status::Done => {
                            let s = if it.is_dir { "✔ папка".into() } else { format!("✔ готово · {}", theme::human_speed(it.speed())) };
                            ui.label(RichText::new(s).size(12.0).color(theme::GREEN));
                        }
                        Status::Skipped => {
                            ui.label(RichText::new("⏭ пропущен").size(12.0).color(TEXT_WEAK));
                        }
                        Status::Cancelled => {
                            ui.label(RichText::new("✖ отменён").size(12.0).color(theme::YELLOW));
                        }
                        Status::Failed(e) => {
                            ui.add(egui::Label::new(RichText::new(format!("⚠ {e}")).size(12.0).color(theme::RED)).truncate())
                                .on_hover_text(e);
                        }
                    });
                    row.col(|ui| {
                        if matches!(it.status, Status::Queued | Status::Active) {
                            if ui.small_button("✖").on_hover_text("Отменить").clicked() {
                                cancel = Some(it.id);
                            }
                        } else if matches!(it.status, Status::Failed(_) | Status::Cancelled)
                            && ui.small_button("↻").on_hover_text("Повторить").clicked()
                        {
                            retry = Some(it.id);
                        }
                    });
                });
            });
        if let Some(id) = cancel {
            self.transfers.cancel(id);
        }
        if let Some(id) = retry {
            self.transfers.retry(id);
        }
    }

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let resp = egui::Modal::new(egui::Id::unique("dialog")).frame(modal_frame()).show(ctx, |ui| {
            match &mut dialog {
                Dialog::Mkdir { side, name } => {
                    ui.set_width(380.0);
                    ui.label(theme::bold("Новая папка", 17.0));
                    ui.add_space(8.0);
                    let where_ = if *side == Side::Local { self.local.path.clone() } else { self.remote.path.clone() };
                    field_label(ui, &format!("Будет создана в {where_}"));
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)));
                    r.request_focus();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let ok = theme::primary_button(ui, "Создать").clicked() || enter;
                        if ui.button("Отмена").clicked() {
                            keep = false;
                        }
                        if ok {
                            let (s, n) = (*side, name.clone());
                            self.do_mkdir(s, &n);
                            keep = false;
                        }
                    });
                }
                Dialog::Rename { side, from, to } => {
                    ui.set_width(420.0);
                    ui.label(theme::bold("Переименовать", 17.0));
                    ui.add_space(8.0);
                    field_label(ui, &format!("Новое имя для «{from}»"));
                    let r = ui.add(egui::TextEdit::singleline(to).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)));
                    r.request_focus();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let ok = theme::primary_button(ui, "Переименовать").clicked() || enter;
                        if ui.button("Отмена").clicked() {
                            keep = false;
                        }
                        if ok {
                            let (s, f, t) = (*side, from.clone(), to.clone());
                            self.do_rename(s, &f, &t);
                            keep = false;
                        }
                    });
                }
                Dialog::Delete { side, names } => {
                    ui.set_width(420.0);
                    ui.label(theme::bold("Удалить?", 17.0));
                    ui.add_space(6.0);
                    let where_ = if *side == Side::Local { "с этого компьютера" } else { "с сервера" };
                    let what = if names.len() == 1 { format!("«{}»", names[0]) } else { format!("{} объектов", names.len()) };
                    ui.label(format!("{what} будет удалено {where_} без возможности восстановления. Папки удаляются вместе с содержимым."));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let del = ui
                            .add(egui::Button::new(RichText::new("🗑 Удалить").color(Color32::WHITE).strong()).fill(theme::RED.gamma_multiply(0.85)))
                            .clicked();
                        if ui.button("Отмена").clicked() {
                            keep = false;
                        }
                        if del {
                            let (s, n) = (*side, names.clone());
                            self.do_delete(s, &n);
                            keep = false;
                        }
                    });
                }
                Dialog::Password { site, password } => {
                    ui.set_width(380.0);
                    ui.label(theme::bold("Введите пароль", 17.0));
                    ui.add_space(6.0);
                    field_label(ui, &format!("{}@{}", site.user, site.endpoint()));
                    let r = ui.add(
                        egui::TextEdit::singleline(password).password(true).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 6)),
                    );
                    r.request_focus();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let ok = theme::primary_button(ui, "Подключиться").clicked() || enter;
                        if ui.button("Отмена").clicked() {
                            keep = false;
                        }
                        if ok {
                            let mut s = (**site).clone();
                            s.password = password.clone();
                            s.remember_password = true;
                            // remember it for next time (saved sites only)
                            if let Some(stored) = self.sites.iter_mut().find(|x| x.id == s.id && s.id != 0) {
                                stored.password = s.password.clone();
                                stored.remember_password = true;
                                self.save_sites();
                            }
                            keep = false;
                            self.dialog = None;
                            self.connect(s);
                        }
                    });
                }
                Dialog::Settings => {
                    ui.set_width(440.0);
                    ui.label(theme::bold("Настройки", 17.0));
                    ui.add_space(10.0);
                    egui::Grid::new("settings").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
                        ui.label("Если файл существует");
                        egui::ComboBox::from_id_salt("exists")
                            .selected_text(self.settings.exists_action.label())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for a in ExistsAction::ALL {
                                    ui.selectable_value(&mut self.settings.exists_action, a, a.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Одновременных передач");
                        if ui.add(egui::DragValue::new(&mut self.settings.max_transfers).range(1..=10)).changed() {
                            self.transfers.queue.lock().unwrap().max_workers = self.settings.max_transfers;
                        }
                        ui.end_row();
                        ui.label("Потоков на один файл").on_hover_text(
                            "Большие файлы (от 16 МБ) качаются по частям через несколько соединений. \
                             Загрузка на сервер по частям работает только по SFTP. 1 — выключено.",
                        );
                        if ui
                            .add(egui::DragValue::new(&mut self.settings.segments).range(1..=MAX_SEGMENTS))
                            .changed()
                        {
                            self.transfers.queue.lock().unwrap().segments = self.settings.segments;
                        }
                        ui.end_row();
                        ui.label("Прогресс передачи");
                        egui::ComboBox::from_id_salt("progress_style")
                            .selected_text(self.settings.progress_style.label())
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for s in ProgressStyle::ALL {
                                    ui.selectable_value(&mut self.settings.progress_style, s, s.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Скрытые локальные файлы");
                        if ui.checkbox(&mut self.settings.show_hidden_local, "показывать").changed() {
                            let p = self.local.path.clone();
                            self.navigate_local(&p);
                        }
                        ui.end_row();
                        ui.label("Масштаб интерфейса");
                        // Applied when the slider is released: rescaling while
                        // dragging moves the slider out from under the cursor.
                        let r = ui.add(egui::Slider::new(&mut self.settings.ui_scale, 0.75..=1.75).step_by(0.05));
                        let released = r.drag_stopped() || (r.changed() && !r.dragged());
                        if released && (ctx.zoom_factor() - self.settings.ui_scale).abs() > 0.001 {
                            ctx.set_zoom_factor(self.settings.ui_scale);
                            self.save_settings();
                        }
                        ui.end_row();
                    });
                    ui.add_space(14.0);
                    if theme::primary_button(ui, "Готово").clicked() {
                        self.transfers.queue.lock().unwrap().exists_action = self.settings.exists_action;
                        self.save_settings();
                        keep = false;
                    }
                }
                Dialog::About => {
                    ui.set_width(420.0);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                        crate::icon::paint_logo(ui.painter(), rect);
                        ui.vertical(|ui| {
                            ui.label(theme::bold("Skiff", 22.0));
                            ui.label(RichText::new(format!("версия {}", env!("CARGO_PKG_VERSION"))).color(TEXT_WEAK));
                        });
                    });
                    ui.add_space(10.0);
                    ui.label("Быстрый FTP / FTPS / SFTP клиент на чистом Rust. Идеи и поведение основаны на клиенте FileZilla.");
                    ui.add_space(6.0);
                    ui.label(RichText::new("Лицензия GNU GPL v3 или новее. Шрифт Inter — SIL OFL 1.1.").color(TEXT_WEAK).size(12.0));
                    ui.add_space(14.0);
                    if theme::primary_button(ui, "Закрыть").clicked() {
                        keep = false;
                    }
                }
            }
        });
        if resp.should_close() {
            keep = false;
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    pub fn prompts(&mut self, ctx: &egui::Context) {
        if let Some((req, _)) = self.trust_prompts.front() {
            let req = req.clone();
            let mut decision = None;
            egui::Modal::new(egui::Id::unique("trust")).frame(modal_frame()).show(ctx, |ui| {
                ui.set_width(520.0);
                let changed = req.previous.is_some();
                let title = match (req.kind, changed) {
                    (TrustKind::SshHostKey, false) => "Неизвестный ключ сервера",
                    (TrustKind::SshHostKey, true) => "⚠ Ключ сервера изменился!",
                    (TrustKind::TlsCertificate, false) => "Непроверенный сертификат",
                    (TrustKind::TlsCertificate, true) => "⚠ Сертификат сервера изменился!",
                };
                ui.label(theme::bold(title, 18.0).color(if changed { theme::RED } else { theme::TEXT_STRONG }));
                ui.add_space(6.0);
                let text = if changed {
                    "Отпечаток отличается от сохранённого ранее. Это может означать атаку «человек посередине» — или что сервер переустановили. Продолжайте, только если уверены."
                } else {
                    "Подлинность сервера не удалось подтвердить автоматически. Сверьте отпечаток с администратором сервера."
                };
                ui.label(text);
                ui.add_space(10.0);
                egui::Grid::new("trust-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    field_label(ui, "Сервер");
                    ui.label(RichText::new(&req.endpoint).strong());
                    ui.end_row();
                    field_label(ui, "Отпечаток");
                    ui.add(egui::Label::new(RichText::new(&req.fingerprint).monospace().size(12.0).color(theme::CYAN)).wrap());
                    ui.end_row();
                    if let Some(prev) = &req.previous {
                        field_label(ui, "Был");
                        ui.add(egui::Label::new(RichText::new(prev).monospace().size(12.0).color(TEXT_WEAK)).wrap());
                        ui.end_row();
                    }
                    field_label(ui, "Детали");
                    ui.add(egui::Label::new(RichText::new(&req.details).size(12.0)).wrap());
                    ui.end_row();
                });
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Доверять всегда").clicked() {
                        decision = Some(TrustDecision::Always);
                    }
                    if ui.button("Только сейчас").clicked() {
                        decision = Some(TrustDecision::Once);
                    }
                    if ui.button("Отмена").clicked() {
                        decision = Some(TrustDecision::Reject);
                    }
                });
            });
            if let Some(d) = decision
                && let Some((_, tx)) = self.trust_prompts.pop_front()
            {
                let _ = tx.send(d);
            }
            return;
        }

        if let Some((q, _)) = self.exists_prompts.front() {
            let q = q.clone();
            let mut answer = None;
            let mut all = self.exists_apply_all;
            egui::Modal::new(egui::Id::unique("exists")).frame(modal_frame()).show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(theme::bold("Файл уже существует", 18.0));
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(&q.target).monospace().size(12.0)).wrap());
                ui.add_space(10.0);
                let fmt = |s: u64, m: Option<chrono::DateTime<chrono::Local>>| {
                    format!(
                        "{} · {}",
                        theme::human_size(s),
                        m.map(|m| m.format("%d.%m.%Y %H:%M:%S").to_string()).unwrap_or_else(|| "дата неизвестна".into())
                    )
                };
                egui::Grid::new("exists-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    field_label(ui, if q.upload { "Локальный (новый)" } else { "На сервере (новый)" });
                    ui.label(fmt(q.source_size, q.source_modified));
                    ui.end_row();
                    field_label(ui, if q.upload { "На сервере" } else { "Локальный" });
                    ui.label(fmt(q.target_size, q.target_modified));
                    ui.end_row();
                });
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::primary_button(ui, "Перезаписать").clicked() {
                        answer = Some(ExistsAnswer::Overwrite);
                    }
                    if ui.button("Если новее").clicked() {
                        answer = Some(ExistsAnswer::OverwriteIfNewer);
                    }
                    if ui.button("Докачать").clicked() {
                        answer = Some(ExistsAnswer::Resume);
                    }
                    if ui.button("Переименовать").clicked() {
                        answer = Some(ExistsAnswer::Rename);
                    }
                    if ui.button("Пропустить").clicked() {
                        answer = Some(ExistsAnswer::Skip);
                    }
                });
                ui.add_space(8.0);
                ui.checkbox(&mut all, "Применить ко всем файлам в очереди");
            });
            self.exists_apply_all = all;
            if let Some(a) = answer
                && let Some((_, tx)) = self.exists_prompts.pop_front()
            {
                let _ = tx.send((a, all));
            }
        }
    }
}

fn progress_cell(ui: &mut egui::Ui, it: &crate::transfer::Item, style: ProgressStyle) {
    match style {
        ProgressStyle::Dots => dots_bar(ui, it),
        ProgressStyle::Bar if it.parts.len() > 1 => segmented_bar(ui, it),
        ProgressStyle::Bar => {
            let f = it.fraction();
            ui.add(
                egui::ProgressBar::new(f)
                    .desired_width(ui.available_width() - 6.0)
                    .desired_height(18.0)
                    .fill(ACCENT)
                    .corner_radius(5)
                    .text(RichText::new(progress_text(it)).size(11.5).color(Color32::WHITE)),
            );
        }
    }
}

fn progress_text(it: &crate::transfer::Item) -> String {
    let mut s = format!("{:.0}%  ·  {}", it.fraction() * 100.0, theme::human_speed(it.speed()));
    if it.parts.len() > 1 {
        s += &format!("  ·  {}", crate::transfer::threads(it.parts.len()));
    }
    s
}

/// The ranges to draw: the real parts of a segmented transfer, or one range
/// for an ordinary one.
fn display_parts(it: &crate::transfer::Item) -> Vec<Part> {
    if it.parts.is_empty() {
        vec![Part { start: 0, end: it.size, pos: it.transferred.min(it.size) }]
    } else {
        it.parts.clone()
    }
}

/// Bytes of `lo..hi` that are already on disk.
fn covered(parts: &[Part], lo: u64, hi: u64) -> u64 {
    // Data before the first range was there before this run (resume).
    let mut got = parts.first().map_or(0, |p| p.start.clamp(lo, hi) - lo);
    for p in parts {
        let (a, b) = (p.start.max(lo), p.pos.min(hi));
        if b > a {
            got += b - a;
        }
    }
    got
}

fn draw_label(painter: &egui::Painter, rect: egui::Rect, text: String) {
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.5), Color32::WHITE);
    let pos = egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
    painter.galley_with_override_text_color(pos + egui::vec2(1.0, 1.0), galley.clone(), Color32::from_black_alpha(220));
    painter.galley(pos, galley, Color32::WHITE);
}

fn parts_tooltip(resp: egui::Response, parts: &[Part]) {
    if parts.len() < 2 {
        return;
    }
    resp.on_hover_ui(|ui| {
        for (i, p) in parts.iter().enumerate() {
            let len = p.end.saturating_sub(p.start).max(1);
            let got = p.pos.saturating_sub(p.start).min(len);
            ui.label(format!(
                "Поток {}: {:.0}%  ({} из {})",
                i + 1,
                got as f64 / len as f64 * 100.0,
                theme::human_size(got),
                theme::human_size(len)
            ));
        }
    });
}

/// Progress bar of a segmented transfer: every connection fills its own
/// stretch of the file, so with 4 threads the bar grows in 4 places.
fn segmented_bar(ui: &mut egui::Ui, it: &crate::transfer::Item) {
    let width = (ui.available_width() - 6.0).max(40.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let parts = display_parts(it);
    let size = it.size.max(1) as f64;
    let x = |b: u64| rect.left() + (b.min(it.size) as f64 / size) as f32 * rect.width();
    painter.rect_filled(rect, 5.0, BG2);
    if let Some(first) = parts.first()
        && first.start > 0
    {
        let r = egui::Rect::from_x_y_ranges(rect.left()..=x(first.start), rect.y_range());
        painter.rect_filled(r, 0.0, theme::ACCENT_DIM);
    }
    for p in &parts {
        let color = if p.pos >= p.end { theme::GREEN } else { ACCENT };
        painter.rect_filled(egui::Rect::from_x_y_ranges(x(p.start)..=x(p.pos), rect.y_range()), 0.0, color);
        if p.start > 0 {
            let sx = x(p.start);
            painter.line_segment(
                [egui::pos2(sx, rect.top()), egui::pos2(sx, rect.bottom())],
                egui::Stroke::new(1.0, theme::BG0),
            );
        }
    }
    painter.rect_stroke(rect, 5.0, egui::Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    draw_label(&painter, rect, progress_text(it));
    parts_tooltip(resp, &parts);
}

/// Grid of dots, one per block of the file, like a torrent client's piece
/// map. Blocks are numbered column by column, so each connection's range
/// is a vertical band that lights up from left to right.
fn dots_bar(ui: &mut egui::Ui, it: &crate::transfer::Item) {
    const ROWS: usize = 3;
    const PITCH: f32 = 5.0;
    const DOT: f32 = 3.5;
    const TEXT_H: f32 = 14.0;
    let width = (ui.available_width() - 6.0).max(40.0);
    let (cell, resp) = ui.allocate_exact_size(egui::vec2(width, TEXT_H + ROWS as f32 * PITCH + 1.0), egui::Sense::hover());
    let painter = ui.painter_at(cell);
    painter.text(
        cell.left_top(),
        egui::Align2::LEFT_TOP,
        progress_text(it),
        egui::FontId::proportional(11.0),
        theme::TEXT,
    );
    let rect = egui::Rect::from_min_max(cell.min + egui::vec2(0.0, TEXT_H), cell.max);
    let parts = display_parts(it);
    let cols = ((rect.width() + PITCH - DOT) / PITCH).floor().max(1.0) as usize;
    let blocks = (cols * ROWS) as u64;
    let size = it.size.max(1);
    let empty = theme::BG3;
    for b in 0..blocks {
        let lo = size * b / blocks;
        let hi = (size * (b + 1) / blocks).max(lo + 1);
        let got = covered(&parts, lo, hi);
        let color = if got >= hi - lo {
            ACCENT
        } else if got > 0 {
            theme::ACCENT_DIM
        } else {
            empty
        };
        let (col, row) = (b as usize / ROWS, b as usize % ROWS);
        let min = rect.min + egui::vec2(col as f32 * PITCH, row as f32 * PITCH);
        painter.rect_filled(egui::Rect::from_min_size(min, egui::vec2(DOT, DOT)), 1.0, color);
    }
    parts_tooltip(resp, &parts);
}
