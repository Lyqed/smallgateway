//! Std-only MOCK STS for the GB-7 demo + conformance suite.
//!
//! No real AWS account exists in this environment; this binary proves the
//! MECHANISM (AssumeRole with session tags → credentials whose identity
//! carries the tag-set → SigV4 verification at the "Bedrock" side). The
//! contract with `mock_upstream --require-sigv4`, via
//! `gateway_core::aws::mock`:
//!
//! - `AccessKeyId` = `ASIAMOCK<n>` — a per-process counter, so a test can
//!   PROVE the credential cache: two requests with the same tag-set echo
//!   the same access key (one exchange), a different tag-set mints the
//!   next counter value;
//! - `SecretAccessKey` = a deterministic derivation from the access key id
//!   (both sides can compute it; nothing is shared out of band);
//! - `SessionToken` = base64url(JSON of the granted tags) — the mock
//!   Bedrock decodes it and echoes the tags, proving the attribution rode
//!   the CREDENTIALS, not a header.
//!
//! Live-AWS differences (documented follow-up, crates/gatewayd/README.md):
//! real STS authenticates the AssumeRole call itself (SigV4 with base
//! credentials), returns opaque tokens, and enforces role trust policies.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gateway_core::aws::{form_decode, mock, rfc3339};

static ISSUED: AtomicU64 = AtomicU64::new(0);

fn main() {
    let mut port = 6199u16;
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
    eprintln!("[mock-sts] listening on 127.0.0.1:{port}");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s) {
                    eprintln!("[mock-sts] connection error: {e}");
                }
            }
            Err(e) => eprintln!("[mock-sts] accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut content_length = 0usize;
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
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body);

    // Parse the AssumeRole Query-API form.
    let mut role_arn = String::new();
    let mut duration_secs: u64 = 900;
    let mut tag_keys: Vec<(usize, String)> = Vec::new();
    let mut tag_values: Vec<(usize, String)> = Vec::new();
    let mut action = String::new();
    for pair in body.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = form_decode(value);
        match name {
            "Action" => action = value,
            "RoleArn" => role_arn = value,
            "DurationSeconds" => duration_secs = value.parse().unwrap_or(900),
            _ => {
                // Tags.member.N.Key / Tags.member.N.Value
                if let Some(rest) = name.strip_prefix("Tags.member.") {
                    if let Some((n, field)) = rest.split_once('.') {
                        if let Ok(n) = n.parse::<usize>() {
                            match field {
                                "Key" => tag_keys.push((n, value)),
                                "Value" => tag_values.push((n, value)),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
    if action != "AssumeRole" {
        let msg = format!("unsupported Action {action:?}");
        eprintln!("[mock-sts] 400: {msg}");
        return respond(stream, 400, &format!("<Error><Message>{msg}</Message></Error>"));
    }
    tag_keys.sort();
    tag_values.sort();
    let tags: Vec<(String, String)> = tag_keys
        .into_iter()
        .zip(tag_values)
        .map(|((_, k), (_, v))| (k, v))
        .collect();

    let n = ISSUED.fetch_add(1, Ordering::SeqCst) + 1;
    let access_key_id = format!("ASIAMOCK{n:04}");
    let secret = mock::secret_for(&access_key_id);
    let token = mock::session_token(&tags);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expiration = rfc3339(now + duration_secs);

    eprintln!(
        "[mock-sts] AssumeRole #{n}: role={role_arn} tags={} -> {access_key_id} (expires {expiration})",
        tags.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(","),
    );

    let xml = format!(
        "<AssumeRoleResponse><AssumeRoleResult><Credentials>\
         <AccessKeyId>{access_key_id}</AccessKeyId>\
         <SecretAccessKey>{secret}</SecretAccessKey>\
         <SessionToken>{token}</SessionToken>\
         <Expiration>{expiration}</Expiration>\
         </Credentials></AssumeRoleResult></AssumeRoleResponse>"
    );
    respond(stream, 200, &xml)
}

fn respond(mut w: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    write!(
        w,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    w.flush()
}
