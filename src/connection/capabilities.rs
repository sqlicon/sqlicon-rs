//! Informix Server capabilities negotiated during handshake.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServerCapabilities {
    pub cap_1: i32,
    pub cap_2: i32,
    pub cap_3: i32,
    pub has_pam: bool,
    pub has_bigint: bool,
    pub has_savepoints: bool,
    pub extended_describe: bool,
    pub large_tuple_mode: bool,
    pub long_row_id: bool,
    pub float_to_dec: bool,
    pub read_only: bool,
    pub remove_64k_limit: bool,
}

impl ServerCapabilities {
    pub fn parse_protocols(protocols: &[u8]) -> Self {
        let mut caps = Self::default();
        if protocols.len() >= 6 {
            caps.has_pam = (protocols[5] & 0x08) != 0;
        }
        if protocols.len() >= 8 {
            caps.remove_64k_limit = (protocols[7] & 0x02) != 0;
            caps.extended_describe = (protocols[7] & 0x01) != 0;
        }
        if protocols.len() >= 9 {
            caps.large_tuple_mode = (protocols[8] & 0x04) != 0;
            caps.long_row_id = (protocols[8] & 0x02) != 0;
        }
        caps
    }
}
