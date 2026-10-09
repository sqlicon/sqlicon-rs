//! Live test suite for Smart Large Objects (BLOB / CLOB), Batch Execution, and Query Cancellation.

use sqlicon_rs::{
    ConnectParams, Connection, ParameterValue,
};
use std::time::Duration;

#[tokio::test]
async fn test_live_sblob_read_write_seek() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping sblob test: cannot connect to engine ({e})");
            return;
        }
    };

    let tbl = "t_test_live_sblob";
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT, b BLOB, c CLOB)"))
        .await
        .expect("Create table with BLOB/CLOB");

    conn.execute(&format!(
        "INSERT INTO {tbl} VALUES (1, FILETOBLOB('/etc/hosts', 'server'), FILETOCLOB('/etc/hosts', 'server'))"
    ))
    .await
    .expect("Insert via FILETOBLOB/FILETOCLOB");

    let res = conn.query(&format!("SELECT id, b, c FROM {tbl} WHERE id = 1")).await.expect("Query LOB");
    assert_eq!(res.rows.len(), 1);
    let row = &res.rows[0];
    let hex_b = row.get_display_value(1).expect("Get hex locator for BLOB");
    let hex_c = row.get_display_value(2).expect("Get hex locator for CLOB");
    assert!(!hex_b.is_empty());
    assert!(!hex_c.is_empty());

    // 1. Open BLOB for read
    let lofd = conn.sblob_open(&hex_b, 4).await.expect("Open BLOB read-only");
    assert!(lofd > 0);

    // Read first 20 bytes
    let b1 = conn.sblob_read(lofd, 20).await.expect("Read 20 bytes from BLOB");
    assert_eq!(b1.len(), 20);

    // Seek backwards 10 bytes and read 15 bytes
    let b2 = conn.sblob_read_seek(lofd, -10, 15).await.expect("Read seek from BLOB");
    assert_eq!(b2.len(), 15);
    assert_eq!(&b1[10..20], &b2[0..10]);

    conn.sblob_close(lofd).await.expect("Close BLOB descriptor");

    // 2. Convenience direct read for CLOB
    let clob_data = conn.read_sblob(&hex_c, 50).await.expect("Convenience read CLOB");
    assert_eq!(clob_data.len(), 50);

    // Cleanup
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}

#[tokio::test]
async fn test_live_batch_execution() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping batch test: cannot connect to engine ({e})");
            return;
        }
    };

    let tbl = "t_test_live_batch";
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT, val VARCHAR(50))"))
        .await
        .expect("Create batch test table");

    // 1. SQL statement batch execution
    let sqls = [
        format!("INSERT INTO {tbl} VALUES (1, 'alpha')"),
        format!("INSERT INTO {tbl} VALUES (2, 'beta')"),
        format!("INSERT INTO {tbl} VALUES (3, 'gamma')"),
    ];
    let sql_refs: Vec<&str> = sqls.iter().map(|s| s.as_str()).collect();
    let batch_res = conn.execute_batch(&sql_refs).await.expect("Execute SQL batch");
    assert_eq!(batch_res.len(), 3);
    assert_eq!(batch_res.success_count(), 3);
    assert_eq!(batch_res.error_count(), 0);

    // 2. Prepared DML parameter batching
    let stmt = conn.prepare(&format!("INSERT INTO {tbl} VALUES (?, ?)")).await.expect("Prepare insert");
    let param_rows = vec![
        vec![ParameterValue::Int(10), ParameterValue::String("batch10".to_string())],
        vec![ParameterValue::Int(20), ParameterValue::String("batch20".to_string())],
        vec![ParameterValue::Int(30), ParameterValue::String("batch30".to_string())],
    ];
    let stmt_batch_res = conn.execute_stmt_batch(&stmt, &param_rows).await.expect("Execute stmt batch");
    assert_eq!(stmt_batch_res.len(), 3);
    assert_eq!(stmt_batch_res.success_count(), 3);
    assert_eq!(stmt_batch_res.error_count(), 0);

    let count_res = conn.query(&format!("SELECT COUNT(*) FROM {tbl}")).await.expect("Query count");
    let total_str = count_res.rows[0].get_display_value(0).unwrap();
    let total_rows: i32 = total_str.parse().unwrap();
    assert_eq!(total_rows, 6);


    let _ = conn.close_statement(stmt.id()).await;
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}

#[tokio::test]
async fn test_live_cancellation_handle() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping cancel test: cannot connect to engine ({e})");
            return;
        }
    };

    let cancel_handle = conn.cancel_handle();

    // Spawn a cancellation task that triggers after 50ms while query executes
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel_handle.cancel();
    });

    // Execute a query that sleeps on server if possible, or simple query
    // In Informix, sysmaster:sysdual query is fast, but cancelling should not panic
    let _ = conn.query("SELECT FIRST 100 tabname FROM sysmaster:systables, sysmaster:systables").await;
    let _ = cancel_task.await;
}
