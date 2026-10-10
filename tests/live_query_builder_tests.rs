//! Live tests for the `sqlx`-style fluent query builder (`sqlicon_rs::query`).

use sqlicon_rs::{query, ConnectParams, Connection, Error};

#[tokio::test]
async fn test_live_query_builder_crud() {
    let host = std::env::var("INFORMIX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let params = ConnectParams::new(&host, "9089", "ol_iep")
        .database("sqli_log_test")
        .credentials("admin", "admin")
        .locales("de_DE.1252", "de_DE.1252");

    let mut conn = match Connection::connect(params).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping query builder test: cannot connect to engine ({e})");
            return;
        }
    };

    let tbl = format!("sqlicon_test_query_builder_{}", std::process::id());
    let _ = query(&format!("DROP TABLE {tbl}")).execute(&mut conn).await;
    query(&format!("CREATE TABLE {tbl} (id INT, name VARCHAR(30))"))
        .execute(&mut conn)
        .await
        .expect("create table");

    // execute() with bound parameters.
    let inserted = query(&format!("INSERT INTO {tbl} VALUES (?, ?)"))
        .bind(1)
        .bind("alice")
        .execute(&mut conn)
        .await
        .expect("insert via query builder");
    assert_eq!(inserted.rows_affected, 1);

    query(&format!("INSERT INTO {tbl} VALUES (?, ?)"))
        .bind(2)
        .bind("bob")
        .execute(&mut conn)
        .await
        .expect("insert second row");

    // fetch_all().
    let all = query(&format!("SELECT id, name FROM {tbl} ORDER BY id"))
        .fetch_all(&mut conn)
        .await
        .expect("fetch_all");
    assert_eq!(all.rows.len(), 2);
    assert_eq!(all.rows[0].get_string(1).unwrap().unwrap(), "alice");

    // fetch_one() with a bound parameter.
    let row = query(&format!("SELECT name FROM {tbl} WHERE id = ?"))
        .bind(2)
        .fetch_one(&mut conn)
        .await
        .expect("fetch_one");
    assert_eq!(row.get_string(0).unwrap().unwrap(), "bob");

    // fetch_one() on an empty result set returns Error::Eof.
    let missing = query(&format!("SELECT name FROM {tbl} WHERE id = ?"))
        .bind(999)
        .fetch_one(&mut conn)
        .await;
    assert!(matches!(missing, Err(Error::Eof)));

    // fetch_optional() distinguishes "no row" from an error.
    let none = query(&format!("SELECT name FROM {tbl} WHERE id = ?"))
        .bind(999)
        .fetch_optional(&mut conn)
        .await
        .expect("fetch_optional on empty result");
    assert!(none.is_none());

    let some = query(&format!("SELECT name FROM {tbl} WHERE id = ?"))
        .bind(1)
        .fetch_optional(&mut conn)
        .await
        .expect("fetch_optional on matching row")
        .expect("row present");
    assert_eq!(some.get_string(0).unwrap().unwrap(), "alice");

    // execute() for DELETE.
    let deleted = query(&format!("DELETE FROM {tbl} WHERE id = ?"))
        .bind(1)
        .execute(&mut conn)
        .await
        .expect("delete via query builder");
    assert_eq!(deleted.rows_affected, 1);

    let _ = query(&format!("DROP TABLE {tbl}")).execute(&mut conn).await;
}
