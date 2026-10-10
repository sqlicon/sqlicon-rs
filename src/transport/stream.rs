//! Asynchronous Tokio Transport Stream wrapper for Informix SQLI communication.
//! Supports both TCP (`onsoctcp`) and Unix domain sockets (`onipcstr`).

use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};
use crate::protocol::sl_header::SlHeader;
use bytes::BytesMut;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

enum InnerStream {
    Tcp(TcpStream),
    /// Boxed: `TlsStream` carries the whole rustls session state and would otherwise bloat
    /// every `InnerStream` (including the much smaller Tcp/Unix variants) by its size.
    Tls(Box<TlsStream<TcpStream>>),
    #[cfg(unix)]
    Unix(UnixStream),
}

fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Permissive verifier used when `ssl_verify_peer` is false, matching OpenSSL's
/// `SSL_VERIFY_NONE`: the handshake's cryptographic signatures are still checked (so the
/// session remains protected against passive eavesdropping), but the certificate chain and
/// hostname are not validated against any trust store -- the connection is not protected
/// against an active man-in-the-middle presenting any certificate.
#[derive(Debug)]
struct NoCertVerification(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Builds a rustls client config mirroring libsqli's `sqli_tls_connect`: verified mode loads
/// `ssl_ca_file` if given, else the platform's native trust store (matching
/// `SSL_CTX_set_default_verify_paths`); unverified mode installs [`NoCertVerification`]
/// (matching `SSL_VERIFY_NONE`).
fn build_tls_config(verify_peer: bool, ca_file: Option<&str>) -> Result<rustls::ClientConfig> {
    let builder = rustls::ClientConfig::builder();
    let config = if verify_peer {
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = ca_file {
            let pem = std::fs::read(path).map_err(Error::Io)?;
            let mut reader = std::io::BufReader::new(&pem[..]);
            for cert in rustls_pemfile::certs(&mut reader) {
                let cert = cert.map_err(Error::Io)?;
                roots.add(cert).map_err(|e| {
                    Error::Configuration(format!("invalid TLS CA certificate in {path}: {e}"))
                })?;
            }
            if roots.is_empty() {
                return Err(Error::Configuration(format!(
                    "no certificates found in TLS CA file {path}"
                )));
            }
        } else {
            let result = rustls_native_certs::load_native_certs();
            for cert in result.certs {
                let _ = roots.add(cert);
            }
            if roots.is_empty() {
                return Err(Error::Configuration(
                    "failed to load any certificates from the platform's native trust store"
                        .into(),
                ));
            }
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        let provider = rustls::crypto::ring::default_provider();
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerification(Arc::new(provider))))
            .with_no_client_auth()
    };
    Ok(config)
}

pub struct SqliStream {
    stream: InnerStream,
    buf: BytesMut,
}

impl SqliStream {
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect(&addr).await.map_err(Error::Io)?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream: InnerStream::Tcp(stream),
            buf: BytesMut::with_capacity(8192),
        })
    }

    /// Connects over TCP and establishes a TLS session before any SQLI bytes are exchanged,
    /// matching libsqli's `sqli_tls_connect` (which wraps the socket in OpenSSL prior to the
    /// CONREQ handshake). `SQLI_SSL_VERIFY`/`SQLI_SSL_CA_FILE` environment variables are
    /// consulted as a fallback, as in libsqli.
    pub async fn connect_tls(host: &str, port: u16, params: &ConnectParams) -> Result<Self> {
        let addr = format!("{host}:{port}");
        let tcp = TcpStream::connect(&addr).await.map_err(Error::Io)?;
        let _ = tcp.set_nodelay(true);

        let verify_peer = params.ssl_verify_peer || env_bool("SQLI_SSL_VERIFY");
        let ca_file = params
            .ssl_ca_file
            .clone()
            .or_else(|| std::env::var("SQLI_SSL_CA_FILE").ok())
            .filter(|s| !s.is_empty());

        let config = build_tls_config(verify_peer, ca_file.as_deref())?;
        let connector = TlsConnector::from(Arc::new(config));

        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| Error::InvalidArgument(format!("invalid TLS server name: {host}")))?;

        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(Error::Io)?;

        Ok(Self {
            stream: InnerStream::Tls(Box::new(tls)),
            buf: BytesMut::with_capacity(8192),
        })
    }

    #[cfg(unix)]
    pub async fn connect_unix<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let stream = UnixStream::connect(path).await.map_err(Error::Io)?;
        Ok(Self {
            stream: InnerStream::Unix(stream),
            buf: BytesMut::with_capacity(8192),
        })
    }

    pub fn has_buffered_data(&self) -> bool {
        !self.buf.is_empty()
    }

    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Non-blocking attempt to read any immediately available socket data into `self.buf`.
    /// No-op on a TLS stream: `tokio_rustls::TlsStream` has no non-blocking `try_read`
    /// equivalent (it isn't a raw socket read -- it would need to drive the TLS record layer),
    /// so callers always fall through to the timeout-bounded read in [`Self::peek_array`].
    pub fn try_fill_buf(&mut self) {
        let mut chunk = [0u8; 4096];
        loop {
            let res = match &mut self.stream {
                InnerStream::Tcp(s) => s.try_read(&mut chunk),
                InnerStream::Tls(_) => break,
                #[cfg(unix)]
                InnerStream::Unix(s) => s.try_read(&mut chunk),
            };
            match res {
                Ok(0) => break,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Ensures that `self.buf` contains at least `needed` bytes.
    pub async fn ensure_buffered(&mut self, needed: usize) -> Result<()> {
        while self.buf.len() < needed {
            let mut chunk = [0u8; 4096];
            let n = match &mut self.stream {
                InnerStream::Tcp(s) => s.read(&mut chunk).await.map_err(Error::Io)?,
                InnerStream::Tls(s) => s.read(&mut chunk).await.map_err(Error::Io)?,
                #[cfg(unix)]
                InnerStream::Unix(s) => s.read(&mut chunk).await.map_err(Error::Io)?,
            };
            if n == 0 {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Connection closed by server while waiting for data",
                )));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
        Ok(())
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.ensure_buffered(buf.len()).await?;
        let bytes = self.buf.split_to(buf.len());
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    pub async fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf).await?;
        Ok(u16::from_be_bytes(buf))
    }

    pub async fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf).await?;
        Ok(u32::from_be_bytes(buf))
    }

    pub async fn read_bytes(&mut self, count: usize) -> Result<BytesMut> {
        self.ensure_buffered(count).await?;
        Ok(self.buf.split_to(count))
    }

    pub async fn drain(&mut self, count: usize) -> Result<()> {
        let _ = self.read_bytes(count).await?;
        Ok(())
    }

    /// Peeks `N` bytes from the buffer, attempting to read from socket within `timeout_ms` if not enough bytes.
    pub async fn peek_array<const N: usize>(&mut self, timeout_ms: u64) -> Result<Option<[u8; N]>> {
        self.try_fill_buf();
        if self.buf.len() >= N {
            let mut out = [0u8; N];
            out.copy_from_slice(&self.buf[..N]);
            return Ok(Some(out));
        }
        if timeout_ms > 0 {
            let fut = async {
                while self.buf.len() < N {
                    let mut chunk = [0u8; 4096];
                    let n = match &mut self.stream {
                        InnerStream::Tcp(s) => s.read(&mut chunk).await?,
                        InnerStream::Tls(s) => s.read(&mut chunk).await?,
                        #[cfg(unix)]
                        InnerStream::Unix(s) => s.read(&mut chunk).await?,
                    };
                    if n == 0 {
                        break;
                    }
                    self.buf.extend_from_slice(&chunk[..n]);
                }
                std::io::Result::Ok(())
            };
            let _ = tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await;
        }
        if self.buf.len() >= N {
            let mut out = [0u8; N];
            out.copy_from_slice(&self.buf[..N]);
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }

    pub async fn peek_u16(&mut self, timeout_ms: u64) -> Result<Option<u16>> {
        let arr = self.peek_array::<2>(timeout_ms).await?;
        Ok(arr.map(u16::from_be_bytes))
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        match &mut self.stream {
            InnerStream::Tcp(s) => s.write_all(buf).await.map_err(Error::Io)?,
            InnerStream::Tls(s) => s.write_all(buf).await.map_err(Error::Io)?,
            #[cfg(unix)]
            InnerStream::Unix(s) => s.write_all(buf).await.map_err(Error::Io)?,
        }
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        match &mut self.stream {
            InnerStream::Tcp(s) => s.flush().await.map_err(Error::Io)?,
            InnerStream::Tls(s) => s.flush().await.map_err(Error::Io)?,
            #[cfg(unix)]
            InnerStream::Unix(s) => s.flush().await.map_err(Error::Io)?,
        }
        Ok(())
    }

    /// Reads an SL-framed packet: reads 6-byte header, then the body of (pdu_size - 6) bytes.
    pub async fn read_sl_pdu(&mut self) -> Result<(SlHeader, BytesMut)> {
        let mut hdr_bytes = [0u8; SlHeader::SIZE];
        self.read_exact(&mut hdr_bytes).await?;
        let header = SlHeader::decode_from_slice(&hdr_bytes)?;

        if (header.pdu_size as usize) < SlHeader::SIZE {
            return Err(Error::Protocol(format!(
                "Invalid SL pdu_size: {} (less than header size)",
                header.pdu_size
            )));
        }

        let body_len = (header.pdu_size as usize) - SlHeader::SIZE;
        let body = if body_len > 0 {
            self.read_bytes(body_len).await?
        } else {
            BytesMut::new()
        };

        Ok((header, body))
    }

    /// Returns `true` if this stream is a TCP socket (as opposed to a Unix domain socket).
    /// Out-of-band `MSG_OOB` cancellation is only meaningful on TCP, matching libsqli's
    /// `is_tcp_connection()` check in `sqli_cancel_posix.c`.
    pub fn is_tcp(&self) -> bool {
        match &self.stream {
            InnerStream::Tcp(_) | InnerStream::Tls(_) => true,
            #[cfg(unix)]
            InnerStream::Unix(_) => false,
        }
    }

    #[cfg(unix)]
    pub fn raw_fd(&self) -> std::os::unix::io::RawFd {
        use std::os::unix::io::AsRawFd;
        match &self.stream {
            InnerStream::Tcp(s) => s.as_raw_fd(),
            InnerStream::Tls(s) => s.as_raw_fd(),
            InnerStream::Unix(s) => s.as_raw_fd(),
        }
    }

    #[cfg(windows)]
    pub fn raw_socket(&self) -> std::os::windows::io::RawSocket {
        use std::os::windows::io::AsRawSocket;
        match &self.stream {
            InnerStream::Tcp(s) => s.as_raw_socket(),
            InnerStream::Tls(s) => s.as_raw_socket(),
        }
    }
}
