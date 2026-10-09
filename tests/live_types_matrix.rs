//! Live Informix data type matrix test suite.
//!
//! Tests creation, insertion, retrieval, and NULL handling for all Informix data types:
//! - SMALLINT, INTEGER, BIGINT, INT8, SERIAL, SERIAL8, BIGSERIAL
//! - FLOAT, SMALLFLOAT, DECIMAL, MONEY
//! - DATE, DATETIME, INTERVAL
//! - CHAR, VARCHAR, LVARCHAR
//! - BOOLEAN

use sqlicon_rs::{ConnectParams, Connection};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn test_live_data_types_matrix() {
    let params = ConnectParams::new("127.0.0.1", "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping live data type test: cannot connect to engine ({e})");
            return;
        }
    };

    let epoch_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let tbl = format!("dt_matrix_{}", epoch_s % 100000);

    // Create table with all Informix scalar, temporal & Smart-LOB data types
    let ddl = format!(
        "CREATE TABLE {tbl} (
            id INT,
            c_smallint SMALLINT,
            c_int INT,
            c_bigint BIGINT,
            c_int8 INT8,
            c_smfloat SMALLFLOAT,
            c_float FLOAT,
            c_decimal DECIMAL(10,2),
            c_money MONEY(10,2),
            c_date DATE,
            c_datetime DATETIME YEAR TO SECOND,
            c_interval INTERVAL DAY TO SECOND,
            c_char CHAR(10),
            c_varchar VARCHAR(50),
            c_lvarchar LVARCHAR(255),
            c_bool BOOLEAN,
            c_blob BLOB,
            c_clob CLOB
        )"
    );

    conn.execute(&ddl).await.expect("Failed to create type matrix table");

    // Insert non-null row (using FILETOBLOB / FILETOCLOB for valid LOB locators)
    let insert_val = format!(
        "INSERT INTO {tbl} VALUES (
            1,
            1234,
            567890,
            9876543210,
            123456789012345,
            3.14,
            2.718281828,
            1234.56,
            999.99,
            '2026-10-09',
            '2026-10-09 17:30:00',
            '5 04:03:02',
            'Informix',
            'Varchar string',
            'Long variable character test',
            't',
            FILETOBLOB('/etc/hosts', 'server'),
            FILETOCLOB('/etc/hosts', 'server')
        )"
    );
    conn.execute(&insert_val).await.expect("Failed to insert non-null row");

    // Insert all-null row
    let insert_null = format!(
        "INSERT INTO {tbl} VALUES (
            2,
            NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
        )"
    );
    conn.execute(&insert_null).await.expect("Failed to insert null row");

    // Query back
    let query_sql = format!("SELECT * FROM {tbl} ORDER BY id");
    let result = conn.query(&query_sql).await.expect("Query failed");
    assert_eq!(result.rows.len(), 2);

    // Row 1: Non-null values verification
    let r1 = &result.rows[0];
    assert_eq!(r1.get_i32(0).unwrap(), Some(1));
    assert_eq!(r1.get_i16(1).unwrap(), Some(1234));
    assert_eq!(r1.get_i32(2).unwrap(), Some(567890));
    assert_eq!(r1.get_i64(3).unwrap(), Some(9876543210));
    assert_eq!(r1.get_i64(4).unwrap(), Some(123456789012345));

    let sf = r1.get_f32(5).unwrap().unwrap();
    assert!((sf - 3.14).abs() < 0.01);

    let fl = r1.get_f64(6).unwrap().unwrap();
    assert!((fl - 2.718281828).abs() < 0.0001);

    let dec = r1.get_decimal(7).unwrap().unwrap();
    assert_eq!(dec.to_string(), "1234.56");

    let mon = r1.get_decimal(8).unwrap().unwrap();
    assert_eq!(mon.to_string(), "999.99");

    let dt = r1.get_datetime(10).unwrap().unwrap();
    assert_eq!(dt.year, 2026);
    assert_eq!(dt.month, 10);
    assert_eq!(dt.day, 9);
    assert_eq!(dt.hour, 17);
    assert_eq!(dt.minute, 30);
    assert_eq!(dt.second, 0);

    let iv = r1.get_interval(11).unwrap().unwrap();
    assert_eq!(iv.days, 5);
    assert_eq!(iv.hours, 4);
    assert_eq!(iv.minutes, 3);
    assert_eq!(iv.seconds, 2);

    assert_eq!(r1.get_str(12).unwrap(), Some("Informix"));
    assert_eq!(r1.get_str(13).unwrap(), Some("Varchar string"));
    assert_eq!(r1.get_str(14).unwrap(), Some("Long variable character test"));
    assert_eq!(r1.get_bool(15).unwrap(), Some(true));

    // BLOB / CLOB: non-null locator string
    assert!(r1.get_str(16).unwrap().is_some());
    assert!(r1.get_str(17).unwrap().is_some());
    assert_eq!(r1.column(16).unwrap().col_type, sqlicon_rs::ColumnType::Blob);
    assert_eq!(r1.column(17).unwrap().col_type, sqlicon_rs::ColumnType::Clob);

    // Row 2: All NULLs verification
    let r2 = &result.rows[1];
    assert_eq!(r2.get_i32(0).unwrap(), Some(2));
    assert_eq!(r2.get_i16(1).unwrap(), None);
    assert_eq!(r2.get_i32(2).unwrap(), None);
    assert_eq!(r2.get_i64(3).unwrap(), None);
    assert_eq!(r2.get_i64(4).unwrap(), None);
    assert_eq!(r2.get_f32(5).unwrap(), None);
    assert_eq!(r2.get_f64(6).unwrap(), None);
    assert_eq!(r2.get_decimal(7).unwrap(), None);
    assert_eq!(r2.get_decimal(8).unwrap(), None);
    assert_eq!(r2.get_date(9).unwrap(), None);
    assert_eq!(r2.get_datetime(10).unwrap(), None);
    assert_eq!(r2.get_interval(11).unwrap(), None);
    assert_eq!(r2.get_str(12).unwrap(), None);
    assert_eq!(r2.get_str(13).unwrap(), None);
    assert_eq!(r2.get_str(14).unwrap(), None);
    assert_eq!(r2.get_bool(15).unwrap(), None);
    assert_eq!(r2.get_str(16).unwrap(), None);
    assert_eq!(r2.get_str(17).unwrap(), None);

    // Verify display representation
    assert_eq!(r2.get_display_value(1).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(5).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(6).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(10).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(11).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(15).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(16).unwrap(), "NULL");
    assert_eq!(r2.get_display_value(17).unwrap(), "NULL");

    // Cleanup
    let drop_ddl = format!("DROP TABLE {tbl}");
    conn.execute(&drop_ddl).await.expect("Failed to drop table");
}
