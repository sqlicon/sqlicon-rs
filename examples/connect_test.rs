use sqlicon_rs::{ConnectParams, Connection};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let params = ConnectParams::new("127.0.0.1", "9089", "ol_iep")
        .database("unorech")
        .credentials("admin", "admin")
        .locales("de_DE.utf8", "de_DE.1252");

    println!("Connecting to Informix at {}:{} (server: {})...", params.hostname, params.service, params.server);

    match Connection::connect(params).await {
        Ok(mut conn) => {
            println!("Successfully connected!");
            println!("Server capabilities: {:?}", conn.capabilities());

            println!("\nTesting prepare: SELECT FIRST 5 tabname, owner FROM systables WHERE tabid >= 100");
            match conn.prepare("SELECT FIRST 5 tabname, owner FROM systables WHERE tabid >= 100").await {
                Ok(stmt) => {
                    println!("Prepare succeeded! Stmt ID: {}, columns: {}", stmt.id(), stmt.columns().len());
                    for (i, col) in stmt.columns().iter().enumerate() {
                        println!("  Col #{i}: {} ({}, encoded_len={}, start_pos={})", col.name, col.col_type, col.encoded_length, col.col_start_pos);
                    }
                }
                Err(e) => {
                    eprintln!("Prepare failed: {e}");
                }
            }

            println!("\nTesting query: SELECT FIRST 5 tabid, tabname, owner, created FROM systables WHERE tabid >= 100");
            match conn.query("SELECT FIRST 5 tabid, tabname, owner, created FROM systables WHERE tabid >= 100").await {
                Ok(result) => {
                    println!("Query succeeded! Returned {} rows (affected: {}):", result.rows.len(), result.rows_affected);
                    for (row_idx, row) in result.rows.iter().enumerate() {
                        let tabid = row.get_i32(0)?.map(|v| v.to_string()).unwrap_or_else(|| "<null>".into());
                        let tabname = row.get_str(1)?.unwrap_or("<null>");
                        let owner = row.get_str(2)?.unwrap_or("<null>");
                        let created = row.get_date(3)?.map(|d| d.to_string()).unwrap_or_else(|| "<null>".into());
                        println!("  Row #{row_idx}: tabid = {:<6} tabname = {:<20} owner = {:<10} created = {}", tabid, tabname.trim(), owner.trim(), created);
                    }
                }
                Err(e) => {
                    eprintln!("Query failed: {e}");
                }
            }
        }
        Err(e) => {
            eprintln!("Connection failed: {e}");
        }
    }

    Ok(())
}
