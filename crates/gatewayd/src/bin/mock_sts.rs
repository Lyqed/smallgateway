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
//! Two-hop role chain support (the live-STS shape):
//! - `Action=AssumeRoleWithWebIdentity` mints BASE credentials from a
//!   non-empty `WebIdentityToken` (token-authed, never signed — real STS
//!   semantics);
//! - with `--require-signed-chain`, `Action=AssumeRole` MUST arrive
//!   SigV4-signed (service=sts) by mock-issued base credentials: the mock
//!   re-derives the secret from the access key id, recomputes the
//!   signature over the received headers, and independently checks the
//!   signed payload hash against the actual body. An unsigned or
//!   mis-signed chain call is refused with 403 — so a two-hop test can
//!   never pass vacuously.
//!
//! Remaining live-AWS differences: real STS returns opaque tokens and
//! enforces role trust policies.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gateway_core::aws::{self, form_decode, mock, rfc3339, SignParams};

static ISSUED: AtomicU64 = AtomicU64::new(0);

fn main() {
    let mut port = 6199u16;
    let mut require_signed_chain = false;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--require-signed-chain" => {
                require_signed_chain = true;
                i += 1;
                continue;
            }
            _ if i + 1 >= args.len() => {
                eprintln!("flag {} needs a value", args[i]);
                std::process::exit(2);
            }
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
    eprintln!(
        "[mock-sts] listening on 127.0.0.1:{port} (require-signed-chain={require_signed_chain})"
    );

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s, require_signed_chain) {
                    eprintln!("[mock-sts] connection error: {e}");
                }
            }
            Err(e) => eprintln!("[mock-sts] accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream, require_signed_chain: bool) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let mut content_length = 0usize;
    let mut headers: Vec<(String, String)> = Vec::new();
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
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    let body_bytes = body.clone();
    let body = String::from_utf8_lossy(&body);

    // Parse the Query-API form.
    let mut role_arn = String::new();
    let mut session_name = String::new();
    let mut web_identity_token = String::new();
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
            "RoleSessionName" => session_name = value,
            "WebIdentityToken" => web_identity_token = value,
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

    match action.as_str() {
        "AssumeRoleWithWebIdentity" => {
            if web_identity_token.is_empty() {
                eprintln!("[mock-sts] 400: empty WebIdentityToken");
                return respond(
                    stream,
                    400,
                    "<Error><Message>WebIdentityToken is required</Message></Error>",
                );
            }
            let n = ISSUED.fetch_add(1, Ordering::SeqCst) + 1;
            let access_key_id = format!("ASIAMOCK{n:04}");
            let secret = mock::secret_for(&access_key_id);
            let token = mock::session_token(&[]);
            let expiration = rfc3339(now() + duration_secs);
            eprintln!(
                "[mock-sts] AssumeRoleWithWebIdentity #{n}: role={role_arn} session={session_name} -> {access_key_id}"
            );
            let xml = format!(
                "<AssumeRoleWithWebIdentityResponse><AssumeRoleWithWebIdentityResult><Credentials>\
                 <AccessKeyId>{access_key_id}</AccessKeyId>\
                 <SecretAccessKey>{secret}</SecretAccessKey>\
                 <SessionToken>{token}</SessionToken>\
                 <Expiration>{expiration}</Expiration>\
                 </Credentials></AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>"
            );
            respond(stream, 200, &xml)
        }
        "AssumeRole" => {
            if require_signed_chain {
                if let Err(e) = verify_chain_signature(&headers, &body_bytes) {
                    eprintln!("[mock-sts] 403: {e}");
                    return respond(
                        stream,
                        403,
                        &format!("<Error><Message>{e}</Message></Error>"),
                    );
                }
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
            let expiration = rfc3339(now() + duration_secs);
            eprintln!(
                "[mock-sts] AssumeRole #{n}: role={role_arn} session={session_name} tags={} -> {access_key_id} (expires {expiration})",
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
        other => {
            let msg = format!("unsupported Action {other:?}");
            eprintln!("[mock-sts] 400: {msg}");
            respond(stream, 400, &format!("<Error><Message>{msg}</Message></Error>"))
        }
    }
}

/// Verify the SigV4 signature of a chained AssumeRole call: recompute the
/// signature from the received headers using the mock-derived secret, and
/// independently check the signed payload hash against the actual body (a
/// signature over the wrong body must not pass).
fn verify_chain_signature(headers: &[(String, String)], body: &[u8]) -> Result<(), String> {
    let get = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let auth = get("authorization").ok_or("chained AssumeRole must be SigV4-signed")?;
    let parsed = aws::parse_authorization(auth)?;
    let secret = mock::secret_for(&parsed.access_key_id);
    let payload_hash = get("x-amz-content-sha256").ok_or("no x-amz-content-sha256")?;
    if payload_hash != aws::sha256_hex(body) {
        return Err("payload hash does not match the request body".to_string());
    }
    let timestamp = get("x-amz-date").ok_or("no x-amz-date")?;
    let mut signed: Vec<(String, String)> = Vec::new();
    for name in &parsed.signed_headers {
        let value = get(name).ok_or_else(|| format!("signed header {name:?} missing"))?;
        signed.push((name.clone(), value.to_string()));
    }
    if parsed.service != "sts" {
        return Err(format!("signed for service {:?}, want sts", parsed.service));
    }
    let params = SignParams {
        method: "POST",
        path: "/",
        query: "",
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
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn respond(mut w: TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        _ => "Bad Request",
    };
    write!(
        w,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    w.flush()
}
