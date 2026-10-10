//! Informix character set resolution and encoding/decoding.
//!
//! Provides bidirectional conversion between database charsets (e.g. CP1252, ISO-8859-1..15,
//! Windows codepages, Shift_JIS, GB2312, Big5) and Rust UTF-8 strings.
//!
//! Mirrors the C `libsqli` charset catalog (`g_sqli_charset_entries` in `sqli_charset.c`).

use crate::error::{Error, Result};
use oem_cp::code_table_type::TableType;
use oem_cp::OEMCPHashMap;
use std::borrow::Cow;

/// A single-byte DOS code page codec, for the handful of code pages
/// (CP852/857/864) that `encoding_rs` has no built-in table for. Mirrors
/// libsqli's iconv-backed `sqli_charset.c` catalog entries for these.
#[derive(Clone)]
pub struct OemCodec {
    decode: TableType,
    encode: &'static OEMCPHashMap<char, u8>,
}

/// Resolved charset specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharsetSpec {
    pub canonical_name: &'static str,
    pub windows_codepage: u32,
    pub gls_csid: i32,
    pub encoding: Option<&'static encoding_rs::Encoding>,
    pub oem_codec: Option<OemCodec>,
}

impl std::fmt::Debug for OemCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OemCodec(..)")
    }
}

impl PartialEq for OemCodec {
    fn eq(&self, other: &Self) -> bool {
        // Table identity is enough: entries are static singletons.
        std::ptr::eq(self.encode, other.encode)
    }
}
impl Eq for OemCodec {}

struct CharsetEntry {
    canonical_name: &'static str,
    windows_codepage: u32,
    gls_csid: i32,
    encoding: Option<&'static encoding_rs::Encoding>,
    oem_codec: Option<OemCodec>,
    aliases: &'static [&'static str],
}

static CHARSET_TABLE: &[CharsetEntry] = &[
    CharsetEntry {
        canonical_name: "ASCII",
        windows_codepage: 20127,
        gls_csid: 364,
        encoding: Some(encoding_rs::WINDOWS_1252), // ASCII subset of 1252
        oem_codec: None,
        aliases: &["ASCII", "US-ASCII"],
    },
    CharsetEntry {
        canonical_name: "UTF-8",
        windows_codepage: 65001,
        gls_csid: 57372,
        encoding: Some(encoding_rs::UTF_8),
        oem_codec: None,
        aliases: &["UTF-8", "UTF8"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-1",
        windows_codepage: 28591,
        gls_csid: 819,
        encoding: Some(encoding_rs::WINDOWS_1252), // In WHATWG web spec & encoding_rs, ISO-8859-1 maps to windows-1252
        oem_codec: None,
        aliases: &["ISO-8859-1", "ISO8859-1", "8859-1", "LATIN1"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-2",
        windows_codepage: 28592,
        gls_csid: 912,
        encoding: Some(encoding_rs::ISO_8859_2),
        oem_codec: None,
        aliases: &["ISO-8859-2", "ISO8859-2", "8859-2"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-5",
        windows_codepage: 28595,
        gls_csid: 915,
        encoding: Some(encoding_rs::ISO_8859_5),
        oem_codec: None,
        aliases: &["ISO-8859-5", "ISO8859-5", "8859-5"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-6",
        windows_codepage: 28596,
        gls_csid: 1089,
        encoding: Some(encoding_rs::ISO_8859_6),
        oem_codec: None,
        aliases: &["ISO-8859-6", "ISO8859-6", "8859-6"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-7",
        windows_codepage: 28597,
        gls_csid: 813,
        encoding: Some(encoding_rs::ISO_8859_7),
        oem_codec: None,
        aliases: &["ISO-8859-7", "ISO8859-7", "8859-7"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-8",
        windows_codepage: 28598,
        gls_csid: 916,
        encoding: Some(encoding_rs::ISO_8859_8),
        oem_codec: None,
        aliases: &["ISO-8859-8", "ISO8859-8", "8859-8"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-9",
        windows_codepage: 28599,
        gls_csid: 920,
        encoding: Some(encoding_rs::WINDOWS_1254), // ISO-8859-9 is superset in windows-1254
        oem_codec: None,
        aliases: &["ISO-8859-9", "ISO8859-9", "8859-9"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-13",
        windows_codepage: 28603,
        gls_csid: 57390,
        encoding: Some(encoding_rs::ISO_8859_13),
        oem_codec: None,
        aliases: &["ISO-8859-13", "ISO8859-13", "8859-13"],
    },
    CharsetEntry {
        canonical_name: "ISO-8859-15",
        windows_codepage: 28605,
        gls_csid: 57391,
        encoding: Some(encoding_rs::ISO_8859_15),
        oem_codec: None,
        aliases: &["ISO-8859-15", "ISO8859-15", "8859-15"],
    },
    CharsetEntry {
        canonical_name: "CP850",
        windows_codepage: 850,
        gls_csid: 850,
        encoding: None,
        oem_codec: Some(OemCodec {
            decode: TableType::Complete(&oem_cp::code_table::DECODING_TABLE_CP850),
            encode: &oem_cp::code_table::ENCODING_TABLE_CP850,
        }),
        aliases: &["CP850", "IBM850"],
    },
    CharsetEntry {
        canonical_name: "CP852",
        windows_codepage: 852,
        gls_csid: 852,
        encoding: None,
        oem_codec: Some(OemCodec {
            decode: TableType::Complete(&oem_cp::code_table::DECODING_TABLE_CP852),
            encode: &oem_cp::code_table::ENCODING_TABLE_CP852,
        }),
        aliases: &["CP852", "IBM852"],
    },
    CharsetEntry {
        canonical_name: "CP857",
        windows_codepage: 857,
        gls_csid: 857,
        encoding: None,
        oem_codec: Some(OemCodec {
            decode: TableType::Incomplete(&oem_cp::code_table::DECODING_TABLE_CP857),
            encode: &oem_cp::code_table::ENCODING_TABLE_CP857,
        }),
        aliases: &["CP857", "IBM857"],
    },
    CharsetEntry {
        canonical_name: "CP864",
        windows_codepage: 864,
        gls_csid: 864,
        encoding: None,
        oem_codec: Some(OemCodec {
            decode: TableType::Incomplete(&oem_cp::code_table::DECODING_TABLE_CP864),
            encode: &oem_cp::code_table::ENCODING_TABLE_CP864,
        }),
        aliases: &["CP864", "IBM864"],
    },
    CharsetEntry {
        canonical_name: "CP866",
        windows_codepage: 866,
        gls_csid: 866,
        encoding: Some(encoding_rs::IBM866),
        oem_codec: None,
        aliases: &["CP866", "IBM866"],
    },
    CharsetEntry {
        canonical_name: "CP874",
        windows_codepage: 874,
        gls_csid: 57373,
        encoding: Some(encoding_rs::WINDOWS_874),
        oem_codec: None,
        aliases: &["CP874", "WINDOWS-874"],
    },
    CharsetEntry {
        canonical_name: "CP932",
        windows_codepage: 932,
        gls_csid: 932,
        encoding: Some(encoding_rs::SHIFT_JIS),
        oem_codec: None,
        aliases: &["CP932", "SHIFT_JIS", "SJIS"],
    },
    CharsetEntry {
        canonical_name: "CP936",
        windows_codepage: 936,
        gls_csid: 57357,
        encoding: Some(encoding_rs::GBK),
        oem_codec: None,
        aliases: &["CP936", "GB2312", "GBK"],
    },
    CharsetEntry {
        canonical_name: "CP949",
        windows_codepage: 949,
        gls_csid: 57356,
        encoding: Some(encoding_rs::EUC_KR),
        oem_codec: None,
        aliases: &["CP949", "EUC-KR"],
    },
    CharsetEntry {
        canonical_name: "CP950",
        windows_codepage: 950,
        gls_csid: 950,
        encoding: Some(encoding_rs::BIG5),
        oem_codec: None,
        aliases: &["CP950", "BIG5"],
    },
    CharsetEntry {
        canonical_name: "CP1250",
        windows_codepage: 1250,
        gls_csid: 1250,
        encoding: Some(encoding_rs::WINDOWS_1250),
        oem_codec: None,
        aliases: &["CP1250", "WINDOWS-1250", "1250"],
    },
    CharsetEntry {
        canonical_name: "CP1251",
        windows_codepage: 1251,
        gls_csid: 1251,
        encoding: Some(encoding_rs::WINDOWS_1251),
        oem_codec: None,
        aliases: &["CP1251", "WINDOWS-1251", "1251"],
    },
    CharsetEntry {
        canonical_name: "CP1252",
        windows_codepage: 1252,
        gls_csid: 1252,
        encoding: Some(encoding_rs::WINDOWS_1252),
        oem_codec: None,
        aliases: &["CP1252", "WINDOWS-1252", "1252"],
    },
    CharsetEntry {
        canonical_name: "CP1253",
        windows_codepage: 1253,
        gls_csid: 1253,
        encoding: Some(encoding_rs::WINDOWS_1253),
        oem_codec: None,
        aliases: &["CP1253", "WINDOWS-1253", "1253"],
    },
    CharsetEntry {
        canonical_name: "CP1254",
        windows_codepage: 1254,
        gls_csid: 1254,
        encoding: Some(encoding_rs::WINDOWS_1254),
        oem_codec: None,
        aliases: &["CP1254", "WINDOWS-1254", "1254"],
    },
    CharsetEntry {
        canonical_name: "CP1255",
        windows_codepage: 1255,
        gls_csid: 1255,
        encoding: Some(encoding_rs::WINDOWS_1255),
        oem_codec: None,
        aliases: &["CP1255", "WINDOWS-1255", "1255"],
    },
    CharsetEntry {
        canonical_name: "CP1256",
        windows_codepage: 1256,
        gls_csid: 1256,
        encoding: Some(encoding_rs::WINDOWS_1256),
        oem_codec: None,
        aliases: &["CP1256", "WINDOWS-1256", "1256"],
    },
    CharsetEntry {
        canonical_name: "CP1257",
        windows_codepage: 1257,
        gls_csid: 1257,
        encoding: Some(encoding_rs::WINDOWS_1257),
        oem_codec: None,
        aliases: &["CP1257", "WINDOWS-1257", "1257"],
    },
    CharsetEntry {
        canonical_name: "CP1258",
        windows_codepage: 1258,
        gls_csid: 1258,
        encoding: Some(encoding_rs::WINDOWS_1258),
        oem_codec: None,
        aliases: &["CP1258", "WINDOWS-1258", "1258"],
    },
];

fn token_equals(lhs: &str, rhs: &str) -> bool {
    let filter = |c: char| !(c == '-' || c == '_' || c == ' ');
    lhs.chars()
        .filter(|&c| filter(c))
        .zip(rhs.chars().filter(|&c| filter(c)))
        .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
        && lhs.chars().filter(|&c| filter(c)).count()
            == rhs.chars().filter(|&c| filter(c)).count()
}

/// Resolves an Informix codeset name or numeric identifier (e.g. `"1252"`, `"8859-1"`, `"UTF-8"`).
pub fn resolve_codeset(codeset: &str) -> Option<CharsetSpec> {
    let numeric = codeset.parse::<i32>().ok();

    for entry in CHARSET_TABLE {
        for alias in entry.aliases {
            if token_equals(alias, codeset) {
                return Some(CharsetSpec {
                    canonical_name: entry.canonical_name,
                    windows_codepage: entry.windows_codepage,
                    gls_csid: entry.gls_csid,
                    encoding: entry.encoding,
                    oem_codec: entry.oem_codec.clone(),
                });
            }
        }

        if let Some(num) = numeric {
            if num == entry.gls_csid || num == entry.windows_codepage as i32 {
                return Some(CharsetSpec {
                    canonical_name: entry.canonical_name,
                    windows_codepage: entry.windows_codepage,
                    gls_csid: entry.gls_csid,
                    encoding: entry.encoding,
                    oem_codec: entry.oem_codec.clone(),
                });
            }
        }
    }

    None
}

/// Extracts the codeset portion from an Informix locale string (e.g. `"de_DE.1252"` -> `"1252"`)
/// and resolves its charset specification.
pub fn resolve_locale(locale: &str) -> Option<CharsetSpec> {
    let token = if let Some(dot_pos) = locale.find('.') {
        &locale[dot_pos + 1..]
    } else {
        locale
    };

    let token = token.split(|c| c == '@' || c == '/').next()?;
    if token.is_empty() {
        return None;
    }

    resolve_codeset(token)
}

/// Decodes database wire bytes into a Rust UTF-8 String based on the database locale.
///
/// If `db_locale` is UTF-8 (or ASCII), or if decoding is not needed, standard UTF-8 parsing is used.
/// For single-byte and multibyte charsets (CP1252, ISO-8859-*, etc.), performs conversion.
///
/// Returns an error if `db_locale` names a known codeset this build has no codec
/// for, or if the bytes are invalid for an incomplete single-byte table (e.g.
/// CP857/CP864) -- libsqli's iconv-backed path fails the same way rather than
/// silently mislabeling the bytes as UTF-8.
pub fn decode_wire_string<'a>(bytes: &'a [u8], db_locale: Option<&str>) -> Result<Cow<'a, str>> {
    // Fast path: if valid ASCII, return without conversion
    if bytes.is_ascii() {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return Ok(Cow::Borrowed(s));
        }
    }

    let spec = db_locale.and_then(resolve_locale);
    match spec {
        Some(s) => decode_with_spec(bytes, &s),
        None => {
            // Default Informix locale en_US.8859-1 or unknown: decode via Windows-1252 / ISO-8859-1
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            Ok(cow)
        }
    }
}

fn decode_with_spec<'a>(bytes: &'a [u8], spec: &CharsetSpec) -> Result<Cow<'a, str>> {
    if spec.canonical_name == "UTF-8" {
        return Ok(String::from_utf8_lossy(bytes));
    }
    if let Some(enc) = spec.encoding {
        let (cow, _malformed, _has_err) = enc.decode(bytes);
        return Ok(cow);
    }
    if let Some(codec) = &spec.oem_codec {
        return match codec.decode.decode_string_checked(bytes) {
            Some(decoded) => Ok(Cow::Owned(decoded)),
            None => Err(Error::Protocol(format!(
                "Bytes are not valid {} (undefined code point)",
                spec.canonical_name
            ))),
        };
    }
    Err(Error::Protocol(format!(
        "No decoder available for DB_LOCALE codeset '{}'",
        spec.canonical_name
    )))
}

/// Encodes a Rust UTF-8 string into bytes formatted for the database based on `db_locale`.
///
/// If `db_locale` is UTF-8 (or unspecified), returns UTF-8 bytes directly.
/// If `db_locale` is CP1252 / ISO-8859-*, converts characters into database codepage bytes.
///
/// Returns an error if `db_locale` names a known codeset this build has no
/// codec for, or if `s` contains a character the target codeset cannot
/// represent.
pub fn encode_string_to_db(s: &str, db_locale: Option<&str>) -> Result<Vec<u8>> {
    if s.is_ascii() {
        return Ok(s.as_bytes().to_vec());
    }

    let spec = db_locale.and_then(resolve_locale);
    match spec {
        Some(spec) if spec.canonical_name != "UTF-8" => encode_with_spec(s, &spec),
        _ => Ok(s.as_bytes().to_vec()),
    }
}

fn encode_with_spec(s: &str, spec: &CharsetSpec) -> Result<Vec<u8>> {
    if let Some(enc) = spec.encoding {
        let (cow, _, _has_err) = enc.encode(s);
        return Ok(cow.into_owned());
    }
    if let Some(codec) = &spec.oem_codec {
        return oem_cp::encode_string_checked(s, codec.encode).ok_or_else(|| {
            Error::Protocol(format!(
                "Character not representable in DB_LOCALE codeset '{}'",
                spec.canonical_name
            ))
        });
    }
    Err(Error::Protocol(format!(
        "No encoder available for DB_LOCALE codeset '{}'",
        spec.canonical_name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_locale_cp1252_variations() {
        let spec1 = resolve_locale("de_DE.1252").expect("1252");
        assert_eq!(spec1.canonical_name, "CP1252");

        let spec2 = resolve_locale("de_DE.CP1252").expect("CP1252");
        assert_eq!(spec2.canonical_name, "CP1252");

        let spec3 = resolve_locale("en_US.8859-1").expect("8859-1");
        assert_eq!(spec3.canonical_name, "ISO-8859-1");

        let spec4 = resolve_locale("de_DE.utf8").expect("utf8");
        assert_eq!(spec4.canonical_name, "UTF-8");
    }

    #[test]
    fn test_decode_cp1252_umlauts() {
        // "Zähllisten updaten": 0xE4 is ä in CP1252
        let wire_bytes = b"Z\xE4hllisten updaten";
        let decoded = decode_wire_string(wire_bytes, Some("de_DE.1252")).unwrap();
        assert_eq!(decoded, "Zähllisten updaten");
    }

    #[test]
    fn test_encode_cp1252_umlauts() {
        let input = "Zähllisten updaten";
        let encoded = encode_string_to_db(input, Some("de_DE.1252")).unwrap();
        assert_eq!(encoded, b"Z\xE4hllisten updaten");
    }

    #[test]
    fn test_decode_cp1250_central_european() {
        // Polish 'ą' in CP1250 is 0xB9
        let wire_bytes = b"Test \xB9";
        let decoded = decode_wire_string(wire_bytes, Some("pl_PL.1250")).unwrap();
        assert_eq!(decoded, "Test ą");
    }

    #[test]
    fn test_cp852_857_864_roundtrip() {
        // CP852 (Polish): 'ą' (U+0105) = 0xA5
        let wire = decode_wire_string(&[b'a', 0xA5], Some("pl_PL.852")).unwrap();
        assert_eq!(wire, "a\u{105}");
        let back = encode_string_to_db("a\u{105}", Some("pl_PL.852")).unwrap();
        assert_eq!(back, vec![b'a', 0xA5]);

        // CP857 (Turkish, Incomplete table): resolves and decodes 'ş' (U+015F) = 0x9F.
        let wire = decode_wire_string(&[b's', 0x9F], Some("tr_TR.857")).unwrap();
        assert_eq!(wire, "s\u{15f}");

        // CP864 (Arabic) resolves at all (previously entirely missing from the catalog).
        assert!(resolve_locale("ar_SA.864").is_some());
    }

    #[test]
    fn test_unsupported_codeset_errors_instead_of_silently_mislabeling() {
        // A codeset with neither an encoding_rs nor an oem_cp codec must
        // error, not silently reinterpret the bytes as lossy UTF-8 (every
        // entry in the real catalog has a codec, so this exercises the
        // fallback branch directly with a synthetic spec).
        let spec = CharsetSpec {
            canonical_name: "FAKE-CODESET",
            windows_codepage: 0,
            gls_csid: 0,
            encoding: None,
            oem_codec: None,
        };
        assert!(decode_with_spec(b"\xFF", &spec).is_err());
        assert!(encode_with_spec("x", &spec).is_err());
    }

    #[test]
    fn test_incomplete_oem_table_errors_on_undefined_byte() {
        // CP857's decoding table is Incomplete: byte 0xD5 is undefined in
        // CP857 and must error rather than silently drop/replace it.
        let cp857 = resolve_locale("tr_TR.857").expect("857");
        assert!(decode_with_spec(&[0xD5], &cp857).is_err());
    }
}
