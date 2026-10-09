//! Comprehensive test suite for Informix Database Charsets & Locales.

use sqlicon_rs::types::charset::{decode_wire_string, encode_string_to_db, resolve_codeset, resolve_locale};

#[test]
fn test_all_catalog_locales_resolve() {
    // CP1252 / Western Europe
    let sp1252 = resolve_locale("de_DE.1252").expect("de_DE.1252");
    assert_eq!(sp1252.canonical_name, "CP1252");
    assert_eq!(sp1252.windows_codepage, 1252);
    assert_eq!(sp1252.gls_csid, 1252);

    let sp_cp1252 = resolve_locale("de_DE.CP1252").expect("de_DE.CP1252");
    assert_eq!(sp_cp1252.canonical_name, "CP1252");

    // ISO-8859-1 (Latin-1)
    let sp8859_1 = resolve_locale("en_US.8859-1").expect("en_US.8859-1");
    assert_eq!(sp8859_1.canonical_name, "ISO-8859-1");
    assert_eq!(sp8859_1.gls_csid, 819);

    // ISO-8859-2 (Central / Eastern Europe)
    let sp8859_2 = resolve_locale("cs_CZ.8859-2").expect("cs_CZ.8859-2");
    assert_eq!(sp8859_2.canonical_name, "ISO-8859-2");
    assert_eq!(sp8859_2.gls_csid, 912);

    // ISO-8859-5 (Cyrillic)
    let sp8859_5 = resolve_locale("ru_RU.8859-5").expect("ru_RU.8859-5");
    assert_eq!(sp8859_5.canonical_name, "ISO-8859-5");
    assert_eq!(sp8859_5.gls_csid, 915);

    // ISO-8859-15 (Latin-9 with Euro)
    let sp8859_15 = resolve_locale("fr_FR.8859-15").expect("fr_FR.8859-15");
    assert_eq!(sp8859_15.canonical_name, "ISO-8859-15");
    assert_eq!(sp8859_15.gls_csid, 57391);

    // Windows-1250 (Central European)
    let sp1250 = resolve_locale("pl_PL.1250").expect("pl_PL.1250");
    assert_eq!(sp1250.canonical_name, "CP1250");
    assert_eq!(sp1250.windows_codepage, 1250);

    // Windows-1251 (Cyrillic)
    let sp1251 = resolve_locale("ru_RU.1251").expect("ru_RU.1251");
    assert_eq!(sp1251.canonical_name, "CP1251");

    // Asian charsets: Shift_JIS (CP932), GB2312/GBK (CP936), Big5 (CP950), EUC-KR (CP949)
    assert_eq!(resolve_locale("ja_JP.sjis").expect("sjis").canonical_name, "CP932");
    assert_eq!(resolve_locale("ja_JP.932").expect("932").canonical_name, "CP932");
    assert_eq!(resolve_locale("zh_CN.gb2312").expect("gb2312").canonical_name, "CP936");
    assert_eq!(resolve_locale("zh_TW.big5").expect("big5").canonical_name, "CP950");
    assert_eq!(resolve_locale("ko_KR.949").expect("949").canonical_name, "CP949");

    // Numeric GLS CSID lookups
    assert_eq!(resolve_codeset("57372").expect("57372").canonical_name, "UTF-8");
    assert_eq!(resolve_codeset("819").expect("819").canonical_name, "ISO-8859-1");
    assert_eq!(resolve_codeset("1252").expect("1252").canonical_name, "CP1252");
}

#[test]
fn test_multilingual_roundtrips() {
    // 1. German Umlauts & Euro in CP1252
    let german_text = "abzüglich bereits berechneter Abschläge: 150 €";
    let cp1252_encoded = encode_string_to_db(german_text, Some("de_DE.1252"));
    let german_decoded = decode_wire_string(&cp1252_encoded, Some("de_DE.1252"));
    assert_eq!(german_decoded, german_text);

    // 2. Central European in CP1250
    let polish_text = "Zażółć gęślą jaźń";
    let cp1250_encoded = encode_string_to_db(polish_text, Some("pl_PL.1250"));
    let polish_decoded = decode_wire_string(&cp1250_encoded, Some("pl_PL.1250"));
    assert_eq!(polish_decoded, polish_text);

    // 3. Cyrillic in CP1251
    let russian_text = "Привет мир Informix";
    let cp1251_encoded = encode_string_to_db(russian_text, Some("ru_RU.1251"));
    let russian_decoded = decode_wire_string(&cp1251_encoded, Some("ru_RU.1251"));
    assert_eq!(russian_decoded, russian_text);

    // 4. Japanese Shift_JIS in CP932
    let japanese_text = "日本語テスト";
    let cp932_encoded = encode_string_to_db(japanese_text, Some("ja_JP.932"));
    let japanese_decoded = decode_wire_string(&cp932_encoded, Some("ja_JP.932"));
    assert_eq!(japanese_decoded, japanese_text);

    // 5. UTF-8 pass-through
    let utf8_text = "🚀 Universal UTF-8 String 🌍";
    let utf8_encoded = encode_string_to_db(utf8_text, Some("en_US.utf8"));
    let utf8_decoded = decode_wire_string(&utf8_encoded, Some("en_US.utf8"));
    assert_eq!(utf8_decoded, utf8_text);
}
