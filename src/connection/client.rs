//! Asynchronous Informix SQLI Connection.

use crate::connection::cancel::{CancelOperation, CancelOutcome, CancelSnapshot, LC_DISCARDED};
use crate::connection::capabilities::ServerCapabilities;
use crate::connection::handshake::*;
use crate::connection::params::ConnectParams;
use crate::connection::sblob::{
    build_ifx_lo_spec, SblobHandle, SblobOpenMode, SblobOptions, SblobType, IFX_LO_SPEC_SIZE, SBLOB_LOCATOR_MAX,
};
use crate::error::{Error, Result};
use crate::protocol::constants::*;
use crate::protocol::opcodes::Opcode;
use crate::result::column::ColumnInfo;
use crate::result::resultset::{ExecuteResult, QueryResult};
use crate::result::row::Row;
use crate::statement::{CallResult, CallStatement, PreparedStatement};
use crate::transport::SqliStream;
use crate::types::column_type::ColumnType;
use bytes::{BufMut, BytesMut};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tracing::{debug, info, trace};

/// Local classification of a dispatch outcome, used only to drive
/// [`Connection::execute_stmt_cancelable`]'s outcome/discard decision -- mirrors the kinds of
/// `sqli_status` that `sqli_cancel_operation_finish` distinguishes (OK, server error, I/O,
/// timeout, protocol-with-no-SQLCODE, other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Ok,
    Server,
    IoError,
    Timeout,
    ProtoError,
    Other,
}

/// Connection state matching Informix client lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Closed,
    Connecting,
    Handshake,
    Ready,
    InTransaction,
    Error,
}

#[derive(Default, Debug)]
pub struct TurnResult {
    pub statement_type: u16,
    pub statement_id: u16,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Row>,
    pub rows_affected: i64,
    pub row_id: i64,
    pub warnings: u16,
    pub saw_done: bool,
    /// Generated SERIAL/SERIAL8/BIGSERIAL id, from the DONE `errd1` field or
    /// a trailing SQ_INSERTDONE (opcode 94) payload.
    pub generated_serial: Option<i64>,
}

/// Decodes the 10-byte Informix INT8 wire format used by SQ_INSERTDONE's
/// SERIAL8 payload: `[sign: i16 BE][lo32: u32 BE][hi32: u32 BE]`. Mirrors
/// libsqli's `sqli_decode_ifx_int8` (sqli_dispatch.c). Returns `None` when
/// `sign == 0`, matching the C function's `is_null` output for that case.
fn decode_ifx_int8_serial(buf: &[u8; 10]) -> Option<i64> {
    let sign = i16::from_be_bytes([buf[0], buf[1]]);
    if sign == 0 {
        return None;
    }
    let lo = u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]) as u64;
    let hi = u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]) as u64;
    let mag = (hi << 32) | lo;
    Some(if sign < 0 {
        if mag >= (i64::MAX as u64 + 1) {
            i64::MIN
        } else {
            -(mag as i64)
        }
    } else if mag > i64::MAX as u64 {
        i64::MAX
    } else {
        mag as i64
    })
}

pub fn is_likely_opcode(v: u16) -> bool {
    matches!(
        v,
        0   // SQLI_SQ_DBOPEN_FLAGS
        | 1 // SQLI_SQ_COMMAND
        | 2 // SQLI_SQ_PREPARE
        | 3 // SQLI_SQ_CURNAME
        | 4 // SQLI_SQ_ID
        | 5 // SQLI_SQ_BIND
        | 6 // SQLI_SQ_OPEN
        | 7 // SQLI_SQ_EXECUTE
        | 8 // SQLI_SQ_DESCRIBE
        | 9 // SQLI_SQ_NFETCH
        | 10 // SQLI_SQ_CLOSE
        | 11 // SQLI_SQ_RELEASE
        | 12 // SQLI_SQ_EOT
        | 13 // SQLI_SQ_ERR
        | 14 // SQLI_SQ_TUPLE
        | 15 // SQLI_SQ_DONE
        | 35 // SQLI_SQ_BEGIN
        | 19 // SQLI_SQ_RET_TYPE
        | 22 // SQLI_SQ_NDESCRIBE
        | 25 // SQLI_SQ_TUPID
        | 36 // SQLI_SQ_DBOPEN
        | 37 // SQLI_SQ_DBCLOSE
        | 49 // SQLI_SQ_WANTDONE
        | 55 // SQLI_SQ_COST
        | 56 // SQLI_SQ_EXIT
        | 81 // SQLI_SQ_INFO
        | 94 // SQLI_SQ_INSERTDONE
        | 99 // SQLI_SQ_XACTSTAT
    )
}

pub fn is_cost_follow_opcode(v: u16) -> bool {
    matches!(
        v,
        12   // SQ_EOT
        | 15 // SQ_DONE
        | 13 // SQ_ERR
        | 14 // SQ_TUPLE
        | 8  // SQ_DESCRIBE
        | 81 // SQ_INFO
        | 94 // SQ_INSERTDONE
        | 99 // SQ_XACTSTAT
        | 56 // SQ_EXIT
    )
}

/// Represents an active, asynchronous connection to an IBM Informix Dynamic Server.
///
/// `Connection` manages the low-level TCP/TLS stream, protocol handshake, statement preparation,
/// query execution, transaction states, and Smart-LOB descriptors.
///
/// All socket I/O is asynchronous and non-blocking using Tokio.
pub struct Connection {
    stream: SqliStream,
    params: ConnectParams,
    capabilities: ServerCapabilities,
    state: ConnectionState,
    in_transaction: bool,
    autocommit: bool,
    in_batch: bool,
    /// Cached `SQ_GETROUTINE` handle for `function informix.ifx_lo_create(...)` plus the
    /// database name it was resolved for, as in libsqli's `sqli_conn_t` fastpath cache.
    lo_create_fphandle: i32,
    lo_create_dbname: String,
    /// Shared with any in-flight [`CancelOperation`]: bit `LC_PINNED` while a cancelable
    /// execute owns this connection, bit `LC_DISCARDED` permanently once a cancellation
    /// attempt has touched it. Persists for the connection's whole lifetime so a discarded
    /// connection can never be re-pinned by a later cancel operation.
    lifecycle: Arc<AtomicU8>,
}

/// Transaction isolation levels supported by Informix engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    /// Committed Read (dirty reads not allowed, reads committed data only).
    CommittedRead = 0,
    /// Repeatable Read (re-reading rows within a transaction yields identical results).
    RepeatableRead = 1,
    /// Serializable isolation level.
    Serializable = 3,
    /// Cursor Stability (shared lock held on the current row of an active cursor).
    CursorStability = 4,
}

impl Connection {
    /// Establishes an asynchronous connection to the Informix engine using the supplied parameters.
    ///
    /// Performs the initial three-way handshake:
    /// 1. Sends an SL-framed `CONREQ` session request (`SL_HEADER`).
    /// 2. Validates server acceptance (`SL_TYPE_CONACC`).
    /// 3. Negotiates protocols and server capabilities via `SQ_PROTOCOLS`.
    /// 4. Opens the target database (`SQ_DBOPEN`) and authenticates credentials.
    /// Establishes an asynchronous connection parsing parameters from an Informix URI.
    pub async fn connect_uri(uri: &str) -> Result<Self> {
        let params = ConnectParams::from_uri(uri)?;
        Self::connect(params).await
    }

    pub async fn connect(params: ConnectParams) -> Result<Self> {
        let is_unix_socket = params.service.starts_with('/');

        // libsqli (sqli_handshake.c) unconditionally clears the password for
        // onipcstr connections before any downstream auth step (local Unix
        // sockets authenticate via OS credentials, not a transmitted
        // password), so a PAM challenge received over this transport must
        // not echo back whatever password the caller happened to supply.
        let mut params = params;
        if is_unix_socket {
            params.password.clear();
        }

        let (mut stream, mut caps) = if is_unix_socket {
            #[cfg(unix)]
            {
                debug!("Connecting to Unix socket: {}", params.service);
                let mut stream = SqliStream::connect_unix(&params.service).await?;
                debug!("Connected Unix socket, initiating onipcstr handshake");

                // Step 2a: Read 12-byte server greeting
                let mut greeting = [0u8; 12];
                stream.read_exact(&mut greeting).await?;
                debug!("Received IPC greeting: {:02x?}", greeting);

                // Step 2b: Send "sqAZ" IPC preamble
                let preamble = build_ipc_preamble(&params)?;
                debug!("Sending IPC preamble ({} bytes)", preamble.len());
                stream.write_all(&preamble).await?;
                stream.flush().await?;

                // Step 2c: Read raw CONACC response: frame_len(2) + body
                let frame_len = stream.read_u16().await? as usize;
                if frame_len < 2 {
                    return Err(Error::Protocol(format!(
                        "Invalid CONACC frame length: {frame_len}"
                    )));
                }
                let body_len = frame_len - 2;
                let body = stream.read_bytes(body_len).await?;
                let caps = parse_conacc_body(&body)?;
                debug!("IPC CONACC accepted, cap_1={}", caps.cap_1);
                (stream, caps)
            }
            #[cfg(not(unix))]
            {
                return Err(Error::InvalidArgument(
                    "Unix domain sockets (onipcstr) are only supported on Unix systems".into(),
                ));
            }
        } else {
            let port: u16 = params.service.parse().map_err(|_| {
                Error::InvalidArgument(format!("Invalid port or service: {}", params.service))
            })?;

            let ssl_enabled = params.ssl_enable || std::env::var("SQLI_SSL_ENABLE").is_ok_and(|v| {
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
            });
            let mut stream = if ssl_enabled {
                debug!("Connecting to {}:{} over TLS", params.hostname, port);
                SqliStream::connect_tls(&params.hostname, port, &params).await?
            } else {
                debug!("Connecting to {}:{}", params.hostname, port);
                SqliStream::connect(&params.hostname, port).await?
            };
            // The SL/CONREQ handshake and everything after it is identical whether or not TLS
            // wraps the socket: libsqli sends the same "tlitcp" transport tag for both
            // onsoctcp and onsocssl, since the TLS session is transparent below the SQLI
            // protocol (sqli_handshake.c).
            debug!("Connected TCP socket, initiating SL/CONREQ handshake");

            // Step 1: Send SL-framed CONREQ
            let conreq = build_conreq_packet(&params, "tlitcp")?;
            stream.write_all(&conreq).await?;
            stream.flush().await?;

            // Step 2: Read SL response header & body
            let (header, body) = stream.read_sl_pdu().await?;
            debug!("Received SL response: type={}, attr={}", header.sl_type, header.sl_attr);

            let caps = match header.sl_type {
                SL_TYPE_CONACC => parse_conacc_body(&body)?,
                SL_TYPE_CONREJ => {
                    let caps = parse_conacc_body(&body)?;
                    debug!("Server sent CONREJ with soft-accept, continuing");
                    caps
                }
                SL_TYPE_REDIRECT => return Err(Error::Protocol("Server returned redirect".into())),
                other => return Err(Error::Protocol(format!("Unexpected SL type: {other}"))),
            };
            (stream, caps)
        };

        // Step 3: SQ_PROTOCOLS exchange
        debug!("Sending SQ_PROTOCOLS");
        let proto_pkt = build_sq_protocols_packet();
        stream.write_all(&proto_pkt).await?;
        stream.flush().await?;

        let proto_op = stream.read_u16().await?;
        if proto_op != Opcode::Protocols as u16 {
            return Err(Error::Protocol(format!(
                "SQ_PROTOCOLS expected opcode {}, got {}",
                Opcode::Protocols as u16,
                proto_op
            )));
        }
        let proto_len = stream.read_u16().await? as usize;
        let proto_bytes = stream.read_bytes(proto_len).await?;
        let server_caps = ServerCapabilities::parse_protocols(&proto_bytes);
        caps.has_pam = server_caps.has_pam;
        caps.remove_64k_limit = server_caps.remove_64k_limit;
        caps.extended_describe = server_caps.extended_describe;
        caps.large_tuple_mode = server_caps.large_tuple_mode;
        caps.long_row_id = server_caps.long_row_id;

        // Drain padding if length was odd
        if proto_len % 2 != 0 {
            stream.drain(1).await?;
        }

        // Drain trailing SQ_EOT
        let trailing_eot = stream.read_u16().await?;
        if trailing_eot != Opcode::Eot as u16 {
            return Err(Error::Protocol(format!(
                "SQ_PROTOCOLS expected trailing EOT, got {trailing_eot}"
            )));
        }

        // Step 4: PAM Handshake if required
        if caps.has_pam {
            debug!("Starting PAM challenge-response authentication");
            // Send SQ_ACK + SQ_EOT
            let ack_eot = [0x00, Opcode::Ack as u8, 0x00, Opcode::Eot as u8];
            stream.write_all(&ack_eot).await?;
            stream.flush().await?;

            loop {
                let op = stream.read_u16().await?;
                if op == Opcode::Accept as u16 {
                    break;
                } else if op == Opcode::Exit as u16 {
                    return Err(Error::Authentication("Server rejected credentials (SQ_EXIT)".into()));
                } else if op == Opcode::Challenge as u16 {
                    let mut ch_hdr = [0u8; 6];
                    stream.read_exact(&mut ch_hdr).await?;
                    let style = u16::from_be_bytes([ch_hdr[0], ch_hdr[1]]);
                    let chal_len = u16::from_be_bytes([ch_hdr[4], ch_hdr[5]]) as usize;
                    let to_drain = chal_len + if chal_len % 2 != 0 { 1 } else { 0 };
                    stream.drain(to_drain).await?;

                    if style == 3 || style == 4 {
                        // Informational prompt: reply with ACK+EOT
                        let resp = [0x00, Opcode::Ack as u8, 0x00, Opcode::Eot as u8];
                        stream.write_all(&resp).await?;
                        stream.flush().await?;
                    } else {
                        // Password prompt: reply with password
                        let pass_bytes = params.password.as_bytes();
                        let mut resp = bytes::BytesMut::with_capacity(4 + pass_bytes.len() + 2);
                        resp.extend_from_slice(&[0x00, Opcode::Response as u8]);
                        resp.extend_from_slice(&((pass_bytes.len() + 1) as u16).to_be_bytes());
                        resp.extend_from_slice(pass_bytes);
                        resp.extend_from_slice(&[0x00]); // NUL
                        if (pass_bytes.len() + 1) % 2 != 0 {
                            resp.extend_from_slice(&[0x00]); // Pad
                        }
                        resp.extend_from_slice(&[0x00, Opcode::Eot as u8]);
                        stream.write_all(&resp).await?;
                        stream.flush().await?;
                    }
                } else {
                    return Err(Error::Protocol(format!("Unexpected opcode in PAM handshake: {op}")));
                }
            }
        }

        // Step 5: Send SQ_INFO
        debug!("Sending SQ_INFO environment variables");
        let info_pkt = build_sq_info_packet();
        stream.write_all(&info_pkt).await?;
        stream.flush().await?;

        let info_resp = stream.read_u16().await?;
        if info_resp != Opcode::Eot as u16 {
            return Err(Error::Protocol(format!(
                "SQ_INFO expected EOT, got {info_resp}"
            )));
        }

        // Step 6: Send SQ_DBOPEN if database name was specified
        if let Some(ref db_name) = params.database {
            debug!("Opening database: {}", db_name);
            let dbopen_pkt = build_sq_dbopen_packet(db_name);
            stream.write_all(&dbopen_pkt).await?;
            stream.flush().await?;

            let mut conn = Self {
                stream,
                params: params.clone(),
                capabilities: caps,
                state: ConnectionState::Connecting,
                in_transaction: false,
                autocommit: true,
                in_batch: false,
                lo_create_fphandle: 0,
                lo_create_dbname: String::new(),
                lifecycle: Arc::new(AtomicU8::new(0)),
            };

            // First turn for DBOPEN (reads DBOPEN_FLAGS, COMMAND, DONE, COST, etc. up to EOT)
            conn.dispatch_turn(TurnResult::default()).await?;

            // Drain any pending data from second turn if available
            conn.drain_pending_turns().await?;

            conn.state = ConnectionState::Ready;
            info!("Informix connection successfully established");
            return Ok(conn);
        }

        info!("Informix connection successfully established");
        Ok(Self {
            stream,
            params,
            capabilities: caps,
            state: ConnectionState::Ready,
            in_transaction: false,
            autocommit: true,
            in_batch: false,
            lo_create_fphandle: 0,
            lo_create_dbname: String::new(),
            lifecycle: Arc::new(AtomicU8::new(0)),
        })
    }

    /// Returns the negotiated server capabilities for this connection session.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// Returns `true` if this connection has been permanently discarded after a cancellation
    /// attempt via [`execute_stmt_cancelable`](Self::execute_stmt_cancelable). Matches
    /// `libsqli`'s `SQLI_CONN_DISCARDED` lifecycle bit.
    pub fn is_discarded(&self) -> bool {
        self.lifecycle.load(Ordering::SeqCst) & LC_DISCARDED != 0
    }

    fn ensure_not_discarded(&self) -> Result<()> {
        if self.is_discarded() {
            return Err(Error::InvalidState(
                "connection was discarded after a cancellation attempt and can no longer be used"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Returns the current connection state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Returns a reference to the parameters used to establish this connection.
    pub fn params(&self) -> &ConnectParams {
        &self.params
    }

    /// Returns a mutable reference to the parameters used for this connection.
    pub fn params_mut(&mut self) -> &mut ConnectParams {
        &mut self.params
    }

    /// Sets the currently active database name in the connection parameters.
    pub fn set_current_database(&mut self, db: String) {
        self.params.database = Some(db);
    }

    /// Checks if the underlying socket is still responsive by executing a lightweight probe.
    pub async fn ping(&mut self) -> Result<()> {
        let _ = self.query("SELECT FIRST 1 1 FROM systables").await?;
        Ok(())
    }

    /// Returns `true` if the connection state is currently [`ConnectionState::Ready`].
    pub fn is_alive(&self) -> bool {
        self.state == ConnectionState::Ready
    }

    async fn read_str(&mut self) -> Result<String> {
        let len = self.stream.read_u16().await? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.stream.read_bytes(len).await?;
        if len % 2 != 0 {
            self.stream.drain(1).await?;
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    async fn handle_cost(&mut self) -> Result<()> {
        if let Some(next) = self.stream.peek_u16(10).await? {
            if is_cost_follow_opcode(next) {
                return Ok(());
            }
        }
        let _est_rows = self.stream.read_u32().await?;
        let _cost = self.stream.read_u32().await?;
        Ok(())
    }

    async fn drain_info(&mut self) -> Result<()> {
        loop {
            let item_type = self.stream.read_u16().await?;
            if item_type == 0 {
                break;
            }
            let data_len = self.stream.read_u16().await? as usize;
            let pad = if data_len % 2 != 0 { 1 } else { 0 };
            self.stream.drain(data_len + pad).await?;
        }
        Ok(())
    }

    /// Parses a received `SQ_ERR` (opcode 13) payload (already past the opcode itself) into
    /// an [`Error::Server`], matching the sqlcode-dependent trailing-field layouts `libsqli`'s
    /// dispatcher handles (`-619`, `-368`, `0`/`-937`/`100`, and the default case), and drains
    /// the immediately-following `SQ_EOT` if already buffered. Shared by [`dispatch_turn`]'s
    /// own SQ_ERR case and [`fetchblob_materialize`](Self::fetchblob_materialize)'s response
    /// loop, which can also receive a server error mid-materialization.
    async fn read_sq_err(&mut self) -> Result<Error> {
        let sqlcode = self.stream.read_u16().await? as i16;
        let isamcode = self.stream.read_u16().await? as i16;
        let remove64k = self.capabilities.remove_64k_limit;
        let (sqlstate, message) = if sqlcode == -619 {
            self.stream.drain(if remove64k { 6 } else { 4 }).await?;
            let msg = self.read_str().await?;
            ("".to_string(), msg)
        } else if sqlcode == -368 {
            ("".to_string(), "".to_string())
        } else if sqlcode == 0 || sqlcode == -937 || sqlcode == 100 {
            let mut st = [0u8; 5];
            self.stream.read_exact(&mut st).await?;
            let msg = self.read_str().await?;
            (String::from_utf8_lossy(&st).to_string(), msg)
        } else {
            self.stream.drain(if remove64k { 4 } else { 2 }).await?;
            let msg = self.read_str().await?;
            ("".to_string(), msg)
        };

        if let Some(12) = self.stream.peek_u16(10).await? {
            let _ = self.stream.read_u16().await?;
        }

        Ok(Error::Server {
            sqlcode: sqlcode as i32,
            isamcode: isamcode as i32,
            sqlstate,
            message,
        })
    }

    /// Materializes a legacy `BYTE`/`TEXT` column's actual bytes via a dedicated
    /// `SQ_FETCHBLOB` round-trip, given the raw 56-byte locator descriptor already present
    /// in the fetched row's tuple data. Matches `libsqli`'s `sqli_fetchblob_materialize`:
    /// sends `SQ_ID` + `SQ_FETCHBLOB`(38) + the locator bytes (padded if odd) + `SQ_EOT`, then
    /// reads a response made of `SQ_COST`/`SQ_INFO` (ignored), `SQ_BLOB`(39) chunks and/or
    /// `SQ_LODATA`(97) chunk groups (both accumulated), and a trailing `SQ_DONE` -- looping
    /// until `SQ_EOT` ends the exchange. Must be called with the statement/cursor this
    /// locator came from still open, and before any other message is sent on the connection.
    async fn fetchblob_materialize(&mut self, stmt_id: u16, locator: &[u8]) -> Result<Vec<u8>> {
        let mut req = BytesMut::with_capacity(8 + locator.len() + 1);
        req.put_u16(Opcode::Id as u16);
        req.put_u16(stmt_id);
        req.put_u16(Opcode::Fetchblob as u16);
        req.put_slice(locator);
        if locator.len() % 2 != 0 {
            req.put_u8(0);
        }
        req.put_u16(Opcode::Eot as u16);
        self.stream.write_all(&req).await?;
        self.stream.flush().await?;

        let mut accum: Vec<u8> = Vec::new();
        loop {
            let opcode = self.stream.read_u16().await?;
            match opcode {
                x if x == Opcode::Cost as u16 => self.handle_cost().await?,
                x if x == Opcode::Info as u16 => self.drain_info().await?,
                x if x == Opcode::Blob as u16 => {
                    let chunk_len = self.stream.read_u16().await? as usize;
                    let chunk = self.stream.read_bytes(chunk_len).await?;
                    accum.extend_from_slice(&chunk);
                    if chunk_len % 2 != 0 {
                        self.stream.drain(1).await?;
                    }
                }
                97 => {
                    // SQ_LODATA: op_type(2, ignored) + file_size(4, BE i32) + chunked data
                    let _op_type = self.stream.read_u16().await?;
                    let file_size = self.stream.read_u32().await? as i32;
                    if file_size < 0 {
                        return Err(Error::Protocol("SQ_LODATA negative file size".into()));
                    }
                    let mut remaining = file_size as usize;
                    while remaining > 0 {
                        let chunk_len = self.stream.read_u16().await? as usize;
                        if chunk_len > remaining {
                            return Err(Error::Protocol("SQ_LODATA chunk exceeds file size".into()));
                        }
                        let chunk = self.stream.read_bytes(chunk_len).await?;
                        accum.extend_from_slice(&chunk);
                        if chunk_len % 2 != 0 {
                            self.stream.drain(1).await?;
                        }
                        remaining -= chunk_len;
                    }
                }
                x if x == Opcode::Done as u16 => {
                    self.stream.drain(22).await?;
                }
                x if x == Opcode::Eot as u16 => break,
                x if x == Opcode::Err as u16 => return Err(self.read_sq_err().await?),
                other => {
                    return Err(Error::Protocol(format!(
                        "fetchblob_materialize: unexpected opcode {other}"
                    )));
                }
            }
        }

        Ok(accum)
    }

    /// Materializes every non-NULL legacy `BYTE`/`TEXT` column across `rows`, replacing each
    /// one's effective value (via [`Row::set_materialized_lob`]) with the real data fetched
    /// through [`fetchblob_materialize`](Self::fetchblob_materialize). A no-op when `columns`
    /// has no `BYTE`/`TEXT` entries, so ordinary result sets pay no extra round-trips. Must be
    /// called with `stmt_id`'s cursor/statement still open -- callers should materialize
    /// before closing a statement whose result set contained legacy LOB columns.
    async fn materialize_lob_columns(&mut self, stmt_id: u16, rows: &mut [Row]) -> Result<()> {
        let lob_indices: Vec<usize> = rows
            .first()
            .map(|r| {
                (0..r.column_count())
                    .filter(|&i| matches!(r.column(i).map(|c| c.col_type), Some(ColumnType::Byte) | Some(ColumnType::Text)))
                    .collect()
            })
            .unwrap_or_default();

        if lob_indices.is_empty() {
            return Ok(());
        }

        for row in rows.iter_mut() {
            for &idx in &lob_indices {
                let locator = match row.get_raw_column(idx)? {
                    Some(bytes) => bytes.to_vec(),
                    None => continue, // NULL -- nothing to materialize
                };
                let data = self.fetchblob_materialize(stmt_id, &locator).await?;
                row.set_materialized_lob(idx, data);
            }
        }
        Ok(())
    }

    pub async fn drain_pending_turns(&mut self) -> Result<()> {
        for _ in 0..8 {
            if let Some(_next) = self.stream.peek_u16(15).await? {
                self.dispatch_turn(TurnResult::default()).await?;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Central dispatch loop for an Informix SQLI message turn.
    pub async fn dispatch_turn(&mut self, mut turn: TurnResult) -> Result<TurnResult> {
        let mut eof = false;

        while !eof {
            let opcode = self.stream.read_u16().await?;
            trace!("dispatch read opcode: {opcode}");

            match opcode {
                0 => {
                    // SQ_DBOPEN_FLAGS
                    let _flags = self.stream.read_u16().await?;
                }
                1 => {
                    // SQ_COMMAND
                    if let Some(next) = self.stream.peek_u16(10).await? {
                        if !is_likely_opcode(next) {
                            let _payload = self.stream.read_u32().await?;
                        }
                    } else {
                        let _payload = self.stream.read_u32().await?;
                    }
                }
                2 => {
                    // SQ_PREPARE ack
                }
                6 => {
                    // SQ_OPEN ack
                }
                8 => {
                    // SQ_DESCRIBE
                    turn.statement_type = self.stream.read_u16().await?;
                    turn.statement_id = self.stream.read_u16().await?;
                    let _cost_raw = self.stream.read_u32().await?;
                    let _tuple_size = self.stream.read_u16().await?;
                    let field_count = self.stream.read_u16().await? as usize;

                    let names_length = if self.capabilities.extended_describe {
                        self.stream.read_u32().await? as usize
                    } else {
                        self.stream.read_u16().await? as usize
                    };

                    let mut fields_meta = Vec::with_capacity(field_count);
                    for _ in 0..field_count {
                        let field_idx = self.stream.read_u32().await? as i32;
                        let tuple_offset = self.stream.read_u32().await?;
                        let type_raw = self.stream.read_u16().await?;

                        let (encoded_len, extended_type, type_name) = if !self.capabilities.extended_describe {
                            let len = self.stream.read_u16().await? as u32;
                            (len, None, None)
                        } else {
                            let extended_info = self.stream.read_u32().await?;
                            let o_len = self.stream.read_u16().await? as usize;
                            if o_len > 0 { self.stream.drain(o_len).await?; }
                            if o_len % 2 != 0 { self.stream.drain(1).await?; }
                            let n_len = self.stream.read_u16().await? as usize;
                            let t_name = if n_len > 0 {
                                let bytes = self.stream.read_bytes(n_len).await?;
                                Some(String::from_utf8_lossy(&bytes).to_string())
                            } else {
                                None
                            };
                            if n_len % 2 != 0 { self.stream.drain(1).await?; }
                            let _reference = self.stream.read_u16().await?;
                            let _alignment = self.stream.read_u16().await?;
                            let _source_type = self.stream.read_u32().await?;
                            let len = self.stream.read_u32().await?;
                            (len, Some(extended_info), t_name)
                        };

                        fields_meta.push((field_idx, tuple_offset, type_raw, encoded_len, extended_type, type_name));
                    }

                    let names_buf = if names_length > 0 {
                        self.stream.read_bytes(names_length).await?
                    } else {
                        BytesMut::new()
                    };
                    if names_length % 2 != 0 {
                        self.stream.drain(1).await?;
                    }

                    turn.columns.clear();
                    let mut name_offset = 0;
                    for (field_idx, tuple_offset, type_raw, encoded_len, extended_type, type_name) in fields_meta {
                        let col_name = if name_offset < names_buf.len() {
                            let start = name_offset;
                            let end = names_buf[start..]
                                .iter()
                                .position(|&b| b == 0)
                                .map(|p| start + p)
                                .unwrap_or(names_buf.len());
                            let n = String::from_utf8_lossy(&names_buf[start..end]).to_string();
                            name_offset = (end + 1).min(names_buf.len());
                            n
                        } else {
                            format!("col_{field_idx}")
                        };

                        // Map extended types (e.g. extended_info 10 = BLOB, 11 = CLOB, 1 = LVARCHAR, 5 = BOOL)
                        let col_type = match extended_type {
                            Some(10) => ColumnType::Blob,
                            Some(11) => ColumnType::Clob,
                            Some(1) => ColumnType::LVarChar,
                            Some(5) => ColumnType::Bool,
                            _ => ColumnType::from_raw(type_raw).unwrap_or(ColumnType::Char),
                        };

                        let mut col_info = ColumnInfo::new(
                            col_name,
                            col_type,
                            type_raw,
                            encoded_len,
                            tuple_offset,
                            field_idx,
                        );
                        if let Some(tn) = type_name {
                            col_info = col_info.with_type_name(tn);
                        }
                        turn.columns.push(col_info);
                    }
                }
                10 | 11 => {
                    // SQ_CLOSE, SQ_RELEASE
                }
                12 => {
                    // SQ_EOT
                    if let Some(_next) = self.stream.peek_u16(10).await? {
                        // More data follow in this turn
                    } else {
                        eof = true;
                    }
                }
                13 => {
                    // SQ_ERR
                    return Err(self.read_sq_err().await?);
                }
                14 => {
                    // SQ_TUPLE
                    let _warn = self.stream.read_u16().await?;
                    let tuple_size = self.stream.read_u32().await? as usize;
                    let tuple_bytes = self.stream.read_bytes(tuple_size).await?;
                    if tuple_size % 2 != 0 {
                        self.stream.drain(1).await?;
                    }
                    let row = Row::with_locale(
                        tuple_bytes.to_vec(),
                        turn.columns.clone(),
                        Some(self.params.db_locale.clone()),
                    );
                    turn.rows.push(row);
                }
                15 => {
                    // SQ_DONE
                    turn.warnings = self.stream.read_u16().await?;
                    if self.capabilities.long_row_id {
                        let mut b8 = [0u8; 8];
                        self.stream.read_exact(&mut b8).await?;
                        turn.rows_affected = i64::from_be_bytes(b8);
                        self.stream.read_exact(&mut b8).await?;
                        turn.row_id = i64::from_be_bytes(b8);
                    } else {
                        let mut b4 = [0u8; 4];
                        self.stream.read_exact(&mut b4).await?;
                        turn.rows_affected = i32::from_be_bytes(b4) as i64;
                        self.stream.read_exact(&mut b4).await?;
                        turn.row_id = i32::from_be_bytes(b4) as i64;
                    }
                    let errd1 = self.stream.read_u32().await?;
                    if errd1 != 0 && turn.rows_affected > 0 {
                        turn.generated_serial = Some(errd1 as i32 as i64);
                    }

                    // Adaptive tail peek for errd2, errd3
                    if let Some(next) = self.stream.peek_u16(10).await? {
                        if !is_likely_opcode(next) && next != 0 {
                            let _errd2 = self.stream.read_u32().await?;
                            let _errd3 = self.stream.read_u32().await?;
                        }
                    }

                    // Drain trailing SQ_COST, SQ_INFO, SQ_DBOPEN_FLAGS, SQ_EOT if already buffered
                    loop {
                        if let Some(next) = self.stream.peek_u16(10).await? {
                            match next {
                                55 => {
                                    self.stream.read_u16().await?;
                                    self.handle_cost().await?;
                                }
                                81 => {
                                    self.stream.read_u16().await?;
                                    self.drain_info().await?;
                                }
                                0 => {
                                    self.stream.read_u16().await?;
                                    let _ = self.stream.read_u16().await?;
                                }
                                12 => {
                                    self.stream.read_u16().await?;
                                    break;
                                }
                                _ => break,
                            }
                        } else {
                            break;
                        }
                    }

                    turn.saw_done = true;
                    eof = true;
                }
                25 => {
                    // SQ_TUPID
                    let _tupid = self.stream.read_u32().await?;
                }
                36 => {
                    // SQ_DBOPEN echo
                    let name_len = self.stream.read_u16().await? as usize;
                    self.stream.drain(name_len + 2).await?;
                    let _eot = self.stream.read_u16().await?;
                }
                37 => {
                    // SQ_DBCLOSE
                }
                55 => {
                    // SQ_COST
                    self.handle_cost().await?;
                }
                56 => {
                    // SQ_EXIT
                    return Err(Error::Protocol("Server sent SQ_EXIT".into()));
                }
                81 => {
                    // SQ_INFO
                    self.drain_info().await?;
                }
                94 => {
                    // SQ_INSERTDONE: 10-byte SERIAL8 payload (Informix INT8
                    // wire format), overriding any 32-bit value already
                    // captured from DONE's errd1 for SERIAL8/BIGSERIAL columns.
                    let mut buf10 = [0u8; 10];
                    self.stream.read_exact(&mut buf10).await?;
                    if let Some(generated) = decode_ifx_int8_serial(&buf10) {
                        turn.generated_serial = Some(generated);
                    }
                }
                99 => {
                    // SQ_XACTSTAT
                    self.stream.drain(6).await?;
                }
                other => {
                    return Err(Error::Protocol(format!("Unhandled opcode: {other}")));
                }
            }
        }

        Ok(turn)
    }

    /// Prepares an SQL statement on the server.
    ///
    /// Sends `SQ_PREPARE` along with parameter placeholder count, query string,
    /// `SQ_NDESCRIBE`, and `SQ_WANTDONE`. The returned [`PreparedStatement`] contains
    /// metadata about the statement (ID, parameter count, and output column descriptors).
    ///
    /// # Errors
    /// Returns [`Error::Server`] if syntax error, missing table, or permission denied occurs.
    pub async fn prepare(&mut self, sql: &str) -> Result<PreparedStatement> {
        self.ensure_not_discarded()?;
        self.drain_pending_turns().await?;

        let qmarks = sql.chars().filter(|&c| c == '?').count() as u16;
        let sql_bytes = crate::types::charset::encode_string_to_db(sql, Some(&self.params.db_locale))?;

        let mut pkt = BytesMut::with_capacity(16 + sql_bytes.len());
        pkt.put_u16(Opcode::Prepare as u16); // 2
        pkt.put_u16(qmarks);

        if self.capabilities.remove_64k_limit {
            pkt.put_u32(sql_bytes.len() as u32);
        } else {
            pkt.put_u16(sql_bytes.len() as u16);
        }
        pkt.put_slice(&sql_bytes);
        if sql_bytes.len() % 2 != 0 {
            pkt.put_u8(0); // Pad
        }

        // Chained tail: SQ_NDESCRIBE + SQ_WANTDONE + SQ_EOT
        pkt.put_u16(Opcode::Ndescribe as u16); // 22
        pkt.put_u16(Opcode::Wantdone as u16);   // 49
        pkt.put_u16(Opcode::Eot as u16);        // 12

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let turn = self.dispatch_turn(TurnResult::default()).await?;

        Ok(PreparedStatement::new(
            turn.statement_id,
            turn.statement_type,
            qmarks as usize,
            turn.columns,
        ))
    }

    /// Executes a non-query DDL or DML statement (INSERT, UPDATE, DELETE, CREATE, DROP, etc.)
    /// and returns the number of affected rows plus any generated `SERIAL` value.
    ///
    /// Automatically prepares the statement, executes it via `SQ_EXECUTE`, reads the
    /// resulting `SQ_DONE` message for rows affected, and frees the statement on the server.
    ///
    /// # Example
    /// ```no_run
    /// # use sqlicon_rs::Connection;
    /// # async fn run(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    /// let result = conn.execute("UPDATE customers SET status = 'ACTIVE' WHERE balance > 0").await?;
    /// println!("Updated {} rows", result.rows_affected);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&mut self, sql: &str) -> Result<ExecuteResult> {
        let stmt = self.prepare(sql).await?;
        let stmt_id = stmt.id();

        // Send SQ_ID + SQ_EXECUTE + SQ_EOT
        let mut exec_pkt = BytesMut::with_capacity(8);
        exec_pkt.put_u16(Opcode::Id as u16);
        exec_pkt.put_u16(stmt_id);
        exec_pkt.put_u16(Opcode::Execute as u16);
        exec_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&exec_pkt).await?;
        self.stream.flush().await?;

        let turn = self.dispatch_turn(TurnResult::default()).await?;
        if stmt.is_query() {
            self.close_statement(stmt_id).await?;
        }
        Ok(ExecuteResult {
            rows_affected: turn.rows_affected,
            generated_serial: turn.generated_serial,
        })
    }

    /// Streams the actual bytes for any parameter bound against a server-declared legacy
    /// `BYTE`/`TEXT` placeholder, via `SQ_BBIND` (opcode 41) followed by one or more data
    /// chunks (opcode 39, max 1024 bytes each, odd lengths padded) and a zero-length
    /// terminator chunk per parameter. Matches `libsqli`'s `sqli_stmt_execute_bound_params`:
    /// the main `SQ_BIND` message only carries a 56-byte placeholder descriptor for such
    /// parameters (see [`ParameterValue::encode_bind_entry`]); the real data follows here.
    /// Must be sent after the `SQ_BIND` message and before `SQ_EXECUTE`. A no-op if no
    /// parameter needs streaming.
    async fn send_legacy_lob_stream(
        &mut self,
        stmt: &PreparedStatement,
        params: &[crate::statement::ParameterValue],
    ) -> Result<()> {
        let streaming: Vec<(usize, &crate::statement::ParameterValue, ColumnType)> = params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let server_type = stmt.param_server_type(i)?;
                p.needs_lob_streaming(Some(server_type)).then_some((i, p, server_type))
            })
            .collect();

        if streaming.is_empty() {
            return Ok(());
        }

        let mut hdr = BytesMut::with_capacity(4);
        hdr.put_u16(Opcode::Bbind as u16);
        hdr.put_u16(streaming.len() as u16);
        self.stream.write_all(&hdr).await?;

        for (_, p, server_type) in streaming {
            let data = p.lob_stream_bytes(Some(&self.params.db_locale), server_type)?;
            let mut remaining = data.as_slice();
            while !remaining.is_empty() {
                let chunk_len = remaining.len().min(1024);
                let (chunk, rest) = remaining.split_at(chunk_len);
                remaining = rest;

                let mut ch = BytesMut::with_capacity(4 + chunk_len + 1);
                ch.put_u16(Opcode::Blob as u16);
                ch.put_u16(chunk_len as u16);
                ch.put_slice(chunk);
                if chunk_len % 2 != 0 {
                    ch.put_u8(0);
                }
                self.stream.write_all(&ch).await?;
            }

            // Zero-length terminator chunk for this parameter.
            let mut term = BytesMut::with_capacity(4);
            term.put_u16(Opcode::Blob as u16);
            term.put_u16(0);
            self.stream.write_all(&term).await?;
        }

        self.stream.flush().await?;
        Ok(())
    }

    /// Executes a previously prepared DML/DDL statement with bound parameters.
    ///
    /// Binds parameter values via `SQ_BIND` and executes the statement with `SQ_EXECUTE`.
    /// Does not close the prepared statement, allowing it to be reused multiple times.
    ///
    /// # Arguments
    /// * `stmt` - Reference to a [`PreparedStatement`] obtained from [`prepare`](Self::prepare).
    /// * `params` - Slice of [`crate::statement::ParameterValue`] arguments to bind.
    pub async fn execute_stmt(
        &mut self,
        stmt: &PreparedStatement,
        params: &[crate::statement::ParameterValue],
    ) -> Result<ExecuteResult> {
        self.ensure_not_discarded()?;
        let stmt_id = stmt.id();

        // 1. If parameters are present, send SQ_ID + SQ_BIND + paramCount + params + SQ_EOT
        if !params.is_empty() {
            let mut bind_pkt = BytesMut::with_capacity(32 + params.len() * 16);
            bind_pkt.put_u16(Opcode::Id as u16);
            bind_pkt.put_u16(stmt_id);
            bind_pkt.put_u16(Opcode::Bind as u16);
            bind_pkt.put_u16(params.len() as u16);
            for (i, p) in params.iter().enumerate() {
                p.encode_bind_entry(&mut bind_pkt, Some(&self.params.db_locale), stmt.param_server_type(i))?;
            }
            bind_pkt.put_u16(Opcode::Eot as u16);

            self.stream.write_all(&bind_pkt).await?;
            self.stream.flush().await?;

            self.send_legacy_lob_stream(stmt, params).await?;
        }

        // 2. Send SQ_ID + SQ_EXECUTE + SQ_EOT
        let mut exec_pkt = BytesMut::with_capacity(8);
        exec_pkt.put_u16(Opcode::Id as u16);
        exec_pkt.put_u16(stmt_id);
        exec_pkt.put_u16(Opcode::Execute as u16);
        exec_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&exec_pkt).await?;
        self.stream.flush().await?;

        let turn = self.dispatch_turn(TurnResult::default()).await?;
        Ok(ExecuteResult {
            rows_affected: turn.rows_affected,
            generated_serial: turn.generated_serial,
        })
    }

    /// Prepares a callable statement (`EXECUTE PROCEDURE`/`EXECUTE FUNCTION`/`CALL`) for use
    /// with [`call_execute`](Self::call_execute).
    ///
    /// This is a thin wrapper around [`prepare`](Self::prepare) -- matching `libsqli`'s
    /// `sqli_call_prepare`, which has no wire protocol of its own -- so any SQL text `prepare`
    /// accepts works here too. The returned [`CallStatement`] defaults every placeholder to
    /// [`ParamMode::In`](crate::statement::ParamMode::In); use
    /// [`CallStatement::set_param_mode`] beforehand if some placeholders are `OUT`/`INOUT`.
    pub async fn call_prepare(&mut self, sql: &str) -> Result<CallStatement> {
        let stmt = self.prepare(sql).await?;
        Ok(CallStatement::new(stmt))
    }

    /// Executes a previously prepared callable statement, binding `params` positionally --
    /// every placeholder (`IN` and `INOUT` alike) must still be bound with a value, since the
    /// wire protocol has no distinct `OUT`-parameter encoding.
    ///
    /// Branches on [`PreparedStatement::is_query`] exactly like `libsqli`'s
    /// `sqli_call_execute`: a call whose `DESCRIBE` reported output columns is executed via
    /// the ordinary cursor path and its single output row is returned in
    /// [`CallResult::row`](crate::statement::CallResult::row) (mirroring `sqli_result_fetch`'s
    /// one-row pull after execute); a call with no output columns is just executed for its
    /// rows-affected count.
    pub async fn call_execute(
        &mut self,
        call: &CallStatement,
        params: &[crate::statement::ParameterValue],
    ) -> Result<CallResult> {
        if call.stmt().is_query() {
            let qr = self.query_stmt(call.stmt(), params).await?;
            Ok(CallResult {
                rows_affected: qr.rows_affected,
                generated_serial: qr.generated_serial,
                row: qr.rows.into_iter().next(),
                columns: qr.columns,
            })
        } else {
            let result = self.execute_stmt(call.stmt(), params).await?;
            Ok(CallResult {
                rows_affected: result.rows_affected,
                generated_serial: result.generated_serial,
                row: None,
                columns: Vec::new(),
            })
        }
    }

    /// Executes a previously prepared SELECT statement with bound parameters and returns results.
    ///
    /// Binds parameters, opens a cursor (`SQ_OPEN`), and drains all rows using batched `SQ_NFETCH`
    /// turns until `SQ_DONE` or EOF. Does not close the prepared statement.
    pub async fn query_stmt(
        &mut self,
        stmt: &PreparedStatement,
        params: &[crate::statement::ParameterValue],
    ) -> Result<QueryResult> {
        self.ensure_not_discarded()?;
        let stmt_id = stmt.id();
        let columns = stmt.columns_unchecked().to_vec();

        // 1. If parameters are present, send SQ_ID + SQ_BIND + paramCount + params + SQ_EOT
        if !params.is_empty() {
            let mut bind_pkt = BytesMut::with_capacity(32 + params.len() * 16);
            bind_pkt.put_u16(Opcode::Id as u16);
            bind_pkt.put_u16(stmt_id);
            bind_pkt.put_u16(Opcode::Bind as u16);
            bind_pkt.put_u16(params.len() as u16);
            for (i, p) in params.iter().enumerate() {
                p.encode_bind_entry(&mut bind_pkt, Some(&self.params.db_locale), stmt.param_server_type(i))?;
            }
            bind_pkt.put_u16(Opcode::Eot as u16);

            self.stream.write_all(&bind_pkt).await?;
            self.stream.flush().await?;

            self.send_legacy_lob_stream(stmt, params).await?;
        }

        // 2. Open cursor
        let curname = b"_ifxc000000000000";
        let mut open_pkt = BytesMut::with_capacity(32);
        open_pkt.put_u16(Opcode::Id as u16);
        open_pkt.put_u16(stmt_id);
        open_pkt.put_u16(Opcode::Curname as u16);
        open_pkt.put_u16(curname.len() as u16);
        open_pkt.put_slice(curname);
        if curname.len() % 2 != 0 {
            open_pkt.put_u8(0);
        }
        open_pkt.put_u16(Opcode::Open as u16);
        open_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&open_pkt).await?;
        self.stream.flush().await?;

        // Receive OPEN ack turn
        let _ = self.dispatch_turn(TurnResult::default()).await?;

        // 3. Fetch rows loop
        let mut all_rows = Vec::new();
        let rows_affected;
        let last_warnings;
        let mut generated_serial: Option<i64> = None;

        loop {
            let prev_count = all_rows.len();

            let fetch_bufsize = 32768u32;
            let mut fetch_pkt = BytesMut::with_capacity(16);
            fetch_pkt.put_u16(Opcode::Id as u16);
            fetch_pkt.put_u16(stmt_id);
            fetch_pkt.put_u16(Opcode::Nfetch as u16);
            fetch_pkt.put_u32(fetch_bufsize);
            fetch_pkt.put_u16(0);
            fetch_pkt.put_u16(Opcode::Eot as u16);

            self.stream.write_all(&fetch_pkt).await?;
            self.stream.flush().await?;

            let mut turn = TurnResult::default();
            turn.columns = columns.clone();
            turn = self.dispatch_turn(turn).await?;

            all_rows.extend(turn.rows);
            if turn.generated_serial.is_some() {
                generated_serial = turn.generated_serial;
            }

            if all_rows.len() == prev_count || turn.saw_done {
                rows_affected = turn.rows_affected;
                last_warnings = turn.warnings;
                break;
            }
        }

        self.materialize_lob_columns(stmt_id, &mut all_rows).await?;
        Ok(QueryResult::new(columns, all_rows, rows_affected, generated_serial, last_warnings))
    }

    /// Executes a SELECT query and returns the complete [`QueryResult`].
    ///
    /// Prepares the query, opens a cursor, fetches all rows until exhaustion, and closes
    /// the statement handle on the server.
    ///
    /// # Example
    /// ```no_run
    /// # use sqlicon_rs::Connection;
    /// # async fn run(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    /// let res = conn.query("SELECT tabid, tabname FROM systables").await?;
    /// for row in &res.rows {
    ///     println!("Table: {}", row.get_str(1)?.unwrap_or_default());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query(&mut self, sql: &str) -> Result<QueryResult> {
        let stmt = self.prepare(sql).await?;
        let stmt_id = stmt.id();
        let columns = stmt.columns_unchecked().to_vec();

        // 1. Open cursor
        let curname = b"_ifxc000000000000";
        let mut open_pkt = BytesMut::with_capacity(32);
        open_pkt.put_u16(Opcode::Id as u16);
        open_pkt.put_u16(stmt_id);
        open_pkt.put_u16(Opcode::Curname as u16); // 3
        open_pkt.put_u16(curname.len() as u16);
        open_pkt.put_slice(curname);
        if curname.len() % 2 != 0 {
            open_pkt.put_u8(0);
        }
        open_pkt.put_u16(Opcode::Open as u16); // 6
        open_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&open_pkt).await?;
        self.stream.flush().await?;

        // Receive OPEN ack turn
        let _ = self.dispatch_turn(TurnResult::default()).await?;

        // 2. Fetch rows loop
        let mut all_rows = Vec::new();
        let rows_affected;
        let last_warnings;
        let mut generated_serial: Option<i64> = None;

        loop {
            let prev_count = all_rows.len();

            let fetch_bufsize = 32768u32;
            let mut fetch_pkt = BytesMut::with_capacity(16);
            fetch_pkt.put_u16(Opcode::Id as u16);
            fetch_pkt.put_u16(stmt_id);
            fetch_pkt.put_u16(Opcode::Nfetch as u16); // 9
            fetch_pkt.put_u32(fetch_bufsize);
            fetch_pkt.put_u16(0); // array_size = 0
            fetch_pkt.put_u16(Opcode::Eot as u16);

            self.stream.write_all(&fetch_pkt).await?;
            self.stream.flush().await?;

            let mut turn = TurnResult::default();
            turn.columns = columns.clone();
            turn = self.dispatch_turn(turn).await?;

            all_rows.extend(turn.rows);
            if turn.generated_serial.is_some() {
                generated_serial = turn.generated_serial;
            }

            if all_rows.len() == prev_count || turn.saw_done {
                rows_affected = turn.rows_affected;
                last_warnings = turn.warnings;
                break;
            }
        }

        // Materialize legacy BYTE/TEXT locators while the cursor is still open (matching
        // libsqli's `has_lobs` guard that defers `sqli_stmt_close_release`), then close.
        self.materialize_lob_columns(stmt_id, &mut all_rows).await?;
        self.close_statement(stmt_id).await?;
        Ok(QueryResult::new(columns, all_rows, rows_affected, generated_serial, last_warnings))
    }

    /// Prepares an SQL query, opens a cursor, and returns a [`crate::result::RowStream`]
    /// for fetching rows incrementally one-by-one or in chunks.
    ///
    /// This avoids loading entire million-row result sets into memory at once.
    ///
    /// # Example
    /// ```no_run
    /// # use sqlicon_rs::Connection;
    /// # async fn run(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    /// let mut stream = conn.query_stream("SELECT tabid, tabname FROM systables").await?;
    /// while let Some(row) = stream.next().await? {
    ///     println!("Table: {}", row.get_str(1)?.unwrap_or_default());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_stream(&mut self, sql: &str) -> Result<crate::result::RowStream<'_>> {
        let stmt = self.prepare(sql).await?;
        let stmt_id = stmt.id();
        let columns = stmt.columns_unchecked().to_vec();

        // 1. Open cursor
        let curname = b"_ifxc000000000000";
        let mut open_pkt = BytesMut::with_capacity(32);
        open_pkt.put_u16(Opcode::Id as u16);
        open_pkt.put_u16(stmt_id);
        open_pkt.put_u16(Opcode::Curname as u16);
        open_pkt.put_u16(curname.len() as u16);
        open_pkt.put_slice(curname);
        if curname.len() % 2 != 0 {
            open_pkt.put_u8(0);
        }
        open_pkt.put_u16(Opcode::Open as u16);
        open_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&open_pkt).await?;
        self.stream.flush().await?;

        // Receive OPEN ack turn
        let _ = self.dispatch_turn(TurnResult::default()).await?;

        Ok(crate::result::RowStream::new(self, stmt_id, columns))
    }

    /// Fetches a batch of rows from an open cursor.
    pub(crate) async fn fetch_turn(&mut self, stmt_id: u16, columns: &[crate::result::ColumnInfo]) -> Result<TurnResult> {
        let fetch_bufsize = 32768u32;
        let mut fetch_pkt = BytesMut::with_capacity(16);
        fetch_pkt.put_u16(Opcode::Id as u16);
        fetch_pkt.put_u16(stmt_id);
        fetch_pkt.put_u16(Opcode::Nfetch as u16);
        fetch_pkt.put_u32(fetch_bufsize);
        fetch_pkt.put_u16(0); // array_size = 0
        fetch_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&fetch_pkt).await?;
        self.stream.flush().await?;

        let mut turn = TurnResult::default();
        turn.columns = columns.to_vec();
        let mut turn = self.dispatch_turn(turn).await?;

        // Legacy BYTE/TEXT tuple data carries only the 56-byte locator descriptor. The cursor
        // is still open at this point, so materialize the actual bytes now — matching libsqli's
        // `has_lobs` guard in `sqli_query.c`, which deliberately keeps the cursor open for
        // `sqli_fetchblob_materialize` when the result set contains legacy LOB columns.
        self.materialize_lob_columns(stmt_id, &mut turn.rows).await?;
        Ok(turn)
    }

    /// Sends one positioned fetch (`SQ_SFETCH`) and returns the resulting batch.
    ///
    /// Mirrors `libsqli`'s `sqli_send_scroll_fetch` (`sqli_protocol_write.c`):
    /// `SQ_ID + SQ_SFETCH(direction: u16 BE, index: i32 BE, bufsize: u32 BE) + SQ_EOT`. The
    /// response is framed identically to an ordinary `SQ_NFETCH` reply (TUPLE/BLOB/DONE/EOT),
    /// so it's read with the same [`dispatch_turn`](Self::dispatch_turn) used by
    /// [`fetch_turn`](Self::fetch_turn). `direction` is one of
    /// [`SFETCH_LAST`]/[`SFETCH_ABSOLUTE`]; `index` is the 1-based target row for `ABSOLUTE`
    /// and unused (sent as `0`) for `LAST`. Deliberately omits the `SQ_RET_TYPE` variable-
    /// column preamble C conditionally sends here, consistent with `fetch_turn`/`query_stmt`
    /// already omitting it for ordinary `SQ_NFETCH` fetches in this port.
    pub(crate) async fn sfetch_turn(
        &mut self,
        stmt_id: u16,
        columns: &[crate::result::ColumnInfo],
        direction: u16,
        index: i32,
    ) -> Result<TurnResult> {
        let fetch_bufsize = 32768u32;
        let mut pkt = BytesMut::with_capacity(20);
        pkt.put_u16(Opcode::Id as u16);
        pkt.put_u16(stmt_id);
        pkt.put_u16(Opcode::Sfetch as u16);
        pkt.put_u16(direction);
        pkt.put_i32(index);
        pkt.put_u32(fetch_bufsize);
        pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let mut turn = TurnResult::default();
        turn.columns = columns.to_vec();
        let mut turn = self.dispatch_turn(turn).await?;

        self.materialize_lob_columns(stmt_id, &mut turn.rows).await?;
        Ok(turn)
    }

    /// Opens a cursor for `stmt_id`, optionally marking it scrollable.
    ///
    /// Mirrors `libsqli`'s `sqli_send_open` (`sqli_protocol_write.c:156-202`): when
    /// `scrollable` is set, a bare `SQ_SCROLL` (opcode 24, no payload) is inserted between
    /// the `SQ_CURNAME` header and `SQ_OPEN`. Cursor holdability across commit (`SQ_HOLD`)
    /// is out of scope here and never sent.
    async fn open_cursor(&mut self, stmt_id: u16, scrollable: bool) -> Result<()> {
        let curname = b"_ifxc000000000000";
        let mut open_pkt = BytesMut::with_capacity(32);
        open_pkt.put_u16(Opcode::Id as u16);
        open_pkt.put_u16(stmt_id);
        open_pkt.put_u16(Opcode::Curname as u16);
        open_pkt.put_u16(curname.len() as u16);
        open_pkt.put_slice(curname);
        if curname.len() % 2 != 0 {
            open_pkt.put_u8(0);
        }
        if scrollable {
            open_pkt.put_u16(Opcode::Scroll as u16);
        }
        open_pkt.put_u16(Opcode::Open as u16);
        open_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&open_pkt).await?;
        self.stream.flush().await?;

        // Receive OPEN ack turn.
        let _ = self.dispatch_turn(TurnResult::default()).await?;
        Ok(())
    }

    /// Prepares `sql` and opens a scrollable cursor on it, returning a [`ScrollCursor`] for
    /// positioned navigation (`first`/`last`/`absolute`/`relative`/`previous`/`next`).
    ///
    /// Only meaningful for statements that produce result rows; returns
    /// [`Error::InvalidState`] otherwise (mirroring `libsqli`'s scrollability being gated on
    /// `statement_type == 2` in `sqli_result_is_server_scrollable`).
    pub async fn query_scrollable(&mut self, sql: &str) -> Result<crate::result::ScrollCursor<'_>> {
        let stmt = self.prepare(sql).await?;
        if !stmt.is_query() {
            return Err(Error::InvalidState(
                "query_scrollable requires a statement that produces result rows".into(),
            ));
        }
        let stmt_id = stmt.id();
        let columns = stmt.columns_unchecked().to_vec();
        self.open_cursor(stmt_id, true).await?;
        Ok(crate::result::ScrollCursor::new(self, stmt_id, columns))
    }

    /// Like [`query_scrollable`](Self::query_scrollable), but for a previously prepared
    /// statement with bound parameters.
    pub async fn query_stmt_scrollable(
        &mut self,
        stmt: &PreparedStatement,
        params: &[crate::statement::ParameterValue],
    ) -> Result<crate::result::ScrollCursor<'_>> {
        self.ensure_not_discarded()?;
        if !stmt.is_query() {
            return Err(Error::InvalidState(
                "query_stmt_scrollable requires a statement that produces result rows".into(),
            ));
        }
        let stmt_id = stmt.id();
        let columns = stmt.columns_unchecked().to_vec();

        if !params.is_empty() {
            let mut bind_pkt = BytesMut::with_capacity(32 + params.len() * 16);
            bind_pkt.put_u16(Opcode::Id as u16);
            bind_pkt.put_u16(stmt_id);
            bind_pkt.put_u16(Opcode::Bind as u16);
            bind_pkt.put_u16(params.len() as u16);
            for (i, p) in params.iter().enumerate() {
                p.encode_bind_entry(&mut bind_pkt, Some(&self.params.db_locale), stmt.param_server_type(i))?;
            }
            bind_pkt.put_u16(Opcode::Eot as u16);

            self.stream.write_all(&bind_pkt).await?;
            self.stream.flush().await?;

            self.send_legacy_lob_stream(stmt, params).await?;
        }

        self.open_cursor(stmt_id, true).await?;
        Ok(crate::result::ScrollCursor::new(self, stmt_id, columns))
    }

    /// Closes and releases a statement on the server.
    ///
    /// Sends `SQ_ID`, `SQ_CLOSE`, and `SQ_RELEASE` packets to deallocate cursor buffers
    /// and statement handles on the Informix engine.
    pub async fn close_statement(&mut self, stmt_id: u16) -> Result<()> {
        let mut pkt = BytesMut::with_capacity(12);
        pkt.put_u16(Opcode::Id as u16);
        pkt.put_u16(stmt_id);
        pkt.put_u16(Opcode::Close as u16);
        pkt.put_u16(Opcode::Release as u16);
        pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Whether the connection currently has an active transaction.
    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    /// Get current autocommit mode (default true).
    pub fn autocommit(&self) -> bool {
        self.autocommit
    }

    /// Set autocommit mode. Changing to true while in transaction returns InvalidState.
    pub fn set_autocommit(&mut self, on: bool) -> Result<()> {
        if on && !self.autocommit && self.in_transaction {
            return Err(Error::InvalidState(
                "commit or roll back the active transaction before enabling autocommit".into(),
            ));
        }
        self.autocommit = on;
        Ok(())
    }

    /// Starts a transaction and returns a [`Transaction`] scope that must be finished with
    /// [`Transaction::commit`] or [`Transaction::rollback`] -- a `#[must_use]`-enforced
    /// alternative to calling [`begin`](Self::begin)/[`commit`](Self::commit)/
    /// [`rollback`](Self::rollback) directly. See [`Transaction`]'s docs for why dropping it
    /// unfinished only logs a warning rather than rolling back automatically.
    pub async fn transaction(&mut self) -> Result<crate::connection::Transaction<'_>> {
        self.begin().await?;
        Ok(crate::connection::Transaction::new(self))
    }

    /// Starts a transaction using native `SQ_EOT` + `SQ_BEGIN` (opcode 35).
    ///
    /// # Errors
    /// Returns [`Error::InvalidState`] if a transaction is already in progress.
    pub async fn begin(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        if self.in_transaction {
            return Err(Error::InvalidState("Transaction already in progress".into()));
        }

        let mut pkt = BytesMut::with_capacity(4);
        pkt.put_u16(Opcode::Eot as u16);
        pkt.put_u16(Opcode::Begin as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        self.in_transaction = true;
        self.state = ConnectionState::InTransaction;
        Ok(())
    }

    /// Commits the active transaction using native `SQ_EOT` + `SQ_CMMTWORK` (opcode 19).
    ///
    /// # Errors
    /// Returns [`Error::InvalidState`] if no transaction is active.
    pub async fn commit(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        if !self.in_transaction {
            return Err(Error::InvalidState("No active transaction to commit".into()));
        }

        let mut pkt = BytesMut::with_capacity(4);
        pkt.put_u16(Opcode::Eot as u16);
        pkt.put_u16(Opcode::Cmmtwork as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        self.in_transaction = false;
        self.state = ConnectionState::Ready;
        Ok(())
    }

    /// Rolls back the active transaction using native `SQ_EOT` + `SQ_RBWORK` (opcode 20).
    ///
    /// # Errors
    /// Returns [`Error::InvalidState`] if no transaction is active.
    pub async fn rollback(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        if !self.in_transaction {
            return Err(Error::InvalidState("No active transaction to roll back".into()));
        }

        let mut pkt = BytesMut::with_capacity(6);
        pkt.put_u16(Opcode::Eot as u16);
        pkt.put_u16(Opcode::Rbwork as u16);
        pkt.put_u16(0x0000); // 2 bytes padding required by protocol

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        self.in_transaction = false;
        self.state = ConnectionState::Ready;
        Ok(())
    }

    /// Sets the transaction isolation level for subsequent operations.
    ///
    /// Corresponds to `SET ISOLATION TO ...` in Informix SQL.
    ///
    /// # Errors
    /// Returns [`Error::InvalidState`] if called while inside an active transaction.
    pub async fn set_isolation_level(&mut self, level: IsolationLevel) -> Result<()> {
        if self.in_transaction {
            return Err(Error::InvalidState(
                "Isolation level cannot be changed in active transaction".into(),
            ));
        }

        let sql = match level {
            IsolationLevel::CommittedRead => "set isolation to committed read",
            IsolationLevel::RepeatableRead | IsolationLevel::Serializable => {
                "set isolation to repeatable read"
            }
            IsolationLevel::CursorStability => "set isolation to cursor stability",
        };

        self.execute(sql).await?;
        Ok(())
    }

    /// Sets lock wait behavior in seconds (negative for `NOT WAIT`).
    ///
    /// Corresponds to `SET LOCK MODE TO WAIT [n]` or `NOT WAIT`.
    pub async fn set_lock_wait(&mut self, seconds: i32) -> Result<()> {
        let sql = if seconds < 0 {
            "set lock mode to not wait".to_string()
        } else {
            format!("set lock mode to wait {}", seconds)
        };

        self.execute(&sql).await?;
        Ok(())
    }

    /// Ensures a transaction is active before a savepoint set/release/rollback is sent,
    /// matching `libsqli`'s `savepoint_send_core` guard (applied uniformly to all three
    /// savepoint operations): auto-begins when not in autocommit, otherwise errors.
    async fn ensure_savepoint_txn(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        if !self.in_transaction {
            if !self.autocommit {
                self.begin().await?;
            } else {
                return Err(Error::InvalidState(
                    "Savepoint requires an active transaction".into(),
                ));
            }
        }
        Ok(())
    }

    /// Creates a transaction savepoint with the given name.
    ///
    /// Uses native wire opcode `SQ_SQLISETSVPT`.
    ///
    /// # Arguments
    /// * `name` - Identifier for the savepoint.
    /// * `unique` - Whether the savepoint name must be unique within the transaction.
    pub async fn savepoint_set(&mut self, name: &str, unique: bool) -> Result<()> {
        self.ensure_savepoint_txn().await?;

        let mut pkt = BytesMut::with_capacity(16 + name.len());
        pkt.put_u16(Opcode::Sqlisetsvpt as u16);
        pkt.put_u16(name.len() as u16);
        pkt.put_slice(name.as_bytes());
        if name.len() % 2 != 0 {
            pkt.put_u8(0);
        }
        pkt.put_u16(if unique { 1 } else { 0 });
        pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        Ok(())
    }

    /// Releases a transaction savepoint previously established with [`savepoint_set`](Self::savepoint_set).
    ///
    /// Uses native wire opcode `SQ_SQLIRELSVPT`.
    pub async fn savepoint_release(&mut self, name: &str) -> Result<()> {
        self.ensure_savepoint_txn().await?;

        let mut pkt = BytesMut::with_capacity(16 + name.len());
        pkt.put_u16(Opcode::Sqlirelsvpt as u16);
        pkt.put_u16(name.len() as u16);
        pkt.put_slice(name.as_bytes());
        if name.len() % 2 != 0 {
            pkt.put_u8(0);
        }
        pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        Ok(())
    }

    /// Rolls back changes to a transaction savepoint previously established with [`savepoint_set`](Self::savepoint_set).
    ///
    /// Uses native wire opcode `SQ_SQLIRBACKSVPT`.
    pub async fn savepoint_rollback(&mut self, name: &str) -> Result<()> {
        self.ensure_savepoint_txn().await?;

        let mut pkt = BytesMut::with_capacity(16 + name.len());
        pkt.put_u16(Opcode::Sqlirbacksvpt as u16);
        pkt.put_u16(name.len() as u16);
        pkt.put_slice(name.as_bytes());
        if name.len() % 2 != 0 {
            pkt.put_u8(0);
        }
        pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        let _ = self.dispatch_turn(TurnResult::default()).await?;
        Ok(())
    }

    /// Begins a batch execution block using Informix `SQ_BATCHSTART` (opcode 101).
    pub async fn batch_begin(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        let mut pkt = BytesMut::with_capacity(4);
        pkt.put_u16(Opcode::Batchstart as u16);
        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;
        self.in_batch = true;
        Ok(())
    }

    /// Ends a batch execution block using Informix `SQ_BATCHEND` (opcode 102).
    pub async fn batch_end(&mut self) -> Result<()> {
        self.ensure_not_discarded()?;
        let mut pkt = BytesMut::with_capacity(4);
        pkt.put_u16(Opcode::Batchend as u16);
        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;
        self.in_batch = false;
        Ok(())
    }

    /// Executes a batch of SQL statements sequentially, collecting results for each statement.
    ///
    /// Wraps execution within `batch_begin` and `batch_end` framing.
    ///
    /// # Example
    /// ```no_run
    /// # use sqlicon_rs::Connection;
    /// # async fn run(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    /// let stmts = [
    ///     "INSERT INTO t1 VALUES (1, 'A')",
    ///     "INSERT INTO t1 VALUES (2, 'B')",
    ///     "INSERT INTO t1 VALUES (3, 'C')",
    /// ];
    /// let batch_res = conn.execute_batch(&stmts).await?;
    /// println!("Executed {} stmts ({} successful)", batch_res.len(), batch_res.success_count());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_batch(&mut self, sql_statements: &[&str]) -> Result<BatchResult> {
        self.batch_begin().await?;

        let mut items = Vec::with_capacity(sql_statements.len());
        for &sql in sql_statements {
            if sql.trim().is_empty() {
                items.push(BatchItemResult {
                    status: Ok(()),
                    rows_affected: 0,
                });
                continue;
            }

            match self.execute(sql).await {
                Ok(result) => {
                    items.push(BatchItemResult {
                        status: Ok(()),
                        rows_affected: result.rows_affected,
                    });
                }
                Err(e) => {
                    items.push(BatchItemResult {
                        status: Err(e),
                        rows_affected: 0,
                    });
                }
            }
        }

        self.batch_end().await?;
        Ok(BatchResult { items })
    }

    /// Executes a prepared DML statement with multiple parameter sets (parameter batching).
    ///
    /// Runs each row of parameters against the prepared statement, collecting individual execution outcomes.
    ///
    /// Matches `libsqli`'s `sqli_stmt_batch_execute`: a genuine per-row SQL error returned by
    /// the server (`Error::Server`) is recorded as that row's failure and the batch continues,
    /// but any other failure (I/O, protocol, local validation, etc.) means the connection may be
    /// desynchronized or the row never reached the server — the whole batch is aborted immediately
    /// and that error is propagated, discarding results collected so far (not reported as a
    /// per-item failure).
    pub async fn execute_stmt_batch(
        &mut self,
        stmt: &PreparedStatement,
        param_rows: &[Vec<crate::statement::ParameterValue>],
    ) -> Result<BatchResult> {
        let mut items = Vec::with_capacity(param_rows.len());
        for params in param_rows {
            match self.execute_stmt(stmt, params).await {
                Ok(result) => {
                    items.push(BatchItemResult {
                        status: Ok(()),
                        rows_affected: result.rows_affected,
                    });
                }
                Err(e @ Error::Server { .. }) => {
                    items.push(BatchItemResult {
                        status: Err(e),
                        rows_affected: 0,
                    });
                }
                Err(e) => {
                    // Non-SQL failure (I/O, protocol, local validation): the connection may be
                    // desynced or this row's SQ_EXECUTE never completed. Abort the whole batch
                    // rather than silently continuing, matching libsqli's fail-fast behavior.
                    return Err(e);
                }
            }
        }
        Ok(BatchResult { items })
    }

    /// Executes a previously prepared `UPDATE`/`DELETE`/`INSERT` with a fresh, single-use
    /// cancellation capability, matching `libsqli`'s `sqli_execute_cancelable`.
    ///
    /// Call [`operation.requester()`](CancelOperation::requester) *before* calling this method
    /// and hand the resulting [`CancelRequester`] to whichever task or thread should be able to
    /// interrupt the call -- once this method returns, the operation is terminal and the
    /// requester can no longer affect anything.
    ///
    /// Unlike plain [`execute_stmt`](Self::execute_stmt), this:
    /// - only supports prepared `UPDATE`/`DELETE`/`INSERT` on a TCP connection (never `SELECT`,
    ///   batches, LOB streaming, or scalar bindings requiring `WHERE CURRENT OF`);
    /// - requires an active transaction when autocommit is off (call [`begin`](Self::begin)
    ///   first);
    /// - **always discards the connection after any cancellation request is actually sent** --
    ///   the out-of-band `MSG_OOB` byte can desynchronize the wire even when the DML completes
    ///   successfully first, so the connection must never be reused afterwards. Every other
    ///   method on this `Connection` returns `Err(Error::InvalidState)` once that happens.
    ///
    /// Returns `Err` only for a precondition failure before the cancel operation was ever
    /// registered (no SQL was sent in that case). Once registered, this returns `Ok(snapshot)`
    /// regardless of whether the DML succeeded, was interrupted, or failed -- inspect
    /// [`CancelSnapshot::outcome`] to tell those apart.
    pub async fn execute_stmt_cancelable(
        &mut self,
        stmt: &PreparedStatement,
        params: &[crate::statement::ParameterValue],
        operation: &CancelOperation,
    ) -> Result<CancelSnapshot> {
        self.ensure_not_discarded()?;

        if !matches!(stmt.statement_type(), STMT_UPDATE | STMT_DELETE | STMT_INSERT) {
            return Err(Error::Unsupported(
                "cancelable execute only supports prepared UPDATE/DELETE/INSERT".into(),
            ));
        }
        if self.in_batch {
            return Err(Error::Unsupported(
                "cancelable execute is not supported inside a batch".into(),
            ));
        }
        if !self.autocommit && !self.in_transaction {
            return Err(Error::Unsupported(
                "cancelable execute requires an active transaction when autocommit is off (call begin() first)".into(),
            ));
        }
        if params.iter().enumerate().any(|(i, p)| {
            matches!(p, crate::statement::ParameterValue::Sblob(_, _))
                || p.needs_lob_streaming(stmt.param_server_type(i))
        }) {
            return Err(Error::Unsupported(
                "cancelable execute does not support Smart-LOB or legacy BYTE/TEXT streamed parameters".into(),
            ));
        }
        if !self.stream.is_tcp() {
            return Err(Error::Unsupported(
                "cancellation is only supported on TCP connections, not onipcstr".into(),
            ));
        }

        #[cfg(unix)]
        let raw_handle = self.stream.raw_fd();
        #[cfg(windows)]
        let raw_handle = self.stream.raw_socket();

        if !operation.register(self.lifecycle.clone(), raw_handle)? {
            // A cancellation was already requested before registration: no SQL was sent.
            return Ok(operation.snapshot()?);
        }

        let stmt_id = stmt.id();

        if !params.is_empty() {
            let mut bind_pkt = BytesMut::with_capacity(32 + params.len() * 16);
            bind_pkt.put_u16(Opcode::Id as u16);
            bind_pkt.put_u16(stmt_id);
            bind_pkt.put_u16(Opcode::Bind as u16);
            bind_pkt.put_u16(params.len() as u16);
            for (i, p) in params.iter().enumerate() {
                if let Err(e) =
                    p.encode_bind_entry(&mut bind_pkt, Some(&self.params.db_locale), stmt.param_server_type(i))
                {
                    operation.abort_unregistered();
                    return Err(e);
                }
            }
            bind_pkt.put_u16(Opcode::Eot as u16);

            if let Err(e) = self.stream.write_all(&bind_pkt).await.and(Ok(())) {
                operation.abort_unregistered();
                return Err(e);
            }
            if let Err(e) = self.stream.flush().await {
                operation.abort_unregistered();
                return Err(e);
            }
        }

        let mut exec_pkt = BytesMut::with_capacity(8);
        exec_pkt.put_u16(Opcode::Id as u16);
        exec_pkt.put_u16(stmt_id);
        exec_pkt.put_u16(Opcode::Execute as u16);
        exec_pkt.put_u16(Opcode::Eot as u16);

        if let Err(e) = self.stream.write_all(&exec_pkt).await {
            operation.abort_unregistered();
            return Err(e);
        }
        if let Err(e) = self.stream.flush().await {
            operation.abort_unregistered();
            return Err(e);
        }

        // From this point on, a cancel request fires the OOB interrupt immediately. If a
        // request already latched while we were sending, fire it now.
        operation.arm();

        let turn_result = self.dispatch_turn(TurnResult::default()).await;

        let mut generated_serial = None;
        let (status_ok, sqlcode, kind, message) = match &turn_result {
            Ok(turn) if turn.saw_done => {
                generated_serial = turn.generated_serial;
                (true, None, StatusKind::Ok, None)
            }
            Ok(_) => (
                false,
                None,
                StatusKind::ProtoError,
                Some("EXECUTE ended without a terminal DONE response".to_string()),
            ),
            Err(Error::Server { sqlcode, message, .. }) => {
                (false, Some(*sqlcode), StatusKind::Server, Some(message.clone()))
            }
            Err(Error::Io(io_err)) => {
                let kind = if io_err.kind() == std::io::ErrorKind::TimedOut {
                    StatusKind::Timeout
                } else {
                    StatusKind::IoError
                };
                (false, None, kind, Some(io_err.to_string()))
            }
            Err(Error::Protocol(msg)) => (false, None, StatusKind::ProtoError, Some(msg.clone())),
            Err(e) => (false, None, StatusKind::Other, Some(e.to_string())),
        };

        // Matches sqli_cancel_operation_finish: once armed, a non-OK result with no server
        // SQLCODE means we cannot tell whether the DML actually ran, so the connection must be
        // discarded regardless of whether a cancellation was ever requested.
        let response_unknown = !status_ok && sqlcode.unwrap_or(0) == 0;
        let outcome = if status_ok {
            CancelOutcome::Executed
        } else if sqlcode == Some(-213) {
            CancelOutcome::Interrupted
        } else if response_unknown || matches!(kind, StatusKind::IoError | StatusKind::Timeout) {
            CancelOutcome::Unknown
        } else {
            CancelOutcome::Failed
        };

        let mut snapshot = operation.finish(outcome, response_unknown, message);
        snapshot.generated_serial = generated_serial;
        Ok(snapshot)
    }

    /// Creates a new Smart Large Object (BLOB / CLOB) on the server and returns its handle.
    ///
    /// Implements the fastpath path of libsqli's `sqli_sblob_create` (`sblob_create_into`):
    /// the cached `SQ_GETROUTINE` handle for `function informix.ifx_lo_create( ifx_lo_spec,
    /// integer,blob)`, then an `SQ_EXFPROUTINE` request carrying the 596-byte `ifx_lo_spec`
    /// UDT, the open mode and a zero-filled locator, answered by `SQ_FPROUTINE` with the
    /// locator (output param 0) and the `lofd` descriptor (output param 1). The returned
    /// handle must be closed with [`sblob_close`](Self::sblob_close).
    pub async fn sblob_create(
        &mut self,
        sblob_type: SblobType,
        options: &SblobOptions,
    ) -> Result<SblobHandle> {
        self.ensure_not_discarded()?;
        if !self.is_alive() {
            return Err(Error::InvalidState(
                "connection is not ready for smartblob create".into(),
            ));
        }
        options.validate()?;

        let (fp_handle, dbname) = self.get_lo_create_fphandle().await?;

        // libsqli drains up to 8 pending tail packets from previous commands before the
        // fastpath request; `drain_pending_turns` is the async equivalent.
        self.drain_pending_turns().await?;

        let mode = options.effective_mode() as i32;
        let spec = build_ifx_lo_spec(options);

        let mut req = BytesMut::with_capacity(64 + IFX_LO_SPEC_SIZE + SBLOB_LOCATOR_MAX);
        req.put_u16(Opcode::Exfproutine as u16);
        let dblen = dbname.len().min(127);
        req.put_u16(dblen as u16);
        req.put_slice(dbname.as_bytes());
        if (2 + dblen) & 1 != 0 {
            req.put_u8(0);
        }
        req.put_i32(fp_handle);
        req.put_u16(3); // paramCount
        req.put_u16(0); // req_fparam

        req.put_u16(Opcode::Bind as u16);
        req.put_u16(3); // paramCount

        // Param 1: ifx_lo_spec UDT (type 44), payload = BE length + 596 spec bytes.
        req.put_u16(44);
        req.put_u16(0); // owner length
        req.put_u16(11); // table name length
        req.put_slice(b"ifx_lo_spec");
        if (2 + 11) & 1 != 0 {
            req.put_u8(0);
        }
        req.put_u16(0); // not null
        req.put_u16(0); // encoded length
        req.put_u32(IFX_LO_SPEC_SIZE as u32);
        req.put_slice(&spec);

        // Param 2: open mode (integer).
        req.put_u16(2);
        req.put_u16(0); // not null
        req.put_u16(0); // encoded length
        req.put_i32(mode);

        // Param 3: blob UDT, zero-filled locator placeholder.
        req.put_u16(44);
        req.put_u16(0); // owner length
        req.put_u16(4); // table name length
        req.put_slice(b"blob");
        req.put_u16(0); // not null
        req.put_u16(0); // encoded length
        req.put_u32(SBLOB_LOCATOR_MAX as u32);
        req.put_slice(&[0u8; SBLOB_LOCATOR_MAX]);

        req.put_u16(Opcode::Eot as u16);
        self.stream.write_all(&req).await?;
        self.stream.flush().await?;

        let op = loop {
            let op = self.stream.read_u16().await?;
            if op != Opcode::Eot as u16 {
                break op;
            }
        };
        if op == Opcode::Err as u16 {
            return Err(self.read_sq_err().await?);
        }
        if op != Opcode::Fproutine as u16 {
            return Err(Error::Protocol(format!(
                "unexpected opcode {op} in fastpath response (expected SQ_FPROUTINE)"
            )));
        }

        let num_params = self.stream.read_u16().await?;
        let mut created_lofd: i32 = -1;
        let mut created_locator: Vec<u8> = Vec::new();
        let mut locator_truncated = false;

        for i in 0..num_params {
            let raw_type = self.stream.read_u16().await?;
            let sqltype = raw_type & 0xFF;
            let is_distinct = (raw_type & 2048) != 0;

            if (sqltype >= 18 && sqltype != 52 && sqltype != 53) || is_distinct {
                let olen = self.stream.read_u16().await? as usize;
                if olen > 0 {
                    self.stream.drain(olen).await?;
                }
                if (2 + olen) & 1 != 0 {
                    self.stream.drain(1).await?;
                }
                let nlen = self.stream.read_u16().await? as usize;
                if nlen > 0 {
                    self.stream.drain(nlen).await?;
                }
                if (2 + nlen) & 1 != 0 {
                    self.stream.drain(1).await?;
                }
            }

            let indicator = self.stream.read_bytes(4).await?;
            let ind = i16::from_be_bytes([indicator[0], indicator[1]]);
            if ind == -1 {
                continue;
            }

            if matches!(sqltype, 40 | 41 | 44) {
                let udlen = self.stream.read_u32().await? as usize;
                let payload = self.stream.read_bytes(udlen).await?;
                if i == 0 {
                    if udlen > SBLOB_LOCATOR_MAX {
                        locator_truncated = true;
                    } else {
                        created_locator = payload.to_vec();
                    }
                }
                if udlen & 1 != 0 {
                    self.stream.drain(1).await?;
                }
            } else if matches!(sqltype, 2 | 6) {
                let int_buf = self.stream.read_bytes(4).await?;
                let value = i32::from_be_bytes([int_buf[0], int_buf[1], int_buf[2], int_buf[3]]);
                if i == 1 {
                    created_lofd = value;
                }
            } else {
                return Err(Error::Protocol(format!(
                    "unexpected output parameter type {sqltype} in SQ_FPROUTINE response"
                )));
            }
        }

        self.drain_sblob_trailing().await?;

        if locator_truncated {
            if created_lofd >= 0 {
                let _ = self.sblob_close(created_lofd).await;
            }
            return Err(Error::Protocol(
                "smart large object locator exceeds maximum buffer length".into(),
            ));
        }
        if created_lofd < 0 || created_locator.is_empty() {
            if created_lofd >= 0 {
                let _ = self.sblob_close(created_lofd).await;
            }
            return Err(Error::Protocol(
                "failed to retrieve smart large object descriptor or locator".into(),
            ));
        }

        Ok(SblobHandle {
            lofd: created_lofd,
            sblob_type,
            locator: created_locator,
        })
    }

    /// Resolves and caches the fastpath routine handle for `informix.ifx_lo_create` via
    /// `SQ_GETROUTINE`, mirroring libsqli's `sqli_get_lo_create_fphandle`. The handle is
    /// cached for the connection's lifetime, as in `sqli_conn_t`.
    async fn get_lo_create_fphandle(&mut self) -> Result<(i32, String)> {
        if self.lo_create_fphandle > 0 {
            return Ok((self.lo_create_fphandle, self.lo_create_dbname.clone()));
        }

        const SIGNATURE: &[u8] = b"function informix.ifx_lo_create( ifx_lo_spec,integer,blob)";

        let mut req = BytesMut::with_capacity(64 + SIGNATURE.len());
        req.put_u16(Opcode::Getroutine as u16);
        req.put_u8(0); // flag
        req.put_u32(SIGNATURE.len() as u32);
        req.put_slice(SIGNATURE);
        if (4 + SIGNATURE.len()) & 1 != 0 {
            req.put_u8(0);
        }
        req.put_u16(0); // req_fparam
        req.put_u16(Opcode::Eot as u16);
        self.stream.write_all(&req).await?;
        self.stream.flush().await?;

        let op = loop {
            let op = Opcode::try_from(self.stream.read_u16().await?)?;
            match op {
                Opcode::DbopenFlags => self.stream.drain(2).await?,
                Opcode::Eot => {}
                Opcode::Xactstat => self.stream.drain(6).await?,
                Opcode::Err => return Err(self.read_sq_err().await?),
                other => break other,
            }
        };
        if op != Opcode::Getroutine {
            return Err(Error::Protocol(format!(
                "unexpected opcode {op} in SQ_GETROUTINE response"
            )));
        }

        let dblen = self.stream.read_u16().await? as usize;
        let mut dbname = String::new();
        if dblen > 0 {
            let bytes = self.stream.read_bytes(dblen).await?;
            dbname = String::from_utf8_lossy(&bytes).chars().take(127).collect();
        }
        if (2 + dblen) & 1 != 0 {
            self.stream.drain(1).await?;
        }

        let handle = self.stream.read_u32().await? as i32;
        self.drain_sblob_trailing().await?;

        if handle <= 0 {
            return Err(Error::Protocol(
                "server returned an invalid routine handle for ifx_lo_create".into(),
            ));
        }

        self.lo_create_fphandle = handle;
        self.lo_create_dbname = dbname.clone();
        Ok((handle, dbname))
    }

    /// Opens a Smart Large Object (BLOB / CLOB) by hexadecimal locator string.
    pub async fn sblob_open(&mut self, locator_hex: &str, mode: SblobOpenMode) -> Result<i32> {
        let mode_code = mode as i32;
        let sql = format!("SELECT ifx_lo_open('{locator_hex}'::BLOB, {mode_code}) FROM sysmaster:sysdual");
        let res = self.query(&sql).await?;
        if res.rows.is_empty() {
            return Err(Error::Server {
                sqlcode: -1,
                isamcode: 0,
                sqlstate: "".to_string(),
                message: "smartblob open query returned no rows".to_string(),
            });
        }
        let lofd_val: i32 = res.rows[0].get_i32(0)?.ok_or_else(|| Error::Protocol(
            "smartblob open query returned NULL handle".to_string(),
        ))?;
        if lofd_val < 0 {
            return Err(Error::Protocol("invalid smartblob handle returned by server".to_string()));
        }
        Ok(lofd_val)
    }

    /// Closes an open Smart Large Object descriptor handle.
    pub async fn sblob_close(&mut self, lofd: i32) -> Result<()> {
        let sql = format!("SELECT ifx_lo_close({lofd}) FROM sysmaster:sysdual");
        let _ = self.query(&sql).await?;
        Ok(())
    }

    /// Reads up to `max_bytes` from an open Smart Large Object descriptor using SQ_LODATA (subCom = 0).
    pub async fn sblob_read(&mut self, lofd: i32, max_bytes: usize) -> Result<Vec<u8>> {
        if max_bytes == 0 {
            return Ok(Vec::new());
        }

        let lofd_u16 = lofd as u16;
        let req_len = (max_bytes.min(0x7FFF_FFFF)) as i32;

        let mut req = BytesMut::with_capacity(14);
        req.put_u16(Opcode::Lodata as u16);
        req.put_u16(0); // subCom = 0 (LO_READ)
        req.put_u16(lofd_u16);
        req.put_i32(req_len);
        req.put_u16(32000); // bufferSize = 32000
        req.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&req).await?;
        self.stream.flush().await?;

        // Read response header: skip any EOTs until opcode
        let resp_op = loop {
            let op = self.stream.read_u16().await?;
            if op != Opcode::Eot as u16 {
                break op;
            }
        };


        if resp_op == Opcode::Err as u16 {
            // Drain error
            let sqlcode = self.stream.read_u16().await? as i16;
            let isamcode = self.stream.read_u16().await? as i16;
            self.stream.drain(if self.capabilities.remove_64k_limit { 4 } else { 2 }).await?;
            let msg = self.read_str().await?;
            return Err(Error::Server {
                sqlcode: sqlcode as i32,
                isamcode: isamcode as i32,
                sqlstate: "".to_string(),
                message: msg,
            });
        }

        if resp_op != Opcode::Lodata as u16 {
            return Err(Error::Protocol(format!(
                "Unexpected opcode in SQ_LODATA read response: {resp_op}"
            )));
        }

        // Response body: optype(2), file_size(4). The read path ignores optype, as libsqli does.
        let _optype = self.stream.read_u16().await?;
        let file_size = self.stream.read_u32().await? as usize;

        if file_size == 0 {
            // Drain trailing SQ_DONE / SQ_EOT
            self.drain_sblob_trailing().await?;
            return Ok(Vec::new());
        }

        let stream_expected = file_size.min(max_bytes);
        let mut total_streamed = 0usize;
        let mut result_data = Vec::with_capacity(stream_expected);

        while total_streamed < stream_expected {
            let chlen = self.stream.read_u16().await? as usize;
            if chlen == 0 {
                break;
            }

            let to_read = chlen.min(max_bytes.saturating_sub(result_data.len()));
            if to_read > 0 {
                let chunk = self.stream.read_bytes(to_read).await?;
                result_data.extend_from_slice(&chunk);
            }
            if chlen > to_read {
                let discard = chlen - to_read;
                self.stream.drain(discard).await?;
            }

            if chlen % 2 != 0 {
                self.stream.drain(1).await?;
            }

            total_streamed += chlen;
        }

        self.drain_sblob_trailing().await?;
        Ok(result_data)
    }

    /// Reads with relative seek displacement from an open Smart Large Object using SQ_LODATA (subCom = 1).
    pub async fn sblob_read_seek(
        &mut self,
        lofd: i32,
        offset: i64,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        if max_bytes == 0 {
            return Ok(Vec::new());
        }

        let lofd_u16 = lofd as u16;
        let req_len = (max_bytes.min(0x7FFF_FFFF)) as i32;

        let sign: u16 = if offset < 0 { u16::MAX } else { 1 };
        let mag: u64 = if offset < 0 {
            (-(offset + 1)) as u64 + 1
        } else {
            offset as u64
        };
        let low32 = (mag & 0xFFFF_FFFF) as u32;
        let high32 = (mag >> 32) as u32;

        let mut req = BytesMut::with_capacity(26);
        req.put_u16(Opcode::Lodata as u16);
        req.put_u16(1); // subCom = 1 (LO_READWITHSEEK)
        req.put_u16(lofd_u16);
        req.put_i32(req_len);
        req.put_u16(32000); // bufferSize = 32000
        req.put_u16(sign);
        req.put_u32(low32);
        req.put_u32(high32);
        req.put_u16(1); // whence = 1 (LO_SEEK_CUR)
        req.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&req).await?;
        self.stream.flush().await?;

        let resp_op = loop {
            let op = self.stream.read_u16().await?;
            if op != Opcode::Eot as u16 {
                break op;
            }
        };

        if resp_op == Opcode::Err as u16 {
            let sqlcode = self.stream.read_u16().await? as i16;
            let isamcode = self.stream.read_u16().await? as i16;
            self.stream.drain(if self.capabilities.remove_64k_limit { 4 } else { 2 }).await?;
            let msg = self.read_str().await?;
            return Err(Error::Server {
                sqlcode: sqlcode as i32,
                isamcode: isamcode as i32,
                sqlstate: "".to_string(),
                message: msg,
            });
        }

        if resp_op != Opcode::Lodata as u16 {
            return Err(Error::Protocol(format!(
                "Unexpected opcode in SQ_LODATA seek response: {resp_op}"
            )));
        }

        let _optype = self.stream.read_u16().await?;
        let remaining_size = self.stream.read_u32().await? as usize;

        if remaining_size == 0 {
            self.drain_sblob_trailing().await?;
            return Ok(Vec::new());
        }

        let stream_expected = remaining_size.min(max_bytes);
        let mut total_streamed = 0usize;
        let mut result_data = Vec::with_capacity(stream_expected);

        while total_streamed < stream_expected {
            let chlen = self.stream.read_u16().await? as usize;
            if chlen == 0 {
                break;
            }

            let to_read = chlen.min(max_bytes.saturating_sub(result_data.len()));
            if to_read > 0 {
                let chunk = self.stream.read_bytes(to_read).await?;
                result_data.extend_from_slice(&chunk);
            }
            if chlen > to_read {
                let discard = chlen - to_read;
                self.stream.drain(discard).await?;
            }

            if chlen % 2 != 0 {
                self.stream.drain(1).await?;
            }

            total_streamed += chlen;
        }

        self.drain_sblob_trailing().await?;
        Ok(result_data)
    }

    /// Writes data to an open Smart Large Object using SQ_LODATA (subCom = 2).
    ///
    /// The server buffers the bytes in the descriptor until the descriptor is closed: a read
    /// through the same `lofd` reports a zero file size, and the data only becomes visible
    /// through the locator after [`sblob_close`](Self::sblob_close).
    pub async fn sblob_write(&mut self, lofd: i32, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        let lofd_u16 = lofd as u16;
        let req_len = data.len().min(0x7FFF_FFFF) as i32;

        let mut req = BytesMut::with_capacity(12);
        req.put_u16(Opcode::Lodata as u16);
        req.put_u16(2); // subCom = 2 (LO_WRITE)
        req.put_u16(lofd_u16);
        req.put_i32(req_len);
        req.put_u16(32000); // bufferSize

        self.stream.write_all(&req).await?;

        // Stream chunks of at most 32000 bytes
        let mut rem = data;
        while !rem.is_empty() {
            let chunk_size = rem.len().min(32000);
            let mut ch_hdr = [0u8; 2];
            ch_hdr[0] = (chunk_size >> 8) as u8;
            ch_hdr[1] = (chunk_size & 0xFF) as u8;
            self.stream.write_all(&ch_hdr).await?;
            self.stream.write_all(&rem[..chunk_size]).await?;
            if chunk_size % 2 != 0 {
                self.stream.write_all(&[0u8]).await?;
            }
            rem = &rem[chunk_size..];
        }

        // Send SQ_EOT
        let mut eot = BytesMut::with_capacity(2);
        eot.put_u16(Opcode::Eot as u16);
        self.stream.write_all(&eot).await?;
        self.stream.flush().await?;

        // Receive write ack: opcode(2), optype(2), file_size(4)
        let resp_op = loop {
            let op = self.stream.read_u16().await?;
            if op != Opcode::Eot as u16 {
                break op;
            }
        };


        if resp_op == Opcode::Err as u16 {
            let sqlcode = self.stream.read_u16().await? as i16;
            let isamcode = self.stream.read_u16().await? as i16;
            self.stream.drain(if self.capabilities.remove_64k_limit { 4 } else { 2 }).await?;
            let msg = self.read_str().await?;
            return Err(Error::Server {
                sqlcode: sqlcode as i32,
                isamcode: isamcode as i32,
                sqlstate: "".to_string(),
                message: msg,
            });
        }

        if resp_op != Opcode::Lodata as u16 {
            return Err(Error::Protocol(format!(
                "Unexpected opcode in smartblob write response: {resp_op}"
            )));
        }

        let optype = self.stream.read_u16().await?;
        let resp_size = self.stream.read_u32().await? as i32;

        self.drain_sblob_trailing().await?;

        if optype != 2 || resp_size < 0 {
            return Err(Error::Protocol(format!(
                "smartblob write failed: optype={optype} resp_size={resp_size}"
            )));
        }

        Ok(data.len())
    }

    /// Convenience method to read Smart-LOB content directly given a hex locator: opens,
    /// reads, and closes the descriptor in one call. Named to match the rest of the
    /// `sblob_*` family (`sblob_open`/`sblob_close`/`sblob_read`/`sblob_write`/
    /// `sblob_create`) instead of standing out as `read_sblob`.
    pub async fn sblob_read_by_locator(&mut self, locator_hex: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let lofd = self.sblob_open(locator_hex, SblobOpenMode::ReadOnly).await?;
        let res = self.sblob_read(lofd, max_bytes).await;
        let _ = self.sblob_close(lofd).await;
        res
    }

    /// Drains the trailing SQ_DONE/SQ_EOT opcodes the server always sends after an SQ_LODATA
    /// read completes. Matches `libsqli`'s `sqli_sblob_read`/`sqli_sblob_read_seek` trailing
    /// loop (`sqli_sblob.c`): it is a hard protocol guarantee that one of these opcodes follows,
    /// so this blocks on the read exactly like the rest of this file's dispatch loop does,
    /// rather than racing a fixed timeout that could truncate the drain under latency and
    /// desynchronize the next request.
    /// Drains the trailer of an SBLOB/fastpath turn until `SQ_EOT`, as every trailing loop in
    /// libsqli's `sqli_sblob.c` does: `SQ_DONE` carries a 10-byte payload, every other opcode
    /// is skipped **without consuming payload** — the server terminates a chunk stream with a
    /// bare opcode-0 marker followed by `SQ_EOT`, so a payload-consuming arm would desync the
    /// stream and block. Status packets with fixed payloads (`SQ_DBOPEN_FLAGS`, `SQ_XACTSTAT`)
    /// are only consumed in the fastpath header loop, not here. The loop is bounded so a
    /// malformed trailer cannot spin forever.
    async fn drain_sblob_trailing(&mut self) -> Result<()> {
        for _ in 0..64 {
            let raw = self.stream.read_u16().await?;
            match Opcode::try_from(raw) {
                Ok(Opcode::Eot) => return Ok(()),
                Ok(Opcode::Done) => self.stream.drain(10).await?,
                Ok(_) => {}
                Err(_) => {}
            }
        }
        Err(Error::Protocol(
            "unbounded SQ_LODATA trailer without SQ_EOT (protocol desync)".into(),
        ))
    }
}

/// Represents the execution result of a single statement in a batch.
#[derive(Debug)]
pub struct BatchItemResult {
    pub status: Result<()>,
    pub rows_affected: i64,
}

/// Collection of batch execution items.
#[derive(Debug)]
pub struct BatchResult {
    pub items: Vec<BatchItemResult>,
}

impl BatchResult {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn success_count(&self) -> usize {
        self.items.iter().filter(|i| i.status.is_ok()).count()
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|i| i.status.is_err()).count()
    }
}

/// Cancellation handle to interrupt active queries on a connection out-of-band.
#[derive(Debug, Clone, Copy)]
pub struct CancelHandle {
    #[cfg(unix)]
    fd: std::os::unix::io::RawFd,
    #[cfg(windows)]
    socket: std::os::windows::io::RawSocket,
}

impl CancelHandle {
    /// Sends an urgent out-of-band TCP cancellation byte to interrupt the running query.
    ///
    /// Uses native OS urgent data:
    /// - On Unix: `libc::send(fd, ..., MSG_OOB)`
    /// - On Windows: Winsock `send(socket, ..., MSG_OOB)`
    pub fn cancel(&self) -> Result<()> {
        let interrupt_byte = 0x42u8;

        #[cfg(unix)]
        {
            let sent = unsafe {
                libc::send(
                    self.fd,
                    &interrupt_byte as *const _ as *const libc::c_void,
                    1,
                    libc::MSG_OOB | libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if sent == 1 {
                Ok(())
            } else {
                Err(Error::Io(std::io::Error::last_os_error()))
            }
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::Networking::WinSock::{send, MSG_OOB, SOCKET, SOCKET_ERROR};
            let sent = unsafe {
                send(
                    self.socket as SOCKET,
                    &interrupt_byte as *const u8,
                    1,
                    MSG_OOB as i32,
                )
            };
            if sent != SOCKET_ERROR && sent == 1 {
                Ok(())
            } else {
                Err(Error::Io(std::io::Error::last_os_error()))
            }
        }
    }
}


#[cfg(test)]
mod generated_serial_tests {
    use super::*;

    #[test]
    fn decode_ifx_int8_serial_zero_sign_is_null() {
        // sign == 0 means "no value" (is_null in libsqli's
        // sqli_decode_ifx_int8), used by SQ_INSERTDONE to signal that no
        // SERIAL8/BIGSERIAL id was generated for this insert.
        let buf = [0u8; 10];
        assert_eq!(decode_ifx_int8_serial(&buf), None);
    }

    #[test]
    fn decode_ifx_int8_serial_positive_value() {
        // sign=+1, lo32=42, hi32=0 -> 42
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&1i16.to_be_bytes());
        buf[2..6].copy_from_slice(&42u32.to_be_bytes());
        buf[6..10].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(decode_ifx_int8_serial(&buf), Some(42));
    }

    #[test]
    fn decode_ifx_int8_serial_negative_value() {
        // sign=-1, lo32=100, hi32=0 -> -100
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&(-1i16).to_be_bytes());
        buf[2..6].copy_from_slice(&100u32.to_be_bytes());
        buf[6..10].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(decode_ifx_int8_serial(&buf), Some(-100));
    }

    #[test]
    fn decode_ifx_int8_serial_large_bigserial_value() {
        // A BIGSERIAL-range value spanning both 32-bit words:
        // hi32=1, lo32=0 -> magnitude 2^32.
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&1i16.to_be_bytes());
        buf[2..6].copy_from_slice(&0u32.to_be_bytes());
        buf[6..10].copy_from_slice(&1u32.to_be_bytes());
        assert_eq!(decode_ifx_int8_serial(&buf), Some(1i64 << 32));
    }

    #[test]
    fn decode_ifx_int8_serial_saturates_at_i64_bounds() {
        // Magnitude exceeding i64::MAX saturates instead of wrapping,
        // matching libsqli's saturating cast.
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&1i16.to_be_bytes());
        buf[2..6].copy_from_slice(&u32::MAX.to_be_bytes());
        buf[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode_ifx_int8_serial(&buf), Some(i64::MAX));

        buf[0..2].copy_from_slice(&(-1i16).to_be_bytes());
        assert_eq!(decode_ifx_int8_serial(&buf), Some(i64::MIN));
    }
}
