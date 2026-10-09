//! Asynchronous Tokio TCP Stream wrapper for Informix SQLI communication.

use crate::error::{Error, Result};
use crate::protocol::sl_header::SlHeader;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct SqliStream {
    stream: TcpStream,
}

impl SqliStream {
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect(&addr).await.map_err(Error::Io)?;
        let _ = stream.set_nodelay(true);
        Ok(Self { stream })
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.stream.read_exact(buf).await.map_err(Error::Io)?;
        Ok(())
    }

    pub async fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf).await?;
        Ok(u16::from_be_bytes(buf))
    }

    pub async fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf).await?;
        Ok(u32::from_be_bytes(buf))
    }

    pub async fn read_bytes(&mut self, count: usize) -> Result<BytesMut> {
        let mut buf = BytesMut::zeroed(count);
        self.read_exact(&mut buf).await?;
        Ok(buf)
    }

    pub async fn drain(&mut self, mut count: usize) -> Result<()> {
        let mut temp = [0u8; 256];
        while count > 0 {
            let chunk = count.min(temp.len());
            self.read_exact(&mut temp[..chunk]).await?;
            count -= chunk;
        }
        Ok(())
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.stream.write_all(buf).await.map_err(Error::Io)?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.stream.flush().await.map_err(Error::Io)?;
        Ok(())
    }

    /// Reads an SL-framed packet: reads 6-byte header, then the body of (pdu_size - 6) bytes.
    pub async fn read_sl_pdu(&mut self) -> Result<(SlHeader, BytesMut)> {
        let mut hdr_bytes = [0u8; SlHeader::SIZE];
        self.read_exact(&mut hdr_bytes).await?;
        let header = SlHeader::decode_from_slice(&hdr_bytes)?;

        if (header.pdu_size as usize) < SlHeader::SIZE {
            return Err(Error::Protocol(format!(
                "Invalid SL pdu_size: {} (less than header size)",
                header.pdu_size
            )));
        }

        let body_len = (header.pdu_size as usize) - SlHeader::SIZE;
        let body = if body_len > 0 {
            self.read_bytes(body_len).await?
        } else {
            BytesMut::new()
        };

        Ok((header, body))
    }
}
