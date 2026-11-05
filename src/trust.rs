//! Trust-on-first-use store for SSH host keys and TLS certificates.
//!
//! Like FileZilla, unknown keys/certificates are shown to the user, who can
//! accept them once or permanently. A changed key triggers a loud warning.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use suppaftp::rustls;
use suppaftp::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use suppaftp::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use suppaftp::rustls::{DigitallySignedStruct, SignatureScheme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustKind {
    SshHostKey,
    TlsCertificate,
}

#[derive(Clone, Debug)]
pub struct TrustRequest {
    pub kind: TrustKind,
    pub endpoint: String,
    pub fingerprint: String,
    pub details: String,
    /// Fingerprint stored earlier, if the key has CHANGED.
    pub previous: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustDecision {
    Reject,
    Once,
    Always,
}

pub type Prompter = Arc<dyn Fn(TrustRequest) -> TrustDecision + Send + Sync>;

pub struct TrustStore {
    path: PathBuf,
    map: Mutex<HashMap<String, String>>,
    session: Mutex<HashSet<String>>,
    prompt_lock: Mutex<()>,
    prompter: Prompter,
}

impl TrustStore {
    pub fn load(prompter: Prompter) -> Arc<Self> {
        let path = crate::config::config_dir().join("trusted.json");
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Arc::new(Self {
            path,
            map: Mutex::new(map),
            session: Mutex::new(HashSet::new()),
            prompt_lock: Mutex::new(()),
            prompter,
        })
    }

    fn save(&self) {
        let map = self.map.lock().unwrap().clone();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec_pretty(&map) {
            let _ = std::fs::write(&self.path, data);
        }
    }

    fn key(kind: TrustKind, endpoint: &str) -> String {
        match kind {
            TrustKind::SshHostKey => format!("ssh:{endpoint}"),
            TrustKind::TlsCertificate => format!("tls:{endpoint}"),
        }
    }

    /// Returns true when the connection may proceed. Blocks while the user decides.
    pub fn verify(&self, kind: TrustKind, endpoint: &str, fingerprint: &str, details: &str) -> bool {
        let key = Self::key(kind, endpoint);
        let session_key = format!("{key}={fingerprint}");
        // Only one prompt at a time; re-check after acquiring, another worker
        // may have just been granted the same key.
        let _guard = self.prompt_lock.lock().unwrap();
        let previous = self.map.lock().unwrap().get(&key).cloned();
        if previous.as_deref() == Some(fingerprint)
            || self.session.lock().unwrap().contains(&session_key)
        {
            return true;
        }
        let decision = (self.prompter)(TrustRequest {
            kind,
            endpoint: endpoint.to_string(),
            fingerprint: fingerprint.to_string(),
            details: details.to_string(),
            previous,
        });
        match decision {
            TrustDecision::Reject => false,
            TrustDecision::Once => {
                self.session.lock().unwrap().insert(session_key);
                true
            }
            TrustDecision::Always => {
                self.map.lock().unwrap().insert(key, fingerprint.to_string());
                self.save();
                true
            }
        }
    }
}

pub fn sha256_fingerprint(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// rustls verifier: certificates that validate against the WebPKI roots are
/// accepted silently, everything else (self-signed, expired, wrong host...)
/// goes through the trust store / user prompt.
#[derive(Debug)]
pub struct TofuVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    endpoint: String,
    store: Arc<TrustStoreHandle>,
}

/// Debug-able wrapper so the verifier can derive Debug.
pub struct TrustStoreHandle(pub Arc<TrustStore>);
impl std::fmt::Debug for TrustStoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrustStore")
    }
}

pub fn tls_client_config(store: Arc<TrustStore>, endpoint: &str) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .expect("webpki verifier");
    let verifier = TofuVerifier {
        inner,
        endpoint: endpoint.to_string(),
        store: Arc::new(TrustStoreHandle(store)),
    };
    // FTPS servers (vsftpd, FileZilla Server, ProFTPD…) require every data
    // connection to resume the control connection's TLS session. rustls'
    // TLS 1.3 tickets are single-use, so after a couple of transfers a data
    // connection would fall back to a full handshake and be refused
    // ("522 session reuse required"). TLS 1.2 session IDs can be resumed any
    // number of times, so FTPS is pinned to TLS 1.2 with a store that always
    // keeps the control connection's session.
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS12])
        .expect("tls versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::store(Arc::new(StickySessionStore::default()))
        .tls12_resumption(rustls::client::Tls12Resumption::SessionIdOrTickets);
    Arc::new(config)
}

/// Session store for one FTPS connection: remembers the first TLS 1.2
/// session (the control channel's) and never forgets it.
#[derive(Debug, Default)]
struct StickySessionStore {
    tls12: Mutex<Option<rustls::client::Tls12ClientSessionValue>>,
    kx: Mutex<Option<rustls::NamedGroup>>,
}

impl rustls::client::ClientSessionStore for StickySessionStore {
    fn set_kx_hint(&self, _server_name: ServerName<'static>, group: rustls::NamedGroup) {
        *self.kx.lock().unwrap() = Some(group);
    }
    fn kx_hint(&self, _server_name: &ServerName<'_>) -> Option<rustls::NamedGroup> {
        *self.kx.lock().unwrap()
    }
    fn set_tls12_session(
        &self,
        _server_name: ServerName<'static>,
        value: rustls::client::Tls12ClientSessionValue,
    ) {
        let mut slot = self.tls12.lock().unwrap();
        if slot.is_none() {
            *slot = Some(value);
        }
    }
    fn tls12_session(&self, _server_name: &ServerName<'_>) -> Option<rustls::client::Tls12ClientSessionValue> {
        self.tls12.lock().unwrap().clone()
    }
    fn remove_tls12_session(&self, _server_name: &ServerName<'static>) {}
    fn insert_tls13_ticket(
        &self,
        _server_name: ServerName<'static>,
        _value: rustls::client::Tls13ClientSessionValue,
    ) {
    }
    fn take_tls13_ticket(
        &self,
        _server_name: &ServerName<'static>,
    ) -> Option<rustls::client::Tls13ClientSessionValue> {
        None
    }
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Ok(v) => Ok(v),
            Err(err) => {
                let fp = sha256_fingerprint(end_entity.as_ref());
                let details = format!("Сертификат не прошёл проверку: {err}");
                if self.store.0.verify(TrustKind::TlsCertificate, &self.endpoint, &fp, &details) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(err)
                }
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}
