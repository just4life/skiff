//! Transfer queue with a pool of worker threads. Each worker keeps its own
//! connection so transfers never block browsing.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::config::{ExistsAction, Site};
use crate::events::{AppEvent, ExistsAnswer, ExistsQuestion, Notifier};
use crate::remote::{self, LogLevel, RemoteFs};
use crate::trust::TrustStore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Queued,
    Active,
    Done,
    Skipped,
    Failed(String),
    Cancelled,
}

impl Status {
    pub fn is_finished(&self) -> bool {
        matches!(self, Status::Done | Status::Skipped | Status::Failed(_) | Status::Cancelled)
    }
}

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: String,
    pub is_dir: bool,
    pub size: u64,
    pub transferred: u64,
    pub status: Status,
    pub site: Arc<Site>,
    pub cancel: Arc<AtomicBool>,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
    /// Bytes skipped thanks to resume; excluded from speed calculation.
    pub resumed_from: u64,
}

impl Item {
    pub fn speed(&self) -> f64 {
        let Some(start) = self.started else { return 0.0 };
        let end = self.finished.unwrap_or_else(Instant::now);
        let secs = end.duration_since(start).as_secs_f64().max(0.001);
        (self.transferred.saturating_sub(self.resumed_from)) as f64 / secs
    }

    pub fn fraction(&self) -> f32 {
        if self.size == 0 {
            if self.status == Status::Done { 1.0 } else { 0.0 }
        } else {
            (self.transferred as f64 / self.size as f64).min(1.0) as f32
        }
    }
}

pub struct Queue {
    pub items: Vec<Item>,
    next_id: u64,
    workers: usize,
    pub max_workers: usize,
    pub exists_action: ExistsAction,
    pub paused: bool,
}

#[derive(Clone)]
pub struct TransferManager {
    pub queue: Arc<Mutex<Queue>>,
    trust: Arc<TrustStore>,
    notify: Notifier,
    ask_lock: Arc<Mutex<()>>,
}

pub struct NewTransfer {
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: String,
    pub is_dir: bool,
    pub size: u64,
}

impl TransferManager {
    pub fn new(trust: Arc<TrustStore>, notify: Notifier, max_workers: usize, exists: ExistsAction) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Queue {
                items: Vec::new(),
                next_id: 1,
                workers: 0,
                max_workers: max_workers.max(1),
                exists_action: exists,
                paused: false,
            })),
            trust,
            notify,
            ask_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn enqueue(&self, site: &Site, items: Vec<NewTransfer>) {
        let site = Arc::new(site.clone());
        {
            let mut q = self.queue.lock().unwrap();
            for t in items {
                let id = q.next_id;
                q.next_id += 1;
                q.items.push(Item {
                    id,
                    direction: t.direction,
                    local: t.local,
                    remote: t.remote,
                    is_dir: t.is_dir,
                    size: t.size,
                    transferred: 0,
                    status: Status::Queued,
                    site: site.clone(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    started: None,
                    finished: None,
                    resumed_from: 0,
                });
            }
        }
        self.ensure_workers();
    }

    pub fn ensure_workers(&self) {
        let mut q = self.queue.lock().unwrap();
        if q.paused {
            return;
        }
        // every active item occupies a worker; queued ones may need new workers
        let needed = q
            .items
            .iter()
            .filter(|i| matches!(i.status, Status::Queued | Status::Active))
            .count();
        while q.workers < q.max_workers && q.workers < needed {
            q.workers += 1;
            let me = self.clone();
            let n = q.workers;
            std::thread::Builder::new()
                .name(format!("transfer-{n}"))
                .spawn(move || me.worker())
                .expect("spawn transfer worker");
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.queue.lock().unwrap().paused = paused;
        if !paused {
            self.ensure_workers();
        }
    }

    pub fn cancel(&self, id: u64) {
        let mut q = self.queue.lock().unwrap();
        if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
            it.cancel.store(true, Ordering::Relaxed);
            if it.status == Status::Queued {
                it.status = Status::Cancelled;
            }
        }
    }

    pub fn cancel_all(&self) {
        let mut q = self.queue.lock().unwrap();
        for it in q.items.iter_mut().filter(|i| !i.status.is_finished()) {
            it.cancel.store(true, Ordering::Relaxed);
            if it.status == Status::Queued {
                it.status = Status::Cancelled;
            }
        }
    }

    pub fn retry(&self, id: u64) {
        {
            let mut q = self.queue.lock().unwrap();
            if let Some(it) = q.items.iter_mut().find(|i| i.id == id)
                && matches!(it.status, Status::Failed(_) | Status::Cancelled)
            {
                it.status = Status::Queued;
                it.transferred = 0;
                it.cancel = Arc::new(AtomicBool::new(false));
                it.started = None;
                it.finished = None;
            }
        }
        self.ensure_workers();
    }

    pub fn clear_finished(&self, pred: impl Fn(&Status) -> bool) {
        self.queue.lock().unwrap().items.retain(|i| !pred(&i.status));
    }

    fn take_next(&self) -> Option<Item> {
        let mut q = self.queue.lock().unwrap();
        if !q.paused
            && let Some(it) = q.items.iter_mut().find(|i| i.status == Status::Queued)
        {
            it.status = Status::Active;
            it.started = Some(Instant::now());
            return Some(it.clone());
        }
        q.workers -= 1;
        None
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Item)) {
        let mut q = self.queue.lock().unwrap();
        if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
            f(it);
        }
    }

    fn worker(self) {
        let mut conn: Option<(Arc<Site>, Box<dyn RemoteFs>)> = None;
        while let Some(item) = self.take_next() {
            // (Re)connect when the item belongs to another site.
            let reuse = matches!(&conn, Some((s, _)) if **s == *item.site);
            if !reuse {
                if let Some((_, mut c)) = conn.take() {
                    c.close();
                }
                let log = self.notify.logger(Some("передача".into()));
                match remote::connect(&item.site, self.trust.clone(), log) {
                    Ok(c) => conn = Some((item.site.clone(), c)),
                    Err(e) => {
                        self.fail(&item, format!("{e:#}"));
                        continue;
                    }
                }
            }
            let fs = &mut conn.as_mut().unwrap().1;
            let result = if item.is_dir {
                self.expand(&item, &mut **fs)
            } else {
                self.transfer(&item, &mut **fs)
            };
            match result {
                Ok(status) => {
                    self.update(item.id, |it| {
                        it.status = status;
                        it.finished = Some(Instant::now());
                        if it.status == Status::Done {
                            it.transferred = it.transferred.max(it.size);
                        }
                    });
                    if !item.is_dir {
                        match item.direction {
                            Direction::Upload => self
                                .notify
                                .send(AppEvent::RemoteChanged(remote::parent(&item.remote))),
                            Direction::Download => {
                                if let Some(p) = item.local.parent() {
                                    self.notify.send(AppEvent::LocalChanged(p.to_path_buf()))
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let cancelled = item.cancel.load(Ordering::Relaxed);
                    if cancelled {
                        self.update(item.id, |it| {
                            it.status = Status::Cancelled;
                            it.finished = Some(Instant::now());
                        });
                    } else {
                        self.fail(&item, format!("{e:#}"));
                    }
                    // The connection state is unknown after a failure; start fresh.
                    if let Some((_, mut c)) = conn.take() {
                        if cancelled || c.keepalive().is_ok() {
                            conn = Some((item.site.clone(), c));
                        } else {
                            c.close();
                        }
                    }
                }
            }
        }
        if let Some((_, mut c)) = conn.take() {
            c.close();
        }
    }

    fn fail(&self, item: &Item, msg: String) {
        let what = match item.direction {
            Direction::Upload => item.local.display().to_string(),
            Direction::Download => item.remote.clone(),
        };
        self.notify.send(AppEvent::Log(LogLevel::Error, format!("Ошибка передачи {what}: {msg}")));
        self.update(item.id, |it| {
            it.status = Status::Failed(msg);
            it.finished = Some(Instant::now());
        });
    }

    /// Directory items are expanded into child items right after themselves.
    fn expand(&self, item: &Item, fs: &mut dyn RemoteFs) -> Result<Status> {
        let mut children = Vec::new();
        match item.direction {
            Direction::Upload => {
                if fs.stat(&item.remote).ok().flatten().is_none() {
                    let _ = fs.mkdir(&item.remote);
                }
                let mut entries: Vec<_> = std::fs::read_dir(&item.local)
                    .with_context(|| format!("не удалось прочитать {}", item.local.display()))?
                    .flatten()
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for e in entries {
                    let Ok(meta) = std::fs::metadata(e.path()) else { continue };
                    let name = e.file_name().to_string_lossy().to_string();
                    children.push(NewTransfer {
                        direction: Direction::Upload,
                        local: e.path(),
                        remote: remote::join(&item.remote, &name),
                        is_dir: meta.is_dir(),
                        size: if meta.is_dir() { 0 } else { meta.len() },
                    });
                }
                self.notify.send(AppEvent::RemoteChanged(remote::parent(&item.remote)));
            }
            Direction::Download => {
                std::fs::create_dir_all(&item.local)
                    .with_context(|| format!("не удалось создать {}", item.local.display()))?;
                let mut entries = fs.list(&item.remote)?;
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                for e in entries {
                    children.push(NewTransfer {
                        direction: Direction::Download,
                        local: item.local.join(sanitize_local_name(&e.name)),
                        remote: remote::join(&item.remote, &e.name),
                        is_dir: e.is_dir,
                        size: e.size,
                    });
                }
                if let Some(p) = item.local.parent() {
                    self.notify.send(AppEvent::LocalChanged(p.to_path_buf()));
                }
            }
        }
        // Insert children right after the directory item to keep order natural.
        {
            let mut q = self.queue.lock().unwrap();
            let pos = q.items.iter().position(|i| i.id == item.id).map(|p| p + 1).unwrap_or(q.items.len());
            let mut new_items = Vec::with_capacity(children.len());
            for t in children {
                let id = q.next_id;
                q.next_id += 1;
                new_items.push(Item {
                    id,
                    direction: t.direction,
                    local: t.local,
                    remote: t.remote,
                    is_dir: t.is_dir,
                    size: t.size,
                    transferred: 0,
                    status: Status::Queued,
                    site: item.site.clone(),
                    cancel: item.cancel.clone(),
                    started: None,
                    finished: None,
                    resumed_from: 0,
                });
            }
            q.items.splice(pos..pos, new_items);
        }
        self.ensure_workers();
        Ok(Status::Done)
    }

    fn decide_exists(&self, q: ExistsQuestion) -> (ExistsAnswer, bool) {
        let configured = self.queue.lock().unwrap().exists_action;
        let direct = match configured {
            ExistsAction::Ask => None,
            ExistsAction::Overwrite => Some(ExistsAnswer::Overwrite),
            ExistsAction::OverwriteIfNewer => Some(ExistsAnswer::OverwriteIfNewer),
            ExistsAction::Resume => Some(ExistsAnswer::Resume),
            ExistsAction::Skip => Some(ExistsAnswer::Skip),
        };
        if let Some(a) = direct {
            return (a, false);
        }
        let _g = self.ask_lock.lock().unwrap();
        // Another worker may have set "apply to all" while we waited.
        let configured = self.queue.lock().unwrap().exists_action;
        if configured != ExistsAction::Ask {
            drop(_g);
            return self.decide_exists(q);
        }
        let (answer, all) = self
            .notify
            .ask(|tx| AppEvent::FileExists(q, tx))
            .unwrap_or((ExistsAnswer::Skip, false));
        if all {
            let action = match answer {
                ExistsAnswer::Overwrite => Some(ExistsAction::Overwrite),
                ExistsAnswer::OverwriteIfNewer => Some(ExistsAction::OverwriteIfNewer),
                ExistsAnswer::Resume => Some(ExistsAction::Resume),
                ExistsAnswer::Skip => Some(ExistsAction::Skip),
                ExistsAnswer::Rename => None,
            };
            if let Some(a) = action {
                self.queue.lock().unwrap().exists_action = a;
            }
        }
        (answer, all)
    }

    fn transfer(&self, item: &Item, fs: &mut dyn RemoteFs) -> Result<Status> {
        let id = item.id;
        let cancel = item.cancel.clone();
        let mut progress = |n: u64| {
            self.update(id, |it| it.transferred += n);
            !cancel.load(Ordering::Relaxed)
        };
        match item.direction {
            Direction::Download => {
                let mut local = item.local.clone();
                let mut offset = 0;
                if let Ok(meta) = std::fs::metadata(&local) {
                    let remote_info = fs.stat(&item.remote).ok().flatten();
                    let q = ExistsQuestion {
                        upload: false,
                        target: local.display().to_string(),
                        source_size: item.size,
                        source_modified: remote_info.as_ref().and_then(|r| r.modified),
                        target_size: meta.len(),
                        target_modified: meta.modified().ok().map(Into::into),
                    };
                    match self.decide_exists(q.clone()).0 {
                        ExistsAnswer::Skip => return Ok(Status::Skipped),
                        ExistsAnswer::Overwrite => {}
                        ExistsAnswer::OverwriteIfNewer => {
                            if !(q.source_modified > q.target_modified) {
                                return Ok(Status::Skipped);
                            }
                        }
                        ExistsAnswer::Resume => {
                            if meta.len() >= item.size && item.size > 0 {
                                return Ok(Status::Skipped);
                            }
                            offset = meta.len();
                        }
                        ExistsAnswer::Rename => local = unique_local(&local),
                    }
                }
                if let Some(p) = local.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut file = if offset > 0 {
                    OpenOptions::new().append(true).open(&local)?
                } else {
                    File::create(&local)
                        .with_context(|| format!("не удалось создать {}", local.display()))?
                };
                self.update(id, |it| {
                    it.transferred = offset;
                    it.resumed_from = offset;
                });
                let mut w = std::io::BufWriter::with_capacity(256 * 1024, &mut file);
                fs.download(&item.remote, offset, &mut w, &mut progress)?;
                use std::io::Write;
                w.flush()?;
                self.notify.send(AppEvent::Log(
                    LogLevel::Success,
                    format!("Скачан {} → {}", item.remote, local.display()),
                ));
            }
            Direction::Upload => {
                let mut target = item.remote.clone();
                let mut offset = 0;
                let local_meta = std::fs::metadata(&item.local)?;
                if let Some(existing) = fs.stat(&target).ok().flatten() {
                    let q = ExistsQuestion {
                        upload: true,
                        target: target.clone(),
                        source_size: local_meta.len(),
                        source_modified: local_meta.modified().ok().map(Into::into),
                        target_size: existing.size,
                        target_modified: existing.modified,
                    };
                    match self.decide_exists(q.clone()).0 {
                        ExistsAnswer::Skip => return Ok(Status::Skipped),
                        ExistsAnswer::Overwrite => {}
                        ExistsAnswer::OverwriteIfNewer => {
                            if !(q.source_modified > q.target_modified) {
                                return Ok(Status::Skipped);
                            }
                        }
                        ExistsAnswer::Resume => {
                            if existing.size >= local_meta.len() && local_meta.len() > 0 {
                                return Ok(Status::Skipped);
                            }
                            offset = existing.size;
                        }
                        ExistsAnswer::Rename => target = unique_remote(fs, &target),
                    }
                }
                let mut file = File::open(&item.local)
                    .with_context(|| format!("не удалось открыть {}", item.local.display()))?;
                if offset > 0 {
                    file.seek(SeekFrom::Start(offset))?;
                }
                self.update(id, |it| {
                    it.size = local_meta.len();
                    it.transferred = offset;
                    it.resumed_from = offset;
                });
                let mut r = std::io::BufReader::with_capacity(256 * 1024, file);
                fs.upload(&target, offset, &mut r, &mut progress)?;
                self.notify.send(AppEvent::Log(
                    LogLevel::Success,
                    format!("Загружен {} → {}", item.local.display(), target),
                ));
            }
        }
        Ok(Status::Done)
    }
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn unique_local(path: &std::path::Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (stem, ext) = split_ext(&name);
    for n in 1.. {
        let candidate = path.with_file_name(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn unique_remote(fs: &mut dyn RemoteFs, path: &str) -> String {
    let dir = remote::parent(path);
    let name = remote::file_name(path).to_string();
    let (stem, ext) = split_ext(&name);
    for n in 1..1000 {
        let candidate = remote::join(&dir, &format!("{stem} ({n}){ext}"));
        if !matches!(fs.stat(&candidate), Ok(Some(_))) {
            return candidate;
        }
    }
    path.to_string()
}

/// Remote names may contain characters that are invalid locally (Windows).
pub fn sanitize_local_name(name: &str) -> String {
    if cfg!(windows) {
        name.chars()
            .map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c })
            .collect()
    } else {
        name.replace('/', "_")
    }
}
