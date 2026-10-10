//! Live tests for the `Transaction` RAII scope (`Connection::transaction`).

use sqlicon_rs::{ConnectParams, Connection};

async fn connect() -> Option<Connection> {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    match Connection::connect(params).await {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("Skipping transaction guard test: cannot connect to engine ({e})");
            None
        }
    }
}

#[tokio::test]
async fn test_live_transaction_commit() {
    let Some(mut conn) = connect().await else { return };
    let tbl = format!("sqlicon_test_tx_commit_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT)")).await.expect("create table");

    {
        let mut tx = conn.transaction().await.expect("begin transaction");
        tx.connection()
            .execute(&format!("INSERT INTO {tbl} VALUES (1)"))
            .await
            .expect("insert inside transaction");
        tx.commit().await.expect("commit transaction");
    }

    let res = conn.query(&format!("SELECT COUNT(*) FROM {tbl}")).await.expect("count rows");
    assert_eq!(res.rows[0].get_display_value(0).unwrap(), "1");

    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}

#[tokio::test]
async fn test_live_transaction_rollback() {
    let Some(mut conn) = connect().await else { return };
    let tbl = format!("sqlicon_test_tx_rollback_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT)")).await.expect("create table");

    {
        let mut tx = conn.transaction().await.expect("begin transaction");
        tx.connection()
            .execute(&format!("INSERT INTO {tbl} VALUES (1)"))
            .await
            .expect("insert inside transaction");
        tx.rollback().await.expect("rollback transaction");
    }

    let res = conn.query(&format!("SELECT COUNT(*) FROM {tbl}")).await.expect("count rows");
    assert_eq!(res.rows[0].get_display_value(0).unwrap(), "0");

    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}
