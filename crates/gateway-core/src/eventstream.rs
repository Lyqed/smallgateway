//! AWS event-stream binary framing (the Bedrock wire format): an incremental
//! CRC-checked decoder, plus an encoder used to build test fixtures.
//!
//! Frame layout: `total_len:u32be | headers_len:u32be | prelude_crc:u32be |
//! headers | payload | message_crc:u32be`. Header values support only type 7
//! (string), which covers `:message-type`, `:event-type`, and
//! `:content-type` — all Bedrock uses.
//!
//! Promoted unchanged from `spikes/event-model/src/eventstream.rs` (Phase 0,
//! Spike A).

use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub headers: HashMap<String, String>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes buffered awaiting a complete frame (partial-frame state).
    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 12 {
                break;
            }
            let total = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as usize;
            if total < 16 {
                return Err(format!("frame too short: {total} bytes"));
            }
            if self.buf.len() < total {
                break;
            }
            let frame: Vec<u8> = self.buf.drain(..total).collect();
            out.push(decode_frame(&frame)?);
        }
        Ok(out)
    }
}

fn decode_frame(frame: &[u8]) -> Result<Frame, String> {
    let headers_len = u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize;
    let prelude_crc = u32::from_be_bytes(frame[8..12].try_into().unwrap());
    if crc32fast::hash(&frame[0..8]) != prelude_crc {
        return Err("prelude CRC mismatch".into());
    }
    let msg_crc_offset = frame.len() - 4;
    let message_crc = u32::from_be_bytes(frame[msg_crc_offset..].try_into().unwrap());
    if crc32fast::hash(&frame[..msg_crc_offset]) != message_crc {
        return Err("message CRC mismatch".into());
    }
    let headers_end = 12 + headers_len;
    if headers_end > msg_crc_offset {
        return Err("headers overrun payload".into());
    }

    let mut headers = HashMap::new();
    let mut pos = 12;
    while pos < headers_end {
        let name_len = frame[pos] as usize;
        pos += 1;
        if pos + name_len + 3 > headers_end {
            return Err("truncated header".into());
        }
        let name = String::from_utf8_lossy(&frame[pos..pos + name_len]).into_owned();
        pos += name_len;
        let value_type = frame[pos];
        pos += 1;
        if value_type != 7 {
            return Err(format!("unsupported header value type {value_type}"));
        }
        let value_len =
            u16::from_be_bytes(frame[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + value_len > headers_end {
            return Err("truncated header value".into());
        }
        let value = String::from_utf8_lossy(&frame[pos..pos + value_len]).into_owned();
        pos += value_len;
        headers.insert(name, value);
    }

    Ok(Frame {
        headers,
        payload: frame[headers_end..msg_crc_offset].to_vec(),
    })
}

pub fn encode_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut header_bytes = Vec::new();
    for (name, value) in headers {
        header_bytes.push(name.len() as u8);
        header_bytes.extend_from_slice(name.as_bytes());
        header_bytes.push(7u8);
        header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        header_bytes.extend_from_slice(value.as_bytes());
    }
    let total = 12 + header_bytes.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&(total as u32).to_be_bytes());
    frame.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    let prelude_crc = crc32fast::hash(&frame[0..8]);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());
    frame.extend_from_slice(&header_bytes);
    frame.extend_from_slice(payload);
    let message_crc = crc32fast::hash(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_across_byte_boundaries() {
        let frame = encode_frame(
            &[(":message-type", "event"), (":event-type", "messageStart")],
            br#"{"role":"assistant"}"#,
        );
        let mut decoder = FrameDecoder::new();
        let mut frames = Vec::new();
        for b in &frame {
            frames.extend(decoder.feed(&[*b]).unwrap());
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].headers.get(":event-type").map(String::as_str),
            Some("messageStart")
        );
        assert_eq!(frames[0].payload, br#"{"role":"assistant"}"#);
        assert_eq!(decoder.pending_bytes(), 0);
    }

    #[test]
    fn corrupted_payload_fails_crc() {
        let mut frame = encode_frame(&[(":event-type", "metadata")], b"{}");
        let payload_pos = frame.len() - 5;
        frame[payload_pos] ^= 0xFF;
        let mut decoder = FrameDecoder::new();
        assert!(decoder.feed(&frame).is_err());
    }
}
