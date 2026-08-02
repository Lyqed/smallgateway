//! GB-8's auth half: the gateway mints the Google credential itself.
//!
//! Two hops, the live-GCP Workload Identity Federation shape:
//!
//! 1. the platform's OIDC token is exchanged at Google STS
//!    (`POST /v1/token`) for a FEDERATED access token. The grant/token
//!    types are the FULL URNs — real Google STS rejects shorthands;
//! 2. the federated token calls iamcredentials
//!    `serviceAccounts/{email}:generateAccessToken` (lifetime in the
//!    `"<n>s"` string form) for a SERVICE-ACCOUNT access token;
//! 3. that bearer signs the upstream Vertex request.
//!
//! The SA token carries NO per-caller identity: per-caller attribution
//! rides the GB-8 billing labels in the request body, which is exactly why
//! cross-caller token reuse cannot misattribute cost. The cache is keyed by
//! (sa, scopes, pool, provider, endpoints) — two auth blocks sharing an SA
//! across different pools never collide. Honest boundary, documented: Cloud
//! Audit Logs' delegation info shows the MINTING principal, so cached reuse
//! smears the security audit trail across callers; billing is unaffected.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use log::info;

use gateway_core::aws::REFRESH_MARGIN_SECS;
use gateway_core::config::VertexAuth;

use crate::aws_auth::http_post;

/// Whole-exchange timeout for one token hop.
const GCP_TIMEOUT: Duration = Duration::from_secs(5);

/// One cached SA access token.
#[derive(Debug, Clone)]
struct Token {
    bearer: String,
    expiration_unix: u64,
}

/// The SA-token cache. Same refresh-margin discipline as the AWS
/// credential cache.
#[derive(Debug, Default)]
pub struct GcpTokenCache {
    inner: Mutex<HashMap<String, Token>>,
}

impl GcpTokenCache {
    pub fn new() -> Self {
        Self::default()
    }
}

fn cache_key(auth: &VertexAuth) -> String {
    format!(
        "{}|{}|{}|{}|{}:{}|{}:{}",
        auth.service_account_email,
        auth.scopes.join(" "),
        auth.wif.pool_id,
        auth.wif.provider_id,
        auth.sts_endpoint.host,
        auth.sts_endpoint.port,
        auth.iam_endpoint.host,
        auth.iam_endpoint.port,
    )
}

/// The SA bearer for this provider: cache hit, or the two-hop mint.
/// Returns `(bearer, cache_hit)`.
pub async fn bearer_for(
    cache: &GcpTokenCache,
    auth: &VertexAuth,
    now_unix: u64,
) -> Result<(String, bool), String> {
    let key = cache_key(auth);
    {
        let map = cache.inner.lock().expect("gcp token cache lock");
        if let Some(t) = map.get(&key) {
            if t.expiration_unix > now_unix + REFRESH_MARGIN_SECS {
                return Ok((t.bearer.clone(), true));
            }
        }
    }
    let token = mint(auth, now_unix).await?;
    let bearer = token.bearer.clone();
    cache
        .inner
        .lock()
        .expect("gcp token cache lock")
        .insert(key, token);
    Ok((bearer, false))
}

async fn mint(auth: &VertexAuth, now_unix: u64) -> Result<Token, String> {
    let oidc = crate::aws_auth::read_web_identity_token(&auth.web_identity_token)?;

    // Hop 1: Google STS token exchange. FULL URNs or real STS rejects it.
    let sts_body = format!(
        "grant_type={}&audience={}&requested_token_type={}&subject_token_type={}&subject_token={}&scope={}",
        url_encode("urn:ietf:params:oauth:grant-type:token-exchange"),
        url_encode(&auth.wif.audience()),
        url_encode("urn:ietf:params:oauth:token-type:access_token"),
        url_encode("urn:ietf:params:oauth:token-type:jwt"),
        url_encode(&oidc),
        url_encode(&auth.scopes.join(" ")),
    );
    let (status, body) = tokio::time::timeout(
        GCP_TIMEOUT,
        http_post(
            &auth.sts_endpoint,
            "/v1/token",
            "application/x-www-form-urlencoded",
            &sts_body,
            &[],
        ),
    )
    .await
    .map_err(|_| "GCP STS token exchange timed out".to_string())??;
    if status != 200 {
        return Err(format!(
            "GCP STS token exchange returned {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let federated = json_str_field(&body, "access_token")
        .ok_or_else(|| "GCP STS response has no access_token".to_string())?;

    // Hop 2: mint the SA access token. lifetime is the "<n>s" string form.
    let iam_path = format!(
        "/v1/projects/-/serviceAccounts/{}:generateAccessToken",
        auth.service_account_email
    );
    let iam_body = format!(
        "{{\"scope\":[{}],\"lifetime\":\"{}s\"}}",
        auth.scopes
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(","),
        auth.lifetime_secs,
    );
    let bearer_header = [("authorization".to_string(), format!("Bearer {federated}"))];
    let (status, body) = tokio::time::timeout(
        GCP_TIMEOUT,
        http_post(
            &auth.iam_endpoint,
            &iam_path,
            "application/json",
            &iam_body,
            &bearer_header,
        ),
    )
    .await
    .map_err(|_| "GCP generateAccessToken timed out".to_string())??;
    if status != 200 {
        return Err(format!(
            "GCP generateAccessToken returned {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let bearer = json_str_field(&body, "accessToken")
        .ok_or_else(|| "generateAccessToken response has no accessToken".to_string())?;
    let expiration_unix = json_str_field(&body, "expireTime")
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|dt| dt.timestamp().max(0) as u64)
        .unwrap_or(now_unix + u64::from(auth.lifetime_secs));
    info!(
        "[gb8] SA token minted: sa={} expires_unix={expiration_unix}",
        auth.service_account_email
    );
    Ok(Token { bearer, expiration_unix })
}

/// Narrow JSON string-field extractor (the two Google responses are flat;
/// serde_json stays available if these ever grow structure).
fn json_str_field(body: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get(field)?.as_str().map(str::to_string)
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
