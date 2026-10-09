use sqlicon_rs::Connection;

#[tokio::test]
async fn test_live_unix_socket_query() {
    let sock_path = std::env::var("INFORMIX_SOCKET").unwrap_or_else(|_| "/INFORMIXTMP/ol_ies.str".to_string());
    if !std::path::Path::new(&sock_path).exists() {
        eprintln!("Skipping live Unix socket test: socket {sock_path} does not exist");
        return;
    }

    let uri = format!(
        "informix+onipcstr://informix@/sqli_log_test?INFORMIXSERVER=ol_ies&SOCKET_PATH={sock_path}&DB_LOCALE=de_DE.1252"
    );

    let mut conn = match Connection::connect_uri(&uri).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping live Unix socket test: connect failed ({e})");
            return;
        }
    };

    let res = conn
        .query("SELECT FIRST 3 tabid, tabname FROM systables WHERE tabid >= 1")
        .await
        .expect("Query via Unix domain socket failed");

    assert_eq!(res.rows.len(), 3);
    assert_eq!(res.rows[0].get_display_value(1).unwrap(), "systables");
}
