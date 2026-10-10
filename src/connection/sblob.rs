//! Smart Large Object (SBLOB) handle types and the `ifx_lo_create` fastpath request layout.
//!
//! The wire protocol itself (`SQ_GETROUTINE`/`SQ_EXFPROUTINE`/`SQ_FPROUTINE`) is executed by
//! [`Connection::sblob_create`](crate::Connection::sblob_create); this module holds the
//! caller-facing option/handle types and the byte-exact 596-byte `ifx_lo_spec` descriptor,
//! mirroring `libsqli`'s `sqli_sblob.c`.

use crate::error::{Error, Result};

/// Maximum locator length accepted from an `SQ_FPROUTINE` output parameter
/// (`SQLI_SBLOB_LOCATOR_MAX` in `sqli_sblob_internal.h`).
pub const SBLOB_LOCATOR_MAX: usize = 72;

/// Size of the `ifx_lo_spec` UDT transmitted as fastpath parameter 1.
pub const IFX_LO_SPEC_SIZE: usize = 596;

/// Smart large object type (`sqli_sblob_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SblobType {
    Blob,
    Clob,
}

/// Open modes for a smart large object descriptor. Mode `0` in [`SblobOptions`] means the
/// default (`WriteOnly`), matching `libsqli`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SblobOpenMode {
    Append = 1,
    WriteOnly = 2,
    ReadOnly = 4,
    ReadWrite = 8,
}

/// Creation options for [`Connection::sblob_create`](crate::Connection::sblob_create)
/// (`sqli_sblob_options` in `libsqli`). All size fields use `-1` for "unset"; `open_mode`
/// `None` selects the default write-only mode.
#[derive(Debug, Clone)]
pub struct SblobOptions {
    /// Open mode; `None` selects the default (`WriteOnly`), matching `libsqli`'s `0`.
    pub open_mode: Option<SblobOpenMode>,
    /// Storage space name; at most 127 characters.
    pub sbspace: Option<String>,
    /// Estimated size in bytes (`-1` = unset).
    pub estimated_bytes: i64,
    /// Maximum size in bytes (`-1` = unset).
    pub maximum_bytes: i64,
    /// Extent size in KiB (`-1` = unset).
    pub extent_kib: i32,
    /// Opaque create flags transmitted verbatim into the spec header.
    pub create_flags: u32,
}

impl Default for SblobOptions {
    fn default() -> Self {
        Self {
            open_mode: None,
            sbspace: None,
            estimated_bytes: -1,
            maximum_bytes: -1,
            extent_kib: -1,
            create_flags: 0,
        }
    }
}

impl SblobOptions {
    /// Validates the option values exactly as `sqli_sblob_create` does, returning
    /// [`Error::InvalidArgument`] for each rejected field. `open_mode` needs no range
    /// check here: as an `Option<SblobOpenMode>` it cannot hold an invalid value.
    pub fn validate(&self) -> Result<()> {
        if let Some(sbspace) = &self.sbspace {
            if sbspace.len() >= 128 {
                return Err(Error::InvalidArgument(
                    "sbspace name exceeds maximum length (127)".into(),
                ));
            }
        }
        if self.estimated_bytes < -1 || self.maximum_bytes < -1 || self.extent_kib < -1 {
            return Err(Error::InvalidArgument(
                "smart large object size options must be >= -1".into(),
            ));
        }
        Ok(())
    }

    /// Effective open mode: `None` maps to the default `WriteOnly`, as in `libsqli`.
    pub fn effective_mode(&self) -> SblobOpenMode {
        self.open_mode.unwrap_or(SblobOpenMode::WriteOnly)
    }
}

/// A created smart large object handle (`sqli_sblob_t`): the server-assigned descriptor
/// handle (`lofd`) plus the locator bytes usable for binding (`ParameterValue::Sblob`) or
/// hex rendering. The handle is closed with [`Connection::sblob_close`](crate::Connection::sblob_close).
#[derive(Debug, Clone)]
pub struct SblobHandle {
    pub lofd: i32,
    pub sblob_type: SblobType,
    pub locator: Vec<u8>,
}

impl SblobHandle {
    /// Locator rendered as the hex string the SQL `ifx_lo_*` functions accept.
    pub fn locator_hex(&self) -> String {
        let mut hex = String::with_capacity(self.locator.len() * 2);
        for &b in &self.locator {
            use std::fmt::Write;
            let _ = write!(hex, "{b:02x}");
        }
        hex
    }
}

/// Builds the byte-exact 596-byte `ifx_lo_spec` UDT payload (`sqli_sblob.c`):
/// magic `0xdadafeed`, BE create flags, sign-magnitude estimated/maximum sizes, BE extent,
/// sbspace name, and the raw column descriptor region with offsets 404..411 set to `0xFF`.
pub(crate) fn build_ifx_lo_spec(options: &SblobOptions) -> [u8; IFX_LO_SPEC_SIZE] {
    let mut spec = [0u8; IFX_LO_SPEC_SIZE];
    spec[0] = 0xda;
    spec[1] = 0xda;
    spec[2] = 0xfe;
    spec[3] = 0xed;

    let flags = options.create_flags;
    spec[4] = ((flags >> 24) & 0xFF) as u8;
    spec[5] = ((flags >> 16) & 0xFF) as u8;
    spec[6] = ((flags >> 8) & 0xFF) as u8;
    spec[7] = (flags & 0xFF) as u8;

    write_long_sign_mag(&mut spec[12..24], options.estimated_bytes.max(-1));
    write_long_sign_mag(&mut spec[24..36], options.maximum_bytes.max(-1));

    let uext = options.extent_kib.max(-1) as u32;
    spec[36] = ((uext >> 24) & 0xFF) as u8;
    spec[37] = ((uext >> 16) & 0xFF) as u8;
    spec[38] = ((uext >> 8) & 0xFF) as u8;
    spec[39] = (uext & 0xFF) as u8;

    if let Some(sbspace) = &self_sbspace(options) {
        let bytes = sbspace.as_bytes();
        let len = bytes.len().min(127);
        spec[40..40 + len].copy_from_slice(&bytes[..len]);
    }

    // rawColDescriptor (428 bytes starting at offset 168): offsets 404..411 are -1.
    for k in 404..=411 {
        spec[168 + k] = 0xFF;
    }

    spec
}

fn self_sbspace(options: &SblobOptions) -> Option<&String> {
    options.sbspace.as_ref().filter(|s| !s.is_empty())
}

/// Informix sign-magnitude 8-byte integer encoding (`write_long_sign_mag` in `sqli_sblob.c`):
/// low 32 bits big-endian, high 32 bits big-endian, then a 2-byte sign (`0x0001`/`0xFFFF`)
/// and two zero bytes.
fn write_long_sign_mag(dest: &mut [u8], val: i64) {
    let sign: u16 = if val < 0 { 0xFFFF } else { 0x0001 };
    let mag = val.unsigned_abs();
    let low32 = (mag & 0xFFFF_FFFF) as u32;
    let high32 = (mag >> 32) as u32;
    dest[0] = ((low32 >> 24) & 0xFF) as u8;
    dest[1] = ((low32 >> 16) & 0xFF) as u8;
    dest[2] = ((low32 >> 8) & 0xFF) as u8;
    dest[3] = (low32 & 0xFF) as u8;
    dest[4] = ((high32 >> 24) & 0xFF) as u8;
    dest[5] = ((high32 >> 16) & 0xFF) as u8;
    dest[6] = ((high32 >> 8) & 0xFF) as u8;
    dest[7] = (high32 & 0xFF) as u8;
    dest[8] = ((sign >> 8) & 0xFF) as u8;
    dest[9] = (sign & 0xFF) as u8;
    dest[10] = 0;
    dest[11] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ifx_lo_spec_default_layout() {
        let spec = build_ifx_lo_spec(&SblobOptions::default());
        assert_eq!(&spec[0..4], &[0xda, 0xda, 0xfe, 0xed]);
        assert_eq!(&spec[4..8], &[0, 0, 0, 0]);
        // Unset sizes encode as sign-magnitude -1: magnitude 1, sign 0xFFFF.
        assert_eq!(&spec[12..24], &[0, 0, 0, 1, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0]);
        assert_eq!(&spec[24..36], &[0, 0, 0, 1, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0]);
        assert_eq!(&spec[36..40], &[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(spec[40..168].iter().all(|&b| b == 0));
        assert!(spec[168..596].iter().enumerate().all(|(i, &b)| {
            if (404..=411).contains(&i) { b == 0xFF } else { b == 0 }
        }));
    }

    #[test]
    fn ifx_lo_spec_option_values() {
        let opts = SblobOptions {
            open_mode: Some(SblobOpenMode::ReadOnly),
            sbspace: Some("bspace1".to_string()),
            estimated_bytes: 1000,
            maximum_bytes: 0x1_0000_0000,
            extent_kib: 64,
            create_flags: 0x1234_5678,
        };
        opts.validate().unwrap();
        assert_eq!(opts.effective_mode(), SblobOpenMode::ReadOnly);

        let spec = build_ifx_lo_spec(&opts);
        assert_eq!(&spec[4..8], &[0x12, 0x34, 0x56, 0x78]);
        // 1000: low32 BE, high32 0, positive sign.
        assert_eq!(&spec[12..24], &[0, 0, 0x03, 0xE8, 0, 0, 0, 0, 0x00, 0x01, 0, 0]);
        // 0x1_0000_0000: low32 0, high32 1.
        assert_eq!(&spec[24..36], &[0, 0, 0, 0, 0, 0, 0, 1, 0x00, 0x01, 0, 0]);
        assert_eq!(&spec[36..40], &[0, 0, 0, 64]);
        assert_eq!(&spec[40..47], b"bspace1");
        assert_eq!(spec[47], 0);
    }

    #[test]
    fn option_validation_rejects_out_of_range_values() {
        // open_mode has no out-of-range case anymore: Option<SblobOpenMode> cannot
        // represent an invalid value, so there's nothing left to reject here.
        let mut opts = SblobOptions::default();
        opts.sbspace = Some("x".repeat(128));
        assert!(matches!(opts.validate(), Err(Error::InvalidArgument(_))));

        let mut opts = SblobOptions::default();
        opts.extent_kib = -2;
        assert!(matches!(opts.validate(), Err(Error::InvalidArgument(_))));

        assert!(SblobOptions::default().validate().is_ok());
        assert_eq!(SblobOptions::default().effective_mode(), SblobOpenMode::WriteOnly);
    }
}
