//! Transfer queue with a pool of worker threads. Each worker keeps its own
//! connection so transfers never block browsing.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Ranges of a segmented transfer, one per connection, for the progress bar.
    pub parts: Vec<Part>,
}

#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
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
    /// Connections used for one large file (1 = no segmentation).
    pub segments: usize,
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
    pub fn new(
        trust: Arc<TrustStore>,
        notify: Notifier,
        max_workers: usize,
        segments: usize,
        exists: ExistsAction,
    ) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Queue {
                items: Vec::new(),
                next_id: 1,
                workers: 0,
                max_workers: max_workers.max(1),
                segments: segments.clamp(1, MAX_SEGMENTS),
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
                    parts: Vec::new(),
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
                it.parts.clear();
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
                    parts: Vec::new(),
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
                if let Some((size, n)) = self.plan_segments(item, fs, &item.remote, offset) {
                    if offset == 0 {
                        File::create(&local)
                            .with_context(|| format!("не удалось создать {}", local.display()))?;
                    }
                    self.update(id, |it| {
                        it.size = size;
                        it.transferred = offset;
                        it.resumed_from = offset;
                    });
                    let job = Job { dir: Direction::Download, local: &local, remote: &item.remote };
                    self.segmented(item, fs, &job, offset, size, n)?;
                    self.notify.send(AppEvent::Log(
                        LogLevel::Success,
                        format!("Скачан {} → {} ({})", item.remote, local.display(), threads(n)),
                    ));
                    return Ok(Status::Done);
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
                if fs.can_upload_ranges()
                    && let Some(n) = self.segment_count(local_meta.len().saturating_sub(offset))
                {
                    let size = local_meta.len();
                    self.update(id, |it| {
                        it.size = size;
                        it.transferred = offset;
                        it.resumed_from = offset;
                    });
                    // Create (or truncate) the target before the ranges are written.
                    fs.set_size(&target, offset)?;
                    let job = Job { dir: Direction::Upload, local: &item.local, remote: &target };
                    self.segmented(item, fs, &job, offset, size, n)?;
                    self.notify.send(AppEvent::Log(
                        LogLevel::Success,
                        format!("Загружен {} → {} ({})", item.local.display(), target, threads(n)),
                    ));
                    return Ok(Status::Done);
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

    /// How many connections to use for `remaining` bytes, `None` for one.
    fn segment_count(&self, remaining: u64) -> Option<usize> {
        let wanted = self.queue.lock().unwrap().segments;
        let n = wanted.min((remaining / MIN_SEGMENT) as usize);
        (n > 1).then_some(n)
    }

    /// Decide on a segmented download; returns the exact remote size and the
    /// number of segments. The listing size may be stale, so ask the server.
    fn plan_segments(&self, item: &Item, fs: &mut dyn RemoteFs, path: &str, offset: u64) -> Option<(u64, usize)> {
        if item.size.saturating_sub(offset) < 2 * MIN_SEGMENT || self.queue.lock().unwrap().segments < 2 {
            return None;
        }
        let size = fs.stat(path).ok().flatten().filter(|e| !e.is_dir)?.size;
        Some((size, self.segment_count(size.saturating_sub(offset))?))
    }

    /// Transfer `from..size` of one file over `n` connections: the worker's own
    /// connection takes the first range, extra connections take the rest.
    /// Ranges whose connection could not be opened or failed are finished
    /// afterwards on the worker's connection. On failure the target is cut
    /// back to the contiguous prefix so that "resume" stays correct.
    fn segmented(&self, item: &Item, fs: &mut dyn RemoteFs, job: &Job, from: u64, size: u64, n: usize) -> Result<()> {
        let chunk = (size - from) / n as u64;
        let segs: Vec<Segment> = (0..n)
            .map(|i| {
                let start = from + chunk * i as u64;
                // A download's last range reads to EOF in case the file grew.
                let end = match (i + 1 == n, job.dir) {
                    (false, _) => start + chunk,
                    (true, Direction::Download) => u64::MAX,
                    (true, Direction::Upload) => size,
                };
                Segment { idx: i, start, end, pos: AtomicU64::new(start), done: AtomicBool::new(false) }
            })
            .collect();
        self.update(item.id, |it| {
            it.parts = segs.iter().map(|s| Part { start: s.start, end: s.end.min(size), pos: s.start }).collect();
        });
        let abort = AtomicBool::new(false);
        self.notify.send(AppEvent::Log(
            LogLevel::Info,
            format!("{}: {} по ~{} МБ", remote::file_name(job.remote), threads(n), chunk / (1024 * 1024)),
        ));

        let first = std::thread::scope(|s| {
            for (i, seg) in segs.iter().enumerate().skip(1) {
                let me = self.clone();
                let abort = &abort;
                let spawned = std::thread::Builder::new()
                    .name(format!("segment-{i}"))
                    .spawn_scoped(s, move || {
                        if abort.load(Ordering::Relaxed) || item.cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let log = me.notify.logger(Some(format!("поток {}", i + 1)));
                        match remote::connect(&item.site, me.trust.clone(), log) {
                            Ok(mut c) => {
                                if let Err(e) = me.run_segment(item, &mut *c, job, seg, abort) {
                                    me.segment_warning(i, &e);
                                }
                                c.close();
                            }
                            Err(e) => me.segment_warning(i, &e),
                        }
                    });
                // No thread for this range: the worker's connection finishes it below.
                if let Err(e) = spawned {
                    self.segment_warning(i, &e.into());
                }
            }
            let r = self.run_segment(item, fs, job, &segs[0], &abort);
            if r.is_err() {
                abort.store(true, Ordering::Relaxed);
            }
            r
        });

        let mut result = first;
        if result.is_ok() {
            for seg in segs.iter().skip(1).filter(|s| !s.done.load(Ordering::Relaxed)) {
                if let Err(e) = self.run_segment(item, fs, job, seg, &abort) {
                    result = Err(e);
                    break;
                }
            }
        }
        if result.is_err() {
            let prefix = contiguous_prefix(&segs);
            let cut = match job.dir {
                Direction::Download => OpenOptions::new()
                    .write(true)
                    .open(job.local)
                    .and_then(|f| f.set_len(prefix))
                    .map_err(anyhow::Error::from),
                Direction::Upload => fs.set_size(job.remote, prefix),
            };
            if let Err(e) = cut {
                log::warn!("не удалось обрезать недокачанный файл: {e:#}");
            }
        }
        result
    }

    fn advance(&self, id: u64, part: usize, n: u64) {
        self.update(id, |it| {
            it.transferred += n;
            if let Some(p) = it.parts.get_mut(part) {
                p.pos += n;
            }
        });
    }

    fn segment_warning(&self, i: usize, e: &anyhow::Error) {
        self.notify.send(AppEvent::Log(
            LogLevel::Info,
            format!("Поток {} не справился ({e:#}), его часть докачает основное соединение", i + 1),
        ));
    }

    /// Move the rest of one range (`pos..end`) over `fs`.
    fn run_segment(&self, item: &Item, fs: &mut dyn RemoteFs, job: &Job, seg: &Segment, abort: &AtomicBool) -> Result<()> {
        let pos = seg.pos.load(Ordering::Relaxed);
        let stop = || item.cancel.load(Ordering::Relaxed) || abort.load(Ordering::Relaxed);
        if stop() {
            anyhow::bail!("Передача отменена");
        }
        let id = item.id;
        match job.dir {
            Direction::Download => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(job.local)
                    .with_context(|| format!("не удалось открыть {}", job.local.display()))?;
                file.seek(SeekFrom::Start(pos))?;
                let mut w = RangeWriter { file, seg, full: false, report: |n| self.advance(id, seg.idx, n) };
                let r = fs.download(job.remote, pos, &mut w, &mut |_| !stop());
                // Reaching the end of the range aborts the stream on purpose.
                if r.is_err() && !w.full {
                    return r;
                }
                if !w.full && seg.end != u64::MAX && seg.pos.load(Ordering::Relaxed) < seg.end {
                    anyhow::bail!("файл на сервере оказался короче ожидаемого");
                }
            }
            Direction::Upload => {
                let mut file = File::open(job.local)
                    .with_context(|| format!("не удалось открыть {}", job.local.display()))?;
                file.seek(SeekFrom::Start(pos))?;
                let mut input = file.take(seg.end - pos);
                fs.upload_at(job.remote, pos, &mut input, &mut |n| {
                    seg.pos.fetch_add(n, Ordering::Relaxed);
                    self.advance(id, seg.idx, n);
                    !stop()
                })?;
                if seg.pos.load(Ordering::Relaxed) < seg.end {
                    anyhow::bail!("локальный файл короче ожидаемого");
                }
            }
        }
        seg.done.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// Smallest range worth its own connection.
const MIN_SEGMENT: u64 = 8 * 1024 * 1024;
pub const MAX_SEGMENTS: usize = 8;

/// One file transfer split into ranges.
struct Job<'a> {
    dir: Direction,
    local: &'a Path,
    remote: &'a str,
}

/// A byte range of a segmented transfer; `pos` is how far it got.
struct Segment {
    idx: usize,
    start: u64,
    end: u64,
    pos: AtomicU64,
    done: AtomicBool,
}

pub fn threads(n: usize) -> String {
    match n % 10 {
        1 if n % 100 != 11 => format!("{n} поток"),
        2..=4 if !(12..=14).contains(&(n % 100)) => format!("{n} потока"),
        _ => format!("{n} потоков"),
    }
}

/// End of the data that is complete from the first range onwards.
fn contiguous_prefix(segs: &[Segment]) -> u64 {
    let mut prefix = segs.first().map_or(0, |s| s.start);
    for s in segs {
        prefix = s.pos.load(Ordering::Relaxed);
        if !s.done.load(Ordering::Relaxed) {
            break;
        }
    }
    prefix
}

/// Writes one range of a download into the local file and refuses anything
/// past the range end, which makes the protocol layer drop the stream.
struct RangeWriter<'a, F: FnMut(u64)> {
    file: File,
    seg: &'a Segment,
    full: bool,
    report: F,
}

impl<F: FnMut(u64)> Write for RangeWriter<'_, F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let pos = self.seg.pos.load(Ordering::Relaxed);
        let left = self.seg.end.saturating_sub(pos);
        if left == 0 {
            self.full = true;
            return Err(std::io::Error::other("конец диапазона"));
        }
        let n = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        self.file.write_all(&buf[..n])?;
        self.seg.pos.fetch_add(n as u64, Ordering::Relaxed);
        (self.report)(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
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

/// Segmented transfers against real servers, e.g.
/// `SKIFF_IT="sftp://tester:secret123@127.0.0.1:2222/tmp ftp://…" cargo test segmented -- --ignored`
#[cfg(test)]
mod live {
    use super::*;
    use crate::trust::TrustDecision;
    use std::time::Duration;

    struct Env {
        tm: TransferManager,
        rx: crossbeam_channel::Receiver<AppEvent>,
        site: Site,
    }

    impl Env {
        fn run(&self, t: NewTransfer, cancel_at: Option<u64>) -> (Status, Vec<String>) {
            self.tm.enqueue(&self.site, vec![t]);
            let mut logs = Vec::new();
            loop {
                while let Ok(ev) = self.rx.try_recv() {
                    if let AppEvent::Log(_, m) = ev {
                        logs.push(m);
                    }
                }
                let q = self.tm.queue.lock().unwrap();
                let it = q.items.last().unwrap().clone();
                drop(q);
                if let Some(at) = cancel_at
                    && it.transferred >= at
                    && !it.status.is_finished()
                {
                    self.tm.cancel(it.id);
                }
                if it.status.is_finished() && q_workers_idle(&self.tm) {
                    return (it.status, logs);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn q_workers_idle(tm: &TransferManager) -> bool {
        tm.queue.lock().unwrap().workers == 0
    }

    fn env(url: &str, exists: ExistsAction) -> (Env, String) {
        let mut site = Site::default();
        let dir = site.apply_host_url(url).unwrap_or_else(|| "/".into());
        let trust = TrustStore::load(Arc::new(|_| TrustDecision::Once));
        let (tx, rx) = crossbeam_channel::unbounded();
        let tm = TransferManager::new(trust, Notifier::new(tx, egui::Context::default()), 1, 4, exists);
        (Env { tm, rx, site }, dir)
    }

    #[test]
    #[ignore]
    fn segmented_roundtrip() {
        let urls = std::env::var("SKIFF_IT").expect("SKIFF_IT");
        let tmp = std::env::temp_dir().join(format!("skiff-seg-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let data: Vec<u8> = (0..50_000_003u64).map(|i| (i * 7 + i / 4093) as u8).collect();
        let src = tmp.join("src.bin");
        std::fs::write(&src, &data).unwrap();
        for url in urls.split_whitespace() {
            eprintln!("==== {url}");
            let (e, dir) = env(url, ExistsAction::Overwrite);
            let remote = remote::join(&dir, &format!("skiff-seg-{}.bin", std::process::id()));
            let up = NewTransfer {
                direction: Direction::Upload,
                local: src.clone(),
                remote: remote.clone(),
                is_dir: false,
                size: data.len() as u64,
            };
            let (st, logs) = e.run(up, None);
            assert_eq!(st, Status::Done, "{logs:#?}");
            eprintln!("upload: {:?}", logs.iter().filter(|l| l.contains("Загружен")).collect::<Vec<_>>());

            let dst = tmp.join("dst.bin");
            let down = || NewTransfer {
                direction: Direction::Download,
                local: dst.clone(),
                remote: remote.clone(),
                is_dir: false,
                size: data.len() as u64,
            };
            let (st, logs) = e.run(down(), None);
            assert_eq!(st, Status::Done, "{logs:#?}");
            assert!(logs.iter().any(|l| l.contains("Скачан") && l.contains("4 потока")), "{logs:#?}");
            assert!(std::fs::read(&dst).unwrap() == data, "download differs");

            // Cancel halfway: what stays on disk must be a correct prefix.
            let (st, _) = e.run(down(), Some(20_000_000));
            assert_eq!(st, Status::Cancelled);
            let part = std::fs::read(&dst).unwrap();
            eprintln!("cancelled at prefix {}", part.len());
            assert!(part.len() < data.len());
            assert!(part[..] == data[..part.len()], "partial file is not a prefix");

            // …and resume finishes it.
            e.tm.queue.lock().unwrap().exists_action = ExistsAction::Resume;
            let (st, logs) = e.run(down(), None);
            assert_eq!(st, Status::Done, "{logs:#?}");
            assert!(std::fs::read(&dst).unwrap() == data, "resumed download differs");

            // Segmented upload cancel + resume (SFTP only actually segments).
            e.tm.queue.lock().unwrap().exists_action = ExistsAction::Overwrite;
            let up = || NewTransfer {
                direction: Direction::Upload,
                local: src.clone(),
                remote: remote.clone(),
                is_dir: false,
                size: data.len() as u64,
            };
            let (st, _) = e.run(up(), Some(20_000_000));
            assert_eq!(st, Status::Cancelled);
            e.tm.queue.lock().unwrap().exists_action = ExistsAction::Resume;
            let (st, logs) = e.run(up(), None);
            assert_eq!(st, Status::Done, "{logs:#?}");
            e.tm.queue.lock().unwrap().exists_action = ExistsAction::Overwrite;
            let (st, _) = e.run(down(), None);
            assert_eq!(st, Status::Done);
            assert!(std::fs::read(&dst).unwrap() == data, "resumed upload differs");

            let trust = TrustStore::load(Arc::new(|_| TrustDecision::Once));
            let mut fs = remote::connect(&e.site, trust, Arc::new(|_, _| {})).unwrap();
            fs.remove(&remote).unwrap();
            fs.close();
            std::fs::remove_file(&dst).unwrap();
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
