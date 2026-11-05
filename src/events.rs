//! Messages flowing from worker threads back to the UI thread.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::Sender;

use crate::remote::{LogLevel, Logger, RemoteEntry};
use crate::trust::{TrustDecision, TrustRequest};

#[derive(Clone, Debug)]
pub struct ExistsQuestion {
    pub upload: bool,
    pub target: String,
    pub source_size: u64,
    pub source_modified: Option<chrono::DateTime<chrono::Local>>,
    pub target_size: u64,
    pub target_modified: Option<chrono::DateTime<chrono::Local>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExistsAnswer {
    Overwrite,
    OverwriteIfNewer,
    Resume,
    Skip,
    Rename,
}

pub enum AppEvent {
    Log(LogLevel, String),
    Connected { session: u64, home: String },
    Listing { session: u64, path: String, entries: Vec<RemoteEntry> },
    OpFailed { session: u64, message: String },
    Disconnected { session: u64, error: Option<String> },
    Busy { session: u64, busy: bool },
    RemoteChanged(String),
    LocalChanged(PathBuf),
    Trust(TrustRequest, Sender<TrustDecision>),
    FileExists(ExistsQuestion, Sender<(ExistsAnswer, bool)>),
}

#[derive(Clone)]
pub struct Notifier {
    tx: Sender<AppEvent>,
    ctx: egui::Context,
}

impl Notifier {
    pub fn new(tx: Sender<AppEvent>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    pub fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }

    pub fn logger(&self, prefix: Option<String>) -> Logger {
        let me = self.clone();
        Arc::new(move |level, msg| {
            let msg = match &prefix {
                Some(p) => format!("[{p}] {msg}"),
                None => msg,
            };
            me.send(AppEvent::Log(level, msg));
        })
    }

    /// Ask the UI something and block the calling worker until it answers.
    pub fn ask<T: Send + 'static>(&self, make: impl FnOnce(Sender<T>) -> AppEvent) -> Option<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(make(tx));
        rx.recv().ok()
    }
}
