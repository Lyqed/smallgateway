//! GB-7's wire half: the STS `AssumeRole` exchange (a minimal HTTP/1.1
//! client over tokio) and the SigV4 signing of upstream Bedrock requests.
//!
//! The pure parts — request body, response parsing, signature math, the
//! per-tag-set credential cache — live in `gateway_core::aws`; this module
//! only moves bytes. Session tags are resolved from ATTRIBUTION values by
//! the proxy before calling in here; nothing in this file ever reads a
//! caller header.
//!
//! Documented follow-up (no real AWS account exists in this environment):
//! live STS requires the AssumeRole call itself to be signed with base
//! credentials (instance profile / env), and live Bedrock requires a
//! signed payload hash instead of UNSIGNED-PAYLOAD. The mechanism —
//! exchange, cache, sign, verify — is proven against the mock STS + mock
//! Bedrock pair in tests/ and demo.sh; see README.md "GB-7 status".

use std::time::Duration;

use log::info;
use pingora::http::RequestHeader;
use pingora::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use gateway_core::aws::{
    self, cache_key, Credentials, CredentialCache, SignParams, UNSIGNED_PAYLOAD,
};
use gateway_core::config::{StsConfig, Upstream};

/// Whole-exchange timeout for one STS call.
const STS_TIMEOUT: Duration = Duration::from_secs(5);

/// Get credentials for this (resolved role identity, tag-set): cache hit,
/// or one AssumeRole exchange. `role_arn` and `session_name` arrive RESOLVED
/// (templates already rendered and sanitized by the proxy layer); both are
/// part of the cache identity so per-request session names never serve one
/// caller's CloudTrail identity to another. Returns the credentials plus
/// whether the cache served them.
pub async fn credentials_for(
    cache: &CredentialCache,
    sts: &StsConfig,
    role_arn: &str,
    session_name: &str,
    session_tags: &[(String, String)],
    now_unix: u64,
) -> Result<(Credentials, bool), String> {
    let endpoint = format!("{}:{}", sts.endpoint.host, sts.endpoint.port);
    let key = cache_key(role_arn, session_name, sts.duration_secs, &endpoint, session_tags);
    if let Some(creds) = cache.get(&key, now_unix) {
        return Ok((creds, true));
    }
    let creds = assume_role(sts, role_arn, session_name, session_tags).await?;
    cache.put(key, creds.clone());
    Ok((creds, false))
}

/// One `AssumeRole` exchange against the configured STS endpoint.
async fn assume_role(
    sts: &StsConfig,
    role_arn: &str,
    session_name: &str,
    session_tags: &[(String, String)],
) -> Result<Credentials, String> {
    let body = aws::assume_role_body(
        role_arn,
        session_name,
        sts.duration_secs,
        session_tags,
    );
    let (status, response) = tokio::time::timeout(
        STS_TIMEOUT,
        http_post_form(&sts.endpoint, "/", &body),
    )
    .await
    .map_err(|_| format!("STS call to {}:{} timed out", sts.endpoint.host, sts.endpoint.port))??;
    if status != 200 {
        return Err(format!(
            "STS returned {status}: {}",
            response.chars().take(200).collect::<String>()
        ));
    }
    let creds = aws::parse_assume_role_response(&response)?;
    info!(
        "[gb7] AssumeRole ok: role={role_arn} session_name={session_name} tags={} access_key={} expires_unix={}",
        session_tags
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(","),
        creds.access_key_id,
        creds.expiration_unix,
    );
    Ok(creds)
}

/// Minimal HTTP/1.1 form POST: write the request, read to EOF
/// (`Connection: close`), split head from body.
async fn http_post_form(
    endpoint: &Upstream,
    path: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", endpoint.host, endpoint.port);
    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("cannot connect to STS {addr}: {e}"))?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("STS write: {e}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("STS read: {e}"))?;
    let text = String::from_utf8_lossy(&response);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed STS response: {}", text.chars().take(80).collect::<String>()))?;
    let payload = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, payload))
}

/// SigV4-sign the outbound Bedrock request in place: sets `host` (the
/// canonical host must be what the upstream receives), `x-amz-date`,
/// `x-amz-security-token`, `x-amz-content-sha256` (UNSIGNED-PAYLOAD — the
/// documented live-AWS follow-up), and `authorization`.
pub fn sign_bedrock_request(
    upstream_request: &mut RequestHeader,
    upstream: &Upstream,
    region: &str,
    creds: &Credentials,
    now_unix: u64,
) -> Result<()> {
    let host = format!("{}:{}", upstream.host, upstream.port);
    let (timestamp, _) = aws::amz_date(now_unix);
    let path = upstream_request.uri.path().to_string();
    let query = upstream_request.uri.query().unwrap_or("").to_string();

    upstream_request.insert_header("host", host.clone())?;
    upstream_request.insert_header("x-amz-date", timestamp.clone())?;
    upstream_request.insert_header("x-amz-security-token", creds.session_token.clone())?;
    upstream_request.insert_header("x-amz-content-sha256", UNSIGNED_PAYLOAD)?;

    let signed_headers = vec![
        ("host".to_string(), host),
        ("x-amz-content-sha256".to_string(), UNSIGNED_PAYLOAD.to_string()),
        ("x-amz-date".to_string(), timestamp.clone()),
        ("x-amz-security-token".to_string(), creds.session_token.clone()),
    ];
    let params = SignParams {
        method: upstream_request.method.as_str(),
        path: &path,
        query: &query,
        region,
        service: "bedrock",
        headers: &signed_headers,
        payload_hash: UNSIGNED_PAYLOAD,
        timestamp: &timestamp,
    };
    let authorization = params.authorization(&creds.access_key_id, &creds.secret_access_key);
    upstream_request.insert_header("authorization", authorization)?;
    Ok(())
}
