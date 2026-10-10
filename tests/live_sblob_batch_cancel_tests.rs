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
async fn test_live_sblob_create_write_read() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping sblob create test: cannot connect to engine ({e})");
            return;
        }
    };

    // Fastpath create (SQ_GETROUTINE + SQ_EXFPROUTINE) in read/write mode, so the same
    // descriptor can be written and then read back.
    let options = sqlicon_rs::connection::sblob::SblobOptions {
        open_mode: sqlicon_rs::connection::sblob::SblobOpenMode::ReadWrite as i32,
        sbspace: None,
        estimated_bytes: 1024,
        maximum_bytes: -1,
        extent_kib: -1,
        create_flags: 0,
    };

    let handle = match conn.sblob_create(sqlicon_rs::connection::sblob::SblobType::Blob, &options).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping sblob create test: {e}");
            return;
        }
    };

    assert!(handle.lofd > 0);
    assert!(!handle.locator.is_empty() && handle.locator.len() <= 72);
    assert!(!handle.locator_hex().is_empty());

    let payload = b"sqlicon-rs fastpath create payload";
    let written = conn.sblob_write(handle.lofd, payload).await.expect("Write to created BLOB");
    assert_eq!(written, payload.len());

    // The write path buffers data in the descriptor, so a read through the same `lofd`
    // reports a zero file size; the bytes become visible through the locator only after the
    // descriptor is closed (verified against a live server).
    let buffered = conn.sblob_read(handle.lofd, 64).await.expect("Read through open descriptor");
    assert!(buffered.is_empty());

    conn.sblob_close(handle.lofd).await.expect("Close created descriptor");

    // The locator returned by create must be usable through the SQL path as well.
    let reopened = conn.sblob_open(&handle.locator_hex(), 4).await.expect("Reopen by locator");
    let via_sql = conn.sblob_read(reopened, 64).await.expect("Read via reopened handle");
    assert_eq!(via_sql, payload);
    conn.sblob_close(reopened).await.expect("Close reopened descriptor");
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

    // Unlike the old general-purpose CancelHandle (which could "cancel" an arbitrary SELECT),
    // libsqli's sqli_execute_cancelable only supports a prepared UPDATE/DELETE/INSERT.
    let tbl = format!("t_test_live_cancel_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT, val VARCHAR(30))"))
        .await
        .expect("create table failed");
    conn.execute(&format!("INSERT INTO {tbl} VALUES (1, 'before')"))
        .await
        .expect("seed insert failed");

    let stmt = conn
        .prepare(&format!("UPDATE {tbl} SET val = 'after' WHERE id = ?"))
        .await
        .expect("prepare failed");

    let operation = sqlicon_rs::CancelOperation::new();
    let requester = operation.requester();

    // Request cancellation concurrently. Against a fast local statement the DML will likely
    // already be done by the time this fires -- CancelOutcome::Executed is a valid, documented
    // race per libsqli's contract, not a test failure.
    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        requester.request()
    });

    let params_row = [ParameterValue::Int(1)];
    let result = conn
        .execute_stmt_cancelable(&stmt, &params_row, &operation)
        .await
        .expect("execute_stmt_cancelable failed");
    let disposition = cancel_task.await.expect("cancel task panicked");
    println!("cancel disposition={disposition:?} outcome={:?}", result.outcome);

    assert!(result.started);
    let snapshot = operation
        .snapshot()
        .expect("operation did not reach a terminal state");
    assert_eq!(snapshot.outcome, result.outcome);

    // Core safety property the old CancelHandle lacked: the connection reports itself
    // permanently discarded exactly when the snapshot says a cancellation attempt touched it,
    // and every further operation on it must then fail instead of silently misbehaving.
    assert_eq!(conn.is_discarded(), snapshot.connection_discarded);
    if conn.is_discarded() {
        assert!(conn.execute("SELECT FIRST 1 1 FROM systables").await.is_err());
    } else {
        let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    }
}
