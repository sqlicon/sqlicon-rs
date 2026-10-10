use sqlicon_rs::{ConnectParams, Connection, IsolationLevel, ParameterValue};

#[tokio::test]
async fn test_live_transactions_and_savepoints() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping live transaction test: cannot connect to engine ({e})");
            return;
        }
    };
    assert!(!conn.in_transaction());
    assert!(conn.autocommit());

    // savepoint_release/rollback must guard against "no active transaction" the same
    // way savepoint_set already does, matching libsqli's uniform savepoint_send_core check.
    assert!(conn.savepoint_release("nope").await.is_err());
    assert!(conn.savepoint_rollback("nope").await.is_err());
    assert!(!conn.in_transaction());

    let tbl = format!("test_tx_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {}", tbl)).await;
    conn.execute(&format!("CREATE TABLE {} (id INT, val VARCHAR(30))", tbl))
        .await
        .expect("create table failed");

    // 1. Transaction commit test
    conn.begin().await.expect("begin failed");
    assert!(conn.in_transaction());

    conn.execute(&format!("INSERT INTO {} VALUES (1, 'commited_value')", tbl))
        .await
        .expect("insert failed");
    conn.commit().await.expect("commit failed");
    assert!(!conn.in_transaction());

    let res = conn
        .query(&format!("SELECT val FROM {} WHERE id = 1", tbl))
        .await
        .expect("select failed");
    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0].get_string(0).unwrap().as_deref(), Some("commited_value"));

    // 2. Transaction rollback test
    conn.begin().await.expect("begin 2 failed");
    conn.execute(&format!("INSERT INTO {} VALUES (2, 'rolled_back_value')", tbl))
        .await
        .expect("insert 2 failed");
    conn.rollback().await.expect("rollback failed");
    assert!(!conn.in_transaction());

    let res2 = conn
        .query(&format!("SELECT val FROM {} WHERE id = 2", tbl))
        .await
        .expect("select 2 failed");
    assert_eq!(res2.rows.len(), 0);

    // 3. Savepoint test
    conn.begin().await.expect("begin 3 failed");
    conn.execute(&format!("INSERT INTO {} VALUES (3, 'before_sp')", tbl))
        .await
        .expect("insert 3 failed");

    conn.savepoint_set("sp1", true).await.expect("savepoint_set failed");

    conn.execute(&format!("INSERT INTO {} VALUES (4, 'after_sp')", tbl))
        .await
        .expect("insert 4 failed");

    conn.savepoint_rollback("sp1").await.expect("savepoint_rollback failed");
    conn.commit().await.expect("commit 3 failed");

    let res3 = conn
        .query(&format!("SELECT id FROM {} ORDER BY id", tbl))
        .await
        .expect("select 3 failed");
    assert_eq!(res3.rows.len(), 2); // id 1 and id 3

    // 4. Parameter binding test
    let stmt = conn
        .prepare(&format!("INSERT INTO {} VALUES (?, ?)", tbl))
        .await
        .expect("prepare failed");
    assert_eq!(stmt.parameter_count(), 2);

    let params_row = [ParameterValue::Int(10), ParameterValue::String("param_bound".into())];
    let result = conn
        .execute_stmt(&stmt, &params_row)
        .await
        .expect("execute_stmt failed");
    assert_eq!(result.rows_affected, 1);

    let res4 = conn
        .query(&format!("SELECT val FROM {} WHERE id = 10", tbl))
        .await
        .expect("select 4 failed");
    assert_eq!(res4.rows.len(), 1);
    assert_eq!(res4.rows[0].get_string(0).unwrap().as_deref(), Some("param_bound"));

    // 5. Isolation level test
    conn.set_isolation_level(IsolationLevel::CommittedRead)
        .await
        .expect("isolation level failed");

    // Cleanup
    let _ = conn.execute(&format!("DROP TABLE {}", tbl)).await;
}
