//! Asynchronous Informix SQLI Connection.

use crate::connection::capabilities::ServerCapabilities;
use crate::connection::handshake::*;
use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};
use crate::protocol::constants::*;
use crate::protocol::opcodes::Opcode;
use crate::transport::SqliStream;
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
    #[allow(dead_code)]
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
}
