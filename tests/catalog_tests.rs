use sqlicon_rs::lookup_error_message;

#[test]
fn test_catalog_lookup_standard_codes() {
    assert_eq!(
        lookup_error_message(-201),
        Some("A syntax error has occurred near position: %s.")
    );
    assert_eq!(
        lookup_error_message(-206),
        Some("The specified table (%s) is not in the database.")
    );
    assert_eq!(
        lookup_error_message(-329),
        Some("Database (%s) not found or no system permission.")
    );
    assert_eq!(
        lookup_error_message(-908),
        Some("Socket connection to server (%s) failed. Check your server is reachable from this client on the host:port specified.")
    );
    assert_eq!(
        lookup_error_message(100),
        Some("ISAM error: duplicate value for a record with unique key.")
    );
    assert_eq!(
        lookup_error_message(107),
        Some("ISAM error: record is locked.")
    );
    assert_eq!(lookup_error_message(-999999), None);
}
