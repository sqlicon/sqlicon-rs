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
