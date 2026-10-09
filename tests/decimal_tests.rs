use sqlicon_rs::types::decimal::{
    decimal_decode_wire, decimal_encode_wire, decimal_wire_size, Decimal, DECIMAL_WIRE_CAPACITY,
};

const FLOATING32: u16 = 0x20ff;
const FIXED8_4: u16 = 0x0804;

#[test]
fn test_fixed_fixtures_from_c() {
    struct Case {
        descriptor: u16,
        bytes: &'static [u8],
        length: usize,
        text: &'static str,
    }

    let cases = [
        Case {
            descriptor: FIXED8_4,
            bytes: &[0xc2, 1, 23, 45, 0],
            length: 5,
            text: "123.4500",
        },
        Case {
            descriptor: FIXED8_4,
            bytes: &[0x3d, 98, 76, 55, 0],
            length: 5,
            text: "-123.4500",
        },
        Case {
            descriptor: FIXED8_4,
            bytes: &[0x80, 0, 0, 0, 0],
            length: 5,
            text: "0.0000",
        },
        Case {
            descriptor: 0x0302,
            bytes: &[0xc1, 1, 23],
            length: 3,
            text: "1.23",
        },
        Case {
            descriptor: 0x0303,
            bytes: &[0x40, 90, 0],
            length: 3,
            text: "-0.001",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0x80, 1],
            length: 18,
            text: "1E-130",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0x7f, 99],
            length: 18,
            text: "-1E-130",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0xff, 10],
            length: 18,
            text: "1E+125",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0, 90],
            length: 18,
            text: "-1E+125",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0xc2, 1, 23, 45],
            length: 18,
            text: "123.45",
        },
        Case {
            descriptor: FLOATING32,
            bytes: &[0xc2, 10],
            length: 18,
            text: "1E+3",
        },
    ];

    for case in &cases {
        let mut wire_buf = vec![0u8; case.length];
        wire_buf[..case.bytes.len()].copy_from_slice(case.bytes);

        let decoded = decimal_decode_wire(&wire_buf, case.descriptor).expect("failed decode");
        assert_eq!(decoded.to_string(), case.text, "Decoded text mismatch");

        let parsed: Decimal = case.text.parse().expect("failed parse");
        let mut out_bytes = [0u8; DECIMAL_WIRE_CAPACITY];
        let len = decimal_encode_wire(&parsed, case.descriptor, &mut out_bytes)
            .expect("failed encode");
        assert_eq!(len, case.length, "Encoded length mismatch");
        assert_eq!(
            &out_bytes[..case.bytes.len()],
            case.bytes,
            "Encoded bytes mismatch for {}",
            case.text
        );
    }
}

#[test]
fn test_all_descriptors_exhaustive() {
    let mut accepted = 0;
    for descriptor in 0..=u16::MAX {
        let precision = (descriptor >> 8) as u8;
        let scale = (descriptor & 0xff) as u8;
        let valid = precision >= 1 && precision <= 32 && (scale <= precision || scale == 255);
        match decimal_wire_size(descriptor) {
            Ok(width) => {
                assert!(valid, "Descriptor 0x{:04x} should not be valid", descriptor);
                let expected_width = ((precision as usize) + ((scale as usize) & 1) + 3) / 2;
                assert_eq!(width, expected_width);
                accepted += 1;
            }
            Err(_) => {
                assert!(!valid, "Descriptor 0x{:04x} should be valid", descriptor);
            }
        }
    }
    assert_eq!(accepted, 592, "Expected exactly 592 valid descriptors");
}

#[test]
fn test_scale_and_canonical_format_cases() {
    let cases = [
        ("123.4500", "123.4500", 4),
        ("+00123.4500", "123.4500", 4),
        (".5", "0.5", 1),
        ("1.", "1", 0),
        ("-0.00", "0.00", 2),
        ("1e3", "1E+3", -3),
        ("1.2300E+2", "123.00", 2),
        ("0.000001", "0.000001", 6),
        ("0.0000001", "1E-7", 7),
        ("-0.00000012300", "-1.2300E-7", 11),
        ("0E-7", "0E-7", 7),
        ("1E+2147483648", "1E+2147483648", i32::MIN),
        ("1E-2147483647", "1E-2147483647", i32::MAX),
        ("0E+2147483648", "0E+2147483648", i32::MIN),
    ];

    for &(input, expected_output, expected_scale) in &cases {
        if let Ok(d) = input.parse::<Decimal>() {
            assert_eq!(d.scale, expected_scale, "Scale mismatch for {}", input);
            assert_eq!(
                d.to_string(),
                expected_output,
                "Format mismatch for {}",
                input
            );
        }
    }
}

#[test]
fn test_parse_rejects_malformed() {
    let invalid = [
        "", "+", "-", ".", "1..0", " 1", "1 ", "1,2", "NaN", "Infinity",
        "NULL", "1e", "1e+", "1e-", "1e2x", "1e2.0", "1_000", "0x10",
    ];

    for bad in &invalid {
        assert!(
            bad.parse::<Decimal>().is_err(),
            "Malformed '{}' should be rejected",
            bad
        );
    }
}

#[test]
fn test_int64_boundaries_and_null() {
    let null_dec = Decimal::null();
    assert!(null_dec.is_null);
    assert_eq!(null_dec.to_string(), "NULL");

    let zero = Decimal::zero(2);
    assert_eq!(zero.scale, 2);
    assert_eq!(zero.to_string(), "0.00");

    let d_i64 = Decimal::from_i64(-42, 0);
    assert_eq!(d_i64.to_string(), "-42");

    let d_i64_scaled = Decimal::from_i64(12345, 2);
    assert_eq!(d_i64_scaled.to_string(), "123.45");
}
