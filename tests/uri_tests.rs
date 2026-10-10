//! Tests verifying Informix connection URI parsing in Rust according to spec and C test_sqli_uri.c

use sqlicon_rs::ConnectParams;

#[test]
fn test_uri_onsoctcp_valid_full() {
    let uri = "informix+onsoctcp://admin:secret@db-host:9088/customer_db?INFORMIXSERVER=ol_informix1&CLIENT_LOCALE=en_US.utf8&DB_LOCALE=de_DE.1252";
    let params = ConnectParams::from_uri(uri).expect("Should parse valid URI");
    assert_eq!(params.hostname, "db-host");
    assert_eq!(params.service, "9088");
    assert_eq!(params.server, "ol_informix1");
    assert_eq!(params.database.as_deref(), Some("customer_db"));
    assert_eq!(params.username, "admin");
    assert_eq!(params.password, "secret");
    assert_eq!(params.client_locale, "en_US.utf8");
    assert_eq!(params.db_locale, "de_DE.1252");
}

#[test]
fn test_uri_onsoctcp_default_db_and_url_decoding() {
    let uri = "informix+onsoctcp://user:p%40ss%3Dword@db-host:9088/?INFORMIXSERVER=srv";
    let params = ConnectParams::from_uri(uri).expect("Should parse URI with default DB");
    assert_eq!(params.hostname, "db-host");
    assert_eq!(params.service, "9088");
    assert_eq!(params.server, "srv");
    assert_eq!(params.database.as_deref(), Some("informix"));
    assert_eq!(params.username, "user");
    assert_eq!(params.password, "p@ss=word");
}

#[test]
fn test_uri_url_decoding_multibyte_utf8() {
    // %C3%A9 is the two-byte UTF-8 encoding of 'é'; byte-by-byte decoding
    // (treating each decoded byte as its own char) would mangle this.
    let uri = "informix+onsoctcp://caf%C3%A9:p%C3%A4ss@db-host:9088/?INFORMIXSERVER=srv";
    let params = ConnectParams::from_uri(uri).expect("Should parse URI with UTF-8 userinfo");
    assert_eq!(params.username, "café");
    assert_eq!(params.password, "päss");
}

#[test]
fn test_uri_invalid_cases() {
    // Missing '://'
    assert!(ConnectParams::from_uri("not-a-uri").is_err());
    // Unsupported scheme
    assert!(ConnectParams::from_uri("mysql://user:pass@host:3306/db").is_err());
    // Missing INFORMIXSERVER
    assert!(ConnectParams::from_uri("informix+onsoctcp://user:pass@host:9088/db").is_err());
    // Missing port for onsoctcp
    assert!(ConnectParams::from_uri("informix+onsoctcp://user:pass@host/db?INFORMIXSERVER=srv").is_err());
}

#[test]
fn test_uri_onipcstr_full() {
    let uri = "informix+onipcstr:///customer_db?INFORMIXSERVER=ol_informix1_str";
    let params = ConnectParams::from_uri(uri).expect("Should parse onipcstr URI");
    assert_eq!(params.server, "ol_informix1_str");
    assert_eq!(params.database.as_deref(), Some("customer_db"));
    assert!(params.service.ends_with("ol_informix1_str.str") || params.service.contains("ol_informix1_str"));
}

#[test]
fn test_uri_onipcstr_custom_socket() {
    let uri = "informix+onipcstr:///db?INFORMIXSERVER=s&SOCKET_PATH=/var/lib/informix/socket";
    let params = ConnectParams::from_uri(uri).expect("Should parse onipcstr custom socket");
    assert_eq!(params.service, "/var/lib/informix/socket");
    assert_eq!(params.server, "s");
}

#[test]
fn test_uri_onipcstr_explicit_user() {
    let uri = "informix+onipcstr://custom_user@/db?INFORMIXSERVER=srv1";
    let params = ConnectParams::from_uri(uri).expect("Should parse explicit user");
    assert_eq!(params.username, "custom_user");
    assert_eq!(params.server, "srv1");
}
