//! Std-only mock streaming upstream for the gatewayd demo.
//!
//! Promoted from `spikes/proxy-pingora/src/bin/mock_upstream.rs` (Phase 0,
//! Spike B), with one Phase 1 addition: every `x-attr-*` request header it
//! receives is echoed back as an `x-echo-attr-*` response header (and
//! logged), so the demo can prove which attribution values actually reached
//! the upstream — the GB-3 overwrite becomes visible in curl output.
//!
//! Serves a spike-event-model fixture over HTTP/1.1 chunked transfer,
//! one frame per chunk with a delay between chunks, so anything downstream
//! can only see the body incrementally. For `--provider bedrock` the JSONL
//! fixture is encoded into real event-stream binary frames (CRCs included)
//! and split into fixed-size chunks so frame boundaries never align with
//! chunk boundaries.
//!
//! Usage:
//!   mock_upstream --port 6190 \
//!     --fixture ../../spikes/event-model/fixtures/openai.sse \
//!     --provider openai --delay-ms 80

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use gateway_core::adapters::bedrock::encode_jsonl_fixture;

const BEDROCK_CHUNK: usize = 96;

fn main() {
    let mut port = 6190u16;
    let mut fixture = String::from("../../spikes/event-model/fixtures/openai.sse");
    let mut provider = String::from("openai");
    let mut delay_ms = 80u64;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i + 1 < args.len() {
        match args[i].as_str() {
            "--port" => port = args[i + 1].parse().expect("port"),
            "--fixture" => fixture = args[i + 1].clone(),
            "--provider" => provider = args[i + 1].clone(),
            "--delay-ms" => delay_ms = args[i + 1].parse().expect("delay-ms"),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }

    let raw = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("cannot read fixture {fixture}: {e}"));

    let (content_type, frames): (&str, Vec<Vec<u8>>) = if provider == "bedrock" {
        let wire = encode_jsonl_fixture(&raw).expect("encode bedrock fixture");
        let frames = wire
            .chunks(BEDROCK_CHUNK)
            .map(|c| c.to_vec())
            .collect::<Vec<_>>();
        ("application/vnd.amazon.eventstream", frames)
    } else {
        // One SSE event (up to and including its blank-line delimiter) per
        // chunk.
        let frames = raw
            .split_inclusive("\n\n")
            .filter(|f| !f.is_empty())
            .map(|f| f.as_bytes().to_vec())
            .collect::<Vec<_>>();
        ("text/event-stream", frames)
    };

    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!(
        "[mock] serving {} ({} frames, {}ms apart) on 127.0.0.1:{}",
        fixture,
        frames.len(),
        delay_ms,
        port
    );

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[mock] accept error: {e}");
                continue;
            }
        };
        let frames = frames.clone();
        let content_type = content_type.to_string();
        thread::spawn(move || {
            if let Err(e) = handle(stream, &content_type, &frames, delay_ms) {
                eprintln!("[mock] connection error: {e}");
            }
        });
    }
}

fn handle(
    stream: TcpStream,
    content_type: &str,
    frames: &[Vec<u8>],
    delay_ms: u64,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    // Read request head; collect the attribution headers we received (the
    // proof of GB-3) and drain any Content-Length body so the socket is
    // clean before we respond.
    let mut content_length = 0usize;
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut line = String::new();
    reader.read_line(&mut line)?; // request line
    let request_line = line.trim_end().to_string();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(()); // client went away
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            } else if let Some(key) = name.strip_prefix("x-attr-") {
                attrs.push((key.to_string(), value.to_string()));
            }
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }
    for (key, value) in &attrs {
        eprintln!("[mock] received x-attr-{key}: {value}");
    }
    eprintln!("[mock] {request_line} -> streaming {} frames", frames.len());

    let mut w = stream;
    write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n"
    )?;
    // Echo what actually arrived: the caller's forged pin never shows here,
    // the gateway's assigned value does.
    for (key, value) in &attrs {
        write!(w, "x-echo-attr-{key}: {value}\r\n")?;
    }
    write!(w, "\r\n")?;
    w.flush()?;
    for frame in frames {
        write!(w, "{:X}\r\n", frame.len())?;
        w.write_all(frame)?;
        w.write_all(b"\r\n")?;
        w.flush()?;
        thread::sleep(Duration::from_millis(delay_ms));
    }
    w.write_all(b"0\r\n\r\n")?;
    w.flush()?;
    Ok(())
}
