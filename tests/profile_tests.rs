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

#[test]
fn test_profile_corrupt_encrypted_secret_surfaces_error_not_silent_none() {
    let temp_dir = std::env::temp_dir().join(format!("sqlicon_test_corrupt_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).unwrap();
    let config_file = temp_dir.join("profiles.conf");

    // A profile whose password_enc value is garbage (as if the profile were
    // copied to a machine with a different /etc/machine-id, or the file was
    // corrupted). This must NOT be silently treated as "no password stored".
    let contents = "version=2\n\
kdf_method=uuid_uid_v1\n\
default=broken\n\
profile.broken.host=127.0.0.1\n\
profile.broken.server=ol_test\n\
profile.broken.user=informix\n\
profile.broken.password_enc=not-valid-base64-ciphertext\n";
    fs::write(&config_file, contents).unwrap();

    let loaded = ProfileStore::load_from_path(&config_file).expect("load should still succeed");
    let prof = loaded.get("broken").expect("profile not found");

    assert!(prof.password.is_none());
    assert!(
        prof.password_decrypt_error.is_some(),
        "decrypt failure must be surfaced, not silently swallowed"
    );

    // to_connect_params() must error instead of silently connecting with an
    // empty/missing password.
    assert!(prof.to_connect_params().is_err());

    let _ = fs::remove_dir_all(&temp_dir);
}
