//! SQLI protocol constants and wire flags.

/// Session Layer (SL) header size in bytes.
pub const SL_HEADER_SIZE: usize = 6;

/// Protocol family ID for SQLI.
pub const SL_PROT_SQLI: u8 = 60;

/// SL message types.
pub const SL_TYPE_CONREQ: u8 = 1;
pub const SL_TYPE_CONACC: u8 = 2;
pub const SL_TYPE_CONREJ: u8 = 3;
pub const SL_TYPE_REDIRECT: u8 = 13;

/// ASC-BINARY wire tags.
pub const TAG_ASC_BINARY: u16 = 100;
pub const TAG_VERSION_CAPS: u16 = 101;
pub const TAG_STMT_OPTIONS: u16 = 104;
pub const TAG_ENV_VARS: u16 = 106;
pub const TAG_ENV_INFO: u16 = 107;
pub const TAG_PRODUCT_TYPE: u16 = 108;
pub const TAG_APP_NAME: u16 = 116;
pub const TAG_END: u16 = 127;

/// SQLI protocol versions and client capabilities.
pub const SQLI_PROTO_VERSION: u32 = 61;
pub const SQLI_CLIENT_CAP_1: i32 = 316;

/// Standard environment variables forwarded during handshake.
pub const PRIMARY_ENV_VARS: &[&str] = &[
    "INFORMIXSERVER",
    "CLIENT_LOCALE",
    "DB_LOCALE",
    "DBLANG",
    "DBCENTURY",
    "GL_DATE",
    "GL_DATETIME",
    "DBDATE",
    "DBMONEY",
    "DBTIME",
    "DBFLTMASK",
    "FET_BUF_SIZE",
];

pub const SECONDARY_ENV_VARS: &[&str] = &[
    "IFX_EXTDIRECTIVES",
    "IFX_USE_STKT",
];
