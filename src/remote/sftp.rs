//! SFTP backend built on the pure-Rust `russh` + `russh-sftp` stack.
//!
//! The async libraries are driven by a small private tokio runtime so the
//! rest of the application can stay blocking and simple.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, TimeZone};
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::runtime::Runtime;

use super::{LogLevel, Logger, Progress, RemoteEntry, RemoteFs};
use crate::config::Site;
use crate::trust::{TrustKind, TrustStore};

const BUF: usize = 255 * 1024;

struct Handler {
    endpoint: String,
    trust: Arc<TrustStore>,
    log: Logger,
}

impl client::Handler for Handler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let (fp, algo) = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                (key.fingerprint(HashAlg::Sha256).to_string(), key.algorithm().to_string())
            }
            PublicKeyOrCertificate::Certificate(c) => (
                c.public_key().fingerprint(HashAlg::Sha256).to_string(),
                c.algorithm().to_string(),
            ),
        };
        (self.log)(LogLevel::Info, format!("Ключ сервера {algo} {fp}"));
        let trust = self.trust.clone();
        let endpoint = self.endpoint.clone();
        let details = format!("Тип ключа: {algo}");
        // The prompt blocks until the user answers.
        let ok = tokio::task::block_in_place(move || {
            trust.verify(TrustKind::SshHostKey, &endpoint, &fp, &details)
        });
        Ok(ok)
    }
}

pub struct SftpFs {
    rt: Runtime,
    session: Handle<Handler>,
    sftp: SftpSession,
    log: Logger,
}

fn to_entry(name: String, attrs: &FileAttributes) -> RemoteEntry {
    let is_dir = attrs.is_dir();
    let is_link = attrs.is_symlink();
    RemoteEntry {
        name,
        is_dir,
        is_link,
        size: attrs.size.unwrap_or(0),
        modified: attrs
            .mtime
            .and_then(|t| Local.timestamp_opt(t as i64, 0).single())
            .map(|t: DateTime<Local>| t),
        perms: attrs
            .permissions
            .map(|p| super::unix_perms(p, is_dir, is_link))
            .unwrap_or_default(),
    }
}

fn sftp_err(e: russh_sftp::client::error::Error) -> anyhow::Error {
    anyhow!("{e}")
}

impl SftpFs {
    pub fn connect(site: &Site, trust: Arc<TrustStore>, log: Logger) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let host = site.host.trim().to_string();
        let port = site.effective_port();
        log(LogLevel::Info, format!("Подключение к {host}:{port} (SFTP)…"));

        let (session, sftp) = rt.block_on(async {
            let config = Arc::new(client::Config {
                inactivity_timeout: None,
                keepalive_interval: Some(Duration::from_secs(30)),
                keepalive_max: 4,
                ..Default::default()
            });
            let handler = Handler {
                endpoint: site.endpoint(),
                trust,
                log: log.clone(),
            };
            let mut session = tokio::time::timeout(
                Duration::from_secs(20),
                client::connect(config, (host.as_str(), port), handler),
            )
            .await
            .map_err(|_| anyhow!("время ожидания подключения истекло"))??;
            log(LogLevel::Info, "SSH-соединение установлено, аутентификация…".into());

            let user = site.user.trim().to_string();
            let mut authed = false;

            if !site.key_file.trim().is_empty() {
                let pass = (!site.password.is_empty()).then_some(site.password.as_str());
                let key = load_secret_key(site.key_file.trim(), pass)
                    .with_context(|| format!("не удалось загрузить ключ {}", site.key_file))?;
                let hash = session.best_supported_rsa_hash().await?.flatten();
                let res = session
                    .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                    .await?;
                authed = res.success();
                log(
                    if authed { LogLevel::Success } else { LogLevel::Error },
                    format!("Аутентификация по ключу: {}", if authed { "успешно" } else { "отказано" }),
                );
            }

            if !authed {
                let res = session.authenticate_password(&user, &site.password).await?;
                authed = res.success();
            }

            if !authed {
                // Many servers only offer keyboard-interactive for passwords.
                let mut resp = session
                    .authenticate_keyboard_interactive_start(&user, None::<String>)
                    .await?;
                for _ in 0..5 {
                    match resp {
                        KeyboardInteractiveAuthResponse::Success => {
                            authed = true;
                            break;
                        }
                        KeyboardInteractiveAuthResponse::Failure { .. } => break,
                        KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                            let answers =
                                prompts.iter().map(|_| site.password.clone()).collect();
                            resp = session
                                .authenticate_keyboard_interactive_respond(answers)
                                .await?;
                        }
                    }
                }
            }

            if !authed {
                anyhow::bail!("аутентификация не удалась (неверный логин, пароль или ключ)");
            }
            log(LogLevel::Success, "Вход выполнен".into());

            let channel = session.channel_open_session().await?;
            channel.request_subsystem(true, "sftp").await?;
            let sftp = SftpSession::new(channel.into_stream())
                .await
                .map_err(sftp_err)?;
            sftp.set_timeout(60);
            Ok::<_, anyhow::Error>((session, sftp))
        })?;

        Ok(Self { rt, session, sftp, log })
    }
}

impl RemoteFs for SftpFs {
    fn home(&mut self) -> Result<String> {
        self.rt.block_on(self.sftp.canonicalize(".")).map_err(sftp_err)
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        (self.log)(LogLevel::Command, format!("READDIR {path}"));
        let entries = self.rt.block_on(self.sftp.read_dir(path)).map_err(sftp_err)?;
        let mut out = Vec::new();
        for e in entries {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = e.metadata();
            let mut entry = to_entry(name, &attrs);
            if entry.is_link {
                // Follow symlinks so linked directories can be opened.
                let full = super::join(path, &entry.name);
                if let Ok(target) = self.rt.block_on(self.sftp.metadata(full)) {
                    entry.is_dir = target.is_dir();
                    if !entry.is_dir {
                        entry.size = target.size.unwrap_or(entry.size);
                    }
                }
            }
            out.push(entry);
        }
        Ok(out)
    }

    fn stat(&mut self, path: &str) -> Result<Option<RemoteEntry>> {
        match self.rt.block_on(self.sftp.metadata(path)) {
            Ok(a) => Ok(Some(to_entry(super::file_name(path).to_string(), &a))),
            Err(russh_sftp::client::error::Error::Status(s))
                if s.status_code == StatusCode::NoSuchFile =>
            {
                Ok(None)
            }
            Err(e) => Err(sftp_err(e)),
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("MKDIR {path}"));
        self.rt.block_on(self.sftp.create_dir(path)).map_err(sftp_err)
    }

    fn rmdir(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("RMDIR {path}"));
        self.rt.block_on(self.sftp.remove_dir(path)).map_err(sftp_err)
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("REMOVE {path}"));
        self.rt.block_on(self.sftp.remove_file(path)).map_err(sftp_err)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        (self.log)(LogLevel::Command, format!("RENAME {from} → {to}"));
        self.rt.block_on(self.sftp.rename(from, to)).map_err(sftp_err)
    }

    fn download(
        &mut self,
        path: &str,
        offset: u64,
        out: &mut dyn Write,
        progress: Progress,
    ) -> Result<()> {
        (self.log)(LogLevel::Command, format!("GET {path}"));
        let sftp = &self.sftp;
        self.rt.block_on(async {
            let mut file = sftp.open(path).await.map_err(sftp_err)?;
            if offset > 0 {
                file.seek(std::io::SeekFrom::Start(offset)).await?;
            }
            let mut buf = vec![0u8; BUF];
            loop {
                let n = file.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                if !progress(n as u64) {
                    anyhow::bail!("Передача отменена");
                }
            }
            let _ = file.shutdown().await;
            Ok(())
        })
    }

    fn upload(
        &mut self,
        path: &str,
        offset: u64,
        input: &mut dyn Read,
        progress: Progress,
    ) -> Result<()> {
        (self.log)(LogLevel::Command, format!("PUT {path}"));
        let sftp = &self.sftp;
        self.rt.block_on(async {
            let flags = if offset > 0 {
                OpenFlags::WRITE | OpenFlags::CREATE
            } else {
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE
            };
            let mut file = sftp.open_with_flags(path, flags).await.map_err(sftp_err)?;
            if offset > 0 {
                file.seek(std::io::SeekFrom::Start(offset)).await?;
            }
            let mut buf = vec![0u8; BUF];
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n]).await?;
                if !progress(n as u64) {
                    let _ = file.shutdown().await;
                    anyhow::bail!("Передача отменена");
                }
            }
            file.flush().await?;
            file.shutdown().await?;
            Ok(())
        })
    }

    fn keepalive(&mut self) -> Result<()> {
        self.rt
            .block_on(self.sftp.canonicalize("."))
            .map(|_| ())
            .map_err(sftp_err)
    }

    fn close(&mut self) {
        let _ = self.rt.block_on(self.sftp.close());
        let _ = self.rt.block_on(self.session.disconnect(
            russh::Disconnect::ByApplication,
            "",
            "en",
        ));
    }
}
