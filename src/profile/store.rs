//! Profile store parsing, serialization, and management (~/.config/sqlicon/profiles.conf).

use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};
use crate::profile::crypto::{decrypt_profile_secret, encrypt_profile_secret};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub name: String,
    pub host: Option<String>,
    pub port: Option<String>,
    pub server: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub client_locale: Option<String>,
    pub db_locale: Option<String>,
}

impl Profile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn to_connect_params(&self) -> Result<ConnectParams> {
        let host = self.host.as_deref().unwrap_or("127.0.0.1");
        let port = self.port.as_deref().unwrap_or("9088");
        let server = self.server.as_deref().ok_or_else(|| {
            Error::InvalidArgument(format!("Profile '{}' has no server specified", self.name))
        })?;

        let mut params = ConnectParams::new(host, port, server);
        if let Some(ref db) = self.database {
            params = params.database(db);
        }
        if let Some(ref user) = self.user {
            let pass = self.password.as_deref().unwrap_or("");
            params = params.credentials(user, pass);
        }
        let cl = self.client_locale.as_deref().unwrap_or("en_US.utf8");
        let dl = self.db_locale.as_deref().unwrap_or("en_US.8859-1");
        params = params.locales(cl, dl);

        Ok(params)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileStore {
    pub default_name: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
}

impl ProfileStore {
    pub fn config_dir() -> Result<PathBuf> {
        #[cfg(windows)]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                if !appdata.is_empty() {
                    return Ok(PathBuf::from(appdata).join("sqlicon"));
                }
            }
            if let Ok(userprofile) = std::env::var("USERPROFILE") {
                if !userprofile.is_empty() {
                    return Ok(PathBuf::from(userprofile).join("AppData").join("Roaming").join("sqlicon"));
                }
            }
        }
        let home = std::env::var("HOME").map_err(|_| {
            Error::Configuration("HOME (or APPDATA) environment variable not set".into())
        })?;
        Ok(PathBuf::from(home).join(".config").join("sqlicon"))
    }


    pub fn config_file() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("profiles.conf"))
    }

    pub fn load() -> Result<Self> {
        let file_path = Self::config_file()?;
        if !file_path.exists() {
            return Ok(Self::default());
        }
        Self::load_from_path(&file_path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).map_err(Error::Io)?;
        let mut store = Self::default();

        let mut version = 1;
        let mut kdf_method = String::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(val) = trimmed.strip_prefix("version=") {
                if let Ok(v) = val.parse::<i32>() {
                    version = v;
                }
                continue;
            }
            if let Some(val) = trimmed.strip_prefix("kdf_method=") {
                kdf_method = val.to_string();
                continue;
            }
            if let Some(val) = trimmed.strip_prefix("default=") {
                if !val.is_empty() {
                    store.default_name = Some(val.to_string());
                }
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("profile.") {
                let Some(eq_pos) = rest.find('=') else { continue };
                let (key, value) = (&rest[..eq_pos], &rest[eq_pos + 1..]);
                let Some(dot_pos) = key.rfind('.') else { continue };
                let (profile_name, field) = (&key[..dot_pos], &key[dot_pos + 1..]);

                let profile = store
                    .profiles
                    .entry(profile_name.to_string())
                    .or_insert_with(|| Profile::new(profile_name));

                match field {
                    "host" => profile.host = Some(value.to_string()),
                    "port" => profile.port = Some(value.to_string()),
                    "server" => profile.server = Some(value.to_string()),
                    "database" => profile.database = Some(value.to_string()),
                    "user" => profile.user = Some(value.to_string()),
                    "password" => profile.password = Some(value.to_string()),
                    "password_enc" => {
                        if version >= 2 && !kdf_method.is_empty() && kdf_method != "uuid_uid_v1" {
                            // unsupported kdf
                        } else if let Ok(plain) = decrypt_profile_secret(value) {
                            profile.password = Some(plain);
                        }
                    }
                    "client_locale" => profile.client_locale = Some(value.to_string()),
                    "db_locale" => profile.db_locale = Some(value.to_string()),
                    _ => {}
                }
            }
        }

        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir).map_err(Error::Io)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }

        let file_path = Self::config_file()?;
        self.save_to_path(&file_path)
    }

    pub fn save_to_path(&self, file_path: &Path) -> Result<()> {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(Error::Io)?;
        }

        let mut out = String::new();
        out.push_str("version=2\n");
        out.push_str("kdf_method=uuid_uid_v1\n");
        out.push_str("enc_scheme=aes-256-gcm\n");
        out.push_str("enc_fields=password\n");
        out.push_str(&format!("default={}\n", self.default_name.as_deref().unwrap_or("")));

        for (name, p) in &self.profiles {
            if let Some(ref v) = p.host {
                out.push_str(&format!("profile.{name}.host={v}\n"));
            }
            if let Some(ref v) = p.port {
                out.push_str(&format!("profile.{name}.port={v}\n"));
            }
            if let Some(ref v) = p.server {
                out.push_str(&format!("profile.{name}.server={v}\n"));
            }
            if let Some(ref v) = p.database {
                out.push_str(&format!("profile.{name}.database={v}\n"));
            }
            if let Some(ref v) = p.user {
                out.push_str(&format!("profile.{name}.user={v}\n"));
            }
            if let Some(ref v) = p.password {
                if let Ok(enc) = encrypt_profile_secret(v) {
                    out.push_str(&format!("profile.{name}.password_enc={enc}\n"));
                }
            }
            if let Some(ref v) = p.client_locale {
                out.push_str(&format!("profile.{name}.client_locale={v}\n"));
            }
            if let Some(ref v) = p.db_locale {
                out.push_str(&format!("profile.{name}.db_locale={v}\n"));
            }
        }

        fs::write(file_path, out).map_err(Error::Io)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(file_path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    pub fn get_default_or(&self, name: Option<&str>) -> Option<&Profile> {
        if let Some(n) = name {
            self.get(n)
        } else if let Some(ref def) = self.default_name {
            self.get(def)
        } else {
            None
        }
    }
}
