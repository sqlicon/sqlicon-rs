use sqlicon_rs::types::date::{date_decode_wire, date_encode_wire, Date};
use sqlicon_rs::types::datetime::{
    datetime_decode_wire, datetime_encode_wire, interval_decode_wire,
    interval_encode_wire, temporal_wire_size,
};

#[test]
fn test_date_fixtures_from_c() {
    struct Case {
        bytes: [u8; 4],
        year: i32,
        month: u8,
        day: u8,
        is_null: bool,
    }

    let cases = [
        Case {
            bytes: [0, 0, 0, 0],
            year: 1899,
            month: 12,
            day: 31,
            is_null: false,
        },
        Case {
            bytes: [255, 255, 255, 255],
            year: 1899,
            month: 12,
            day: 30,
            is_null: false,
        },
        Case {
            bytes: [0, 0, 0x63, 0xe0],
            year: 1970,
            month: 1,
            day: 1,
            is_null: false,
        },
        Case {
            bytes: [0x80, 0, 0, 0],
            year: 0,
            month: 0,
            day: 0,
            is_null: true,
        },
        Case {
            bytes: [0xff, 0xf5, 0x6a, 0xa6],
            year: 1,
            month: 1,
            day: 1,
            is_null: false,
        },
        Case {
            bytes: [0, 0x2d, 0x24, 0x80],
            year: 9999,
            month: 12,
            day: 31,
            is_null: false,
        },
        Case {
            bytes: [0, 0, 0x8e, 0xe8],
            year: 2000,
            month: 2,
            day: 29,
            is_null: false,
        },
        Case {
            bytes: [0, 0, 0, 60],
            year: 1900,
            month: 3,
            day: 1,
            is_null: false,
        },
    ];

    for case in &cases {
        let date = date_decode_wire(&case.bytes).expect("failed decode date");
        assert_eq!(date.is_null, case.is_null);
        if !case.is_null {
            assert_eq!(date.year, case.year);
            assert_eq!(date.month, case.month);
            assert_eq!(date.day, case.day);
        }

        let mut out = [0u8; 4];
        let len = date_encode_wire(&date, &mut out).expect("failed encode date");
        assert_eq!(len, 4);
        assert_eq!(out, case.bytes);
    }
}

#[test]
fn test_date_invalid_payloads() {
    let outside = [0x7f, 0xff, 0xff, 0xff];
    assert!(date_decode_wire(&outside).is_err());
    assert!(date_decode_wire(&outside[..3]).is_err());

    let mut buf = [0u8; 2];
    let d = Date::new(2024, 2, 29).unwrap();
    assert!(date_encode_wire(&d, &mut buf).is_err());

    // Invalid calendar dates
    assert!(Date::new(2023, 2, 29).is_err()); // Not leap
    assert!(Date::new(2024, 13, 1).is_err()); // Month 13
    assert!(Date::new(2024, 4, 31).is_err()); // April has 30 days
}

#[test]
fn test_date_gregorian_cycle_sampling() {
    // Leap year checks across multiple centuries
    let leap_years = [1904, 1996, 2000, 2004, 2024, 2400];
    let common_centuries = [1900, 2100, 2200, 2300];

    for &year in &leap_years {
        let feb29 = Date::new(year, 2, 29).expect("Feb 29 should exist");
        let mut buf = [0u8; 4];
        date_encode_wire(&feb29, &mut buf).unwrap();
        let decoded = date_decode_wire(&buf).unwrap();
        assert_eq!(decoded, feb29);
    }

    for &year in &common_centuries {
        assert!(
            Date::new(year, 2, 29).is_err(),
            "Year {} is not a leap year",
            year
        );
    }
}

#[test]
fn test_all_temporal_qualifiers() {
    let mut datetime_count = 0;
    let mut interval_count = 0;

    for start in 0..=6 {
        for end in start..=6 {
            let max_fraction = if end == 6 { 5 } else { 0 };
            let min_fraction = if end == 6 { 1 } else { 0 };
            for fraction in min_fraction..=max_fraction {
                let last = if end == 6 { 10 + fraction } else { end * 2 };
                let digits = if start == 6 {
                    fraction
                } else {
                    (if start == 0 { 4 } else { 2 }) + last - start * 2
                };
                let qualifier = ((digits << 8) | (start * 2 << 4) | last) as u16;

                if temporal_wire_size(qualifier, false).is_ok() {
                    datetime_count += 1;
                }

                if start < 2 && end > 1 {
                    continue;
                }

                let max_precision = if start == 6 { 1 } else { 9 };
                for precision in 1..=max_precision {
                    let int_digits = if start == 6 {
                        fraction
                    } else {
                        precision + last - start * 2
                    };
                    let int_qualifier = ((int_digits << 8) | (start * 2 << 4) | last) as u16;
                    if temporal_wire_size(int_qualifier, true).is_ok() {
                        interval_count += 1;
                    }
                }
            }
        }
    }

    assert_eq!(datetime_count, 56, "Expected 56 valid datetime qualifiers");
    assert_eq!(interval_count, 302, "Expected 302 valid interval qualifiers");
}

#[test]
fn test_datetime_and_interval_fixtures() {
    // Full datetime: 2026-06-20 12:34:56
    let full_qual = 0x0e0a;
    let full_bytes = [0xc7, 20, 26, 6, 20, 12, 34, 56];
    let dt = datetime_decode_wire(&full_bytes, full_qual).expect("decode datetime");
    assert_eq!(dt.to_string(), "2026-06-20 12:34:56");

    let mut out = [0u8; 8];
    let len = datetime_encode_wire(&dt, full_qual, &mut out).expect("encode datetime");
    assert_eq!(len, 8);
    assert_eq!(out, full_bytes);

    // Negative interval: -INTERVAL(999.99) SECOND(3) TO FRACTION(2)
    let int_qual = 0x05ac;
    let int_bytes = [0x3d, 90, 0, 1];
    let it = interval_decode_wire(&int_bytes, int_qual).expect("decode interval");
    assert_eq!(it.to_string(), "-999.99");

    let mut int_out = [0u8; 4];
    let len = interval_encode_wire(&it, int_qual, &mut int_out).expect("encode interval");
    assert_eq!(len, 4);
    assert_eq!(int_out, int_bytes);
}
