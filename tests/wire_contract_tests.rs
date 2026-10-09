use bytes::BytesMut;
use sqlicon_rs::protocol::asc::{
    decode_asc_str_from_slice,
    encode_asc_str_padded_to_slice, encode_asc_str_to_slice, pad_even,
};
use sqlicon_rs::protocol::sl_header::SlHeader;

#[test]
fn test_asc_string_encode_hello() {
    let mut buf = [0u8; 64];
    let n = encode_asc_str_to_slice(&mut buf, "hello").expect("encode failed");
    assert_eq!(n, 7); // 2 length + 5 chars
    assert_eq!(buf[0], 0);
    assert_eq!(buf[1], 5);
    assert_eq!(&buf[2..7], b"hello");
}

#[test]
fn test_asc_string_encode_empty() {
    let mut buf = [0u8; 64];
    let n = encode_asc_str_to_slice(&mut buf, "").expect("encode empty");
    assert_eq!(n, 2);
    assert_eq!(buf[0], 0);
    assert_eq!(buf[1], 0);
}

#[test]
fn test_asc_string_encode_overflow() {
    let mut buf = [0u8; 3];
    assert!(encode_asc_str_to_slice(&mut buf, "hello").is_err());
}

#[test]
fn test_asc_encode_padded_odd_and_even() {
    let mut buf = [0u8; 64];
    // "hi" = 2 chars -> 2 length + 2 chars = 4 bytes (already even)
    let n = encode_asc_str_padded_to_slice(&mut buf, "hi").expect("encode padded");
    assert_eq!(n, 4);
    assert_eq!(buf[0], 0);
    assert_eq!(buf[1], 2);
    assert_eq!(&buf[2..4], b"hi");

    // "hi!" = 3 chars -> 2 length + 3 chars + 1 pad = 6 bytes
    let n2 = encode_asc_str_padded_to_slice(&mut buf, "hi!").expect("encode padded");
    assert_eq!(n2, 6);
    assert_eq!(buf[0], 0);
    assert_eq!(buf[1], 3);
    assert_eq!(&buf[2..5], b"hi!");
    assert_eq!(buf[5], 0);
}

#[test]
fn test_sl_header_framing_contract() {
    let header = SlHeader {
        pdu_size: 1024,
        sl_type: 0x01,
        sl_attr: 0x02,
        sl_opts: 0x0003,
    };

    let mut buf = BytesMut::new();
    header.encode(&mut buf);
    assert_eq!(buf.len(), 6);

    let mut slice: &[u8] = &buf;
    let decoded = SlHeader::decode(&mut slice).expect("header decode");
    assert_eq!(decoded.pdu_size, 1024);
    assert_eq!(decoded.sl_type, 0x01);
    assert_eq!(decoded.sl_attr, 0x02);
    assert_eq!(decoded.sl_opts, 0x0003);

    // Buffer too small (< 6 bytes)
    let mut small: &[u8] = &buf[..5];
    assert!(SlHeader::decode(&mut small).is_err());
}

#[test]
fn test_asc_decode_roundtrip() {
    let mut buf = [0u8; 32];
    encode_asc_str_to_slice(&mut buf, "Informix SQLI Driver").unwrap();
    let (decoded, len) = decode_asc_str_from_slice(&buf).unwrap();
    assert_eq!(decoded, "Informix SQLI Driver");
    assert_eq!(len, 2 + "Informix SQLI Driver".len());

    assert_eq!(pad_even(3), 4);
    assert_eq!(pad_even(4), 4);
}
