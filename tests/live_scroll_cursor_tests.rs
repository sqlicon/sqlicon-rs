//! Live test for scrollable/absolute cursor fetch (`Connection::query_scrollable` /
//! [`ScrollCursor`](sqlicon_rs::ScrollCursor)).
//!
//! Seeds a small ordered table and exercises every navigation primitive against it:
//! `first`, forward `next`, a jump via `absolute`, `previous`, `last`, a backward
//! `relative` jump, and an out-of-range `absolute` call.

use sqlicon_rs::{ConnectParams, Connection};

#[tokio::test]
async fn test_live_scroll_cursor_navigation() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping scroll cursor test: cannot connect to engine ({e})");
            return;
        }
    };

    let tbl = format!("sqlicon_test_scroll_{}", std::process::id());
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
    conn.execute(&format!("CREATE TABLE {tbl} (id INT, val INT)"))
        .await
        .expect("create scroll test table");
    for i in 1..=5 {
        conn.execute(&format!("INSERT INTO {tbl} VALUES ({i}, {})", i * 10))
            .await
            .expect("seed row");
    }

    let mut cur = conn
        .query_scrollable(&format!("SELECT id, val FROM {tbl} ORDER BY id"))
        .await
        .expect("open scrollable cursor");

    let row = cur.first().await.expect("first").expect("row 1");
    assert_eq!(row.get_display_value(0).unwrap(), "1");
    assert_eq!(cur.position(), Some(1));

    let row = cur.next().await.expect("next").expect("row 2");
    assert_eq!(row.get_display_value(0).unwrap(), "2");
    let row = cur.next().await.expect("next").expect("row 3");
    assert_eq!(row.get_display_value(0).unwrap(), "3");
    assert_eq!(cur.position(), Some(3));

    let row = cur.absolute(5).await.expect("absolute(5)").expect("row 5");
    assert_eq!(row.get_display_value(0).unwrap(), "5");
    assert_eq!(row.get_display_value(1).unwrap(), "50");

    let row = cur.previous().await.expect("previous").expect("row 4");
    assert_eq!(row.get_display_value(0).unwrap(), "4");

    let row = cur.last().await.expect("last").expect("last row");
    assert_eq!(row.get_display_value(0).unwrap(), "5");
    assert_eq!(cur.position(), Some(5));

    let row = cur.relative(-2).await.expect("relative(-2)").expect("row 3");
    assert_eq!(row.get_display_value(0).unwrap(), "3");

    let out_of_range = cur.absolute(999).await.expect("absolute(999) out of range");
    assert!(out_of_range.is_none());
    assert_eq!(cur.position(), None);

    // next() never wraps around once positioned after the last row.
    let still_none = cur.next().await.expect("next() stays after-last");
    assert!(still_none.is_none());

    // previous() from after-last jumps to the actual last row, matching libsqli.
    let row = cur.previous().await.expect("previous() from after-last").expect("last row");
    assert_eq!(row.get_display_value(0).unwrap(), "5");

    cur.close().await.expect("close scroll cursor");
    let _ = conn.execute(&format!("DROP TABLE {tbl}")).await;
}
