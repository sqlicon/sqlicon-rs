//! Live tests for the stored-procedure/function call convenience API
//! (`Connection::call_prepare`/`call_execute`).
//!
//! Covers both branches of `call_execute`'s `PreparedStatement::is_query()` dispatch: a
//! procedure with a `RETURNING` clause (statement type 56 with output columns -- executed
//! via the cursor path, output row returned) and one with no `RETURNING` clause (pure side
//! effect -- executed directly, only `rows_affected` returned).

use sqlicon_rs::{ConnectParams, Connection, ParameterValue};

#[tokio::test]
async fn test_live_call_procedure_with_return() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping call procedure test: cannot connect to engine ({e})");
            return;
        }
    };

    let proc_name = format!("sqlicon_test_call_add_{}", std::process::id());
    let _ = conn.execute(&format!("DROP PROCEDURE {proc_name}")).await;
    conn.execute(&format!(
        "CREATE PROCEDURE {proc_name}(a INT, b INT) RETURNING INT; RETURN a + b; END PROCEDURE"
    ))
    .await
    .expect("create procedure with RETURNING");

    let call = conn
        .call_prepare(&format!("EXECUTE PROCEDURE {proc_name}(?, ?)"))
        .await
        .expect("call_prepare");
    assert!(call.stmt().is_query());

    let result = conn
        .call_execute(&call, &[ParameterValue::Int(3), ParameterValue::Int(4)])
        .await
        .expect("call_execute with RETURNING");

    let row = result.row.expect("output row for a RETURNING procedure");
    assert_eq!(row.get_display_value(0).unwrap(), "7");

    let _ = conn.execute(&format!("DROP PROCEDURE {proc_name}")).await;
}

#[tokio::test]
async fn test_live_call_procedure_without_return() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping call procedure test: cannot connect to engine ({e})");
            return;
        }
    };

    let pid = std::process::id();
    let tbl = format!("sqlicon_test_call_log_{pid}");
    let proc_name = format!("sqlicon_test_call_log_proc_{pid}");

    let _ = conn.execute(&format!("DROP PROCEDURE {proc_name}")).await;
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (val INT)"))
        .await
        .expect("create log table");
    conn.execute(&format!(
        "CREATE PROCEDURE {proc_name}(a INT); INSERT INTO {tbl} VALUES (a); END PROCEDURE"
    ))
    .await
    .expect("create procedure without RETURNING");

    let call = conn
        .call_prepare(&format!("EXECUTE PROCEDURE {proc_name}(?)"))
        .await
        .expect("call_prepare");
    assert!(!call.stmt().is_query());

    let result = conn
        .call_execute(&call, &[ParameterValue::Int(42)])
        .await
        .expect("call_execute without RETURNING");
    assert!(result.row.is_none());

    let check = conn
        .query(&format!("SELECT val FROM {tbl}"))
        .await
        .expect("query log table");
    assert_eq!(check.rows.len(), 1);
    assert_eq!(check.rows[0].get_display_value(0).unwrap(), "42");

    let _ = conn.execute(&format!("DROP PROCEDURE {proc_name}")).await;
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}
