use sqlicon_rs::{ConnectParams, Connection};

#[tokio::test]
async fn test_live_row_streaming() {
    let mut conn = match Connection::connect(
        ConnectParams::new("127.0.0.1", "9089", "ol_ien")
            .database("vdademo")
            .credentials("admin", "admin")
            .locales("en_US.utf8", "de_DE.1252"),
    )
    .await
    {
        Ok(c) => c,
        Err(_) => return, // Skip if test engine not reachable
    };

    let mut stream = conn
        .query_stream("SELECT tabid, tabname FROM systables WHERE tabid <= 20 ORDER BY tabid")
        .await
        .expect("query_stream failed");

    let cols = stream.columns().to_vec();
    assert_eq!(cols.len(), 2);
    assert_eq!(cols[0].name.to_lowercase(), "tabid");
    assert_eq!(cols[1].name.to_lowercase(), "tabname");

    let mut count = 0;
    while let Some(row) = stream.next().await.expect("stream fetch failed") {
        let tabid: i32 = row.get_i32(0).unwrap().unwrap();
        let name: &str = row.get_str(1).unwrap().unwrap();
        assert!(tabid > 0);
        assert!(!name.is_empty());
        count += 1;
    }

    assert!(count >= 5, "Expected at least 5 system tables with tabid <= 20, got {count}");

    // After stream reaches EOF, next() returns None
    assert!(stream.next().await.unwrap().is_none());

    // Connection can be used for subsequent queries immediately
    let res = conn.query("SELECT FIRST 1 tabname FROM systables").await.expect("query after stream");
    assert_eq!(res.rows.len(), 1);
}
