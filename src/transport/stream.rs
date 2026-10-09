//! Asynchronous Tokio TCP Stream wrapper for Informix SQLI communication.

use crate::error::{Error, Result};
use crate::protocol::sl_header::SlHeader;
use bytes::BytesMut;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct SqliStream {
    stream: TcpStream,
    buf: BytesMut,
}

impl SqliStream {
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect(&addr).await.map_err(Error::Io)?;
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            buf: BytesMut::with_capacity(8192),
        })
    }

    pub fn has_buffered_data(&self) -> bool {
        !self.buf.is_empty()
    }

    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Non-blocking attempt to read any immediately available socket data into `self.buf`.
    pub fn try_fill_buf(&mut self) {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.try_read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Ensures that `self.buf` contains at least `needed` bytes.
    pub async fn ensure_buffered(&mut self, needed: usize) -> Result<()> {
        while self.buf.len() < needed {
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).await.map_err(Error::Io)?;
            if n == 0 {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Connection closed by server while waiting for data",
                )));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
        Ok(())
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.ensure_buffered(buf.len()).await?;
        let bytes = self.buf.split_to(buf.len());
        buf.copy_from_slice(&bytes);
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
        self.ensure_buffered(count).await?;
        Ok(self.buf.split_to(count))
    }

    pub async fn drain(&mut self, count: usize) -> Result<()> {
        let _ = self.read_bytes(count).await?;
        Ok(())
    }

    /// Peeks `N` bytes from the buffer, attempting to read from socket within `timeout_ms` if not enough bytes.
    pub async fn peek_array<const N: usize>(&mut self, timeout_ms: u64) -> Result<Option<[u8; N]>> {
        self.try_fill_buf();
        if self.buf.len() >= N {
            let mut out = [0u8; N];
            out.copy_from_slice(&self.buf[..N]);
            return Ok(Some(out));
        }
        if timeout_ms > 0 {
            let fut = async {
                while self.buf.len() < N {
                    let mut chunk = [0u8; 4096];
                    let n = self.stream.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    self.buf.extend_from_slice(&chunk[..n]);
                }
                std::io::Result::Ok(())
            };
            let _ = tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await;
        }
        if self.buf.len() >= N {
            let mut out = [0u8; N];
            out.copy_from_slice(&self.buf[..N]);
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }

    pub async fn peek_u16(&mut self, timeout_ms: u64) -> Result<Option<u16>> {
        let arr = self.peek_array::<2>(timeout_ms).await?;
        Ok(arr.map(u16::from_be_bytes))
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
