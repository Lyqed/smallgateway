//! Std-only MOCK Google token plane for the GB-8 auth conformance suite:
//! one process serving BOTH endpoints of the WIF chain, so a test cannot
//! pass without speaking the REAL wire shapes.
//!
//! - `POST /v1/token` (Google STS token exchange): requires the FULL URN
//!   grant/token types (`urn:ietf:params:oauth:grant-type:token-exchange`,
//!   `...token-type:jwt`, `...token-type:access_token`), a non-empty
//!   `subject_token`, and an audience of the
//!   `//iam.googleapis.com/projects/...` shape — the exact fields real
//!   Google STS rejects when misspelled. Mints `fed-<n>`.
//! - `POST /v1/projects/-/serviceAccounts/<email>:generateAccessToken`:
//!   requires `Authorization: Bearer fed-*` (a token THIS mock minted) and
//!   a `lifetime` in the `"<n>s"` string form. Mints `sa-token-<n>` with an
//!   RFC3339 `expireTime`.
//!
//! The `<n>` counter makes caching provable: two requests through a warm
//! cache carry the same `sa-token-<n>` to the upstream.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gateway_core::aws::{form_decode, rfc3339};

static ISSUED: AtomicU64 = AtomicU64::new(0);

fn main() {
    let mut port = 6197u16;
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
    eprintln!("[mock-gcp] listening on 127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s) {
                    eprintln!("[mock-gcp] connection error: {e}");
                }
            }
            Err(e) => eprintln!("[mock-gcp] accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let target = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut content_length = 0usize;
    let mut authorization = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(());
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = value.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).to_string();

    if target == "/v1/token" {
        return token_exchange(stream, &body);
    }
    if target.starts_with("/v1/projects/-/serviceAccounts/") && target.ends_with(":generateAccessToken") {
        return generate_access_token(stream, &authorization, &body);
    }
    respond(stream, 404, "{\"error\":\"unknown path\"}")
}

/// Hop 1: the STS token exchange, verified against real Google STS's
/// pickiness (full URNs, the audience shape, a non-empty subject token).
fn token_exchange(stream: TcpStream, body: &str) -> std::io::Result<()> {
    let mut grant_type = String::new();
    let mut subject_token = String::new();
    let mut subject_token_type = String::new();
    let mut requested_token_type = String::new();
    let mut audience = String::new();
    for pair in body.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = form_decode(value);
        match name {
            "grant_type" => grant_type = value,
            "subject_token" => subject_token = value,
            "subject_token_type" => subject_token_type = value,
            "requested_token_type" => requested_token_type = value,
            "audience" => audience = value,
            _ => {}
        }
    }
    let fail = |msg: &str| {
        eprintln!("[mock-gcp] /v1/token 400: {msg}");
        respond(
            stream.try_clone().expect("clone"),
            400,
            &format!("{{\"error\":\"invalid_request\",\"error_description\":\"{msg}\"}}"),
        )
    };
    if grant_type != "urn:ietf:params:oauth:grant-type:token-exchange" {
        return fail("grant_type must be the full token-exchange URN");
    }
    if subject_token_type != "urn:ietf:params:oauth:token-type:jwt" {
        return fail("subject_token_type must be the full jwt URN");
    }
    if requested_token_type != "urn:ietf:params:oauth:token-type:access_token" {
        return fail("requested_token_type must be the full access_token URN");
    }
    if subject_token.is_empty() {
        return fail("subject_token is required");
    }
    if !audience.starts_with("//iam.googleapis.com/projects/")
        || !audience.contains("/workloadIdentityPools/")
        || !audience.contains("/providers/")
    {
        return fail("audience is not a workload-identity-pool provider resource");
    }
    let n = ISSUED.fetch_add(1, Ordering::SeqCst) + 1;
    eprintln!("[mock-gcp] token exchange #{n} ok (audience={audience})");
    respond(
        stream,
        200,
        &format!(
            "{{\"access_token\":\"fed-{n}\",\"issued_token_type\":\"urn:ietf:params:oauth:token-type:access_token\",\"token_type\":\"Bearer\",\"expires_in\":3599}}"
        ),
    )
}

/// Hop 2: generateAccessToken, requiring a federated bearer this mock
/// minted and the string-form lifetime.
fn generate_access_token(
    stream: TcpStream,
    authorization: &str,
    body: &str,
) -> std::io::Result<()> {
    let bearer = authorization.strip_prefix("Bearer ").unwrap_or("");
    if !bearer.starts_with("fed-") {
        eprintln!("[mock-gcp] generateAccessToken 401: bearer {bearer:?} was not minted here");
        return respond(stream, 401, "{\"error\":\"invalid federated token\"}");
    }
    // The "3600s" string form; a bare integer is what real GCP rejects.
    let has_string_lifetime = body.contains("\"lifetime\":\"") && body.contains("s\"");
    if !has_string_lifetime {
        eprintln!("[mock-gcp] generateAccessToken 400: lifetime must be the string form");
        return respond(stream, 400, "{\"error\":\"lifetime must be a duration string\"}");
    }
    let n = ISSUED.fetch_add(1, Ordering::SeqCst) + 1;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expire = rfc3339(now + 3600);
    eprintln!("[mock-gcp] generateAccessToken #{n} ok");
    respond(
        stream,
        200,
        &format!("{{\"accessToken\":\"sa-token-{n}\",\"expireTime\":\"{expire}\"}}"),
    )
}

fn respond(mut w: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
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
