//! Connection parameters for Informix database connections.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub hostname: String,
    pub service: String, // port or service name, e.g. "9088" or "/tmp/ol_informix1210.sock"
    pub server: String,
    pub database: Option<String>,
    pub username: String,
    pub password: String,
    pub client_locale: String,
    pub db_locale: String,
    pub fetch_buf_size: u32,
}

impl Default for ConnectParams {
    fn default() -> Self {
        Self {
            hostname: "127.0.0.1".into(),
            service: "9088".into(),
            server: "ol_informix1210".into(),
            database: None,
            username: "informix".into(),
            password: "".into(),
            client_locale: "en_US.utf8".into(),
            db_locale: "en_US.8859-1".into(),
            fetch_buf_size: 4 * 1024 * 1024,
        }
    }
}

impl ConnectParams {
    pub fn new<H: Into<String>, P: Into<String>, S: Into<String>>(
        hostname: H,
        service: P,
        server: S,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            service: service.into(),
            server: server.into(),
            ..Default::default()
        }
    }

    pub fn database<D: Into<String>>(mut self, db: D) -> Self {
        self.database = Some(db.into());
        self
    }

    pub fn credentials<U: Into<String>, P: Into<String>>(mut self, user: U, pass: P) -> Self {
        self.username = user.into();
        self.password = pass.into();
        self
    }

    pub fn locales<C: Into<String>, D: Into<String>>(mut self, client: C, db: D) -> Self {
        self.client_locale = client.into();
        self.db_locale = db.into();
        self
    }

    /// Parse connection parameters from an Informix connection URI:
    /// e.g. `informix+onsoctcp://user:pass@host:port/dbname?INFORMIXSERVER=server&DB_LOCALE=de_DE.1252`
    pub fn from_uri(uri: &str) -> crate::error::Result<Self> {
        let colon_slash = uri.find("://").ok_or_else(|| {
            crate::error::Error::InvalidArgument("Invalid URI: missing '://'".into())
        })?;

        let scheme = &uri[..colon_slash];
        if scheme != "informix+onsoctcp" && scheme != "informix+onsocssl" && scheme != "informix+onipcstr" {
            return Err(crate::error::Error::InvalidArgument(format!(
                "Unsupported URI scheme '{scheme}': must be informix+onsoctcp, informix+onsocssl, or informix+onipcstr"
            )));
        }

        let rest = &uri[colon_slash + 3..];
        let (auth_and_path, query_str) = match rest.find('?') {
            Some(idx) => (&rest[..idx], Some(&rest[idx + 1..])),
            None => (rest, None),
        };

        let (authority, path) = match auth_and_path.find('/') {
            Some(idx) => (&auth_and_path[..idx], &auth_and_path[idx + 1..]),
            None => (auth_and_path, ""),
        };

        let mut username = String::new();
        let mut password = String::new();
        let host_port = match authority.rfind('@') {
            Some(idx) => {
                let userinfo = &authority[..idx];
                if let Some(c) = userinfo.find(':') {
                    username = percent_decode(&userinfo[..c]);
                    password = percent_decode(&userinfo[c + 1..]);
                } else {
                    username = percent_decode(userinfo);
                }
                &authority[idx + 1..]
            }
            None => authority,
        };

        let (hostname, service) = if scheme == "informix+onipcstr" {
            ("".to_string(), "".to_string())
        } else {
            let (h, s) = match host_port.find(':') {
                Some(idx) => (&host_port[..idx], &host_port[idx + 1..]),
                None => {
                    return Err(crate::error::Error::InvalidArgument(
                        "Port is required for onsoctcp/onsocssl URI".into(),
                    ));
                }
            };
            if h.is_empty() {
                return Err(crate::error::Error::InvalidArgument(
                    "Hostname is required for onsoctcp/onsocssl URI".into(),
                ));
            }
            (h.to_string(), s.to_string())
        };

        let mut server = String::new();
        let mut client_locale = "en_US.utf8".to_string();
        let mut db_locale = "en_US.8859-1".to_string();
        let mut socket_path = String::new();

        if let Some(q) = query_str {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (k, v) = match pair.find('=') {
                    Some(idx) => (&pair[..idx], percent_decode(&pair[idx + 1..])),
                    None => (pair, String::new()),
                };
                match k.to_ascii_uppercase().as_str() {
                    "INFORMIXSERVER" | "SERVER" => server = v,
                    "CLIENT_LOCALE" => client_locale = v,
                    "DB_LOCALE" => db_locale = v,
                    "SOCKET_PATH" => socket_path = v,
                    _ => {}
                }
            }
        }

        if server.is_empty() {
            return Err(crate::error::Error::InvalidArgument(
                "INFORMIXSERVER query parameter is mandatory in connection URI".into(),
            ));
        }

        let database = if path.is_empty() {
            Some("informix".to_string())
        } else {
            Some(path.to_string())
        };

        let service = if scheme == "informix+onipcstr" {
            if !socket_path.is_empty() {
                socket_path
            } else if let Ok(tmp) = std::env::var("INFORMIXTMP") {
                format!("{tmp}/{server}.str")
            } else if std::path::Path::new("/INFORMIXTMP").exists() {
                format!("/INFORMIXTMP/{server}.str")
            } else {
                format!("/tmp/.infosock.{server}")
            }
        } else {
            service
        };

        let username = if scheme == "informix+onipcstr" && username.is_empty() {
            #[cfg(unix)]
            {
                let uid = unsafe { libc::getuid() };
                let pw = unsafe { libc::getpwuid(uid) };
                if !pw.is_null() {
                    let c_str = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) };
                    c_str.to_string_lossy().to_string()
                } else {
                    username
                }
            }
            #[cfg(not(unix))]
            {
                username
            }
        } else {
            username
        };

        Ok(ConnectParams {
            hostname,
            service,
            server,
            database,
            username,
            password,
            client_locale,
            db_locale,
            fetch_buf_size: 4 * 1024 * 1024,
        })
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                let hex_str = [c1, c2];
                if let Ok(hex_s) = std::str::from_utf8(&hex_str) {
                    if let Ok(val) = u8::from_str_radix(hex_s, 16) {
                        out.push(val);
                        continue;
                    }
                }
            }
            out.push(b'%');
        } else if b == b'+' {
            out.push(b' ');
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
