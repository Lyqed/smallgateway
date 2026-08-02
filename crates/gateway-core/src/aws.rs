//! GB-7's pure half: SigV4 request signing, the STS `AssumeRole` wire
//! shapes (with session tags — the invoice-grade join), and the per-tag-set
//! credential cache.
//!
//! Everything here is deterministic and I/O-free: the actual STS HTTP
//! exchange lives in gatewayd (`aws_auth.rs`), and the mock STS / mock
//! Bedrock binaries verify against the SAME primitives, so a signature that
//! round-trips in tests is computed exactly once, one way.
//!
//! **Scope honesty (documented follow-up):** requests are signed with
//! `x-amz-content-sha256: UNSIGNED-PAYLOAD`. Live Bedrock requires the
//! SHA-256 of the payload; wiring the body hash through the proxy's
//! streaming request path — and verifying against a real AWS account —
//! is the recorded follow-up in crates/gatewayd/README.md. The mechanism
//! proven here (AssumeRole with attribution-derived session tags →
//! per-tag-set cached credentials → SigV4-signed upstream requests) is the
//! GB-7 claim.

use std::collections::HashMap;
use std::sync::Mutex;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// Refresh credentials this many seconds before they actually expire.
pub const REFRESH_MARGIN_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration_unix: u64,
}

// ---------------------------------------------------------------- AssumeRole

/// The `AssumeRole` Query-API form body, session tags included
/// (`Tags.member.N.Key/Value`), exactly as STS expects it.
pub fn assume_role_body(
    role_arn: &str,
    session_name: &str,
    duration_secs: u32,
    tags: &[(String, String)],
) -> String {
    let mut body = format!(
        "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName={}&DurationSeconds={}",
        form_encode(role_arn),
        form_encode(session_name),
        duration_secs,
    );
    for (i, (key, value)) in tags.iter().enumerate() {
        let n = i + 1;
        body.push_str(&format!(
            "&Tags.member.{n}.Key={}&Tags.member.{n}.Value={}",
            form_encode(key),
            form_encode(value),
        ));
    }
    body
}

/// Parse the `AssumeRoleResponse` XML (the four credential fields; a
/// deliberately narrow extractor, not an XML parser).
pub fn parse_assume_role_response(xml: &str) -> Result<Credentials, String> {
    let field = |name: &str| -> Result<String, String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = xml
            .find(&open)
            .ok_or_else(|| format!("STS response has no <{name}>"))?
            + open.len();
        let end = xml[start..]
            .find(&close)
            .ok_or_else(|| format!("STS response has unterminated <{name}>"))?;
        Ok(xml[start..start + end].to_string())
    };
    let access_key_id = field("AccessKeyId")?;
    let secret_access_key = field("SecretAccessKey")?;
    let session_token = field("SessionToken")?;
    let expiration = field("Expiration")?;
    let expiration_unix = chrono::DateTime::parse_from_rfc3339(&expiration)
        .map_err(|e| format!("STS Expiration {expiration:?} is not RFC3339: {e}"))?
        .timestamp()
        .max(0) as u64;
    Ok(Credentials {
        access_key_id,
        secret_access_key,
        session_token,
        expiration_unix,
    })
}

// ------------------------------------------------------------------- SigV4

/// `YYYYMMDD'T'HHMMSS'Z'` and `YYYYMMDD` for `x-amz-date` / the scope.
pub fn amz_date(now_unix: u64) -> (String, String) {
    let dt = chrono::DateTime::from_timestamp(now_unix as i64, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).expect("epoch"));
    (
        dt.format("%Y%m%dT%H%M%SZ").to_string(),
        dt.format("%Y%m%d").to_string(),
    )
}

/// RFC3339 UTC timestamp (`2026-08-01T13:00:00Z`) — the STS `Expiration`
/// format; used by the mock STS when minting responses.
pub fn rfc3339(unix: u64) -> String {
    chrono::DateTime::from_timestamp(unix as i64, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).expect("epoch"))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Everything SigV4 hashes. `headers` must contain every header being
/// signed (at least `host` and `x-amz-date`), values exactly as sent.
#[derive(Debug)]
pub struct SignParams<'a> {
    pub method: &'a str,
    /// The request path, exactly as it appears on the wire.
    pub path: &'a str,
    /// The raw query string ("" for none).
    pub query: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub headers: &'a [(String, String)],
    /// Hex SHA-256 of the payload, or [`UNSIGNED_PAYLOAD`].
    pub payload_hash: &'a str,
    /// `YYYYMMDD'T'HHMMSS'Z'` — must equal the `x-amz-date` header.
    pub timestamp: &'a str,
}

impl SignParams<'_> {
    fn scope(&self) -> String {
        let date = &self.timestamp[..8];
        format!("{date}/{}/{}/aws4_request", self.region, self.service)
    }

    fn sorted_headers(&self) -> Vec<(String, String)> {
        let mut hs: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        hs.sort();
        hs
    }

    pub fn signed_headers(&self) -> String {
        self.sorted_headers()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";")
    }

    fn canonical_request(&self) -> String {
        let hs = self.sorted_headers();
        let canonical_headers: String =
            hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let canonical_query = canonical_query(self.query);
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            self.path,
            canonical_query,
            canonical_headers,
            self.signed_headers(),
            self.payload_hash,
        )
    }

    fn string_to_sign(&self) -> String {
        format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            self.timestamp,
            self.scope(),
            hex(&Sha256::digest(self.canonical_request().as_bytes())),
        )
    }

    /// The hex signature over the derived signing key.
    pub fn signature(&self, secret_access_key: &str) -> String {
        let date = &self.timestamp[..8];
        let k_date = hmac(format!("AWS4{secret_access_key}").as_bytes(), date.as_bytes());
        let k_region = hmac(&k_date, self.region.as_bytes());
        let k_service = hmac(&k_region, self.service.as_bytes());
        let k_signing = hmac(&k_service, b"aws4_request");
        hex(&hmac(&k_signing, self.string_to_sign().as_bytes()))
    }

    /// The full `Authorization` header value.
    pub fn authorization(&self, access_key_id: &str, secret_access_key: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            access_key_id,
            self.scope(),
            self.signed_headers(),
            self.signature(secret_access_key),
        )
    }
}

/// Sort query parameters by encoded name/value, preserving encoding.
fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut params: Vec<(&str, &str)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect();
    params.sort();
    params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// A parsed `Authorization: AWS4-HMAC-SHA256 ...` header — what the mock
/// Bedrock uses to recompute and compare (the verification half).
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedAuthorization {
    pub access_key_id: String,
    pub date: String,
    pub region: String,
    pub service: String,
    pub signed_headers: Vec<String>,
    pub signature: String,
}

pub fn parse_authorization(header: &str) -> Result<ParsedAuthorization, String> {
    let rest = header
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or("not a SigV4 Authorization header")?;
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("Credential=") {
            credential = Some(v);
        } else if let Some(v) = part.strip_prefix("SignedHeaders=") {
            signed_headers = Some(v);
        } else if let Some(v) = part.strip_prefix("Signature=") {
            signature = Some(v);
        }
    }
    let credential = credential.ok_or("Authorization has no Credential")?;
    let parts: Vec<&str> = credential.split('/').collect();
    let [access_key_id, date, region, service, tail] = parts.as_slice() else {
        return Err(format!("malformed Credential scope {credential:?}"));
    };
    if *tail != "aws4_request" {
        return Err(format!("Credential scope must end in aws4_request, got {tail:?}"));
    }
    Ok(ParsedAuthorization {
        access_key_id: access_key_id.to_string(),
        date: date.to_string(),
        region: region.to_string(),
        service: service.to_string(),
        signed_headers: signed_headers
            .ok_or("Authorization has no SignedHeaders")?
            .split(';')
            .map(str::to_string)
            .collect(),
        signature: signature.ok_or("Authorization has no Signature")?.to_string(),
    })
}

// -------------------------------------------------------- credential cache

/// Cache credentials per unique (role, endpoint, tag-set), honoring the
/// STS-granted expiry with a refresh margin. One unique tag-set = one STS
/// exchange until expiry — attribution changes force fresh credentials,
/// which is exactly the invoice-grade property.
#[derive(Debug, Default)]
pub struct CredentialCache {
    inner: Mutex<HashMap<String, Credentials>>,
}

/// The cache key: every input that changes the minted credentials. The
/// RESOLVED session name and the duration are part of the identity: with
/// per-request session names, credentials minted under caller A's
/// RoleSessionName must never be served to caller B — the session name is
/// what CloudTrail attributes, and misattribution there is the exact
/// failure this product exists to prevent.
pub fn cache_key(
    role_arn: &str,
    session_name: &str,
    duration_secs: u32,
    endpoint: &str,
    tags: &[(String, String)],
) -> String {
    let mut sorted: Vec<&(String, String)> = tags.iter().collect();
    sorted.sort();
    let tag_part: Vec<String> = sorted.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!(
        "{role_arn}|{session_name}|{duration_secs}|{endpoint}|{}",
        tag_part.join(",")
    )
}

/// Sanitize a rendered RoleSessionName to AWS's real constraint:
/// `[A-Za-z0-9_+=,.@-]`, 2-64 chars. Invalid characters map to `-`; the
/// result is truncated to 64; a result shorter than AWS's 2-char minimum
/// falls back to the validated default rather than reaching STS as a
/// guaranteed ValidationError.
pub fn sanitize_session_name(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | ',' | '.' | '@' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    while out.ends_with(['-', '_']) {
        out.pop();
    }
    if out.chars().count() < 2 {
        return "gatewayd".to_string();
    }
    out
}

impl CredentialCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A hit only while the credentials stay valid past the refresh margin.
    pub fn get(&self, key: &str, now_unix: u64) -> Option<Credentials> {
        let map = self.inner.lock().expect("credential cache lock");
        map.get(key)
            .filter(|c| c.expiration_unix > now_unix + REFRESH_MARGIN_SECS)
            .cloned()
    }

    pub fn put(&self, key: String, creds: Credentials) {
        self.inner
            .lock()
            .expect("credential cache lock")
            .insert(key, creds);
    }
}

// ------------------------------------------------------------ mock helpers

/// Shared derivations for the MOCK STS / mock Bedrock pair (tests + demo
/// only; no real AWS account exists here — docs/gatewayd README record the
/// live-AWS follow-up). The mock STS derives the secret from the access
/// key id, and encodes the granted session tags INTO the session token;
/// the mock Bedrock re-derives the secret to verify the signature and
/// decodes the token to echo the tags — proving the tag-set actually
/// rode the credentials, not a header.
pub mod mock {
    use super::*;

    const MOCK_SEED: &[u8] = b"mock-sts-shared-seed";

    pub fn secret_for(access_key_id: &str) -> String {
        hex(&hmac(MOCK_SEED, access_key_id.as_bytes()))
    }

    pub fn session_token(tags: &[(String, String)]) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let map: serde_json::Map<String, serde_json::Value> = tags
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&serde_json::json!({ "tags": map })).unwrap())
    }

    pub fn decode_session_token(token: &str) -> Result<Vec<(String, String)>, String> {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|e| format!("session token is not base64url: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("session token payload: {e}"))?;
        let tags = v["tags"]
            .as_object()
            .ok_or("session token has no tags object")?;
        Ok(tags
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
            .collect())
    }
}

// ---------------------------------------------------------------- helpers

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// RFC 3986 unreserved-set percent-encoding (what the STS Query API
/// expects for form values).
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Decode a form-encoded value (the mock STS's parsing half).
pub fn form_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?');
                out.push(h);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The official SigV4 example from the AWS documentation
    /// (GET iam ListUsers, 2015-08-30): reproducing its published
    /// signature proves the canonicalization + key derivation against an
    /// external reference, not against ourselves.
    #[test]
    fn sigv4_matches_the_aws_documentation_example() {
        let headers = vec![
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded; charset=utf-8".to_string(),
            ),
            ("host".to_string(), "iam.amazonaws.com".to_string()),
            ("x-amz-date".to_string(), "20150830T123600Z".to_string()),
        ];
        let empty_hash = sha256_hex(b"");
        let params = SignParams {
            method: "GET",
            path: "/",
            query: "Action=ListUsers&Version=2010-05-08",
            region: "us-east-1",
            service: "iam",
            headers: &headers,
            payload_hash: &empty_hash,
            timestamp: "20150830T123600Z",
        };
        assert_eq!(
            params.signature("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
        let auth = params.authorization("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY");
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request"
        ));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"));
    }

    #[test]
    fn authorization_round_trips_through_the_parser() {
        let headers = vec![
            ("host".to_string(), "127.0.0.1:6190".to_string()),
            ("x-amz-date".to_string(), "20260801T120000Z".to_string()),
            ("x-amz-security-token".to_string(), "tok".to_string()),
        ];
        let params = SignParams {
            method: "POST",
            path: "/model/anthropic.claude/converse-stream",
            query: "",
            region: "us-east-1",
            service: "bedrock",
            headers: &headers,
            payload_hash: UNSIGNED_PAYLOAD,
            timestamp: "20260801T120000Z",
        };
        let auth = params.authorization("ASIAMOCK0001", "secret");
        let parsed = parse_authorization(&auth).unwrap();
        assert_eq!(parsed.access_key_id, "ASIAMOCK0001");
        assert_eq!(parsed.region, "us-east-1");
        assert_eq!(parsed.service, "bedrock");
        assert_eq!(
            parsed.signed_headers,
            vec!["host", "x-amz-date", "x-amz-security-token"]
        );
        // The verifier's recomputation matches (same params, same secret).
        assert_eq!(parsed.signature, params.signature("secret"));
        // And a different secret does not.
        assert_ne!(parsed.signature, params.signature("other"));
    }

    #[test]
    fn assume_role_body_carries_session_tags_in_order() {
        let body = assume_role_body(
            "arn:aws:iam::123:role/gateway",
            "gatewayd",
            900,
            &[
                ("team".to_string(), "ml-research".to_string()),
                ("env".to_string(), "prod".to_string()),
            ],
        );
        assert!(body.contains("Action=AssumeRole"));
        assert!(body.contains("RoleArn=arn%3Aaws%3Aiam%3A%3A123%3Arole%2Fgateway"));
        assert!(body.contains("Tags.member.1.Key=team"));
        assert!(body.contains("Tags.member.1.Value=ml-research"));
        assert!(body.contains("Tags.member.2.Key=env"));
        assert!(body.contains("Tags.member.2.Value=prod"));
    }

    #[test]
    fn assume_role_response_parses_and_bad_xml_is_named() {
        let xml = r#"<AssumeRoleResponse><AssumeRoleResult><Credentials>
            <AccessKeyId>ASIAMOCK0001</AccessKeyId>
            <SecretAccessKey>sk</SecretAccessKey>
            <SessionToken>tok</SessionToken>
            <Expiration>2026-08-01T13:00:00Z</Expiration>
            </Credentials></AssumeRoleResult></AssumeRoleResponse>"#;
        let creds = parse_assume_role_response(xml).unwrap();
        assert_eq!(creds.access_key_id, "ASIAMOCK0001");
        assert_eq!(creds.session_token, "tok");
        assert!(creds.expiration_unix > 1_700_000_000);

        let err = parse_assume_role_response("<nope/>").unwrap_err();
        assert!(err.contains("AccessKeyId"), "{err}");
    }

    #[test]
    fn cache_hits_only_while_valid_and_expires_with_margin() {
        let cache = CredentialCache::new();
        let key = cache_key(
            "arn:role",
            "gatewayd",
            900,
            "127.0.0.1:6199",
            &[("team".to_string(), "ml".to_string())],
        );
        let creds = Credentials {
            access_key_id: "a".into(),
            secret_access_key: "s".into(),
            session_token: "t".into(),
            expiration_unix: 1000,
        };
        cache.put(key.clone(), creds.clone());
        assert_eq!(cache.get(&key, 900), Some(creds));
        // Inside the refresh margin → treated as expired.
        assert_eq!(cache.get(&key, 1000 - REFRESH_MARGIN_SECS), None);
        assert_eq!(cache.get(&key, 2000), None);
    }

    #[test]
    fn cache_key_is_order_insensitive_but_value_sensitive() {
        let a = cache_key("r", "s", 900, "e", &[("a".into(), "1".into()), ("b".into(), "2".into())]);
        let b = cache_key("r", "s", 900, "e", &[("b".into(), "2".into()), ("a".into(), "1".into())]);
        let c = cache_key("r", "s", 900, "e", &[("a".into(), "1".into()), ("b".into(), "3".into())]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn cache_key_separates_session_names_and_durations() {
        // Per-request session names: caller A's CloudTrail identity must
        // never be served to caller B off the cache, even with an identical
        // tag-set. Same for a changed requested duration.
        let tags = [("t".to_string(), "x".to_string())];
        let a = cache_key("r", "acme-app-1", 900, "e", &tags);
        let b = cache_key("r", "acme-app-2", 900, "e", &tags);
        let c = cache_key("r", "acme-app-1", 3600, "e", &tags);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn sanitize_session_name_matches_aws_constraints() {
        // Invalid chars map to '-', 64-char truncation, trailing junk
        // trimmed, and the 2-char AWS minimum falls back to the default.
        assert_eq!(sanitize_session_name("acme-app-user"), "acme-app-user");
        assert_eq!(sanitize_session_name("A B/C"), "A-B-C");
        assert_eq!(sanitize_session_name("x"), "gatewayd");
        assert_eq!(sanitize_session_name(""), "gatewayd");
        assert_eq!(sanitize_session_name("a!!"), "gatewayd");
        let long = "a".repeat(80);
        assert_eq!(sanitize_session_name(&long).chars().count(), 64);
        assert_eq!(sanitize_session_name("ok@user.name+x=y,z"), "ok@user.name+x=y,z");
    }

    #[test]
    fn mock_token_round_trips_tags_and_secret_derivation_is_stable() {
        let tags = vec![("team".to_string(), "ml".to_string())];
        let token = mock::session_token(&tags);
        assert_eq!(mock::decode_session_token(&token).unwrap(), tags);
        assert_eq!(mock::secret_for("ASIAX"), mock::secret_for("ASIAX"));
        assert_ne!(mock::secret_for("ASIAX"), mock::secret_for("ASIAY"));
    }

    #[test]
    fn form_encoding_round_trips() {
        let original = "arn:aws:iam::123:role/gw team+x";
        assert_eq!(form_decode(&form_encode(original)), original);
    }
}
