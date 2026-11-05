//! Local file system browsing helpers.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Local};

use crate::remote::RemoteEntry;

/// Lists a local directory using the same entry type as the remote side so
/// both panes share one table widget.
pub fn list(dir: &Path, show_hidden: bool) -> Result<Vec<RemoteEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && is_hidden(&entry) {
            continue;
        }
        let Ok(link_meta) = entry.metadata() else { continue };
        let is_link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        // follow symlinks for type/size
        let meta = std::fs::metadata(entry.path()).unwrap_or(link_meta);
        let modified = meta.modified().ok().map(DateTime::<Local>::from);
        out.push(RemoteEntry {
            name,
            is_dir: meta.is_dir(),
            is_link,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified,
            perms: perms(&meta),
        });
    }
    Ok(out)
}

#[cfg(unix)]
fn perms(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    crate::remote::unix_perms(meta.permissions().mode(), meta.is_dir(), false)
}

#[cfg(not(unix))]
fn perms(meta: &std::fs::Metadata) -> String {
    if meta.permissions().readonly() { "r-".into() } else { "rw".into() }
}

#[cfg(unix)]
fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(windows)]
fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .map(|m| m.file_attributes() & HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
fn is_hidden(_entry: &std::fs::DirEntry) -> bool {
    false
}

/// Root entry points: `/` + home on Unix, drive letters on Windows.
pub fn roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", letter as char));
            if p.exists() {
                v.push(p);
            }
        }
    }
    #[cfg(not(windows))]
    v.push(PathBuf::from("/"));
    if let Some(h) = dirs::home_dir() {
        v.push(h);
    }
    for d in [dirs::desktop_dir(), dirs::download_dir(), dirs::document_dir()]
        .into_iter()
        .flatten()
    {
        if d.exists() && !v.contains(&d) {
            v.push(d);
        }
    }
    v
}

pub fn delete(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Opens a file or folder with the program the OS associates with it.
pub fn open_external(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    // Explorer opens a file with its associated program and a folder in a
    // new window, without cmd's quoting pitfalls (&, ^, %).
    let child = Command::new("explorer.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let child = Command::new("open").arg(path).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let child = Command::new("xdg-open").arg(path).spawn();
    child.map(|_| ())
}
