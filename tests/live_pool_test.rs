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

    // PooledConnection::drop() now returns the connection to idle_queue synchronously
    // (before releasing its semaphore permit), so no settling delay is needed here.
    assert!(pool.idle_count().await > 0);
}

/// Regression test for a race where `PooledConnection::drop()` released its semaphore
/// permit before the connection was actually pushed back onto `idle_queue` (the push used
/// to happen in a separately spawned task). A concurrent `acquire_internal` could acquire
/// the freed permit, see an empty idle_queue, and open a brand-new connection -- exceeding
/// `max_connections`, which libsqli's fixed-size slot array cannot do. Hammer acquire/release
/// far more times than `max_connections` and assert the idle queue never ends up holding more
/// connections than the pool's configured maximum.
#[tokio::test]
async fn test_live_connection_pool_never_exceeds_max_connections() {
    let params = ConnectParams::new("127.0.0.1", "9089", "ol_ien")
        .database("vdademo")
        .credentials("admin", "admin")
        .locales("en_US.utf8", "de_DE.1252");

    if sqlicon_rs::Connection::connect(params.clone()).await.is_err() {
        return; // Skip if engine is not reachable
    }

    const MAX_CONNECTIONS: usize = 3;
    let options = PoolOptions::default()
        .max_connections(MAX_CONNECTIONS)
        .acquire_timeout(Duration::from_secs(10))
        .test_on_borrow(false);

    let pool = ConnectionPool::with_options(params, options)
        .await
        .expect("pool initialization failed");

    let mut handles = Vec::new();
    for _ in 0..(MAX_CONNECTIONS * 3) {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..5 {
                let conn = pool.acquire().await.expect("acquire connection failed");
                // Drop immediately to race the permit release against the idle-queue push.
                drop(conn);
                assert!(
                    pool.idle_count().await <= MAX_CONNECTIONS,
                    "idle queue exceeded max_connections"
                );
            }
        }));
    }

    for h in handles {
        h.await.expect("task join failed");
    }

    assert!(pool.idle_count().await <= MAX_CONNECTIONS);
}
