//! Handshake message generation and parsing (CONREQ, CONACC, SQ_PROTOCOLS, SQ_INFO, SQ_DBOPEN).

use crate::connection::capabilities::ServerCapabilities;
use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};
use crate::protocol::constants::*;
use crate::protocol::opcodes::Opcode;
use crate::protocol::sl_header::SlHeader;
use bytes::{BufMut, BytesMut};

/// Builds the complete `CONREQ` frame (SL Header + ASC-BINARY body).
pub fn build_conreq_packet(params: &ConnectParams, transport: &str) -> Result<BytesMut> {
    let mut body = BytesMut::with_capacity(512);

    // Tag 100: ASC-BINARY wrapper
    body.put_u16(TAG_ASC_BINARY);

    // Tag 101: Version/Capability block
    body.put_u16(TAG_VERSION_CAPS);

    // Protocol version = 61
    body.put_u32(SQLI_PROTO_VERSION);

    // Float type: length(2) + "IEEEM\0" = 8 bytes
    let float_type = b"IEEEM\0";
    body.put_u16(float_type.len() as u16);
    body.put_slice(float_type);

    // Tag 108: Product type (12 bytes: "sqlexec\0\0\0\0\0")
    body.put_u16(TAG_PRODUCT_TYPE);
    let mut prod = [0u8; 12];
    prod[..7].copy_from_slice(b"sqlexec");
    body.put_slice(&prod);

    // Product version: "9.280\0"
    let version = b"9.280\0";
    body.put_u16(version.len() as u16);
    body.put_slice(version);

    // RDS number: "RDS#R000000\0"
    let rds = b"RDS#R000000\0";
    body.put_u16(rds.len() as u16);
    body.put_slice(rds);

    // App ID: "sqli\0"
    let app_id = b"sqli\0";
    body.put_u16(app_id.len() as u16);
    body.put_slice(app_id);

    // Capabilities
    body.put_i32(SQLI_CLIENT_CAP_1); // Cap_1
    body.put_i32(0);                 // Cap_2
    body.put_i32(0);                 // Cap_3
    body.put_u16(1);                 // flags = 1

    // Username (len + user + NUL)
    let user_bytes = params.username.as_bytes();
    body.put_u16((user_bytes.len() + 1) as u16);
    body.put_slice(user_bytes);
    body.put_u8(0);

    // Password (len + pass + NUL)
    let pass_bytes = params.password.as_bytes();
    body.put_u16((pass_bytes.len() + 1) as u16);
    body.put_slice(pass_bytes);
    body.put_u8(0);

    // Protocol name: 8 bytes, "ol\0\0\0\0\0\0"
    let mut proto_name = [0u8; 8];
    proto_name[..2].copy_from_slice(b"ol");
    body.put_slice(&proto_name);

    // Protocol version: 4-byte BE integer = 61
    body.put_u32(SQLI_PROTO_VERSION);

    // Transport: 8 bytes fixed
    let mut transport_name = [0u8; 8];
    let tlen = transport.len().min(7);
    transport_name[..tlen].copy_from_slice(&transport.as_bytes()[..tlen]);
    body.put_slice(&transport_name);

    // Network level: 4-byte int = 1
    body.put_u32(1);

    // Tag 104: Statement options (type=11, stmtoptions=3)
    body.put_u16(TAG_STMT_OPTIONS);
    body.put_u16(11);
    body.put_u32(3);

    // Server name (len + srv + NUL)
    let srv_bytes = params.server.as_bytes();
    body.put_u16((srv_bytes.len() + 1) as u16);
    body.put_slice(srv_bytes);
    body.put_u8(0);

    // Database name: empty (len=0)
    body.put_u16(0);

    // Reserved four shorts = 0
    body.put_u16(0);
    body.put_u16(0);
    body.put_u16(0);
    body.put_u16(0);

    // Tag 106: Environment variables
    body.put_u16(TAG_ENV_VARS);
    let env_pairs: &[(&str, &str)] = &[
        ("DBPATH", "."),
        ("CLIENT_LOCALE", &params.client_locale),
        ("CLNT_PAM_CAPABLE", "1"),
        ("IFX_UPDDESC", "1"),
        ("DB_LOCALE", &params.db_locale),
        ("NODEFDAC", "no"),
    ];

    body.put_u16(env_pairs.len() as u16);
    for (k, v) in env_pairs {
        body.put_u16((k.len() + 1) as u16);
        body.put_slice(k.as_bytes());
        body.put_u8(0);

        body.put_u16((v.len() + 1) as u16);
        body.put_slice(v.as_bytes());
        body.put_u8(0);
    }

    // Tag 107: Environment info
    body.put_u16(TAG_ENV_INFO);
    body.put_u32(0); // reserved
    body.put_u32(std::process::id()); // PID
    body.put_u32(0); // Thread ID placeholder

    // Hostname
    let host_bytes = params.hostname.as_bytes();
    body.put_u16((host_bytes.len() + 1) as u16);
    body.put_slice(host_bytes);
    body.put_u8(0);

    // 2-byte reserved
    body.put_u16(0);

    // CWD
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    let cwd_bytes = cwd.as_bytes();
    body.put_u16((cwd_bytes.len() + 1) as u16);
    body.put_slice(cwd_bytes);
    body.put_u8(0);

    // Tag 116: App name block
    let app_name = b"sqlicon\0";
    body.put_u16(TAG_APP_NAME);
    body.put_u16((10 + app_name.len()) as u16);
    body.put_u32(0);
    body.put_u32(0);
    body.put_u16(app_name.len() as u16);
    body.put_slice(app_name);

    // Tag 127: End
    body.put_u16(TAG_END);

    let pdu_size = (SL_HEADER_SIZE + body.len()) as u16;
    let sl_header = SlHeader::new(pdu_size, SL_TYPE_CONREQ, SL_PROT_SQLI, 0);

    let mut frame = BytesMut::with_capacity(pdu_size as usize);
    sl_header.encode(&mut frame);
    frame.put_slice(&body);

    Ok(frame)
}

/// Parses the ASC-BINARY body of `CONACC` or `CONREJ`.
pub fn parse_conacc_body(body: &[u8]) -> Result<ServerCapabilities> {
    if body.len() < 4 {
        return Err(Error::Protocol("CONACC body too short".into()));
    }

    let mut pos = 0;
    let mut cap_1: i32 = 0;

    // Scan header tags
    if pos + 4 <= body.len() {
        pos += 4; // skip tag 100 and 101
    }
    if pos + 4 <= body.len() {
        pos += 4; // skip protocol version
    }

    // Skip float type [2-byte len][data]
    if pos + 2 <= body.len() {
        let ft_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2 + ft_len;
    }

    // Skip tag 108 + 12 bytes
    if pos + 14 <= body.len() {
        pos += 14;
    }

    // Skip product version [2-byte len][data]
    if pos + 2 <= body.len() {
        let pv_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2 + pv_len;
    }

    // Skip RDS [2-byte len][data]
    if pos + 2 <= body.len() {
        let rds_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2 + rds_len;
    }

    // Skip App ID [2-byte len][data]
    if pos + 2 <= body.len() {
        let aid_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2 + aid_len;
    }

    // Read Cap_1
    if pos + 4 <= body.len() {
        cap_1 = i32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
        pos += 4;
    }

    // Scan forward for tag 102 to check svcError
    while pos + 8 <= body.len() {
        let tag = u16::from_be_bytes([body[pos], body[pos + 1]]);
        if tag == 102 {
            let svc_error = i16::from_be_bytes([body[pos + 6], body[pos + 7]]);
            if svc_error != 0 {
                return Err(Error::Authentication(format!(
                    "Server rejected connection with svcError: {svc_error}"
                )));
            }
            break;
        }
        if tag == TAG_END {
            break;
        }
        pos += 1;
    }

    let mut caps = ServerCapabilities::default();
    caps.cap_1 = cap_1;
    Ok(caps)
}

/// Builds the `SQ_PROTOCOLS` exchange request packet.
pub fn build_sq_protocols_packet() -> BytesMut {
    let mut buf = BytesMut::with_capacity(16);
    buf.put_u16(Opcode::Protocols as u16); // 126
    buf.put_u16(9); // length = 9
    buf.put_slice(&[
        0xFF, 0xFC, 0x7F, 0xFC, 0x3C, 0x8C, 0xAA, 0x97, 0x06, 0x00, // 9 bytes + 1 pad
    ]);
    buf.put_u16(Opcode::Eot as u16); // chained SQ_EOT (12)
    buf
}

/// Builds the `SQ_INFO` environment variables packet.
pub fn build_sq_info_packet() -> BytesMut {
    let envs: &[(&str, &str)] = &[
        ("DBTEMP", "/tmp"),
        ("SUBQCACHE SZ", "10"),
    ];

    let mut max_name: usize = 0;
    let mut max_val: usize = 0;
    let mut tot_len: usize = 4; // 4 = max_name(2) + max_val(2)

    for (k, v) in envs {
        let nlp = (k.len() + 1) & !1;
        let vlp = (v.len() + 1) & !1;
        tot_len += 2 + nlp + 2 + vlp;
        if nlp > max_name { max_name = nlp; }
        if vlp > max_val { max_val = vlp; }
    }
    tot_len += 4; // end_name(2) + end_val(2)

    let mut buf = BytesMut::with_capacity(64);
    buf.put_u16(Opcode::Info as u16); // 81
    buf.put_u16(6); // item_type = 6
    buf.put_u16(tot_len as u16);
    buf.put_u16(max_name as u16);
    buf.put_u16(max_val as u16);

    for (k, v) in envs {
        buf.put_u16(k.len() as u16);
        buf.put_slice(k.as_bytes());
        if k.len() % 2 != 0 { buf.put_u8(0); }

        buf.put_u16(v.len() as u16);
        buf.put_slice(v.as_bytes());
        if v.len() % 2 != 0 { buf.put_u8(0); }
    }

    buf.put_u16(0); // end_name
    buf.put_u16(0); // end_val
    buf.put_u16(Opcode::Eot as u16); // chained SQ_EOT

    buf
}

/// Builds the `SQ_DBOPEN` packet.
pub fn build_sq_dbopen_packet(db_name: &str) -> BytesMut {
    let db = if db_name.is_empty() { "informix" } else { db_name };
    let db_bytes = db.as_bytes();

    let mut buf = BytesMut::with_capacity(32);
    buf.put_u16(Opcode::Dbopen as u16); // 36
    buf.put_u16(db_bytes.len() as u16);
    buf.put_slice(db_bytes);
    if db_bytes.len() % 2 != 0 {
        buf.put_u8(0); // pad to even
    }
    buf.put_u16(0); // flags = 0
    buf.put_u16(Opcode::Eot as u16); // chained SQ_EOT

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_conreq_packet() {
        let params = ConnectParams::new("127.0.0.1", "9088", "ol_informix1210")
            .credentials("informix", "password");
        let packet = build_conreq_packet(&params, "tlitcp").unwrap();
        assert!(packet.len() > SL_HEADER_SIZE);

        let header = SlHeader::decode_from_slice(&packet).unwrap();
        assert_eq!(header.pdu_size as usize, packet.len());
        assert_eq!(header.sl_type, SL_TYPE_CONREQ);
        assert_eq!(header.sl_attr, SL_PROT_SQLI);
    }

    #[test]
    fn test_build_sq_protocols() {
        let packet = build_sq_protocols_packet();
        assert_eq!(packet.len(), 16);
        assert_eq!(u16::from_be_bytes([packet[0], packet[1]]), Opcode::Protocols as u16);
        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), Opcode::Eot as u16);
    }

    #[test]
    fn test_build_sq_dbopen() {
        let packet = build_sq_dbopen_packet("stores7");
        assert_eq!(u16::from_be_bytes([packet[0], packet[1]]), Opcode::Dbopen as u16);
        assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), 7); // "stores7" len
        assert_eq!(&packet[4..11], b"stores7");
        assert_eq!(packet[11], 0); // padding
        assert_eq!(u16::from_be_bytes([packet[12], packet[13]]), 0); // flags
        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), Opcode::Eot as u16);
    }
}
