use sqlicon_rs::lookup_error_message;

#[test]
fn test_catalog_lookup_standard_codes() {
    assert_eq!(
        lookup_error_message(-201).as_deref(),
        Some("A syntax error has occurred.")
    );
    assert_eq!(
        lookup_error_message(-206).as_deref(),
        Some("The specified table (%s) is not in the database.")
    );
    assert_eq!(
        lookup_error_message(-329).as_deref(),
        Some("Database not found or no system permission.")
    );
    assert_eq!(
        lookup_error_message(-908).as_deref(),
        Some("Attempt to connect to database server (%s) failed.")
    );
    assert!(lookup_error_message(100).unwrap().contains("duplicate value"));
    assert_eq!(
        lookup_error_message(107).as_deref(),
        Some("ISAM error:  record is locked.")
    );
    assert_eq!(
        lookup_error_message(-213).as_deref(),
        Some("Statement timed out or interrupted by user.")
    );
    assert_eq!(
        lookup_error_message(-23197).as_deref(),
        Some("Database locale information mismatch.")
    );
    assert_eq!(lookup_error_message(-999999), None);
}

