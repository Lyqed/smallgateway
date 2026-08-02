//! Std-only MOCK OTLP collector for the telemetry conformance suite: it
//! ACCEPTS `POST /v1/traces` only after STRICTLY validating the OTLP/JSON
//! shape a real collector's OTLP receiver would parse — resource with
//! `service.name`, scope spans, 32-hex traceId / 16-hex spanId, ordered
//! timestamps, typed attribute envelopes. Malformed spans get a 400, so
//! the e2e test cannot pass with an export a real collector would drop.
//!
//! `GET /received` returns every accepted span (flattened) so a test can
//! poll for the request span and assert on its attributes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

use serde_json::Value;

static RECEIVED: Mutex<Vec<Value>> = Mutex::new(Vec::new());

fn main() {
    let mut port = 4318u16;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i + 1 < args.len() {
        match args[i].as_str() {
            "--port" => port = args[i + 1].parse().expect("port"),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!("[mock-otlp] listening on 127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s) {
                    eprintln!("[mock-otlp] connection error: {e}");
                }
            }
            Err(e) => eprintln!("[mock-otlp] accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            return Ok(());
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((name, value)) = h.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;

    match (method.as_str(), target.as_str()) {
        ("POST", "/v1/traces") => match validate_and_extract(&body) {
            Ok(spans) => {
                let n = spans.len();
                RECEIVED.lock().expect("received lock").extend(spans);
                eprintln!("[mock-otlp] accepted {n} span(s)");
                respond(stream, 200, "{}")
            }
            Err(msg) => {
                eprintln!("[mock-otlp] 400: {msg}");
                respond(stream, 400, &format!("{{\"error\":\"{msg}\"}}"))
            }
        },
        ("GET", "/received") => {
            let spans = RECEIVED.lock().expect("received lock").clone();
            respond(stream, 200, &Value::Array(spans).to_string())
        }
        _ => respond(stream, 404, "{\"error\":\"unknown route\"}"),
    }
}

/// The strict shape check a real OTLP receiver implies. Returns the
/// flattened spans (with the resource's service.name attached) on success.
fn validate_and_extract(body: &[u8]) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let resource_spans = v["resourceSpans"]
        .as_array()
        .ok_or("no resourceSpans array")?;
    let mut out = Vec::new();
    for rs in resource_spans {
        let res_attrs = rs["resource"]["attributes"]
            .as_array()
            .ok_or("resource has no attributes")?;
        let service = res_attrs
            .iter()
            .find(|a| a["key"] == "service.name")
            .and_then(|a| a["value"]["stringValue"].as_str())
            .ok_or("resource has no service.name")?
            .to_string();
        let scope_spans = rs["scopeSpans"].as_array().ok_or("no scopeSpans array")?;
        for ss in scope_spans {
            let spans = ss["spans"].as_array().ok_or("no spans array")?;
            for s in spans {
                let trace_id = s["traceId"].as_str().ok_or("span has no traceId")?;
                let span_id = s["spanId"].as_str().ok_or("span has no spanId")?;
                if trace_id.len() != 32 || !trace_id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("traceId is not 32 hex chars".to_string());
                }
                if span_id.len() != 16 || !span_id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("spanId is not 16 hex chars".to_string());
                }
                let start: u64 = s["startTimeUnixNano"]
                    .as_str()
                    .and_then(|t| t.parse().ok())
                    .ok_or("startTimeUnixNano must be a stringified integer")?;
                let end: u64 = s["endTimeUnixNano"]
                    .as_str()
                    .and_then(|t| t.parse().ok())
                    .ok_or("endTimeUnixNano must be a stringified integer")?;
                if end < start {
                    return Err("span ends before it starts".to_string());
                }
                let attrs = s["attributes"].as_array().ok_or("span has no attributes")?;
                for a in attrs {
                    if a["key"].as_str().is_none() || !a["value"].is_object() {
                        return Err("attribute without key or typed value envelope".to_string());
                    }
                }
                let mut flat = s.clone();
                flat["service"] = Value::String(service.clone());
                out.push(flat);
            }
        }
    }
    if out.is_empty() {
        return Err("no spans in export".to_string());
    }
    Ok(out)
}

fn respond(mut w: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    write!(
        w,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    w.flush()
}
