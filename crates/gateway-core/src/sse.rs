//! Minimal incremental SSE parser: bytes in, complete events out.
//!
//! Internal state is only the current unterminated block — bounded state is
//! what makes backpressure trivial: the caller feeds the next chunk only
//! after consuming the events from the previous one.
//!
//! Promoted unchanged from `spikes/event-model/src/sse.rs` (Phase 0, Spike A).

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes buffered awaiting a block terminator (partial-frame state).
    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        // SSE never needs raw carriage returns (JSON payloads escape them),
        // so dropping them makes the block terminator a plain "\n\n".
        self.buf.extend(bytes.iter().copied().filter(|&b| b != b'\r'));
        let mut out = Vec::new();
        while let Some(pos) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = self.buf.drain(..pos + 2).collect();
            if let Some(ev) = parse_block(&block[..pos]) {
                out.push(ev);
            }
        }
        out
    }
}

fn parse_block(block: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(block);
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        } else if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        }
        // Comment lines (":...") and other fields are ignored.
    }
    if data.is_empty() && event.is_none() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_across_arbitrary_chunk_boundaries() {
        let wire = b"event: ping\ndata: {\"a\":1}\n\ndata: two\ndata: lines\n\n";
        let mut whole = SseParser::new();
        let expected = whole.feed(wire);
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[1].data, "two\nlines");

        let mut bytewise = SseParser::new();
        let mut got = Vec::new();
        for b in wire.iter() {
            got.extend(bytewise.feed(&[*b]));
        }
        assert_eq!(got, expected);
        assert_eq!(bytewise.pending_bytes(), 0);
    }

    #[test]
    fn crlf_terminators() {
        let mut p = SseParser::new();
        let events = p.feed(b"data: x\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }
}
