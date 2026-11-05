//! FTP / FTPS (explicit & implicit) backend built on `suppaftp` + rustls.

use std::io::{Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local};
use suppaftp::list::{ListParser, PosixPexQuery};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode, RustlsConnector, RustlsFtpStream, Status};

use super::{LogLevel, Logger, Progress, RemoteEntry, RemoteFs};
use crate::config::{FtpMode, Protocol, Site};
use crate::trust::{TrustStore, tls_client_config};

const TIMEOUT: Duration = Duration::from_secs(20);
const BUF: usize = 128 * 1024;

pub struct FtpFs {
    ftp: RustlsFtpStream,
    log: Logger,
    use_mlsd: bool,
}

fn err(e: FtpError) -> anyhow::Error {
    match e {
        FtpError::UnexpectedResponse(r) => {
            let body = r.as_string().unwrap_or_default();
            let body = body.trim();
            if body.starts_with(|c: char| c.is_ascii_digit()) {
                anyhow!("{body}")
            } else {
                anyhow!("{} {body}", r.status.code())
            }
        }
        other => anyhow!("{other}"),
    }
}

fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("не удалось разрешить имя {host}"))?
        .collect();
    if addrs.is_empty() {
        anyhow::bail!("адрес для {host} не найден");
    }
    Ok(addrs)
}

impl FtpFs {
    pub fn connect(site: &Site, trust: Arc<TrustStore>, log: Logger) -> Result<Self> {
        let host = site.host.trim();
        let port = site.effective_port();
        let addrs = resolve(host, port)?;
        log(LogLevel::Info, format!("Подключение к {host}:{port} ({})…", site.protocol.label()));

        let tls = || RustlsConnector::from(tls_client_config(trust.clone(), &site.endpoint()));

        let mut ftp = if site.protocol == Protocol::FtpsImplicit {
            let mut last = None;
            let mut conn = None;
            for addr in &addrs {
                match RustlsFtpStream::connect_secure_implicit(addr, tls(), host) {
                    Ok(c) => {
                        conn = Some(c);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            conn.ok_or_else(|| err(last.unwrap()))?
        } else {
            let mut last = None;
            let mut conn = None;
            for addr in &addrs {
                match RustlsFtpStream::connect_timeout(*addr, TIMEOUT) {
                    Ok(c) => {
                        conn = Some(c);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            conn.ok_or_else(|| err(last.unwrap()))?
        };
        {
            let sock = ftp.get_ref();
            let _ = sock.set_read_timeout(Some(Duration::from_secs(60)));
            let _ = sock.set_write_timeout(Some(Duration::from_secs(60)));
        }
        if let Some(w) = ftp.get_welcome_msg() {
            for line in w.lines() {
                log(LogLevel::Reply, line.to_string());
            }
        }

        if site.protocol == Protocol::FtpsExplicit {
            log(LogLevel::Command, "AUTH TLS".into());
            ftp = ftp.into_secure(tls(), host).map_err(err).context("ошибка TLS")?;
            log(LogLevel::Info, "TLS-соединение установлено".into());
        }

        if site.ftp_mode == FtpMode::Active {
            ftp = ftp.active_mode(Duration::from_secs(30));
            ftp.set_mode(Mode::Active);
        } else {
            // Like FileZilla: PASV for IPv4 (with the NAT workaround for
            // servers that report a private address), EPSV for IPv6.
            let ipv6 = ftp.get_ref().peer_addr().map(|a| a.is_ipv6()).unwrap_or(false);
            ftp.set_mode(if ipv6 { Mode::ExtendedPassive } else { Mode::Passive });
            ftp.set_passive_nat_workaround(true);
        }

        let (user, pass) = if site.user.trim().is_empty() {
            ("anonymous".to_string(), "anonymous@".to_string())
        } else {
            (site.user.clone(), site.password.clone())
        };
        log(LogLevel::Command, format!("USER {user}"));
        log(LogLevel::Command, "PASS ********".into());
        ftp.login(&user, &pass).map_err(err).context("ошибка входа")?;
        log(LogLevel::Success, "Вход выполнен".into());
        if site.protocol == Protocol::FtpsImplicit {
            // implicit mode skips AUTH TLS, so protect the data channel explicitly
            log(LogLevel::Command, "PBSZ 0 / PROT P".into());
            ftp.custom_command("PBSZ 0", &[Status::CommandOk]).map_err(err)?;
            ftp.custom_command("PROT P", &[Status::CommandOk]).map_err(err)?;
        }
        ftp.transfer_type(FileType::Binary).map_err(err)?;

        let feats = ftp.feat().unwrap_or_default();
        let use_mlsd = feats.keys().any(|k| k.eq_ignore_ascii_case("MLST"));
        if feats.keys().any(|k| k.eq_ignore_ascii_case("UTF8")) {
            let _ = ftp.opts("UTF8", Some("ON"));
        }
        Ok(Self { ftp, log, use_mlsd })
    }

    /// Runs a listing command and reads its data channel, tolerating TLS
    /// data channels closed without close_notify (vsftpd and others).
    fn data_lines(&mut self, cmd: &str) -> std::result::Result<Vec<String>, FtpError> {
        let (_, mut stream) = self
            .ftp
            .custom_data_command(cmd, &[Status::AboutToSend, Status::AlreadyOpen])?;
        let mut raw = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(FtpError::ConnectionError(e)),
            }
        }
        stream.finish()?;
        Ok(String::from_utf8_lossy(&raw)
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn parse(&self, lines: &[String], mlsd: bool) -> Vec<RemoteEntry> {
        let mut out = Vec::with_capacity(lines.len());
        for line in lines {
            let parsed = if mlsd {
                ListParser::parse_mlst(line).or_else(|_| ListParser::parse_mlsd(line))
            } else {
                ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line))
            };
            let Ok(f) = parsed else {
                continue;
            };
            let name = f.name().to_string();
            if name == "." || name == ".." || name.is_empty() {
                continue;
            }
            let is_dir = f.is_directory();
            let is_link = f.is_symlink();
            let mut perms = String::new();
            if !mlsd || f.uid().is_some() {
                perms.push(if is_link { 'l' } else if is_dir { 'd' } else { '-' });
                for who in [PosixPexQuery::Owner, PosixPexQuery::Group, PosixPexQuery::Others] {
                    perms.push(if f.can_read(who) { 'r' } else { '-' });
                    perms.push(if f.can_write(who) { 'w' } else { '-' });
                    perms.push(if f.can_execute(who) { 'x' } else { '-' });
                }
            }
            let modified: DateTime<Local> = f.modified().into();
            let modified = (modified.timestamp() > 0).then_some(modified);
            out.push(RemoteEntry {
                name,
                is_dir,
                is_link,
                size: f.size() as u64,
                modified,
                perms,
            });
        }
        out
    }
}

impl RemoteFs for FtpFs {
    fn home(&mut self) -> Result<String> {
        self.ftp.pwd().map_err(err)
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        if self.use_mlsd {
            (self.log)(LogLevel::Command, format!("MLSD {path}"));
            match self.data_lines(&format!("MLSD {path}")) {
                Ok(lines) => return Ok(self.parse(&lines, true)),
                Err(FtpError::UnexpectedResponse(r)) if matches!(r.status.code(), 500..=504) => {
                    self.use_mlsd = false;
                }
                Err(e) => return Err(err(e)),
            }
        }
        // LIST with a path breaks on names with spaces on some servers: CWD first.
        (self.log)(LogLevel::Command, format!("CWD {path}"));
        self.ftp.cwd(path).map_err(err)?;
        (self.log)(LogLevel::Command, "LIST -a".into());
        let lines = match self.data_lines("LIST -a") {
            Ok(l) => l,
            Err(FtpError::UnexpectedResponse(_)) => self.data_lines("LIST").map_err(err)?,
            Err(e) => return Err(err(e)),
        };
        Ok(self.parse(&lines, false))
    }

    fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>> {
        if self.use_mlsd
            && let Ok(line) = self.ftp.mlst(Some(path))
        {
            let mut v = self.parse(&[line.trim().to_string()], true);
            if let Some(mut e) = v.pop() {
                e.name = super::file_name(path).to_string();
                return Ok(Some(e));
            }
        }
        match self.ftp.size(path) {
            Ok(size) => {
                let modified = self
                    .ftp
                    .mdtm(path)
                    .ok()
                    .map(|t| t.and_utc().with_timezone(&Local));
                Ok(Some(RemoteEntry {
                    name: super::file_name(path).to_string(),
                    is_dir: false,
                    is_link: false,
                    size: size as u64,
                    modified,
                    perms: String::new(),
                }))
            }
            Err(FtpError::UnexpectedResponse(_)) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("MKD {path}"));
        self.ftp.mkdir(path).map_err(err)
    }

    fn rmdir(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("RMD {path}"));
        self.ftp.rmdir(path).map_err(err)
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("DELE {path}"));
        self.ftp.rm(path).map_err(err)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("RNFR {from} → RNTO {to}"));
        self.ftp.rename(from, to).map_err(err)
    }

    fn download(
        &mut self,
        path: &str,
        offset: u64,
        out: &mut dyn Write,
        progress: Progress,
    ) -> Result<()> {
        if offset > 0 {
            (self.log)(LogLevel::Command, format!("REST {offset}"));
            self.ftp.resume_transfer(offset as usize).map_err(err)?;
        }
        (self.log)(LogLevel::Command, format!("RETR {path}"));
        let mut stream = self.ftp.retr_as_stream(path).map_err(err)?;
        let mut buf = vec![0u8; BUF];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // Some servers (vsftpd) close the TLS data channel without
                // close_notify; the 226 reply on the control channel decides.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                    return Err(e.into());
                }
            };
            if let Err(e) = out.write_all(&buf[..n]) {
                let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                return Err(e.into());
            }
            if !progress(n as u64) {
                let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                anyhow::bail!("Передача отменена");
            }
        }
        stream.finish().map_err(err)
    }

    fn upload(
        &mut self,
        path: &str,
        offset: u64,
        input: &mut dyn Read,
        progress: Progress,
    ) -> Result<()> {
        let mut stream = if offset > 0 {
            (self.log)(LogLevel::Command, format!("APPE {path}"));
            self.ftp.append_with_stream(path).map_err(err)?
        } else {
            (self.log)(LogLevel::Command, format!("STOR {path}"));
            self.ftp.put_with_stream(path).map_err(err)?
        };
        let mut buf = vec![0u8; BUF];
        loop {
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                    return Err(e.into());
                }
            };
            if let Err(e) = stream.write_all(&buf[..n]) {
                let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                return Err(e.into());
            }
            if !progress(n as u64) {
                let _ = stream.finish(); // closing the data channel gives exactly one reply (226/426), unlike ABOR
                anyhow::bail!("Передача отменена");
            }
        }
        stream.finish().map_err(err)
    }

    fn keepalive(&mut self) -> Result<()> {
        self.ftp.noop().map_err(err)
    }

    fn close(&mut self) {
        let _ = self.ftp.quit();
    }
}
