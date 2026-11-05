//! Site manager dialog.

use egui::{Color32, Margin, RichText};

use crate::app::App;
use crate::config::{self, FtpMode, Protocol, Site};
use crate::theme::{self, ACCENT, BG1, TEXT_WEAK};

pub struct SiteManager {
    pub sites: Vec<Site>,
    pub selected: Option<usize>,
    pub filter: String,
    pub import_requested: bool,
    pub show_password: bool,
    pub message: Option<(String, Color32)>,
}

impl SiteManager {
    pub fn new(sites: &[Site], select_id: Option<u64>) -> Self {
        let selected = select_id
            .and_then(|id| sites.iter().position(|s| s.id == id))
            .or(if sites.is_empty() { None } else { Some(0) });
        Self {
            sites: sites.to_vec(),
            selected,
            filter: String::new(),
            import_requested: false,
            show_password: false,
            message: None,
        }
    }

    fn import(&mut self, pick: bool) {
        let path = if pick {
            let mut d = rfd::FileDialog::new().add_filter("FileZilla sitemanager.xml", &["xml"]);
            if let Some(p) = config::filezilla_sitemanager_path().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
                d = d.set_directory(p);
            }
            d.pick_file()
        } else {
            config::filezilla_sitemanager_path()
        };
        let Some(path) = path else {
            if !pick {
                self.import(true);
            }
            return;
        };
        match config::import_filezilla(&path, config::next_site_id(&self.sites)) {
            Ok(list) => {
                let n = list.len();
                self.sites.extend(list);
                if n > 0 {
                    self.selected = Some(self.sites.len() - n);
                }
                self.message = Some((format!("Импортировано сайтов: {n} из {}", path.display()), theme::GREEN));
            }
            Err(e) => self.message = Some((format!("Ошибка импорта: {e:#}"), theme::RED)),
        }
    }
}

enum Outcome {
    None,
    Close,
    Connect(Box<Site>),
}

impl App {
    pub fn site_manager_ui(&mut self, ctx: &egui::Context) {
        let Some(mut sm) = self.site_manager.take() else { return };
        if sm.import_requested {
            sm.import_requested = false;
            sm.import(false);
        }
        let mut outcome = Outcome::None;
        let frame = egui::Frame::new()
            .fill(theme::BG2)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(14)
            .inner_margin(Margin::same(0))
            .shadow(egui::Shadow { offset: [0, 16], blur: 40, spread: 0, color: Color32::from_black_alpha(160) });
        let screen = ctx.content_rect();
        let w = (screen.width() - 80.0).clamp(560.0, 900.0);
        let h = (screen.height() - 120.0).clamp(380.0, 600.0);
        let resp = egui::Modal::new(egui::Id::unique("site-manager")).frame(frame).show(ctx, |ui| {
            ui.set_width(w);
            ui.set_height(h);
            // header
            egui::Frame::new().inner_margin(Margin { left: 22, right: 22, top: 18, bottom: 10 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::bold("Менеджер сайтов", 19.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📥 Импорт из FileZilla…").clicked() {
                            sm.import(true);
                        }
                    });
                });
                if let Some((m, c)) = &sm.message {
                    ui.label(RichText::new(m).color(*c).size(12.0));
                }
            });
            ui.separator();

            let body_h = ui.available_height() - 62.0;
            let (body, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), body_h), egui::Sense::hover());
            let left_rect = egui::Rect::from_min_size(body.min, egui::vec2(270.0, body_h));
            let right_rect = egui::Rect::from_min_max(egui::pos2(left_rect.max.x, body.min.y), body.max);
            ui.painter().rect_filled(left_rect, 0.0, BG1);
            {
                // ---- left: list -------------------------------------------
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(left_rect.shrink(10.0)));
                let ui = &mut ui;
                {
                    {
                        ui.add(
                            egui::TextEdit::singleline(&mut sm.filter)
                                .hint_text("🔍 Поиск")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 5)),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("➕ Новый").clicked() {
                                let mut s = Site { id: config::next_site_id(&sm.sites), ..Default::default() };
                                if let Some(sel) = sm.selected.and_then(|i| sm.sites.get(i)) {
                                    s.group = sel.group.clone();
                                }
                                sm.sites.push(s);
                                sm.selected = Some(sm.sites.len() - 1);
                            }
                            let has = sm.selected.is_some();
                            if ui.add_enabled(has, egui::Button::new("📋")).on_hover_text("Дублировать").clicked()
                                && let Some(i) = sm.selected
                            {
                                let mut s = sm.sites[i].clone();
                                s.id = config::next_site_id(&sm.sites);
                                s.name = format!("{} (копия)", s.name);
                                sm.sites.insert(i + 1, s);
                                sm.selected = Some(i + 1);
                            }
                            if ui
                                .add_enabled(has, egui::Button::new(RichText::new("🗑").color(theme::RED)))
                                .on_hover_text("Удалить")
                                .clicked()
                                && let Some(i) = sm.selected
                            {
                                sm.sites.remove(i);
                                sm.selected = if sm.sites.is_empty() { None } else { Some(i.min(sm.sites.len() - 1)) };
                            }
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("sm-list").auto_shrink([false, false]).show(ui, |ui| {
                            let f = sm.filter.to_lowercase();
                            let mut order: Vec<usize> = (0..sm.sites.len())
                                .filter(|&i| {
                                    let s = &sm.sites[i];
                                    f.is_empty()
                                        || s.name.to_lowercase().contains(&f)
                                        || s.host.to_lowercase().contains(&f)
                                        || s.group.to_lowercase().contains(&f)
                                })
                                .collect();
                            order.sort_by(|&a, &b| {
                                let (x, y) = (&sm.sites[a], &sm.sites[b]);
                                x.group.cmp(&y.group).then_with(|| crate::ui::pane::natord(&x.name, &y.name))
                            });
                            let mut last_group: Option<String> = None;
                            let mut connect_now = None;
                            for i in order {
                                let s = &sm.sites[i];
                                if last_group.as_deref() != Some(s.group.as_str()) {
                                    if !s.group.is_empty() {
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            theme::folder_icon(ui, false);
                                            ui.label(RichText::new(&s.group).size(12.0).color(theme::FOLDER));
                                        });
                                    }
                                    last_group = Some(s.group.clone());
                                }
                                let selected = sm.selected == Some(i);
                                let indent = if s.group.is_empty() { "" } else { "    " };
                                let r = ui.add(
                                    egui::Button::selectable(
                                        selected,
                                        RichText::new(format!("{indent}{}", s.name)).color(if selected { theme::TEXT_STRONG } else { theme::TEXT }),
                                    )
                                    .min_size(egui::vec2(ui.available_width(), 28.0)),
                                );
                                if r.clicked() {
                                    sm.selected = Some(i);
                                }
                                if r.double_clicked() {
                                    connect_now = Some(s.clone());
                                }
                            }
                            if let Some(s) = connect_now {
                                outcome = Outcome::Connect(Box::new(s));
                            }
                            if sm.sites.is_empty() {
                                ui.add_space(20.0);
                                ui.label(RichText::new("Нет сохранённых сайтов.\nНажмите «Новый» или импортируйте из FileZilla.").color(TEXT_WEAK));
                            }
                        });
                    }
                }
            }
            {
                // ---- right: editor ----------------------------------------
                let inner = egui::Rect::from_min_max(
                    right_rect.min + egui::vec2(22.0, 14.0),
                    right_rect.max - egui::vec2(22.0, 10.0),
                );
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
                let ui = &mut ui;
                if let Some(i) = sm.selected {
                    let show_password = &mut sm.show_password;
                    let s = &mut sm.sites[i];
                    egui::ScrollArea::vertical().id_salt("sm-form").auto_shrink([false, false]).show(ui, |ui| {
                        site_form(ui, s, show_password);
                    });
                } else {
                    ui.centered_and_justified(|ui| ui.label(RichText::new("Выберите сайт слева").color(TEXT_WEAK)));
                }
            }

            ui.separator();
            egui::Frame::new().inner_margin(Margin { left: 22, right: 22, top: 6, bottom: 14 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let can_connect = sm.selected.is_some();
                    if ui
                        .add_enabled(
                            can_connect,
                            egui::Button::new(RichText::new("⚡ Подключиться").color(Color32::WHITE).strong()).fill(ACCENT),
                        )
                        .clicked()
                        && let Some(i) = sm.selected
                    {
                        outcome = Outcome::Connect(Box::new(sm.sites[i].clone()));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Закрыть").clicked() {
                            outcome = Outcome::Close;
                        }
                        ui.label(RichText::new("Изменения сохраняются автоматически").size(11.0).color(TEXT_WEAK));
                    });
                });
            });
        });
        if resp.should_close() && matches!(outcome, Outcome::None) {
            outcome = Outcome::Close;
        }
        match outcome {
            Outcome::None => {
                // autosave: edits are never lost, even if the app is killed
                if sm.sites != self.sites {
                    self.sites = sm.sites.clone();
                    self.save_sites();
                }
                self.site_manager = Some(sm);
            }
            Outcome::Close => {
                if sm.sites != self.sites {
                    // keep edits: closing is not cancelling in a manager like this
                    self.sites = sm.sites;
                    self.save_sites();
                }
            }
            Outcome::Connect(site) => {
                self.sites = sm.sites;
                self.save_sites();
                self.connect(*site);
            }
        }
    }
}

fn site_form(ui: &mut egui::Ui, s: &mut Site, show_password: &mut bool) {
    let label = |ui: &mut egui::Ui, t: &str| {
        ui.label(RichText::new(t).color(TEXT_WEAK));
    };
    let te = |ui: &mut egui::Ui, v: &mut String, hint: &str| {
        ui.add(egui::TextEdit::singleline(v).hint_text(hint).desired_width(f32::INFINITY).margin(Margin::symmetric(8, 5)))
    };
    egui::Grid::new("site-form").num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| {
        label(ui, "Название");
        te(ui, &mut s.name, "Мой сервер");
        ui.end_row();
        label(ui, "Группа");
        te(ui, &mut s.group, "необязательно, например «Работа»");
        ui.end_row();

        label(ui, "Протокол");
        let before = s.protocol;
        egui::ComboBox::from_id_salt("site-proto").selected_text(s.protocol.label()).width(220.0).show_ui(ui, |ui| {
            for p in Protocol::ALL {
                ui.selectable_value(&mut s.protocol, p, p.label());
            }
        });
        if before != s.protocol && s.port == before.default_port() {
            s.port = 0;
        }
        ui.end_row();

        label(ui, "Хост");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.host).hint_text("example.com").desired_width(ui.available_width() - 110.0).margin(Margin::symmetric(8, 5)));
            ui.label(RichText::new("порт").color(TEXT_WEAK));
            let mut port = if s.port == 0 { String::new() } else { s.port.to_string() };
            if ui
                .add(egui::TextEdit::singleline(&mut port).hint_text(s.protocol.default_port().to_string()).desired_width(60.0).margin(Margin::symmetric(8, 5)))
                .changed()
            {
                s.port = port.trim().parse().unwrap_or(0);
            }
        });
        ui.end_row();

        label(ui, "Пользователь");
        te(ui, &mut s.user, if s.protocol == Protocol::Sftp { "root" } else { "пусто = anonymous" });
        ui.end_row();

        label(ui, if s.key_file.is_empty() { "Пароль" } else { "Пароль ключа" });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut s.password)
                    .password(!*show_password)
                    .desired_width(ui.available_width() - 40.0)
                    .margin(Margin::symmetric(8, 5)),
            );
            if ui.selectable_label(*show_password, "👁").on_hover_text("Показать пароль").clicked() {
                *show_password = !*show_password;
            }
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut s.remember_password, "Запомнить пароль")
            .on_hover_text("Пароли хранятся в sites.json в обфусцированном (не зашифрованном) виде, как и в FileZilla");
        ui.end_row();

        if s.protocol == Protocol::Sftp {
            label(ui, "Ключ SSH");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.key_file).hint_text("~/.ssh/id_ed25519").desired_width(ui.available_width() - 80.0).margin(Margin::symmetric(8, 5)));
                if ui.button("Обзор…").clicked() {
                    let mut d = rfd::FileDialog::new();
                    if let Some(h) = dirs::home_dir() {
                        d = d.set_directory(h.join(".ssh"));
                    }
                    if let Some(p) = d.pick_file() {
                        s.key_file = p.display().to_string();
                    }
                }
            });
            ui.end_row();
        } else {
            label(ui, "Режим FTP");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.ftp_mode, FtpMode::Passive, "Пассивный");
                ui.selectable_value(&mut s.ftp_mode, FtpMode::Active, "Активный");
            });
            ui.end_row();
        }

        label(ui, "Папка на сервере");
        te(ui, &mut s.remote_dir, "по умолчанию — домашняя");
        ui.end_row();
        label(ui, "Локальная папка");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.local_dir).hint_text("не менять").desired_width(ui.available_width() - 80.0).margin(Margin::symmetric(8, 5)));
            if ui.button("Обзор…").clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                s.local_dir = p.display().to_string();
            }
        });
        ui.end_row();
        label(ui, "Комментарий");
        ui.add(egui::TextEdit::multiline(&mut s.comment).desired_rows(2).desired_width(f32::INFINITY));
        ui.end_row();
    });
    if s.protocol == Protocol::Ftp {
        ui.add_space(8.0);
        ui.label(RichText::new("⚠ Обычный FTP передаёт пароль и данные без шифрования. По возможности используйте FTPS или SFTP.").color(theme::YELLOW).size(12.0));
    }
}
