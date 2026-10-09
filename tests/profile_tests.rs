use sqlicon_rs::profile::crypto::{decrypt_profile_secret, encrypt_profile_secret};
use sqlicon_rs::profile::store::{Profile, ProfileStore};
use std::fs;

#[test]
fn test_profile_secret_encryption_roundtrip() {
    let password = "super_secret_db_password_123!";
    let encrypted = encrypt_profile_secret(password).expect("encryption failed");
    assert_ne!(encrypted, password);

    let decrypted = decrypt_profile_secret(&encrypted).expect("decryption failed");
    assert_eq!(decrypted, password);
}

#[test]
fn test_profile_crud_and_store_serialization() {
    let temp_dir = std::env::temp_dir().join(format!("sqlicon_test_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let config_file = temp_dir.join("profiles.conf");

    let mut store = ProfileStore::default();
    let mut prof = Profile::new("test_db");
    prof.host = Some("127.0.0.1".into());
    prof.port = Some("9088".into());
    prof.server = Some("ol_test".into());
    prof.database = Some("stores".into());
    prof.user = Some("informix".into());
    prof.password = Some("testpass".into());

    store.profiles.insert("test_db".into(), prof);
    store.default_name = Some("test_db".into());

    store.save_to_path(&config_file).expect("save failed");

    // Reload and verify
    let loaded = ProfileStore::load_from_path(&config_file).expect("load failed");
    assert_eq!(loaded.default_name, Some("test_db".into()));
    let loaded_prof = loaded.get("test_db").expect("profile test_db not found");
    assert_eq!(loaded_prof.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(loaded_prof.port.as_deref(), Some("9088"));
    assert_eq!(loaded_prof.server.as_deref(), Some("ol_test"));
    assert_eq!(loaded_prof.database.as_deref(), Some("stores"));
    assert_eq!(loaded_prof.user.as_deref(), Some("informix"));
    assert_eq!(loaded_prof.password.as_deref(), Some("testpass"));

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_profile_to_connect_params() {
    let mut prof = Profile::new("dev_profile");
    prof.host = Some("10.0.0.1".into());
    prof.port = Some("9089".into());
    prof.server = Some("ol_iep".into());
    prof.database = Some("unorech".into());
    prof.user = Some("admin".into());
    prof.password = Some("admin".into());
    prof.client_locale = Some("de_DE.utf8".into());
    prof.db_locale = Some("de_DE.1252".into());

    let params = prof.to_connect_params().expect("connect params conversion");
    assert_eq!(params.hostname, "10.0.0.1");
    assert_eq!(params.service, "9089");
    assert_eq!(params.server, "ol_iep");
    assert_eq!(params.database.as_deref(), Some("unorech"));
    assert_eq!(params.username, "admin");
    assert_eq!(params.password, "admin");
    assert_eq!(params.client_locale, "de_DE.utf8");
    assert_eq!(params.db_locale, "de_DE.1252");
}
