use sqlicon_rs::types::date::date_decode_wire;
use sqlicon_rs::types::datetime::{datetime_decode_wire, interval_decode_wire};
use sqlicon_rs::types::decimal::decimal_decode_wire;
use sqlicon_rs::types::ColumnType;

struct WireFixture {
    name: &'static str,
    col_type: ColumnType,
    qualifier: u32,
    wire: &'static [u8],
    wire_length: usize,
    text: &'static str,
    is_null: bool,
}

#[test]
fn test_all_native_wire_fixtures_from_c() {
    let fixtures = [
        WireFixture {
            name: "date_epoch",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0, 0, 0, 0],
            wire_length: 4,
            text: "1899-12-31",
            is_null: false,
        },
        WireFixture {
            name: "date_before_epoch",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0xff, 0xff, 0xff, 0xff],
            wire_length: 4,
            text: "1899-12-30",
            is_null: false,
        },
        WireFixture {
            name: "date_unix_epoch",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0, 0, 0x63, 0xe0],
            wire_length: 4,
            text: "1970-01-01",
            is_null: false,
        },
        WireFixture {
            name: "date_first_year",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0xff, 0xf5, 0x6a, 0xa6],
            wire_length: 4,
            text: "0001-01-01",
            is_null: false,
        },
        WireFixture {
            name: "date_last_year",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0, 0x2d, 0x24, 0x80],
            wire_length: 4,
            text: "9999-12-31",
            is_null: false,
        },
        WireFixture {
            name: "date_leap_century",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0, 0, 0x8e, 0xe8],
            wire_length: 4,
            text: "2000-02-29",
            is_null: false,
        },
        WireFixture {
            name: "date_common_century",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0, 0, 0, 60],
            wire_length: 4,
            text: "1900-03-01",
            is_null: false,
        },
        WireFixture {
            name: "date_null",
            col_type: ColumnType::Date,
            qualifier: 4,
            wire: &[0x80, 0, 0, 0],
            wire_length: 4,
            text: "NULL",
            is_null: true,
        },
        WireFixture {
            name: "decimal_scale",
            col_type: ColumnType::Decimal,
            qualifier: 0x0804,
            wire: &[0xc2, 1, 23, 45, 0],
            wire_length: 5,
            text: "123.4500",
            is_null: false,
        },
        WireFixture {
            name: "decimal_negative",
            col_type: ColumnType::Decimal,
            qualifier: 0x0804,
            wire: &[0x3d, 98, 76, 55, 0],
            wire_length: 5,
            text: "-123.4500",
            is_null: false,
        },
        WireFixture {
            name: "decimal_zero",
            col_type: ColumnType::Decimal,
            qualifier: 0x0804,
            wire: &[0x80, 0, 0, 0, 0],
            wire_length: 5,
            text: "0.0000",
            is_null: false,
        },
        WireFixture {
            name: "decimal_null",
            col_type: ColumnType::Decimal,
            qualifier: 0x0804,
            wire: &[0, 0, 0, 0, 0],
            wire_length: 5,
            text: "NULL",
            is_null: true,
        },
        WireFixture {
            name: "money",
            col_type: ColumnType::Money,
            qualifier: 0x0802,
            wire: &[0xc2, 1, 23, 45, 0],
            wire_length: 5,
            text: "123.45",
            is_null: false,
        },
        WireFixture {
            name: "decimal_minimum",
            col_type: ColumnType::Decimal,
            qualifier: 0x20ff,
            wire: &[0x80, 1],
            wire_length: 18,
            text: "1E-130",
            is_null: false,
        },
        WireFixture {
            name: "decimal_negative_minimum",
            col_type: ColumnType::Decimal,
            qualifier: 0x20ff,
            wire: &[0x7f, 99],
            wire_length: 18,
            text: "-1E-130",
            is_null: false,
        },
        WireFixture {
            name: "decimal_high_exponent",
            col_type: ColumnType::Decimal,
            qualifier: 0x20ff,
            wire: &[0xff, 10],
            wire_length: 18,
            text: "1E+125",
            is_null: false,
        },
        WireFixture {
            name: "decimal_floating_scale",
            col_type: ColumnType::Decimal,
            qualifier: 0x20ff,
            wire: &[0xc2, 1, 23, 45],
            wire_length: 18,
            text: "123.45",
            is_null: false,
        },
        WireFixture {
            name: "datetime_full",
            col_type: ColumnType::DateTime,
            qualifier: 0x0e0a,
            wire: &[0xc7, 20, 26, 6, 20, 12, 34, 56],
            wire_length: 8,
            // A complete YEAR TO SECOND timestamp separates date and time
            // with 'T', matching libsqli's complete_timestamp() formatting
            // (see test_native_temporal_linux.c:94 in the C test suite).
            text: "2026-06-20T12:34:56",
            is_null: false,
        },
        WireFixture {
            name: "datetime_year_one",
            col_type: ColumnType::DateTime,
            qualifier: 0x0400,
            wire: &[0xc6, 1, 0],
            wire_length: 3,
            // YEAR TO YEAR: only the year field is in range, so no
            // month/day placeholder is printed.
            text: "0001",
            is_null: false,
        },
        WireFixture {
            name: "interval_negative",
            col_type: ColumnType::Interval,
            qualifier: 0x05ac,
            wire: &[0x3d, 90, 0, 1],
            wire_length: 4,
            text: "-999.99",
            is_null: false,
        },
        WireFixture {
            name: "interval_odd_leading",
            col_type: ColumnType::Interval,
            qualifier: 0x0e4f,
            wire: &[0xc4, 3, 3, 27, 1, 12, 34, 50, 0],
            wire_length: 9,
            text: "3 03:27:01.12345",
            is_null: false,
        },
    ];

    for fixture in &fixtures {
        let mut wire_buf = vec![0u8; fixture.wire_length];
        wire_buf[..fixture.wire.len()].copy_from_slice(fixture.wire);

        match fixture.col_type {
            ColumnType::Date => {
                let d = date_decode_wire(&wire_buf).expect(fixture.name);
                assert_eq!(d.is_null, fixture.is_null, "{}: is_null", fixture.name);
                assert_eq!(d.to_string(), fixture.text, "{}: text", fixture.name);
            }
            ColumnType::Decimal | ColumnType::Money => {
                let dec = decimal_decode_wire(&wire_buf, fixture.qualifier as u16)
                    .expect(fixture.name);
                assert_eq!(dec.is_null, fixture.is_null, "{}: is_null", fixture.name);
                assert_eq!(dec.to_string(), fixture.text, "{}: text", fixture.name);
            }
            ColumnType::DateTime => {
                let dt = datetime_decode_wire(&wire_buf, fixture.qualifier as u16)
                    .expect(fixture.name);
                assert_eq!(dt.is_null, fixture.is_null, "{}: is_null", fixture.name);
                assert_eq!(dt.to_string(), fixture.text, "{}: text", fixture.name);
            }
            ColumnType::Interval => {
                let it = interval_decode_wire(&wire_buf, fixture.qualifier as u16)
                    .expect(fixture.name);
                assert_eq!(it.is_null, fixture.is_null, "{}: is_null", fixture.name);
                assert_eq!(it.to_string(), fixture.text, "{}: text", fixture.name);
            }
            _ => {}
        }
    }
}
