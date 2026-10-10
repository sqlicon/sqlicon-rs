//! Integration tests for the `onsocssl` TLS transport layer (`SqliStream::connect_tls`).
//!
//! These don't talk to a real Informix engine -- they prove the transport layer itself
//! actually negotiates TLS (handshake, cert verification, hostname checking), the same way
//! libsqli's `sqli_tls_connect` wraps the raw socket in OpenSSL before any SQLI bytes flow.

use rcgen::{generate_simple_self_signed, CertifiedKey};
use sqlicon_rs::{ConnectParams, SqliStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Generates a throwaway self-signed certificate valid for "localhost" and 127.0.0.1.
fn make_test_cert() -> (
    rustls::pki_types::CertificateDer<'static>,
    rustls::pki_types::PrivateKeyDer<'static>,
    String,
) {
    let CertifiedKey { cert, signing_key } = generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])
    .expect("failed to generate self-signed test certificate");
    let pem = cert.pem();
    let cert_der = cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into());
    (cert_der, key_der, pem)
}

/// Starts a minimal TLS echo server on a random local port: accepts one connection,
/// completes the TLS handshake, reads back whatever the client sends, and echoes it.
async fn spawn_tls_echo_server(
    cert_der: rustls::pki_types::CertificateDer<'static>,
    key_der: rustls::pki_types::PrivateKeyDer<'static>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("failed to build test TLS server config");
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server_config));

    let handle = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("accept failed");
        let mut tls = acceptor.accept(tcp).await.expect("server TLS handshake failed");
        let mut buf = [0u8; 5];
        tls.read_exact(&mut buf).await.expect("server read failed");
        assert_eq!(&buf, b"hello");
        tls.write_all(b"world").await.expect("server write failed");
        tls.flush().await.expect("server flush failed");
    });

    (port, handle)
}

#[tokio::test]
async fn connect_tls_without_verification_completes_handshake_over_self_signed_cert() {
    let (cert_der, key_der, _pem) = make_test_cert();
    let (port, server) = spawn_tls_echo_server(cert_der, key_der).await;

    // ssl_verify_peer=false: libsqli's SSL_VERIFY_NONE equivalent. The handshake must still
    // actually negotiate a TLS session (not a plaintext passthrough).
    let params = ConnectParams::new("localhost", port.to_string(), "x").ssl(true, false);
    let mut stream = SqliStream::connect_tls("localhost", port, &params)
        .await
        .expect("TLS connect without verification should still complete a real handshake");

    stream.write_all(b"hello").await.expect("client write failed");
    stream.flush().await.expect("client flush failed");
    let resp = stream.read_bytes(5).await.expect("client read failed");
    assert_eq!(&resp[..], b"world");

    server.await.expect("server task panicked");
}

#[tokio::test]
async fn connect_tls_with_verification_and_no_matching_ca_is_rejected() {
    let (cert_der, key_der, _pem) = make_test_cert();
    let (port, server) = spawn_tls_echo_server(cert_der, key_der).await;

    // ssl_verify_peer=true with no ssl_ca_file falls back to the platform's native trust
    // store, which will never contain this throwaway self-signed cert -- certificate
    // validation must actually reject the connection, not silently accept it.
    let params = ConnectParams::new("localhost", port.to_string(), "x").ssl(true, true);
    let result = SqliStream::connect_tls("localhost", port, &params).await;
    assert!(
        result.is_err(),
        "an untrusted self-signed cert must be rejected when verification is enabled"
    );

    // The server task is still waiting on a handshake that will never complete from this
    // client; abort it instead of hanging the test.
    server.abort();
}

#[tokio::test]
async fn connect_tls_with_verification_and_matching_ca_file_succeeds() {
    let (cert_der, key_der, pem) = make_test_cert();
    let (port, server) = spawn_tls_echo_server(cert_der, key_der).await;

    let ca_path = std::env::temp_dir().join(format!("sqlicon_test_ca_{}.pem", std::process::id()));
    std::fs::write(&ca_path, pem).expect("failed to write test CA file");

    let params = ConnectParams::new("localhost", port.to_string(), "x")
        .ssl(true, true)
        .ssl_ca_file(ca_path.to_string_lossy().to_string());
    let mut stream = SqliStream::connect_tls("localhost", port, &params)
        .await
        .expect("TLS connect should succeed once the self-signed cert is in the CA file");

    stream.write_all(b"hello").await.expect("client write failed");
    stream.flush().await.expect("client flush failed");
    let resp = stream.read_bytes(5).await.expect("client read failed");
    assert_eq!(&resp[..], b"world");

    server.await.expect("server task panicked");
    let _ = std::fs::remove_file(&ca_path);
}
