//! Protocol-independent remote file system API used by the browser session
//! and the transfer workers. Every implementation is blocking and owned by a
//! single worker thread.

mod ftp;
mod sftp;

use std::io::{Read, Write};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Local};

use crate::config::{Protocol, Site};
use crate::trust::TrustStore;

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: Option<DateTime<Local>>,
    pub perms: String,
}

/// Progress callback: receives the number of bytes moved since the last call,
/// returns `false` to abort the transfer.
pub type Progress<'a> = &'a mut dyn FnMut(u64) -> bool;

pub trait RemoteFs: Send {
    fn home(&mut self) -> Result<String>;
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>>;
    /// Size of a file, `None` when it does not exist.
    fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>>;
    fn mkdir(&mut self, path: &str) -> Result<()>;
    fn rmdir(&mut self, path: &str) -> Result<()>;
    fn remove(&mut self, path: &str) -> Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    /// Download `path` starting at `offset` into `out`.
    fn download(&mut self, path: &str, offset: u64, out: &mut dyn Write, progress: Progress)
    -> Result<()>;
    /// Upload `input` to `path`; when `offset > 0` the remote file is appended to.
    fn upload(&mut self, path: &str, offset: u64, input: &mut dyn Read, progress: Progress)
    -> Result<()>;
    /// Cheap liveness check (NOOP / stat).
    fn keepalive(&mut self) -> Result<()>;
    fn close(&mut self);
}

/// Logger used by connections; implemented by the UI side.
pub type Logger = Arc<dyn Fn(LogLevel, String) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Command,
    Reply,
    Success,
    Error,
}

pub fn connect(site: &Site, trust: Arc<TrustStore>, log: Logger) -> Result<Box<dyn RemoteFs>> {
    if site.host.trim().is_empty() {
        anyhow::bail!("Не указан хост");
    }
    match site.protocol {
        Protocol::Sftp => Ok(Box::new(sftp::SftpFs::connect(site, trust, log)?)),
        _ => Ok(Box::new(ftp::FtpFs::connect(site, trust, log)?)),
    }
}

// --- remote (POSIX-style) path helpers -------------------------------------

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "/" {
        format!("/{}", name.trim_start_matches('/'))
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
    }
}

pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

pub fn unix_perms(mode: u32, is_dir: bool, is_link: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_link {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a/", "b"), "/a/b");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(file_name("/a/b.txt"), "b.txt");
        assert_eq!(unix_perms(0o755, true, false), "drwxr-xr-x");
    }
}

/// Live round-trip tests against real servers. Run with e.g.
/// `SKIFF_IT="sftp://tester:secret123@127.0.0.1:2222/tmp" cargo test -- --ignored`
#[cfg(test)]
mod live {
    use super::*;
    use crate::trust::{TrustDecision, TrustStore};

    fn run(url: &str) {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut site = Site::default();
        let dir = site.apply_host_url(url).unwrap_or_else(|| "/".into());
        let trust = TrustStore::load(Arc::new(|req| {
            eprintln!("trust prompt: {:?} {}", req.kind, req.fingerprint);
            TrustDecision::Once
        }));
        let log: Logger = Arc::new(|lvl, m| eprintln!("{lvl:?}: {m}"));
        let mut fs = connect(&site, trust, log).expect("connect");
        eprintln!("home = {}", fs.home().unwrap());
        let base = join(&dir, &format!("skiff-it-{}", std::process::id()));
        fs.mkdir(&base).unwrap();
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let f = join(&base, "файл с пробелом.bin");
        let mut moved = 0u64;
        fs.upload(&f, 0, &mut &data[..1_000_000], &mut |n| { moved += n; true }).unwrap();
        // resume (append) the rest
        fs.upload(&f, 1_000_000, &mut &data[1_000_000..], &mut |n| { moved += n; true }).unwrap();
        assert_eq!(moved, data.len() as u64);
        let st = fs.stat(&f).unwrap().expect("stat");
        assert_eq!(st.size, data.len() as u64);
        let list = fs.list(&base).unwrap();
        assert!(list.iter().any(|e| e.name == "файл с пробелом.bin" && e.size == data.len() as u64), "{list:?}");
        let mut out = Vec::new();
        fs.download(&f, 0, &mut out, &mut |_| true).unwrap();
        assert_eq!(out, data);
        let mut tail = Vec::new();
        fs.download(&f, 2_500_000, &mut tail, &mut |_| true).unwrap();
        assert_eq!(tail, &data[2_500_000..]);
        // cancellation must leave the connection usable
        let r = fs.download(&f, 0, &mut std::io::sink(), &mut |_| false);
        assert!(r.is_err());
        assert!(fs.list(&base).is_ok(), "connection usable after abort");
        let g = join(&base, "renamed.bin");
        fs.rename(&f, &g).unwrap();
        assert!(fs.stat(&f).unwrap().is_none());
        fs.mkdir(&join(&base, "sub")).unwrap();
        assert!(fs.list(&base).unwrap().iter().any(|e| e.name == "sub" && e.is_dir));
        fs.rmdir(&join(&base, "sub")).unwrap();
        fs.remove(&g).unwrap();
        fs.rmdir(&base).unwrap();
        fs.keepalive().unwrap();
        fs.close();
    }

    #[test]
    #[ignore]
    fn live_roundtrip() {
        let urls = std::env::var("SKIFF_IT").expect("SKIFF_IT");
        for url in urls.split_whitespace() {
            eprintln!("==== {url}");
            run(url);
        }
    }
}

#[cfg(test)]
mod live_key {
    use super::*;
    use crate::trust::{TrustDecision, TrustStore};
    #[test]
    #[ignore]
    fn live_key_auth() {
        for spec in std::env::var("SKIFF_IT_KEY").expect("SKIFF_IT_KEY").split_whitespace() {
            // format: keyfile|passphrase|sftp://user@host:port
            let mut parts = spec.splitn(3, '|');
            let (key, pass, url) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
            let mut site = Site::default();
            site.apply_host_url(url);
            site.key_file = key.into();
            site.password = pass.into();
            let trust = TrustStore::load(Arc::new(|_| TrustDecision::Once));
            let mut fs = connect(&site, trust, Arc::new(|l, m| eprintln!("{l:?}: {m}"))).expect("key auth");
            assert!(!fs.list("/").unwrap().is_empty());
            fs.close();
        }
    }
}
