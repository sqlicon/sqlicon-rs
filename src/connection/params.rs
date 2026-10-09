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
}
