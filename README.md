# sqlicon-rs

[![Crates.io](https://img.shields.io/badge/crates.io-v0.1.0-orange.svg)](https://crates.io)
[![Documentation](https://docs.rs/sqlicon-rs/badge.svg)](https://docs.rs/sqlicon-rs)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](Cargo.toml)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-purple.svg)](Cargo.toml)

A pure, asynchronous IBM Informix Dynamic Server client library and CLI written in 100% safe, idiomatic Rust.

`sqlicon-rs` implements the native Informix **SQLI wire protocol** (`onsoctcp`) directly over non-blocking TCP (`tokio`). It requires **zero native CSDK dependencies**, no IBM Informix Client-SDK installation, and runs seamlessly on both **Linux** and **Windows**.

---

## Features

- **Zero CSDK / Native Client Dependency**: Pure Rust implementation of the SQLI session, handshake, and framing layers (`SL_HEADER`, `CONREQ`, `CONACC`, `SQ_PROTOCOLS`, ASC password encryption).
- **Multiple Transports**:
  - **TCP Network Socket** (`onsoctcp`, `informix+onsoctcp://`)
  - **TLS-Encrypted TCP** (`onsocssl`, `informix+onsocssl://`) via pure-Rust `rustls`/`tokio-rustls` -- no OpenSSL dependency. Verifies against the platform's native trust store or a configured CA file by default; see [`ConnectParams::ssl`](src/connection/params.rs).
  - **Local Unix Domain Socket** (`onipcstr`, `informix+onipcstr://` on tmpfs/Linux with "sqAZ" IPC preamble and OS credential handshake)
- **Asynchronous & Non-blocking**: Built on [Tokio](https://tokio.rs/) for high-throughput, non-blocking asynchronous database operations.
- **Cross-Platform**: Tested and verified on **Linux** (x86_64) and **Windows** (x86_64).
- **Comprehensive Informix Data Types**:
  - **Numeric**: `SMALLINT`, `INTEGER`, `BIGINT`, `INT8`, `SERIAL`, `SERIAL8`, `BIGSERIAL`, `FLOAT`, `SMALLFLOAT`, `DECIMAL`, `MONEY`.
  - **Temporal**: `DATE`, `DATETIME` (all qualifiers up to `YEAR TO FRACTION(5)`), `INTERVAL`.
  - **Text & Character**: `CHAR`, `VARCHAR`, `LVARCHAR`, `NCHAR`, `NVARCHAR`, `BOOLEAN`.
  - **Smart Large Objects (SLOB)**: Complete `BLOB` and `CLOB` support with random-access seeking, chunked reading, streaming writes (`SQ_LODATA`), and creation via the `ifx_lo_create` fastpath (`SQ_GETROUTINE` + `SQ_EXFPROUTINE`, [`Connection::sblob_create`](src/connection/client.rs)).
  - **Legacy `BYTE` / `TEXT`**: Parameters bound against a server-declared legacy placeholder are sent as a 56-byte LOB descriptor with the bytes streamed separately (`SQ_BBIND`), and fetched values are materialized through `SQ_FETCHBLOB` while the cursor is open, so row getters return the data rather than the locator.
- **Full Database Charset & GLS Support**: Automatic conversion between database encoding (`DB_LOCALE` such as `de_DE.1252` / Windows-1252, ISO-8859-1..15, Shift-JIS, GB2312, Big5, etc.) and Rust UTF-8 `String`s.
- **Prepared Statements & Parameter Binding**: Fast binary parameter encoding (`?` placeholders) and batched execution.
- **Stored Procedure / Function Calls**: `Connection::call_prepare`/`call_execute` wrap `EXECUTE PROCEDURE`/`EXECUTE FUNCTION` (no dedicated wire opcodes are needed -- just the ordinary prepare/bind/execute path keyed on the statement-type-56 output-column check), with `IN`/`OUT`/`INOUT` parameter-mode bookkeeping (`CallStatement::set_param_mode`/`out_column_index`) mirroring `libsqli`'s `sqli_call` API.
- **Scrollable / Absolute Cursor Fetch**: `Connection::query_scrollable`/`query_stmt_scrollable` open a cursor with `SQ_SCROLL` and return a [`ScrollCursor`](src/result/scroll.rs) supporting `first`/`last`/`absolute(n)`/`relative(offset)`/`previous`/`next` navigation via `SQ_SFETCH`, matching `libsqli`'s scroll-insensitive cursor support.
- **Transactions & Savepoints**: ANSI and Informix transactions (`begin`, `commit`, `rollback`, or the `#[must_use]`-enforced `Connection::transaction()` -> `Transaction` scope), named savepoints (`savepoint_set`, `savepoint_release`, `savepoint_rollback`), and isolation levels (`CommittedRead`, `RepeatableRead`, `Serializable`, `CursorStability`).
- **Out-of-Band Cancellation**: Single-use `CancelOperation`/`CancelRequester` scoped to one prepared `UPDATE`/`DELETE`/`INSERT`, using TCP urgent out-of-band interrupts (`MSG_OOB` via POSIX `send` on Unix and Winsock `send` on Windows) to abort it on the server (`SQLCODE -213`). The connection is always discarded after a cancellation attempt.
- **`sqlhosts` File Resolution**: Parse an Informix `sqlhosts` file and resolve a `dbservername` to its `protocol`/`hostname`/`service` via [`ConnectParams::from_sqlhosts`](src/connection/params.rs) or the standalone [`connection::sqlhosts`](src/connection/sqlhosts.rs) parser/lookup functions, mirroring libsqli's `sqli_parse_sqlhosts`/`sqli_find_sqlhosts_entry`.
- **Offline Error Catalog**: Full embedded 3,412-error Informix message catalog with dictionary expansion matching IBM Informix `finderr`.
- **Encrypted Profile Store**: Built-in CLI profile manager with AES-256-GCM encryption tied to machine-specific hardware IDs (`/etc/machine-id` on Linux, `MachineGuid` on Windows).

---

## Installation

Add `sqlicon-rs` to your `Cargo.toml`:

```toml
[dependencies]
sqlicon-rs = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

---

## Quick Start

### Basic Query Execution

```rust
use sqlicon_rs::{ConnectParams, Connection};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure connection parameters
    let params = ConnectParams::new("127.0.0.1", "9089", "ol_iep")
        .database("stores7")
        .credentials("informix", "secret_password")
        .locales("de_DE.1252", "de_DE.1252");

    // Connect to Informix engine
    let mut conn = Connection::connect(params).await?;

    // Execute query
    let result = conn.query("SELECT customer_num, fname, lname, balance FROM customer").await?;

    for row in &result.rows {
        let id = row.get_display_value(0)?;
        let first_name = row.get_display_value(1)?;
        let last_name = row.get_display_value(2)?;
        println!("Customer #{id}: {first_name} {last_name}");
    }

    Ok(())
}
```

### Connection via URI

You can also connect using an Informix URI:

```rust
use sqlicon_rs::Connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let uri = "informix+onsoctcp://informix:secret@127.0.0.1:9089/stores7?INFORMIXSERVER=ol_iep&DB_LOCALE=de_DE.1252";
    let mut conn = Connection::connect_uri(uri).await?;

    let res = conn.query("SELECT CURRENT YEAR TO SECOND FROM systables WHERE tabid = 1").await?;
    println!("Server time: {}", res.rows[0].get_display_value(0)?);

    Ok(())
}
```

### TLS-Encrypted Connection (`onsocssl`)

```rust
use sqlicon_rs::{ConnectParams, Connection, SslMode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Via URI: the onsocssl scheme enables TLS. SSL_VERIFY_PEER/SSL_CA_FILE are optional
    // query parameters; without them the connection is encrypted but not certificate-verified.
    let uri = "informix+onsocssl://informix:secret@127.0.0.1:9443/stores7?INFORMIXSERVER=ol_iep&SSL_VERIFY_PEER=1&SSL_CA_FILE=/etc/ssl/certs/informix-ca.pem";
    let mut conn = Connection::connect_uri(uri).await?;
    let _ = conn.query("SELECT FIRST 1 1 FROM systables").await?;

    // Equivalently via ConnectParams:
    let params = ConnectParams::new("127.0.0.1", "9443", "ol_iep")
        .database("stores7")
        .credentials("informix", "secret")
        .ssl(SslMode::EnabledVerifyPeer)
        .ssl_ca_file("/etc/ssl/certs/informix-ca.pem");
    let _conn2 = Connection::connect(params).await?;

    Ok(())
}
```

### Local Connection via Unix Domain Sockets (`onipcstr`)

On Linux, you can connect directly through Informix's local stream pipe socket with zero TCP stack overhead:

```rust
use sqlicon_rs::Connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connects via local /INFORMIXTMP/<server>.str stream pipe socket
    let uri = "informix+onipcstr://informix@/stores7?INFORMIXSERVER=ol_informix_str&SOCKET_PATH=/INFORMIXTMP/ol_informix.str&DB_LOCALE=de_DE.1252";
    let mut conn = Connection::connect_uri(uri).await?;

    let res = conn.query("SELECT FIRST 5 tabname FROM systables").await?;
    for row in &res.rows {
        println!("Table: {}", row.get_display_value(0)?);
    }

    Ok(())
}
```

### Prepared Statements & Parameter Binding

```rust
use sqlicon_rs::{Connection, ParameterValue};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = Connection::connect_uri("informix+onsoctcp://informix:secret@127.0.0.1:9089/stores7?INFORMIXSERVER=ol_iep").await?;

    // Prepare statement with placeholders
    let stmt = conn.prepare("SELECT customer_num, fname FROM customer WHERE customer_num = ? AND credit_limit > ?").await?;

    // Bind parameters and execute -- every scalar ParameterValue variant has a `From` impl,
    // so plain Rust values convert via `.into()` instead of naming the variant.
    let params: Vec<ParameterValue> = vec![101.into(), 5000.into()];
    let result = conn.query_stmt(&stmt, &params).await?;

    for row in &result.rows {
        println!("Found: {} - {}", row.get_display_value(0)?, row.get_display_value(1)?);
    }

    Ok(())
}
```

### Transactions and Savepoints

```rust
use sqlicon_rs::{Connection, IsolationLevel};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = Connection::connect_uri("informix+onsoctcp://informix:secret@127.0.0.1:9089/stores7?INFORMIXSERVER=ol_iep").await?;

    conn.set_isolation(IsolationLevel::CommittedRead).await?;
    conn.begin().await?;

    conn.execute("UPDATE account SET balance = balance - 100 WHERE id = 1").await?;
    
    // Set a named savepoint
    conn.savepoint_set("sp1").await?;
    
    match conn.execute("UPDATE account SET balance = balance + 100 WHERE id = 2").await {
        Ok(_) => {
            conn.savepoint_release("sp1").await?;
            conn.commit().await?;
            println!("Transaction committed successfully.");
        }
        Err(e) => {
            eprintln!("Error encountered, rolling back to savepoint: {e}");
            conn.savepoint_rollback("sp1").await?;
            conn.rollback().await?;
        }
    }

    Ok(())
}
```

### Out-of-Band Cancellation of a Prepared DML Statement

Cancellation is a single-use capability scoped to exactly one prepared `UPDATE`/`DELETE`/`INSERT`
execution on a TCP connection (never `SELECT`, batches, or LOB streaming -- matching the native
Informix client's own restrictions). `CancelRequester` is `Send + Sync + Clone` and can be moved
to another Tokio task or thread; sending the interrupt always permanently discards the connection
afterwards, since an out-of-band byte can desynchronize the wire even when the statement completes
successfully first:

```rust
use std::time::Duration;
use sqlicon_rs::{CancelOperation, Connection, ParameterValue};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = Connection::connect_uri("informix+onsoctcp://informix:secret@127.0.0.1:9089/stores7?INFORMIXSERVER=ol_iep").await?;

    let stmt = conn.prepare("UPDATE huge_table SET flag = 1 WHERE id > ?").await?;

    let operation = CancelOperation::new();
    let requester = operation.requester();

    // Spawn a worker to request cancellation after 1 second
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("Requesting out-of-band cancellation: {:?}", requester.request());
    });

    let snapshot = conn
        .execute_stmt_cancelable(&stmt, &[ParameterValue::Int(0)], &operation)
        .await?;
    println!("outcome: {:?}", snapshot.outcome); // Executed / Interrupted / Failed / Unknown

    if conn.is_discarded() {
        println!("connection was discarded after the cancellation attempt; reconnect to continue");
    }

    Ok(())
}
```

---

## CLI Usage

`sqlicon-rs` includes an interactive CLI client for testing, administration, and querying Informix databases.

### Running Queries

```bash
cargo run -- -h 127.0.0.1 -p 9089 -s ol_iep -d stores7 -u informix -w secret -c "SELECT FIRST 10 tabname FROM systables"
```

### Profile Management

Credentials can be stored encrypted with hardware-bound keys:

```bash
# Save profile
cargo run -- profile add prod --host 10.0.0.5 --port 9089 --server ol_prod --database erp --user informix

# Connect using profile
cargo run -- -P prod -c "SELECT count(*) FROM orders"
```

---

## Architecture & Protocols

```
+-------------------------------------------------------------+
|                     User Application                        |
+-------------------------------------------------------------+
                              |
+-------------------------------------------------------------+
|                 sqlicon-rs High-Level API                   |
|   (Connection, PreparedStatement, QueryResult, Row, etc.)   |
+-------------------------------------------------------------+
         |                                           |
+--------------------+                     +------------------+
|   Types & Codesets |                     | CancelOperation  |
| (encoding_rs GLS)  |                     | (TCP MSG_OOB)    |
+--------------------+                     +------------------+
         |                                           |
+-------------------------------------------------------------+
|                 SQLI Wire Protocol Engine                   |
|   (Handshake, Session framing, SQ_PROTOCOLS, ASC auth)     |
+-------------------------------------------------------------+
                              |
+-------------------------------------------------------------+
|                     Tokio Async TCP                         |
+-------------------------------------------------------------+
                              |
                      [ Informix Engine ]
```

---

## Testing

### Unit Tests

```bash
cargo test --lib
```

### Integration Tests with Live Engine

To run integration tests against a live Informix instance:

```bash
export INFORMIX_HOST="127.0.0.1"
export INFORMIX_PORT="9089"
export INFORMIX_SERVER="ol_iep"
export INFORMIX_DB="stores7"
export INFORMIX_USER="informix"
export INFORMIX_PASS="secret"

cargo test --test live_test
```

---

## License

This project is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
