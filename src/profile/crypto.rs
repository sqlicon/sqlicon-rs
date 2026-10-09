//! Cryptographic operations for sqlicon profile passwords (AES-256-GCM / SHA-256).

use crate::error::{Error, Result};
use aes_gcm::aead::{Aead, KeyInit, Nonce, OsRng};
use aes_gcm::Aes256Gcm;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::Path;


#[cfg(unix)]
fn get_platform_key_material() -> Result<String> {
    for path in [
        "/etc/machine-id",
        "/var/lib/dbus/machine-id",
        "/sys/class/dmi/id/product_uuid",
    ] {
        if Path::new(path).exists() {
            if let Ok(content) = fs::read_to_string(path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    let uid = unsafe { libc::getuid() };
                    return Ok(format!("{trimmed}:{uid}"));
                }
            }
        }
    }
    Err(Error::Configuration(
        "Cannot determine machine identifier for profile encryption".into(),
    ))
}

#[cfg(windows)]
fn get_platform_key_material() -> Result<String> {
    // 1. Read MachineGuid from registry via PowerShell or env/registry if possible
    // Try registry via command or standard environment fallbacks
    use std::process::Command;
    let output = Command::new("reg")
        .args(["query", "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography", "/v", "MachineGuid"])
        .output();

    let guid = match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            s.lines()
                .find(|l| l.contains("MachineGuid"))
                .and_then(|l| l.split_whitespace().last())
                .unwrap_or("")
                .trim()
                .to_string()
        }
        _ => String::new(),
    };

    let user = std::env::var("USERNAME").unwrap_or_else(|_| "krm".to_string());
    if !guid.is_empty() {
        Ok(format!("{guid}:{user}"))
    } else {
        Err(Error::Configuration(
            "Cannot determine MachineGuid for profile encryption on Windows".into(),
        ))
    }
}

pub fn derive_profile_key() -> Result<[u8; 32]> {
    let material = get_platform_key_material()?;

    let mut hasher = Sha256::new();
    hasher.update(b"sqlicon:kdf:uuid_uid_v1:");
    hasher.update(material.as_bytes());
    let res = hasher.finalize();

    let mut key = [0u8; 32];
    key.copy_from_slice(&res);
    Ok(key)
}


/// Decrypts a base64-encoded profile password formatted as `[nonce: 12][tag: 16][ciphertext...]`.
pub fn decrypt_profile_secret(b64: &str) -> Result<String> {
    if b64.is_empty() {
        return Ok(String::new());
    }

    let packed = BASE64
        .decode(b64)
        .map_err(|e| Error::Protocol(format!("Invalid base64 in profile password: {e}")))?;

    if packed.len() < 28 {
        return Err(Error::Protocol("Profile secret payload too short".into()));
    }

    let nonce_bytes = &packed[0..12];
    let tag_bytes = &packed[12..28];
    let cipher_bytes = &packed[28..];

    // Aes256Gcm in the Rust `aead` crate expects `[ciphertext..., tag...]`
    let mut combined = Vec::with_capacity(cipher_bytes.len() + tag_bytes.len());
    combined.extend_from_slice(cipher_bytes);
    combined.extend_from_slice(tag_bytes);

    let key = derive_profile_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| Error::Protocol(format!("Failed to initialize AES-GCM cipher: {e}")))?;
    let nonce = Nonce::<Aes256Gcm>::from_slice(nonce_bytes);

    let plain_bytes = cipher
        .decrypt(nonce, combined.as_ref())
        .map_err(|_| Error::Authentication("Failed to decrypt profile password".into()))?;

    String::from_utf8(plain_bytes)
        .map_err(|e| Error::Protocol(format!("Decrypted password is not valid UTF-8: {e}")))
}

/// Encrypts a plaintext password formatted as `[nonce: 12][tag: 16][ciphertext...]` in base64.
pub fn encrypt_profile_secret(plain: &str) -> Result<String> {
    if plain.is_empty() {
        return Ok(String::new());
    }

    use aes_gcm::aead::rand_core::RngCore;

    let key = derive_profile_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| Error::Protocol(format!("Failed to initialize AES-GCM cipher: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::<Aes256Gcm>::from_slice(&nonce_bytes);

    // `encrypt` produces `[ciphertext..., tag: 16]`
    let encrypted = cipher
        .encrypt(nonce, plain.as_bytes())
        .map_err(|e| Error::Protocol(format!("Failed to encrypt password: {e}")))?;

    if encrypted.len() < 16 {
        return Err(Error::Protocol("Encrypted payload too short".into()));
    }

    let split_pos = encrypted.len() - 16;
    let cipher_bytes = &encrypted[..split_pos];
    let tag_bytes = &encrypted[split_pos..];

    // Pack as: [nonce: 12][tag: 16][ciphertext...]
    let mut packed = Vec::with_capacity(12 + 16 + cipher_bytes.len());
    packed.extend_from_slice(&nonce_bytes);
    packed.extend_from_slice(tag_bytes);
    packed.extend_from_slice(cipher_bytes);

    Ok(BASE64.encode(packed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_decrypt_live_vdademo_profile() {
        // From ~/.config/sqlicon/profiles.conf:
        let enc = "2SniQtRpEk1dITZPrBCuBlJq5I2wAJbtutVkhGUTNlk/";
        let plain = decrypt_profile_secret(enc).unwrap();
        assert_eq!(plain, "admin");
    }


    #[test]
    fn test_roundtrip_encryption() {
        let secret = "SuperSecretPassword123!";
        let enc = encrypt_profile_secret(secret).unwrap();
        let dec = decrypt_profile_secret(&enc).unwrap();
        assert_eq!(dec, secret);
    }
}
