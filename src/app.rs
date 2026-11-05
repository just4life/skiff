//! Application state, event handling and top-level layout.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use egui::{Align, Color32, Layout, Margin, RichText};

use crate::config::{self, Site};
use crate::events::{AppEvent, ExistsAnswer, ExistsQuestion, Notifier};
use crate::remote::{self, LogLevel};
use crate::session::{Cmd, SessionHandle};
use crate::theme::{self, ACCENT, BG0, TEXT_WEAK};
use crate::transfer::{Direction, NewTransfer, TransferManager};
use crate::trust::{TrustDecision, TrustRequest, TrustStore};
use crate::ui::pane::{Pane, PaneAction, Side};
use crate::ui::site_manager::SiteManager;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BottomTab {
    Queue,
    Done,
    Failed,
    Log,
}

pub enum Dialog {
    Mkdir { side: Side, name: String },
    Rename { side: Side, from: String, to: String },
    Delete { side: Side, names: Vec<String> },
    Password { site: Box<Site>, password: String },
    Settings,
    About,
}

pub struct Toast {
    pub text: String,
    pub color: Color32,
    pub at: Instant,
}

pub struct LogLine {
    pub time: chrono::DateTime<chrono::Local>,
    pub level: LogLevel,
    pub text: String,
}

pub struct App {
    pub settings: config::Settings,
    pub sites: Vec<Site>,
    pub trust: Arc<TrustStore>,
    pub notify: Notifier,
    rx: Receiver<AppEvent>,
    pub transfers: TransferManager,

    pub session: Option<SessionHandle>,
    session_counter: u64,
    pub conn: ConnState,
    pub remote_busy: bool,
    pub home_dir: String,

    pub local: Pane,
    pub remote: Pane,
    pub active_side: Side,
    pub roots: Vec<PathBuf>,

    pub log: VecDeque<LogLine>,
    pub toasts: Vec<Toast>,
    pub bottom_tab: BottomTab,

    pub quick: Site,
    pub quick_host: String,
    pub quick_port: String,

    pub site_manager: Option<SiteManager>,
    pub dialog: Option<Dialog>,
    pub trust_prompts: VecDeque<(TrustRequest, Sender<TrustDecision>)>,
    pub exists_prompts: VecDeque<(ExistsQuestion, Sender<(ExistsAnswer, bool)>)>,
    pub exists_apply_all: bool,

    remote_refresh_at: Option<Instant>,
    local_refresh: bool,
    pending_dir: Option<String>,
    window_title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = config::load_settings();
        if settings.ui_scale > 0.5 && (settings.ui_scale - 1.0).abs() > 0.01 {
            cc.egui_ctx.set_zoom_factor(settings.ui_scale);
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        let notify = Notifier::new(tx, cc.egui_ctx.clone());
        let prompt_notify = notify.clone();
        let trust = TrustStore::load(Arc::new(move |req| {
            prompt_notify
                .ask(|tx| AppEvent::Trust(req, tx))
                .unwrap_or(TrustDecision::Reject)
        }));
        let transfers = TransferManager::new(
            trust.clone(),
            notify.clone(),
            settings.max_transfers,
            settings.exists_action,
        );

        let mut quick = Site {
            name: "Быстрое подключение".into(),
            protocol: settings.quick_protocol,
            user: settings.quick_user.clone(),
            remember_password: false,
            ..Default::default()
        };
        quick.id = 0;

        let mut app = Self {
            quick_host: settings.quick_host.clone(),
            quick_port: settings.quick_port.clone(),
            settings,
            sites: config::load_sites(),
            trust,
            notify,
            rx,
            transfers,
            session: None,
            session_counter: 0,
            conn: ConnState::Disconnected,
            remote_busy: false,
            home_dir: "/".into(),
            local: Pane::new(Side::Local),
            remote: Pane::new(Side::Remote),
            active_side: Side::Local,
            roots: crate::local::roots(),
            log: VecDeque::new(),
            toasts: Vec::new(),
            bottom_tab: BottomTab::Queue,
            quick,
            site_manager: None,
            dialog: None,
            trust_prompts: VecDeque::new(),
            exists_prompts: VecDeque::new(),
            exists_apply_all: false,
            remote_refresh_at: None,
            local_refresh: false,
            pending_dir: None,
            window_title: "Skiff".into(),
        };
        let start = Some(app.settings.last_local_dir.clone())
            .filter(|d| !d.is_empty() && Path::new(d).is_dir())
            .or_else(|| dirs::home_dir().map(|h| h.display().to_string()))
            .unwrap_or_else(|| "/".into());
        app.navigate_local(&start);
        app.push_log(LogLevel::Info, format!("Skiff {} готов к работе", env!("CARGO_PKG_VERSION")));
        app
    }

    // ---------------------------------------------------------------------
    // helpers
    // ---------------------------------------------------------------------

    pub fn push_log(&mut self, level: LogLevel, text: String) {
        self.log.push_back(LogLine { time: chrono::Local::now(), level, text });
        while self.log.len() > 3000 {
            self.log.pop_front();
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push(Toast { text: text.into(), color, at: Instant::now() });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub fn save_settings(&mut self) {
        self.settings.last_local_dir = self.local.path.clone();
        if let Err(e) = config::save_settings(&self.settings) {
            self.push_log(LogLevel::Error, format!("Не удалось сохранить настройки: {e}"));
        }
    }

    pub fn save_sites(&mut self) {
        if let Err(e) = config::save_sites(&self.sites) {
            self.toast(format!("Не удалось сохранить сайты: {e}"), theme::RED);
        }
    }

    pub fn navigate_local(&mut self, path: &str) {
        let p = PathBuf::from(path);
        match crate::local::list(&p, self.settings.show_hidden_local) {
            Ok(entries) => {
                let canon = std::fs::canonicalize(&p).unwrap_or(p);
                let mut s = canon.display().to_string();
                // Windows verbatim prefix looks ugly
                if let Some(stripped) = s.strip_prefix(r"\\?\") {
                    s = stripped.to_string();
                }
                let changed = s != self.local.path;
                self.local.set_listing(s, entries);
                if changed {
                    self.settings.last_local_dir = self.local.path.clone();
                    let _ = config::save_settings(&self.settings);
                }
            }
            Err(e) => {
                self.local.error = Some(format!("{path}: {e}"));
                self.local.path_edit = self.local.path.clone();
            }
        }
    }

    fn refresh_local(&mut self) {
        let p = self.local.path.clone();
        if let Ok(entries) = crate::local::list(Path::new(&p), self.settings.show_hidden_local) {
            self.local.set_listing(p, entries);
        }
    }

    pub fn remote_cmd(&mut self, cmd: Cmd) {
        if let Some(s) = &self.session {
            s.send(cmd);
        }
    }

    pub fn navigate_remote(&mut self, path: &str) {
        let path = if path.is_empty() { "/".to_string() } else { path.to_string() };
        self.remote_cmd(Cmd::List(path));
    }

    pub fn connected_site(&self) -> Option<&Site> {
        self.session.as_ref().map(|s| &s.site)
    }

    // ---------------------------------------------------------------------
    // connect / disconnect
    // ---------------------------------------------------------------------

    pub fn connect(&mut self, site: Site) {
        let needs_password = !site.remember_password
            && site.password.is_empty()
            && site.key_file.trim().is_empty()
            && !site.user.trim().is_empty()
            && site.user != "anonymous";
        if needs_password {
            self.dialog = Some(Dialog::Password { site: Box::new(site), password: String::new() });
            return;
        }
        self.disconnect(false);
        self.session_counter += 1;
        let id = self.session_counter;
        if !site.local_dir.trim().is_empty() {
            self.navigate_local(&site.local_dir.clone());
        }
        let initial = Some(site.remote_dir.clone()).filter(|d| !d.trim().is_empty());
        self.push_log(LogLevel::Info, format!("Соединение с {}", site.display_url()));
        self.remote.clear();
        self.conn = ConnState::Connecting;
        self.remote_busy = true;
        self.session = Some(SessionHandle::start(
            id,
            site,
            initial,
            self.trust.clone(),
            self.notify.clone(),
        ));
    }

    pub fn disconnect(&mut self, user_initiated: bool) {
        if self.session.take().is_some() && user_initiated {
            self.push_log(LogLevel::Info, "Отключение".into());
        }
        self.conn = ConnState::Disconnected;
        self.remote_busy = false;
        self.remote.clear();
    }

    pub fn quick_connect(&mut self) {
        let mut site = self.quick.clone();
        let path = site.apply_host_url(&self.quick_host.clone());
        if let Ok(p) = self.quick_port.trim().parse::<u16>() {
            site.port = p;
        } else if self.quick_port.trim().is_empty() && !self.quick_host.contains(':') {
            site.port = 0;
        }
        if let Some(p) = path {
            site.remote_dir = p;
        }
        if site.host.is_empty() {
            self.toast("Укажите хост", theme::YELLOW);
            return;
        }
        site.name = site.host.clone();
        // sync parsed values back into the form
        self.quick.protocol = site.protocol;
        self.quick.user = site.user.clone();
        self.quick_host = site.host.clone();
        if site.port != 0 {
            self.quick_port = site.port.to_string();
        }
        self.settings.quick_protocol = site.protocol;
        self.settings.quick_host = self.quick_host.clone();
        self.settings.quick_user = site.user.clone();
        self.settings.quick_port = self.quick_port.clone();
        self.save_settings();
        site.remember_password = true; // the password field is right there
        self.connect(site);
    }

    // ---------------------------------------------------------------------
    // events from workers
    // ---------------------------------------------------------------------

    fn process_events(&mut self) {
        let current = self.session.as_ref().map(|s| s.id);
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Log(level, text) => self.push_log(level, text),
                AppEvent::Connected { session, home } if Some(session) == current => {
                    self.conn = ConnState::Connected;
                    self.home_dir = home;
                    if let Some(s) = &self.session {
                        let text = format!("Подключено к {}", s.site.host);
                        self.toast(text, theme::GREEN);
                    }
                }
                AppEvent::Listing { session, path, entries } if Some(session) == current => {
                    self.remote.set_listing(path, entries);
                    if let Some(name) = self.pending_dir.take() {
                        self.remote.selected.clear();
                        self.remote.selected.insert(name);
                    }
                }
                AppEvent::OpFailed { session, message } if Some(session) == current => {
                    self.toast(message.clone(), theme::RED);
                    if !self.remote.loaded {
                        self.remote.error = Some(message);
                    }
                    self.remote.path_edit = self.remote.path.clone();
                }
                AppEvent::Disconnected { session, error } if Some(session) == current => {
                    self.session = None;
                    self.conn = ConnState::Disconnected;
                    self.remote_busy = false;
                    self.remote.clear();
                    if let Some(e) = error {
                        self.toast(format!("Не удалось подключиться: {e}"), theme::RED);
                        self.remote.error = Some(e);
                    }
                }
                AppEvent::Busy { session, busy } if Some(session) == current => {
                    self.remote_busy = busy;
                }
                AppEvent::RemoteChanged(dir) => {
                    if dir.trim_end_matches('/') == self.remote.path.trim_end_matches('/')
                        || (dir == "/" && self.remote.path == "/")
                    {
                        // debounce: refresh shortly after the LAST change
                        self.remote_refresh_at = Some(Instant::now());
                    }
                }
                AppEvent::LocalChanged(dir) => {
                    if Path::new(&self.local.path) == dir {
                        self.local_refresh = true;
                    }
                }
                AppEvent::Trust(req, tx) => self.trust_prompts.push_back((req, tx)),
                AppEvent::FileExists(q, tx) => {
                    self.exists_apply_all = false;
                    self.exists_prompts.push_back((q, tx))
                }
                _ => {}
            }
        }
        if self.local_refresh {
            self.local_refresh = false;
            self.refresh_local();
        }
        if let Some(t) = self.remote_refresh_at
            && t.elapsed() > Duration::from_millis(300)
            && !self.remote_busy
        {
            self.remote_refresh_at = None;
            let p = self.remote.path.clone();
            if !p.is_empty() {
                self.remote_cmd(Cmd::List(p));
            }
        }
    }

    // ---------------------------------------------------------------------
    // pane actions
    // ---------------------------------------------------------------------

    fn local_full(&self, name: &str) -> PathBuf {
        Path::new(&self.local.path).join(name)
    }

    fn remote_full(&self, name: &str) -> String {
        remote::join(&self.remote.path, name)
    }

    pub fn upload(&mut self, names: Vec<String>) {
        let Some(site) = self.connected_site().cloned() else {
            self.toast("Сначала подключитесь к серверу", theme::YELLOW);
            return;
        };
        if self.conn != ConnState::Connected || self.remote.path.is_empty() {
            return;
        }
        let items = names
            .iter()
            .filter_map(|n| {
                let local = self.local_full(n);
                let meta = std::fs::metadata(&local).ok()?;
                Some(NewTransfer {
                    direction: Direction::Upload,
                    remote: self.remote_full(n),
                    local,
                    is_dir: meta.is_dir(),
                    size: if meta.is_dir() { 0 } else { meta.len() },
                })
            })
            .collect::<Vec<_>>();
        self.enqueue(&site, items);
    }

    pub fn upload_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(site) = self.connected_site().cloned() else {
            self.toast("Сначала подключитесь к серверу", theme::YELLOW);
            return;
        };
        let items = paths
            .into_iter()
            .filter_map(|local| {
                let meta = std::fs::metadata(&local).ok()?;
                let name = local.file_name()?.to_string_lossy().to_string();
                Some(NewTransfer {
                    direction: Direction::Upload,
                    remote: self.remote_full(&name),
                    local,
                    is_dir: meta.is_dir(),
                    size: if meta.is_dir() { 0 } else { meta.len() },
                })
            })
            .collect();
        self.enqueue(&site, items);
    }

    pub fn download(&mut self, names: Vec<String>) {
        let Some(site) = self.connected_site().cloned() else { return };
        let items = names
            .iter()
            .filter_map(|n| {
                let e = self.remote.entry(n)?;
                Some(NewTransfer {
                    direction: Direction::Download,
                    local: self.local_full(&crate::transfer::sanitize_local_name(n)),
                    remote: self.remote_full(n),
                    is_dir: e.is_dir,
                    size: e.size,
                })
            })
            .collect::<Vec<_>>();
        self.enqueue(&site, items);
    }

    fn enqueue(&mut self, site: &Site, items: Vec<NewTransfer>) {
        if items.is_empty() {
            return;
        }
        let n = items.len();
        // a fresh batch gets a fresh "apply to all" choice
        {
            let mut q = self.transfers.queue.lock().unwrap();
            if !q.items.iter().any(|i| !i.status.is_finished()) {
                q.exists_action = self.settings.exists_action;
            }
        }
        self.transfers.enqueue(site, items);
        self.bottom_tab = BottomTab::Queue;
        self.push_log(LogLevel::Info, format!("В очередь добавлено: {n}"));
    }

    fn handle_pane_actions(&mut self, side: Side, actions: Vec<PaneAction>) {
        for a in actions {
            match (side, a) {
                (_, PaneAction::Focus) => self.active_side = side,
                (Side::Local, PaneAction::Open(name)) => {
                    let p = self.local_full(&name);
                    if p.is_dir() {
                        self.navigate_local(&p.display().to_string());
                    } else {
                        self.upload(vec![name]);
                    }
                }
                (Side::Remote, PaneAction::Open(name)) => {
                    if let Some(e) = self.remote.entry(&name) {
                        if e.is_dir {
                            let p = self.remote_full(&name);
                            self.navigate_remote(&p);
                        } else {
                            self.download(vec![name]);
                        }
                    }
                }
                (Side::Local, PaneAction::Up) => {
                    if let Some(parent) = Path::new(&self.local.path).parent() {
                        let child = Path::new(&self.local.path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string());
                        self.navigate_local(&parent.display().to_string());
                        if let Some(c) = child {
                            self.local.selected.insert(c);
                        }
                    }
                }
                (Side::Remote, PaneAction::Up) => {
                    if !self.remote.path.is_empty() && self.remote.path != "/" {
                        self.pending_dir = Some(remote::file_name(&self.remote.path).to_string());
                        let parent = remote::parent(&self.remote.path);
                        self.navigate_remote(&parent);
                    }
                }
                (Side::Local, PaneAction::Back) => {
                    if let Some(prev) = self.local.history.pop() {
                        self.local.suppress_history = true;
                        self.navigate_local(&prev);
                        self.local.suppress_history = false;
                    }
                }
                (Side::Remote, PaneAction::Back) => {
                    if let Some(prev) = self.remote.history.pop() {
                        self.remote.suppress_history = true;
                        self.navigate_remote(&prev);
                    }
                }
                (Side::Local, PaneAction::Home) => {
                    if let Some(h) = dirs::home_dir() {
                        self.navigate_local(&h.display().to_string());
                    }
                }
                (Side::Remote, PaneAction::Home) => {
                    let h = self.home_dir.clone();
                    self.navigate_remote(&h);
                }
                (Side::Local, PaneAction::Navigate(p)) => self.navigate_local(&p),
                (Side::Remote, PaneAction::Navigate(p)) => self.navigate_remote(&p),
                (Side::Local, PaneAction::Refresh) => self.refresh_local(),
                (Side::Remote, PaneAction::Refresh) => {
                    let p = self.remote.path.clone();
                    self.navigate_remote(&p);
                }
                (Side::Local, PaneAction::Transfer(names)) => self.upload(names),
                (Side::Remote, PaneAction::Transfer(names)) => self.download(names),
                (Side::Local, PaneAction::Dropped(names)) => self.download(names),
                (Side::Remote, PaneAction::Dropped(names)) => self.upload(names),
                (s, PaneAction::Mkdir) => {
                    self.dialog = Some(Dialog::Mkdir { side: s, name: "Новая папка".into() })
                }
                (s, PaneAction::Rename(from)) => {
                    self.dialog = Some(Dialog::Rename { side: s, to: from.clone(), from })
                }
                (s, PaneAction::Delete(names)) => {
                    if !names.is_empty() {
                        self.dialog = Some(Dialog::Delete { side: s, names })
                    }
                }
            }
        }
    }

    pub fn do_mkdir(&mut self, side: Side, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        match side {
            Side::Local => {
                let p = self.local_full(name);
                match std::fs::create_dir_all(&p) {
                    Ok(()) => {
                        self.refresh_local();
                        self.local.selected = [name.to_string()].into();
                    }
                    Err(e) => self.toast(format!("Не удалось создать папку: {e}"), theme::RED),
                }
            }
            Side::Remote => {
                let p = self.remote_full(name);
                self.pending_dir = Some(name.to_string());
                self.remote_cmd(Cmd::Mkdir(p));
            }
        }
    }

    pub fn do_rename(&mut self, side: Side, from: &str, to: &str) {
        let to = to.trim();
        if to.is_empty() || to == from {
            return;
        }
        match side {
            Side::Local => match std::fs::rename(self.local_full(from), self.local_full(to)) {
                Ok(()) => {
                    self.refresh_local();
                    self.local.selected = [to.to_string()].into();
                }
                Err(e) => self.toast(format!("Не удалось переименовать: {e}"), theme::RED),
            },
            Side::Remote => {
                let (f, t) = (self.remote_full(from), self.remote_full(to));
                self.pending_dir = Some(to.to_string());
                self.remote_cmd(Cmd::Rename { from: f, to: t });
            }
        }
    }

    pub fn do_delete(&mut self, side: Side, names: &[String]) {
        match side {
            Side::Local => {
                let mut errors = Vec::new();
                for n in names {
                    if let Err(e) = crate::local::delete(&self.local_full(n)) {
                        errors.push(format!("{n}: {e}"));
                    }
                }
                if !errors.is_empty() {
                    self.toast(format!("Ошибка удаления: {}", errors.join("; ")), theme::RED);
                }
                self.refresh_local();
            }
            Side::Remote => {
                let items = names
                    .iter()
                    .filter_map(|n| {
                        let e = self.remote.entry(n)?;
                        Some((self.remote_full(n), e.is_dir && !e.is_link))
                    })
                    .collect();
                self.remote_cmd(Cmd::Delete(items));
            }
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some()
            || self.site_manager.is_some()
            || !self.trust_prompts.is_empty()
            || !self.exists_prompts.is_empty()
            || ctx.memory(|m| m.focused().is_some())
        {
            return;
        }
        let (del, f2, f5, back, enter, sel_all) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::F2),
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Enter),
                i.modifiers.command && i.key_pressed(egui::Key::A),
            )
        });
        let side = self.active_side;
        if side == Side::Remote && self.conn != ConnState::Connected {
            return;
        }
        let pane = if side == Side::Local { &mut self.local } else { &mut self.remote };
        let sel = pane.selection();
        let mut actions = Vec::new();
        if sel_all {
            pane.selected = pane.entries.iter().map(|e| e.name.clone()).collect();
        }
        if del && !sel.is_empty() {
            actions.push(PaneAction::Delete(sel.clone()));
        }
        if f2 && sel.len() == 1 {
            actions.push(PaneAction::Rename(sel[0].clone()));
        }
        if f5 {
            actions.push(PaneAction::Refresh);
        }
        if back {
            actions.push(PaneAction::Up);
        }
        if enter && sel.len() == 1 {
            actions.push(PaneAction::Open(sel[0].clone()));
        }
        self.handle_pane_actions(side, actions);
    }

    // ---------------------------------------------------------------------
    // layout
    // ---------------------------------------------------------------------

    fn central(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        let gap = 10.0;
        let split = self.settings.split.clamp(0.2, 0.8);
        let left_w = ((full.width() - gap) * split).round();
        let left = egui::Rect::from_min_size(full.min, egui::vec2(left_w, full.height()));
        let right = egui::Rect::from_min_max(egui::pos2(left.max.x + gap, full.min.y), full.max);

        // splitter
        let handle = egui::Rect::from_min_max(egui::pos2(left.max.x, full.min.y), egui::pos2(right.min.x, full.max.y));
        let hr = ui.interact(handle, ui.scope_id().with("splitter"), egui::Sense::drag());
        if hr.hovered() || hr.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            ui.painter().line_segment(
                [handle.center_top() + egui::vec2(0.0, 20.0), handle.center_bottom() - egui::vec2(0.0, 20.0)],
                egui::Stroke::new(2.0, ACCENT.gamma_multiply(0.6)),
            );
        }
        if hr.dragged()
            && let Some(p) = ui.ctx().pointer_latest_pos()
        {
            self.settings.split = ((p.x - full.min.x) / full.width()).clamp(0.2, 0.8);
        }
        if hr.drag_stopped() {
            self.save_settings();
        }

        // horizontal splitter between panes and the queue
        let hsplit = egui::Rect::from_min_max(
            egui::pos2(full.min.x, full.max.y - 1.0),
            egui::pos2(full.max.x, full.max.y + 9.0),
        );
        let vr = ui.interact(hsplit, ui.scope_id().with("hsplit"), egui::Sense::drag());
        if vr.hovered() || vr.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            ui.painter().line_segment(
                [hsplit.left_center() + egui::vec2(40.0, 0.0), hsplit.right_center() - egui::vec2(40.0, 0.0)],
                egui::Stroke::new(2.0, ACCENT.gamma_multiply(0.6)),
            );
        }
        if vr.dragged() {
            self.settings.bottom_h -= vr.drag_delta().y;
        }
        if vr.drag_stopped() {
            self.save_settings();
        }

        let local_sub = String::new();
        let la = {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(left));
            let roots = self.roots.clone();
            self.local.ui(&mut child, "Этот компьютер", &local_sub, true, false, self.active_side == Side::Local, &roots)
        };
        let connected = self.conn == ConnState::Connected;
        let remote_title = match (&self.session, self.conn) {
            (Some(s), _) => s.site.name.clone(),
            (None, _) => "Сервер".to_string(),
        };
        let remote_sub = match (&self.session, self.conn) {
            (Some(s), ConnState::Connecting) => format!("подключение к {}…", s.site.endpoint()),
            (Some(s), _) => s.site.display_url(),
            _ => String::new(),
        };
        let ra = {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(right));
            let busy = self.remote_busy || self.conn == ConnState::Connecting;
            self.remote.ui(&mut child, &remote_title, &remote_sub, connected, busy, self.active_side == Side::Remote, &[])
        };
        ui.allocate_rect(full, egui::Sense::hover());
        self.handle_pane_actions(Side::Local, la);
        self.handle_pane_actions(Side::Remote, ra);

        // Files dropped from the OS file manager: upload when over the remote pane.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            let over_remote = ui.ctx().pointer_latest_pos().is_none_or(|p| right.contains(p));
            if over_remote && connected {
                self.upload_paths(dropped);
            } else if let Some(first) = dropped.first() {
                let dir = if first.is_dir() { first.clone() } else { first.parent().map(Path::to_path_buf).unwrap_or_default() };
                self.navigate_local(&dir.display().to_string());
            }
        }
    }

    fn drag_overlay(&self, ctx: &egui::Context) {
        if let Some(p) = egui::DragAndDrop::payload::<crate::ui::pane::DragPayload>(ctx)
            && let Some(pos) = ctx.pointer_latest_pos()
        {
            let text = if p.names.len() == 1 {
                p.names[0].clone()
            } else {
                format!("{} объектов", p.names.len())
            };
            let arrow = if p.from == Side::Local { "⬆" } else { "⬇" };
            egui::Area::new(egui::Id::unique("drag-overlay"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + egui::vec2(14.0, 10.0))
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(theme::ACCENT_DIM)
                        .stroke(egui::Stroke::new(1.0, ACCENT))
                        .corner_radius(8)
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("{arrow}  {text}")).color(theme::TEXT_STRONG));
                        });
                });
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(6));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::unique("toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                let mut remove = None;
                for (i, t) in self.toasts.iter().enumerate() {
                    let r = egui::Frame::new()
                        .fill(theme::BG3)
                        .stroke(egui::Stroke::new(1.0, t.color.gamma_multiply(0.7)))
                        .corner_radius(10)
                        .inner_margin(Margin::symmetric(14, 10))
                        .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) })
                        .show(ui, |ui| {
                            ui.set_width(340.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("●").color(t.color));
                                ui.label(RichText::new(&t.text).color(theme::TEXT_STRONG));
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if r.clicked() {
                        remove = Some(i);
                    }
                    ui.add_space(6.0);
                }
                if let Some(i) = remove {
                    self.toasts.remove(i);
                }
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (active, queued, speed, left) = {
            let q = self.transfers.queue.lock().unwrap();
            let active: Vec<_> = q.items.iter().filter(|i| i.status == crate::transfer::Status::Active && !i.is_dir).collect();
            let queued = q.items.iter().filter(|i| i.status == crate::transfer::Status::Queued).count();
            let speed: f64 = active.iter().map(|i| i.speed()).sum();
            let left: u64 = q
                .items
                .iter()
                .filter(|i| !i.status.is_finished() && !i.is_dir)
                .map(|i| i.size.saturating_sub(i.transferred))
                .sum();
            (active.len(), queued, speed, left)
        };
        ui.horizontal_centered(|ui| {
            let (dot, text) = match self.conn {
                ConnState::Connected => (theme::GREEN, "Подключено"),
                ConnState::Connecting => (theme::YELLOW, "Подключение…"),
                ConnState::Disconnected => (TEXT_WEAK, "Нет подключения"),
            };
            ui.label(RichText::new("●").color(dot).size(10.0));
            ui.label(RichText::new(text).size(12.0).color(TEXT_WEAK));
            if let Some(s) = self.connected_site() {
                let lock = if s.protocol.is_secure() { "🔒 " } else { "⚠ без шифрования · " };
                ui.label(RichText::new(format!("{lock}{}", s.protocol.label())).size(12.0).color(if s.protocol.is_secure() { TEXT_WEAK } else { theme::YELLOW }));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if active > 0 || queued > 0 {
                    ui.label(
                        RichText::new(format!(
                            "↕ {}  ·  активно {active}, в очереди {queued}  ·  осталось {}",
                            theme::human_speed(speed),
                            theme::human_size(left)
                        ))
                        .size(12.0)
                        .color(ACCENT),
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                } else {
                    ui.label(RichText::new("Очередь пуста").size(12.0).color(TEXT_WEAK));
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_events();
        if self.remote_refresh_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("toolbar")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG1)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 10 }),
            )
            .show(ui, |ui| self.toolbar(ui));

        egui::Panel::bottom("statusbar")
            .exact_size(28.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(BG0).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| self.status_bar(ui));

        let max_bottom = (ctx.content_rect().height() - 300.0).max(120.0);
        self.settings.bottom_h = self.settings.bottom_h.clamp(110.0, max_bottom);
        egui::Panel::bottom("bottom")
            .resizable(false)
            .exact_size(self.settings.bottom_h)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(BG0).inner_margin(Margin { left: 12, right: 12, top: 4, bottom: 4 }))
            .show(ui, |ui| self.bottom_panel(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG0).inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 6 }))
            .show(ui, |ui| self.central(ui));

        self.keyboard(&ctx);
        self.dialogs(&ctx);
        self.site_manager_ui(&ctx);
        self.prompts(&ctx);
        self.toasts_ui(&ctx);
        self.drag_overlay(&ctx);

        let title = match self.connected_site() {
            Some(s) => format!("{} — Skiff", s.name),
            None => "Skiff".to_string(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG0.to_normalized_gamma_f32()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.transfers.cancel_all();
        self.save_settings();
    }
}
