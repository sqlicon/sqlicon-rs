//! Live test for legacy (pre-SBLOB) `BYTE` / `TEXT` columns.
//!
//! Covers both sides of the legacy LOB path: binding a parameter against a server-declared
//! legacy placeholder (`SQ_BIND` with a 56-byte descriptor plus the `SQ_BBIND`/opcode-39
//! byte stream) and materializing a fetched value through `SQ_FETCHBLOB` while the cursor is
//! still open, so the getters return the actual data instead of the locator descriptor.

use sqlicon_rs::{ConnectParams, Connection, ParameterValue};

#[tokio::test]
async fn test_live_legacy_byte_text_roundtrip() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping legacy LOB test: cannot connect to engine ({e})");
            return;
        }
    };

    let tbl = format!("t_test_legacy_lob_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    // Legacy BYTE/TEXT columns take no length qualifier.
    conn.execute(&format!("CREATE TABLE {tbl} (id INT, b BYTE, c TEXT)"))
        .await
        .expect("create legacy LOB table");

    // The bind must follow the type the server declared for each placeholder, not the Rust
    // variant alone.
    let stmt = conn
        .prepare(&format!("INSERT INTO {tbl} VALUES (?, ?, ?)"))
        .await
        .expect("prepare legacy LOB insert");
    let row_params = [
        ParameterValue::Int(1),
        ParameterValue::Bytes(b"legacy byte payload".to_vec()),
        ParameterValue::String("legacy text payload".to_string()),
    ];
    conn.execute_stmt(&stmt, &row_params)
        .await
        .expect("insert through legacy placeholder");

    // Reading the row must return the materialized bytes, not the 56-byte locator descriptor.
    let res = conn
        .query(&format!("SELECT id, b, c FROM {tbl} WHERE id = 1"))
        .await
        .expect("query legacy LOB row");
    assert_eq!(res.rows.len(), 1);
    let row = &res.rows[0];
    assert_eq!(
        row.get_raw_column(1).expect("byte column").unwrap(),
        b"legacy byte payload"
    );
    assert_eq!(
        row.get_string(2).expect("text column").unwrap(),
        "legacy text payload"
    );

    // A NULL legacy LOB stays NULL and must not be materialized.
    conn.execute(&format!("INSERT INTO {tbl} VALUES (2, NULL, NULL)"))
        .await
        .expect("insert NULL legacy LOB");
    let null_res = conn
        .query(&format!("SELECT b, c FROM {tbl} WHERE id = 2"))
        .await
        .expect("query NULL legacy LOB");
    assert_eq!(null_res.rows.len(), 1);
    assert!(null_res.rows[0].get_raw_column(0).unwrap().is_none());
    assert!(null_res.rows[0].get_raw_column(1).unwrap().is_none());

    let _ = conn.close_statement(stmt.id()).await;
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}
