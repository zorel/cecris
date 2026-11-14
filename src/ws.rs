use std::io::{self, Read};
use std::net::TcpStream;

use crate::util;

const MAX_MESSAGE: usize = 1 << 20;

pub fn accept_key(client_key: &str) -> String {
    let mut data = String::with_capacity(client_key.len() + 36);
    data.push_str(client_key);
    data.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    util::b64_encode(&util::sha1(data.as_bytes()))
}

pub fn encode(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | (opcode & 0x0f));
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

pub enum Incoming {
    Binary(Vec<u8>),
    Text(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

pub struct Reader {
    stream: TcpStream,
    pending: Vec<u8>,
    frag: Option<(bool, Vec<u8>)>,
}

impl Reader {
    pub fn new(stream: TcpStream, pending: Vec<u8>) -> Self {
        Reader {
            stream,
            pending,
            frag: None,
        }
    }

    fn read_exact_buf(&mut self, out: &mut [u8]) -> io::Result<()> {
        let mut off = 0;
        while off < out.len() {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(out.len() - off);
                out[off..off + n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                off += n;
            } else {
                self.stream.read_exact(&mut out[off..])?;
                off = out.len();
            }
        }
        Ok(())
    }

    pub fn next(&mut self) -> io::Result<Option<Incoming>> {
        loop {
            let mut head = [0u8; 2];
            match self.read_exact_buf(&mut head) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e),
            }
            let fin = head[0] & 0x80 != 0;
            if head[0] & 0x70 != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "reserved bits set"));
            }
            let opcode = head[0] & 0x0f;
            let masked = head[1] & 0x80 != 0;
            let len = match head[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.read_exact_buf(&mut b)?;
                    u16::from_be_bytes(b) as usize
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.read_exact_buf(&mut b)?;
                    let n = u64::from_be_bytes(b);
                    if n > MAX_MESSAGE as u64 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
                    }
                    n as usize
                }
                n => n as usize,
            };
            if len > MAX_MESSAGE {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
            }
            if !masked {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "client frame not masked"));
            }
            let mut mask = [0u8; 4];
            self.read_exact_buf(&mut mask)?;
            let mut payload = vec![0u8; len];
            self.read_exact_buf(&mut payload)?;
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i & 3];
            }

            match opcode {
                0x8 => return Ok(Some(Incoming::Close)),
                0x9 => return Ok(Some(Incoming::Ping(payload))),
                0xA => return Ok(Some(Incoming::Pong)),
                0x1 | 0x2 => {
                    if fin {
                        return Ok(Some(if opcode == 0x1 {
                            Incoming::Text(payload)
                        } else {
                            Incoming::Binary(payload)
                        }));
                    }
                    self.frag = Some((opcode == 0x1, payload));
                }
                0x0 => {
                    let Some((text, mut buf)) = self.frag.take() else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "continuation without start",
                        ));
                    };
                    if buf.len() + payload.len() > MAX_MESSAGE {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
                    }
                    buf.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(if text {
                            Incoming::Text(buf)
                        } else {
                            Incoming::Binary(buf)
                        }));
                    }
                    self.frag = Some((text, buf));
                }
                _ => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "bad opcode"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frame_header_sizes() {
        assert_eq!(encode(0x2, b"hi")[1], 2);
        assert_eq!(encode(0x2, &[0u8; 200])[1], 126);
        assert_eq!(encode(0x2, &[0u8; 70000])[1], 127);
    }
}
