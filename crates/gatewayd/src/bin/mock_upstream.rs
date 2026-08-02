//! Std-only mock streaming upstream for the gatewayd demo + conformance
//! suite.
//!
//! Promoted from `spikes/proxy-pingora/src/bin/mock_upstream.rs` (Phase 0,
//! Spike B), with the Phase 1 additions that make governance visible from
//! the outside:
//!
//! - every `x-attr-*` request header it receives is echoed back as an
//!   `x-echo-attr-*` response header (GB-3: the caller's forged pin never
//!   shows here, the gateway's assigned value does);
//! - `--provider vertex`: the REQUEST BODY's `labels` object is echoed as
//!   `x-echo-label-*` response headers (GB-8: operator labels merged into
//!   the generateContent body, operator wins);
//! - `--provider bedrock` with `--require-sigv4`: the request must carry a
//!   SigV4 `Authorization` header; the mock re-derives the secret from the
//!   access key id (`gateway_core::aws::mock` — the mock STS pair), recomputes
//!   the signature over the received request, rejects a mismatch with 403,
//!   and echoes the session tags DECODED FROM THE SECURITY TOKEN as
//!   `x-echo-session-tag-*` headers plus `x-echo-access-key-id` (GB-7: the
//!   attribution rode the credentials, not a header).
//!
//! Serves a spike-event-model fixture over HTTP/1.1 chunked transfer,
//! one frame per chunk with a delay between chunks, so anything downstream
//! can only see the body incrementally. For `--provider bedrock` the JSONL
//! fixture is encoded into real event-stream binary frames (CRCs included)
//! and split into fixed-size chunks so frame boundaries never align with
//! chunk boundaries.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use gateway_core::adapters::bedrock::encode_jsonl_fixture;
use gateway_core::aws::{self, mock, SignParams};

const BEDROCK_CHUNK: usize = 96;

fn main() {
    let mut port = 6190u16;
    // Bind address. Defaults to 127.0.0.1 so every existing caller (the demo,
    // the tests, the in-pod sidecar) keeps loopback-only behavior unchanged.
    // A k8s data-plane Deployment reaching the mock as a cross-pod Service
    // passes `--bind 0.0.0.0`.
    let mut bind = String::from("127.0.0.1");
    let mut fixture = String::from("../../spikes/event-model/fixtures/openai.sse");
    let mut provider = String::from("openai");
    let mut delay_ms = 80u64;
    let mut require_sigv4 = false;
    let mut require_bearer: Option<String> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--require-sigv4" => {
                require_sigv4 = true;
                i += 1;
                continue;
            }
            _ if i + 1 >= args.len() => {
                eprintln!("flag {} needs a value", args[i]);
                std::process::exit(2);
            }
            "--port" => port = args[i + 1].parse().expect("port"),
            "--bind" => bind = args[i + 1].clone(),
            "--fixture" => fixture = args[i + 1].clone(),
            "--provider" => provider = args[i + 1].clone(),
            "--delay-ms" => delay_ms = args[i + 1].parse().expect("delay-ms"),
            "--require-bearer" => require_bearer = Some(args[i + 1].clone()),
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
        // chunk. Vertex streamGenerateContent is SSE too.
        let frames = raw
            .split_inclusive("\n\n")
            .filter(|f| !f.is_empty())
            .map(|f| f.as_bytes().to_vec())
            .collect::<Vec<_>>();
        ("text/event-stream", frames)
    };

    let listener = TcpListener::bind((bind.as_str(), port))
        .unwrap_or_else(|e| panic!("bind {bind}:{port}: {e}"));
    eprintln!(
        "[mock] serving {} ({} frames, {}ms apart) on {}:{}{}",
        fixture,
        frames.len(),
        delay_ms,
        bind,
        port,
        if require_sigv4 { " [sigv4 required]" } else { "" },
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
        let provider = provider.clone();
        let require_bearer = require_bearer.clone();
        thread::spawn(move || {
            if let Err(e) = handle(stream, &provider, require_sigv4, require_bearer.as_deref(), &content_type, &frames, delay_ms)
            {
                eprintln!("[mock] connection error: {e}");
            }
        });
    }
}

struct Request {
    line: String,
    method: String,
    /// Path + query, exactly as on the wire.
    target: String,
    /// Lowercase name → value, first value wins.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None); // client went away
    }
    let request_line = line.trim_end().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length = 0usize;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            if name == "transfer-encoding" && value.eq_ignore_ascii_case("chunked") {
                chunked = true;
            }
            headers.push((name, value));
        }
    }

    let body = if chunked {
        read_chunked(reader)?
    } else if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        body
    } else {
        Vec::new()
    };
    Ok(Some(Request { line: request_line, method, target, headers, body }))
}

fn read_chunked(reader: &mut BufReader<TcpStream>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line)? == 0 {
            break;
        }
        let size = usize::from_str_radix(size_line.trim(), 16).unwrap_or(0);
        if size == 0 {
            // trailing CRLF after the 0-chunk
            let mut end = String::new();
            let _ = reader.read_line(&mut end);
            break;
        }
        let mut chunk = vec![0u8; size + 2]; // data + CRLF
        reader.read_exact(&mut chunk)?;
        chunk.truncate(size);
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// GB-7 verification: recompute the SigV4 signature over the request as
/// received, using the secret the mock STS would have derived for this
/// access key. Returns the echo headers on success.
fn verify_sigv4(req: &Request) -> Result<Vec<(String, String)>, String> {
    let auth = req.header("authorization").ok_or("no Authorization header")?;
    let parsed = aws::parse_authorization(auth)?;
    let secret = mock::secret_for(&parsed.access_key_id);

    let mut signed: Vec<(String, String)> = Vec::new();
    for name in &parsed.signed_headers {
        let value = req
            .header(name)
            .ok_or_else(|| format!("signed header {name:?} missing from request"))?;
        signed.push((name.clone(), value.to_string()));
    }
    let (path, query) = req.target.split_once('?').unwrap_or((req.target.as_str(), ""));
    let timestamp = req.header("x-amz-date").ok_or("no x-amz-date")?;
    let payload_hash = req
        .header("x-amz-content-sha256")
        .ok_or("no x-amz-content-sha256")?;
    // Live-Bedrock strictness: the payload must be SIGNED (UNSIGNED-PAYLOAD
    // refused) and the declared hash must match the received body — so a
    // signature made before body finalization can never pass.
    if payload_hash == aws::UNSIGNED_PAYLOAD {
        return Err("UNSIGNED-PAYLOAD refused: live Bedrock signs the body".to_string());
    }
    if payload_hash != aws::sha256_hex(&req.body) {
        return Err("payload hash does not match the received body".to_string());
    }
    let params = SignParams {
        method: &req.method,
        path,
        query,
        region: &parsed.region,
        service: &parsed.service,
        headers: &signed,
        payload_hash,
        timestamp,
    };
    let expected = params.signature(&secret);
    if expected != parsed.signature {
        return Err(format!(
            "signature mismatch (expected {expected}, got {})",
            parsed.signature
        ));
    }
    let token = req
        .header("x-amz-security-token")
        .ok_or("no x-amz-security-token")?;
    let tags = mock::decode_session_token(token)?;
    let mut echo = vec![("x-echo-access-key-id".to_string(), parsed.access_key_id.clone())];
    for (key, value) in tags {
        eprintln!("[mock] verified session tag {key}={value}");
        echo.push((format!("x-echo-session-tag-{key}"), value));
    }
    Ok(echo)
}

/// GB-8 visibility: the `labels` object of the received JSON body, echoed
/// as `x-echo-label-*` headers.
fn body_label_echo(body: &[u8]) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let Some(labels) = v["labels"].as_object() else {
        return Vec::new();
    };
    labels
        .iter()
        .map(|(k, val)| {
            let val = val.as_str().map(str::to_string).unwrap_or_else(|| val.to_string());
            eprintln!("[mock] received body label {k}={val}");
            (format!("x-echo-label-{k}"), val)
        })
        .collect()
}

fn respond_403(mut w: TcpStream, reason: &str) -> std::io::Result<()> {
    eprintln!("[mock] 403: {reason}");
    let body = format!("{{\"error\":\"sigv4 verification failed\",\"reason\":\"{reason}\"}}");
    write!(
        w,
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    w.flush()
}

#[allow(clippy::too_many_arguments)] // one call site, a test binary
fn handle(
    stream: TcpStream,
    provider: &str,
    require_sigv4: bool,
    require_bearer: Option<&str>,
    content_type: &str,
    frames: &[Vec<u8>],
    delay_ms: u64,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(req) = read_request(&mut reader)? else {
        return Ok(());
    };

    // GB-3 visibility: which attribution headers actually arrived.
    let mut echo: Vec<(String, String)> = req
        .headers
        .iter()
        .filter_map(|(name, value)| {
            name.strip_prefix("x-attr-").map(|key| {
                eprintln!("[mock] received x-attr-{key}: {value}");
                (format!("x-echo-attr-{key}"), value.clone())
            })
        })
        .collect();

    // GB-7 visibility + enforcement (bedrock with --require-sigv4).
    if provider == "bedrock" && require_sigv4 {
        match verify_sigv4(&req) {
            Ok(sig_echo) => echo.extend(sig_echo),
            Err(reason) => return respond_403(stream, &reason),
        }
    }

    // GB-8 visibility: body labels (vertex).
    if provider == "vertex" {
        echo.extend(body_label_echo(&req.body));
    }

    // GB-8 auth enforcement (--require-bearer <prefix>): the request must
    // carry a gateway-minted SA bearer; the caller's own Authorization (or
    // none) is refused, and the accepted bearer is echoed so a test can
    // PROVE the token cache (same bearer across requests = one mint).
    if let Some(prefix) = require_bearer {
        let auth = req.header("authorization").unwrap_or("");
        let bearer = auth.strip_prefix("Bearer ").unwrap_or("");
        if !bearer.starts_with(prefix) {
            return respond_403(
                stream,
                &format!("expected a Bearer starting {prefix:?}, got {auth:?}"),
            );
        }
        eprintln!("[mock] accepted bearer {bearer}");
        echo.push(("x-echo-bearer".to_string(), bearer.to_string()));
    }

    eprintln!("[mock] {} -> streaming {} frames", req.line, frames.len());

    let mut w = stream;
    write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n"
    )?;
    // Echo what actually arrived: forged values never show here, the
    // gateway's adjudicated ones do.
    for (name, value) in &echo {
        write!(w, "{name}: {value}\r\n")?;
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
