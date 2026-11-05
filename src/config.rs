//! Site manager entries, application settings, persistence and
//! import of FileZilla's `sitemanager.xml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Protocol {
    #[default]
    Ftp,
    /// FTP over TLS, explicit (AUTH TLS), the recommended FTPS mode.
    FtpsExplicit,
    /// FTP over TLS, implicit (legacy, usually port 990).
    FtpsImplicit,
    Sftp,
}

impl Protocol {
    pub const ALL: [Protocol; 4] = [
        Protocol::Sftp,
        Protocol::FtpsExplicit,
        Protocol::Ftp,
        Protocol::FtpsImplicit,
    ];

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ftp | Protocol::FtpsExplicit => 21,
            Protocol::FtpsImplicit => 990,
            Protocol::Sftp => 22,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ftp => "FTP",
            Protocol::FtpsExplicit => "FTPS (явный TLS)",
            Protocol::FtpsImplicit => "FTPS (неявный TLS)",
            Protocol::Sftp => "SFTP (SSH)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Protocol::Ftp => "ftp",
            Protocol::FtpsExplicit => "ftpes",
            Protocol::FtpsImplicit => "ftps",
            Protocol::Sftp => "sftp",
        }
    }

    pub fn is_secure(self) -> bool {
        !matches!(self, Protocol::Ftp)
    }

    pub fn from_scheme(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "ftp" => Protocol::Ftp,
            "ftpes" => Protocol::FtpsExplicit,
            "ftps" => Protocol::FtpsImplicit,
            "sftp" | "ssh" => Protocol::Sftp,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FtpMode {
    #[default]
    Passive,
    Active,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Site {
    pub id: u64,
    pub name: String,
    /// Group/folder shown in the site manager ("" = top level).
    pub group: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Stored base64-obfuscated on disk (same as FileZilla's default), never encrypted.
    #[serde(with = "obfuscated")]
    pub password: String,
    pub remember_password: bool,
    /// SFTP: path to a private key (OpenSSH/PEM). Password is used as the passphrase.
    pub key_file: String,
    pub ftp_mode: FtpMode,
    pub remote_dir: String,
    pub local_dir: String,
    pub comment: String,
}

impl Default for Site {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Новый сайт".into(),
            group: String::new(),
            protocol: Protocol::Sftp,
            host: String::new(),
            port: 0,
            user: String::new(),
            password: String::new(),
            remember_password: true,
            key_file: String::new(),
            ftp_mode: FtpMode::Passive,
            remote_dir: String::new(),
            local_dir: String::new(),
            comment: String::new(),
        }
    }
}

impl Site {
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            self.protocol.default_port()
        } else {
            self.port
        }
    }

    /// Stable key used for trust store entries and connection reuse.
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host.trim(), self.effective_port())
    }

    pub fn display_url(&self) -> String {
        let user = if self.user.is_empty() {
            String::new()
        } else {
            format!("{}@", self.user)
        };
        format!("{}://{}{}", self.protocol.short(), user, self.endpoint())
    }

    /// Parses quickconnect-style host strings like `sftp://user@host:2222/path`.
    /// Returns the remaining path (if any).
    pub fn apply_host_url(&mut self, input: &str) -> Option<String> {
        let mut rest = input.trim();
        if let Some((scheme, r)) = rest.split_once("://") {
            if let Some(p) = Protocol::from_scheme(scheme) {
                self.protocol = p;
            }
            rest = r;
        }
        let (hostpart, path) = match rest.find('/') {
            Some(i) => (&rest[..i], Some(rest[i..].to_string())),
            None => (rest, None),
        };
        let hostpart = if let Some((user, h)) = hostpart.rsplit_once('@') {
            if let Some((u, p)) = user.split_once(':') {
                self.user = u.to_string();
                self.password = p.to_string();
            } else {
                self.user = user.to_string();
            }
            h
        } else {
            hostpart
        };
        // IPv6 literal in brackets
        if let Some(stripped) = hostpart.strip_prefix('[') {
            if let Some((h, after)) = stripped.split_once(']') {
                self.host = h.to_string();
                if let Some(port) = after.strip_prefix(':').and_then(|p| p.parse().ok()) {
                    self.port = port;
                }
            }
        } else if let Some((h, p)) = hostpart.rsplit_once(':')
            && let Ok(port) = p.parse()
        {
            self.host = h.to_string();
            self.port = port;
        } else {
            self.host = hostpart.to_string();
        }
        path
    }
}

mod obfuscated {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        let s = String::deserialize(d)?;
        Ok(base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExistsAction {
    #[default]
    Ask,
    Overwrite,
    OverwriteIfNewer,
    Resume,
    Skip,
}

impl ExistsAction {
    pub const ALL: [ExistsAction; 5] = [
        ExistsAction::Ask,
        ExistsAction::Overwrite,
        ExistsAction::OverwriteIfNewer,
        ExistsAction::Resume,
        ExistsAction::Skip,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ExistsAction::Ask => "Спрашивать",
            ExistsAction::Overwrite => "Перезаписывать",
            ExistsAction::OverwriteIfNewer => "Перезаписывать, если новее",
            ExistsAction::Resume => "Докачивать",
            ExistsAction::Skip => "Пропускать",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub max_transfers: usize,
    pub exists_action: ExistsAction,
    pub show_hidden_local: bool,
    pub last_local_dir: String,
    pub quick_protocol: Protocol,
    pub quick_host: String,
    pub quick_user: String,
    pub quick_port: String,
    pub ui_scale: f32,
    pub split: f32,
    pub bottom_h: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_transfers: 2,
            exists_action: ExistsAction::Ask,
            show_hidden_local: false,
            last_local_dir: String::new(),
            quick_protocol: Protocol::Sftp,
            quick_host: String::new(),
            quick_user: String::new(),
            quick_port: String::new(),
            ui_scale: 1.0,
            split: 0.5,
            bottom_h: 230.0,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("skiff")
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn load_sites() -> Vec<Site> {
    load_json(&config_dir().join("sites.json"))
}

pub fn save_sites(sites: &[Site]) -> Result<()> {
    // Never write passwords the user didn't want remembered.
    let cleaned: Vec<Site> = sites
        .iter()
        .cloned()
        .map(|mut s| {
            if !s.remember_password {
                s.password.clear();
            }
            s
        })
        .collect();
    save_json(&config_dir().join("sites.json"), &cleaned)
}

pub fn load_settings() -> Settings {
    load_json(&config_dir().join("settings.json"))
}

pub fn save_settings(s: &Settings) -> Result<()> {
    save_json(&config_dir().join("settings.json"), s)
}

pub fn next_site_id(sites: &[Site]) -> u64 {
    sites.iter().map(|s| s.id).max().unwrap_or(0) + 1
}

// ---------------------------------------------------------------------------
// FileZilla sitemanager.xml import
// ---------------------------------------------------------------------------

/// Default location of FileZilla's site manager file on this OS.
pub fn filezilla_sitemanager_path() -> Option<PathBuf> {
    let candidates = [
        dirs::config_dir().map(|d| d.join("filezilla").join("sitemanager.xml")),
        dirs::config_dir().map(|d| d.join("FileZilla").join("sitemanager.xml")),
        dirs::home_dir().map(|d| d.join(".filezilla").join("sitemanager.xml")),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

/// FileZilla stores server paths as `<type> <prefix-len> <len> <seg> <len> <seg>...`.
fn decode_fz_path(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() || s.starts_with('/') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let read_num = |i: &mut usize| -> Option<usize> {
        while *i < chars.len() && chars[*i] == ' ' {
            *i += 1;
        }
        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            *i += 1;
        }
        if start == *i {
            return None;
        }
        chars[start..*i].iter().collect::<String>().parse().ok()
    };
    let Some(_server_type) = read_num(&mut i) else {
        return s.to_string();
    };
    let Some(prefix_len) = read_num(&mut i) else {
        return s.to_string();
    };
    if prefix_len > 0 {
        i += 1 + prefix_len; // skip prefix (VMS etc.), unsupported
    }
    let mut segs = Vec::new();
    while let Some(len) = read_num(&mut i) {
        i += 1; // space
        if i + len > chars.len() {
            break;
        }
        segs.push(chars[i..i + len].iter().collect::<String>());
        i += len;
    }
    format!("/{}", segs.join("/"))
}

fn resolve_entity(name: &str) -> String {
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        n => {
            let code = if let Some(h) = n.strip_prefix("#x").or_else(|| n.strip_prefix("#X")) {
                u32::from_str_radix(h, 16).ok()
            } else {
                n.strip_prefix('#').and_then(|d| d.parse().ok())
            };
            code.and_then(char::from_u32).map(String::from).unwrap_or_default()
        }
    }
}

pub fn import_filezilla(path: &Path, start_id: u64) -> Result<Vec<Site>> {
    use quick_xml::events::Event;
    let xml = std::fs::read_to_string(path)
        .with_context(|| format!("не удалось прочитать {}", path.display()))?;
    let mut reader = quick_xml::Reader::from_str(&xml);

    let mut sites = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut cur: Option<Site> = None;
    let mut pass_b64 = false;
    let mut logon_type = 1;
    let mut fz_protocol = 0;
    let mut text = String::new();
    let mut id = start_id;

    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                // A folder's name is the text before its first child element.
                if stack.last().map(String::as_str) == Some("Folder")
                    && let Some(f) = folders.last_mut()
                    && f.is_empty()
                {
                    *f = text.trim().to_string();
                }
                text.clear();
                match name.as_str() {
                    "Server" => {
                        cur = Some(Site {
                            group: folders.join("/"),
                            protocol: Protocol::Ftp,
                            ..Default::default()
                        });
                        logon_type = 1;
                        fz_protocol = 0;
                    }
                    "Folder" => folders.push(String::new()),
                    "Pass" => {
                        pass_b64 = e.attributes().flatten().any(|a| {
                            a.key.as_ref() == "encoding" && a.value.as_ref() == "base64"
                        });
                    }
                    _ => {}
                }
                stack.push(name);
            }
            Event::Text(t) => text.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0)),
            Event::CData(t) => text.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0)),
            Event::GeneralRef(r) => text.push_str(&resolve_entity(&r.into_inner())),
            Event::End(e) => {
                let name = e.name().as_ref().to_string();
                stack.pop();
                let value = std::mem::take(&mut text);
                match name.as_str() {
                    "Server" => {
                        if let Some(mut s) = cur.take() {
                            s.protocol = match fz_protocol {
                                1 => Protocol::Sftp,
                                3 => Protocol::FtpsImplicit,
                                4 => Protocol::FtpsExplicit,
                                _ => Protocol::Ftp,
                            };
                            if s.port == s.protocol.default_port() {
                                s.port = 0;
                            }
                            match logon_type {
                                0 => {
                                    s.user = "anonymous".into();
                                    s.password = "anonymous@".into();
                                }
                                2 | 3 => s.remember_password = false,
                                _ => {}
                            }
                            s.id = id;
                            id += 1;
                            sites.push(s);
                        }
                    }
                    "Folder" => {
                        folders.pop();
                    }
                    field => {
                        let Some(s) = cur.as_mut() else { continue };
                        if stack.last().map(String::as_str) != Some("Server") {
                            continue;
                        }
                        match field {
                            "Host" => s.host = value.trim().to_string(),
                            "Port" => s.port = value.trim().parse().unwrap_or(0),
                            "Protocol" => fz_protocol = value.trim().parse().unwrap_or(0),
                            "User" => s.user = value,
                            "Pass" => {
                                s.password = if pass_b64 {
                                    base64::engine::general_purpose::STANDARD
                                        .decode(value.trim())
                                        .ok()
                                        .and_then(|b| String::from_utf8(b).ok())
                                        .unwrap_or_default()
                                } else {
                                    value
                                }
                            }
                            "Logontype" => logon_type = value.trim().parse().unwrap_or(1),
                            "Keyfile" => s.key_file = value,
                            "Name" => s.name = value.trim().to_string(),
                            "Comments" => s.comment = value,
                            "LocalDir" => s.local_dir = value,
                            "RemoteDir" => s.remote_dir = decode_fz_path(&value),
                            "PasvMode" => {
                                s.ftp_mode = if value.trim() == "MODE_ACTIVE" {
                                    FtpMode::Active
                                } else {
                                    FtpMode::Passive
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fz_path() {
        assert_eq!(decode_fz_path("1 0 4 home 3 bob"), "/home/bob");
        assert_eq!(decode_fz_path("1 0 8 my files"), "/my files");
        assert_eq!(decode_fz_path("/var/www"), "/var/www");
    }

    #[test]
    fn host_url() {
        let mut s = Site::default();
        let p = s.apply_host_url("ftpes://joe:pw@example.com:2121/pub");
        assert_eq!(s.protocol, Protocol::FtpsExplicit);
        assert_eq!(s.user, "joe");
        assert_eq!(s.password, "pw");
        assert_eq!(s.host, "example.com");
        assert_eq!(s.port, 2121);
        assert_eq!(p.as_deref(), Some("/pub"));
    }

    #[test]
    fn import_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FileZilla3><Servers>
<Server><Host>a.example</Host><Port>22</Port><Protocol>1</Protocol><Type>0</Type><User>u</User><Pass encoding="base64">c2VjcmV0</Pass><Logontype>1</Logontype><Name>Alpha</Name><RemoteDir>1 0 3 var 3 www</RemoteDir></Server>
<Folder expanded="1">Work
<Server><Host>b.example</Host><Port>21</Port><Protocol>4</Protocol><Logontype>0</Logontype><Name>Beta &amp; Co</Name></Server>
</Folder>
</Servers></FileZilla3>"#;
        let dir = std::env::temp_dir().join(format!("skiff-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sm.xml");
        std::fs::write(&p, xml).unwrap();
        let sites = import_filezilla(&p, 1).unwrap();
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].protocol, Protocol::Sftp);
        assert_eq!(sites[0].password, "secret");
        assert_eq!(sites[0].remote_dir, "/var/www");
        assert_eq!(sites[0].port, 0);
        assert_eq!(sites[1].group, "Work");
        assert_eq!(sites[1].name, "Beta & Co");
        assert_eq!(sites[1].protocol, Protocol::FtpsExplicit);
        assert_eq!(sites[1].user, "anonymous");
    }
}
