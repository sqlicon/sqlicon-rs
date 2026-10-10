use std::time::Duration;
use sqlicon_rs::{ConnectParams, ConnectionPool, PoolOptions};

#[tokio::test]
async fn test_live_connection_pool_concurrent() {
    let params = ConnectParams::new("127.0.0.1", "9089", "ol_ien")
        .database("vdademo")
        .credentials("admin", "admin")
        .locales("en_US.utf8", "de_DE.1252");

    // Quick connectivity check
    if sqlicon_rs::Connection::connect(params.clone()).await.is_err() {
        return; // Skip if engine is not reachable
    }

    let options = PoolOptions::default()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .test_on_borrow(true);

    let pool = ConnectionPool::with_options(params, options)
        .await
        .expect("pool initialization failed");

    assert_eq!(pool.max_connections(), 5);

    // Run 10 concurrent queries across the pool of 5 connections
    let mut handles = Vec::new();
    for i in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let mut conn = pool.acquire().await.expect("acquire connection failed");
            let res = conn
                .query(&format!("SELECT FIRST 1 tabid FROM systables WHERE tabid >= {i}"))
                .await
                .expect("pool query failed");
            assert_eq!(res.rows.len(), 1);
        }));
    }

    for h in handles {
        h.await.expect("task join failed");
    }

    // Give a brief moment for dropped pooled connections to return to the idle queue
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(pool.idle_count().await > 0);
}
