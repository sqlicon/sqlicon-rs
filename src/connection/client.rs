//! Asynchronous Informix SQLI Connection.

use crate::connection::capabilities::ServerCapabilities;
use crate::connection::handshake::*;
use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};
use crate::protocol::constants::*;
use crate::protocol::opcodes::Opcode;
use crate::result::column::ColumnInfo;
use crate::result::resultset::QueryResult;
use crate::result::row::Row;
use crate::statement::PreparedStatement;
use crate::transport::SqliStream;
use crate::types::column_type::ColumnType;
use bytes::{BufMut, BytesMut};
use tracing::{debug, info};

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

pub struct Connection {
    stream: SqliStream,
    params: ConnectParams,
    capabilities: ServerCapabilities,
    state: ConnectionState,
}

impl Connection {
    pub async fn connect(params: ConnectParams) -> Result<Self> {
        let port: u16 = params.service.parse().map_err(|_| {
            Error::InvalidArgument(format!("Invalid port or service: {}", params.service))
        })?;

        debug!("Connecting to {}:{}", params.hostname, port);
        let mut stream = SqliStream::connect(&params.hostname, port).await?;
        debug!("Connected TCP socket, initiating SL/CONREQ handshake");

        // Step 1: Send SL-framed CONREQ
        let conreq = build_conreq_packet(&params, "tlitcp")?;
        stream.write_all(&conreq).await?;
        stream.flush().await?;

        // Step 2: Read SL response header & body
        let (header, body) = stream.read_sl_pdu().await?;
        debug!("Received SL response: type={}, attr={}", header.sl_type, header.sl_attr);

        let mut caps = match header.sl_type {
            SL_TYPE_CONACC => parse_conacc_body(&body)?,
            SL_TYPE_CONREJ => {
                let caps = parse_conacc_body(&body)?;
                debug!("Server sent CONREJ with soft-accept, continuing");
                caps
            }
            SL_TYPE_REDIRECT => return Err(Error::Protocol("Server returned redirect".into())),
            other => return Err(Error::Protocol(format!("Unexpected SL type: {other}"))),
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

            // Read DBOPEN responses until Done
            loop {
                let op = stream.read_u16().await?;
                if op == Opcode::Done as u16 || op == Opcode::Eot as u16 {
                    break;
                } else if op == Opcode::Err as u16 {
                    let err_len = stream.read_u16().await? as usize;
                    let to_drain = err_len + if err_len % 2 != 0 { 1 } else { 0 };
                    stream.drain(to_drain).await?;
                    return Err(Error::Server {
                        sqlcode: -1,
                        isamcode: 0,
                        sqlstate: "".into(),
                        message: format!("Failed to open database {db_name}"),
                    });
                } else {
                    // Drain generic SQ message with length prefix
                    let msg_len = stream.read_u16().await? as usize;
                    let to_drain = msg_len + if msg_len % 2 != 0 { 1 } else { 0 };
                    stream.drain(to_drain).await?;
                }
            }
        }

        info!("Informix connection successfully established");
        Ok(Self {
            stream,
            params,
            capabilities: caps,
            state: ConnectionState::Ready,
        })
    }

    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }

    pub fn params(&self) -> &ConnectParams {
        &self.params
    }

    /// Prepares an SQL statement on the server.
    pub async fn prepare(&mut self, sql: &str) -> Result<PreparedStatement> {
        let qmarks = sql.chars().filter(|&c| c == '?').count() as u16;
        let sql_bytes = sql.as_bytes();

        let mut pkt = BytesMut::with_capacity(16 + sql_bytes.len());
        pkt.put_u16(Opcode::Prepare as u16); // 2
        pkt.put_u16(qmarks);

        if self.capabilities.remove_64k_limit {
            pkt.put_u32(sql_bytes.len() as u32);
        } else {
            pkt.put_u16(sql_bytes.len() as u16);
        }
        pkt.put_slice(sql_bytes);
        if sql_bytes.len() % 2 != 0 {
            pkt.put_u8(0); // Pad
        }

        // Chained tail: SQ_NDESCRIBE + SQ_WANTDONE + SQ_EOT
        pkt.put_u16(Opcode::Ndescribe as u16); // 22
        pkt.put_u16(Opcode::Wantdone as u16);   // 49
        pkt.put_u16(Opcode::Eot as u16);        // 12

        self.stream.write_all(&pkt).await?;
        self.stream.flush().await?;

        // Read response until DONE / EOT
        let mut statement_type = 0;
        let mut statement_id = 0;
        let mut columns = Vec::new();

        loop {
            let op = self.stream.read_u16().await?;
            if op == Opcode::Describe as u16 {
                statement_type = self.stream.read_u16().await?;
                statement_id = self.stream.read_u16().await?;
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

                    let encoded_len = if !self.capabilities.extended_describe {
                        self.stream.read_u16().await? as u32
                    } else {
                        let _extended_info = self.stream.read_u32().await?;
                        // Read type_owner string
                        let o_len = self.stream.read_u16().await? as usize;
                        if o_len > 0 { self.stream.drain(o_len).await?; }
                        if o_len % 2 != 0 { self.stream.drain(1).await?; }
                        // Read type_name string
                        let n_len = self.stream.read_u16().await? as usize;
                        if n_len > 0 { self.stream.drain(n_len).await?; }
                        if n_len % 2 != 0 { self.stream.drain(1).await?; }

                        let _reference = self.stream.read_u16().await?;
                        let _alignment = self.stream.read_u16().await?;
                        let _source_type = self.stream.read_u32().await?;
                        self.stream.read_u32().await?
                    };

                    fields_meta.push((field_idx, tuple_offset, type_raw, encoded_len));
                }

                // Read column names
                let names_buf = if names_length > 0 {
                    self.stream.read_bytes(names_length).await?
                } else {
                    BytesMut::new()
                };
                if names_length % 2 != 0 {
                    self.stream.drain(1).await?;
                }

                // Parse names
                let mut name_offset = 0;
                for (field_idx, tuple_offset, type_raw, encoded_len) in fields_meta {
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

                    let col_type = ColumnType::from_raw(type_raw).unwrap_or(ColumnType::Char);
                    columns.push(ColumnInfo::new(
                        col_name,
                        col_type,
                        type_raw,
                        encoded_len,
                        tuple_offset,
                        field_idx,
                    ));
                }
            } else if op == Opcode::Done as u16 {
                let _warnings = self.stream.read_u16().await?;
                if self.capabilities.long_row_id {
                    let _ = self.stream.read_bytes(16).await?; // rows + rowid
                } else {
                    let _ = self.stream.read_bytes(8).await?;
                }
                let _errd1 = self.stream.read_u32().await?;
            } else if op == Opcode::Err as u16 {
                let sqlcode = self.stream.read_u16().await? as i16;
                let isamcode = self.stream.read_u16().await? as i16;
                let drain_len = if self.capabilities.remove_64k_limit { 4 } else { 2 };
                self.stream.drain(drain_len).await?;
                let msg_len = self.stream.read_u16().await? as usize;
                let msg_bytes = self.stream.read_bytes(msg_len).await?;
                let message = String::from_utf8_lossy(&msg_bytes).to_string();
                return Err(Error::Server {
                    sqlcode: sqlcode as i32,
                    isamcode: isamcode as i32,
                    sqlstate: "".into(),
                    message,
                });
            } else if op == Opcode::Eot as u16 {
                break;
            } else {
                // Drain unknown SQ message
                let msg_len = self.stream.read_u16().await? as usize;
                let to_drain = msg_len + if msg_len % 2 != 0 { 1 } else { 0 };
                self.stream.drain(to_drain).await?;
            }
        }

        Ok(PreparedStatement::new(
            statement_id,
            statement_type,
            qmarks as usize,
            columns,
        ))
    }

    /// Executes a non-query DDL/DML statement and returns affected rows.
    pub async fn execute(&mut self, sql: &str) -> Result<i64> {
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

        let mut rows_affected: i64 = 0;

        loop {
            let op = self.stream.read_u16().await?;
            if op == Opcode::Done as u16 {
                let _warnings = self.stream.read_u16().await?;
                if self.capabilities.long_row_id {
                    let mut b = [0u8; 8];
                    self.stream.read_exact(&mut b).await?;
                    rows_affected = i64::from_be_bytes(b);
                    self.stream.drain(8).await?; // row_id
                } else {
                    let mut b = [0u8; 4];
                    self.stream.read_exact(&mut b).await?;
                    rows_affected = i32::from_be_bytes(b) as i64;
                    self.stream.drain(4).await?; // row_id
                }
                let _errd1 = self.stream.read_u32().await?;
            } else if op == Opcode::Err as u16 {
                let sqlcode = self.stream.read_u16().await? as i16;
                let isamcode = self.stream.read_u16().await? as i16;
                let drain_len = if self.capabilities.remove_64k_limit { 4 } else { 2 };
                self.stream.drain(drain_len).await?;
                let msg_len = self.stream.read_u16().await? as usize;
                let msg_bytes = self.stream.read_bytes(msg_len).await?;
                let message = String::from_utf8_lossy(&msg_bytes).to_string();
                return Err(Error::Server {
                    sqlcode: sqlcode as i32,
                    isamcode: isamcode as i32,
                    sqlstate: "".into(),
                    message,
                });
            } else if op == Opcode::Eot as u16 {
                break;
            } else {
                let msg_len = self.stream.read_u16().await? as usize;
                let to_drain = msg_len + if msg_len % 2 != 0 { 1 } else { 0 };
                self.stream.drain(to_drain).await?;
            }
        }

        self.close_statement(stmt_id).await?;
        Ok(rows_affected)
    }

    /// Executes a SELECT query and returns the complete QueryResult.
    pub async fn query(&mut self, sql: &str) -> Result<QueryResult> {
        let stmt = self.prepare(sql).await?;
        let stmt_id = stmt.id();
        let columns = stmt.columns().to_vec();

        // 1. Open cursor
        let curname = b"_ifxc000000000000";
        let mut open_pkt = BytesMut::with_capacity(32);
        open_pkt.put_u16(Opcode::Id as u16);
        open_pkt.put_u16(stmt_id);
        open_pkt.put_u16(3); // SQ_CURNAME
        open_pkt.put_u16(curname.len() as u16);
        open_pkt.put_slice(curname);
        if curname.len() % 2 != 0 {
            open_pkt.put_u8(0);
        }
        open_pkt.put_u16(6); // SQ_OPEN
        open_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&open_pkt).await?;
        self.stream.flush().await?;

        // 2. Fetch rows
        let mut fetch_pkt = BytesMut::with_capacity(16);
        fetch_pkt.put_u16(Opcode::Id as u16);
        fetch_pkt.put_u16(stmt_id);
        fetch_pkt.put_u16(Opcode::Nfetch as u16); // 9
        fetch_pkt.put_u16(1000); // target rows
        fetch_pkt.put_u16(Opcode::Eot as u16);

        self.stream.write_all(&fetch_pkt).await?;
        self.stream.flush().await?;

        let mut rows = Vec::new();
        let mut rows_affected: i64 = 0;
        let mut warnings: u16 = 0;

        loop {
            let op = self.stream.read_u16().await?;
            if op == Opcode::Tuple as u16 {
                let _warn = self.stream.read_u16().await?;
                let tuple_size = self.stream.read_u32().await? as usize;
                let tuple_bytes = self.stream.read_bytes(tuple_size).await?;
                if tuple_size % 2 != 0 {
                    self.stream.drain(1).await?;
                }
                rows.push(Row::new(tuple_bytes.to_vec(), columns.clone()));
            } else if op == Opcode::Done as u16 {
                warnings = self.stream.read_u16().await?;
                if self.capabilities.long_row_id {
                    let mut b = [0u8; 8];
                    self.stream.read_exact(&mut b).await?;
                    rows_affected = i64::from_be_bytes(b);
                    self.stream.drain(8).await?;
                } else {
                    let mut b = [0u8; 4];
                    self.stream.read_exact(&mut b).await?;
                    rows_affected = i32::from_be_bytes(b) as i64;
                    self.stream.drain(4).await?;
                }
                let _errd1 = self.stream.read_u32().await?;
            } else if op == Opcode::Err as u16 {
                let sqlcode = self.stream.read_u16().await? as i16;
                let isamcode = self.stream.read_u16().await? as i16;
                let drain_len = if self.capabilities.remove_64k_limit { 4 } else { 2 };
                self.stream.drain(drain_len).await?;
                let msg_len = self.stream.read_u16().await? as usize;
                let msg_bytes = self.stream.read_bytes(msg_len).await?;
                let message = String::from_utf8_lossy(&msg_bytes).to_string();
                return Err(Error::Server {
                    sqlcode: sqlcode as i32,
                    isamcode: isamcode as i32,
                    sqlstate: "".into(),
                    message,
                });
            } else if op == Opcode::Eot as u16 {
                break;
            } else {
                let msg_len = self.stream.read_u16().await? as usize;
                let to_drain = msg_len + if msg_len % 2 != 0 { 1 } else { 0 };
                self.stream.drain(to_drain).await?;
            }
        }

        self.close_statement(stmt_id).await?;
        Ok(QueryResult::new(columns, rows, rows_affected, None, warnings))
    }

    /// Closes and releases a statement on the server.
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
}
