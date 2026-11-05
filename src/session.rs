//! The "browser" connection: one worker thread that owns the remote
//! connection used for listing and file operations in the remote pane.
//! Transfers use their own connections (see `transfer.rs`), like FileZilla.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::config::Site;
use crate::events::{AppEvent, Notifier};
use crate::remote::{self, LogLevel, RemoteFs};
use crate::trust::TrustStore;

pub enum Cmd {
    List(String),
    Mkdir(String),
    /// (path, is_dir) — directories are removed recursively.
    Delete(Vec<(String, bool)>),
    Rename { from: String, to: String },
}

pub struct SessionHandle {
    pub id: u64,
    pub site: Site,
    tx: Sender<Cmd>,
}

impl SessionHandle {
    pub fn start(
        id: u64,
        site: Site,
        initial_dir: Option<String>,
        trust: Arc<TrustStore>,
        notify: Notifier,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let worker_site = site.clone();
        std::thread::Builder::new()
            .name(format!("session-{id}"))
            .spawn(move || run(id, worker_site, initial_dir, trust, notify, rx))
            .expect("spawn session thread");
        Self { id, site, tx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn run(
    id: u64,
    site: Site,
    initial_dir: Option<String>,
    trust: Arc<TrustStore>,
    notify: Notifier,
    rx: Receiver<Cmd>,
) {
    let log = notify.logger(None);
    notify.send(AppEvent::Busy { session: id, busy: true });
    let mut fs = match remote::connect(&site, trust.clone(), log.clone()) {
        Ok(fs) => fs,
        Err(e) => {
            let msg = format!("{e:#}");
            log(LogLevel::Error, format!("Не удалось подключиться: {msg}"));
            notify.send(AppEvent::Disconnected { session: id, error: Some(msg) });
            return;
        }
    };
    let home = fs.home().unwrap_or_else(|_| "/".to_string());
    let start = initial_dir
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| home.clone());
    notify.send(AppEvent::Connected { session: id, home: home.clone() });
    if start == home || !try_list(id, &mut *fs, &start, &notify) {
        // Like FileZilla: a start folder that does not exist is not fatal.
        // Try it relative to home ("/www" → "/home/user/www"), then home.
        let relative = remote::join(&home, start.trim_start_matches('/'));
        if start == home || relative == start || !try_list(id, &mut *fs, &relative, &notify) {
            if start != home {
                notify.send(AppEvent::Log(LogLevel::Info, format!("Папка {start} не найдена, открываю домашнюю")));
            }
            list(id, &mut *fs, &home, &notify);
        }
    }
    notify.send(AppEvent::Busy { session: id, busy: false });

    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(25)) {
            Ok(cmd) => cmd,
            Err(RecvTimeoutError::Timeout) => {
                if fs.keepalive().is_err() {
                    log(LogLevel::Info, "Соединение потеряно, переподключение…".into());
                    match remote::connect(&site, trust.clone(), log.clone()) {
                        Ok(new) => fs = new,
                        Err(e) => {
                            notify.send(AppEvent::Disconnected {
                                session: id,
                                error: Some(format!("{e:#}")),
                            });
                            return;
                        }
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                fs.close();
                log(LogLevel::Info, "Отключено от сервера".into());
                return;
            }
        };

        notify.send(AppEvent::Busy { session: id, busy: true });
        let mut result = execute(id, &mut *fs, &cmd, &notify);
        if result.is_err() && fs.keepalive().is_err() {
            // The connection died (server timeout etc.) — reconnect and retry once.
            log(LogLevel::Info, "Соединение потеряно, переподключение…".into());
            match remote::connect(&site, trust.clone(), log.clone()) {
                Ok(new) => {
                    fs = new;
                    result = execute(id, &mut *fs, &cmd, &notify);
                }
                Err(e) => {
                    notify.send(AppEvent::Disconnected {
                        session: id,
                        error: Some(format!("{e:#}")),
                    });
                    return;
                }
            }
        }
        if let Err(e) = result {
            let message = format!("{e:#}");
            log(LogLevel::Error, message.clone());
            notify.send(AppEvent::OpFailed { session: id, message });
        }
        notify.send(AppEvent::Busy { session: id, busy: false });
    }
}

/// Lists `path` without reporting a failure.
fn try_list(id: u64, fs: &mut dyn RemoteFs, path: &str, notify: &Notifier) -> bool {
    match fs.list(path) {
        Ok(entries) => {
            notify.send(AppEvent::Listing { session: id, path: path.to_string(), entries });
            true
        }
        Err(_) => false,
    }
}

fn list(id: u64, fs: &mut dyn RemoteFs, path: &str, notify: &Notifier) -> bool {
    match fs.list(path) {
        Ok(entries) => {
            notify.send(AppEvent::Log(
                LogLevel::Success,
                format!("Содержимое {path}: {} объектов", entries.len()),
            ));
            notify.send(AppEvent::Listing { session: id, path: path.to_string(), entries });
            true
        }
        Err(e) => {
            let message = format!("Не удалось открыть {path}: {e:#}");
            notify.send(AppEvent::Log(LogLevel::Error, message.clone()));
            notify.send(AppEvent::OpFailed { session: id, message });
            false
        }
    }
}

fn execute(id: u64, fs: &mut dyn RemoteFs, cmd: &Cmd, notify: &Notifier) -> Result<()> {
    match cmd {
        Cmd::List(path) => {
            fs.list(path).map(|entries| {
                notify.send(AppEvent::Listing { session: id, path: path.clone(), entries });
            })?;
        }
        Cmd::Mkdir(path) => {
            fs.mkdir(path)?;
            notify.send(AppEvent::RemoteChanged(remote::parent(path)));
        }
        Cmd::Delete(items) => {
            let mut dirs = std::collections::BTreeSet::new();
            for (path, is_dir) in items {
                delete_recursive(fs, path, *is_dir, notify)?;
                dirs.insert(remote::parent(path));
            }
            for d in dirs {
                notify.send(AppEvent::RemoteChanged(d));
            }
        }
        Cmd::Rename { from, to } => {
            fs.rename(from, to)?;
            notify.send(AppEvent::RemoteChanged(remote::parent(from)));
            if remote::parent(from) != remote::parent(to) {
                notify.send(AppEvent::RemoteChanged(remote::parent(to)));
            }
        }
    }
    Ok(())
}

fn delete_recursive(fs: &mut dyn RemoteFs, path: &str, is_dir: bool, notify: &Notifier) -> Result<()> {
    if is_dir {
        for e in fs.list(path)? {
            let child = remote::join(path, &e.name);
            // never follow symlinked directories when deleting
            delete_recursive(fs, &child, e.is_dir && !e.is_link, notify)?;
        }
        fs.rmdir(path)?;
    } else {
        fs.remove(path)?;
    }
    notify.send(AppEvent::Log(LogLevel::Success, format!("Удалено: {path}")));
    Ok(())
}
