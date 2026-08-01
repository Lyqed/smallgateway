//! Minimal HS256 JWT verification for GB-2 claim mappings.
//!
//! Deliberately narrow: HS256 with a shared secret only, `alg` pinned so an
//! `alg: none` (or RS256-confusion) token is rejected outright, signature
//! checked in constant time via the `hmac` crate, optional `exp` honored.
//! Asymmetric algorithms and JWKS are a later phase; this is the smallest
//! thing that makes "proven" mean something.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, PartialEq, Eq)]
pub enum JwtError {
    Malformed(&'static str),
    WrongAlg(String),
    BadSignature,
    Expired,
}

impl std::fmt::Display for JwtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JwtError::Malformed(what) => write!(f, "malformed token: {what}"),
            JwtError::WrongAlg(alg) => write!(f, "unsupported alg {alg:?} (only HS256)"),
            JwtError::BadSignature => write!(f, "signature verification failed"),
            JwtError::Expired => write!(f, "token expired"),
        }
    }
}

/// Verify an HS256 token and return its claims. `now_unix` is injected so
/// expiry is testable; callers pass wall-clock seconds.
pub fn verify_hs256(
    token: &str,
    secret: &[u8],
    now_unix: u64,
) -> Result<serde_json::Map<String, Value>, JwtError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(sig_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed("expected three dot-separated parts"));
    };

    let header: Value = decode_json(header_b64, "header")?;
    // Pin the algorithm before touching the signature: the header is
    // attacker-controlled until the MAC verifies.
    match header["alg"].as_str() {
        Some("HS256") => {}
        Some(other) => return Err(JwtError::WrongAlg(other.to_string())),
        None => return Err(JwtError::Malformed("header has no alg")),
    }

    let sig = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| JwtError::Malformed("signature is not base64url"))?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(header_b64.as_bytes());
    mac.update(b".");
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig).map_err(|_| JwtError::BadSignature)?;

    let payload: Value = decode_json(payload_b64, "payload")?;
    let claims = payload
        .as_object()
        .ok_or(JwtError::Malformed("payload is not a JSON object"))?
        .clone();
    if let Some(exp) = claims.get("exp").and_then(Value::as_u64) {
        if exp < now_unix {
            return Err(JwtError::Expired);
        }
    }
    Ok(claims)
}

fn decode_json(b64: &str, what: &'static str) -> Result<Value, JwtError> {
    let bytes = URL_SAFE_NO_PAD.decode(b64).map_err(|_| JwtError::Malformed(what))?;
    serde_json::from_slice(&bytes).map_err(|_| JwtError::Malformed(what))
}

/// Mint an HS256 token — for tests and demo harnesses only; the gateway
/// verifies, it never signs.
pub fn sign_hs256(claims: &serde_json::Map<String, Value>, secret: &[u8]) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Value::Object(claims.clone())).unwrap());
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(header.as_bytes());
    mac.update(b".");
    mac.update(payload.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{header}.{payload}.{sig}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"unit-test-secret";
    const NOW: u64 = 1_754_000_000;

    fn claims(json: Value) -> serde_json::Map<String, Value> {
        json.as_object().unwrap().clone()
    }

    #[test]
    fn roundtrip_verifies_and_returns_claims() {
        let token = sign_hs256(&claims(serde_json::json!({"sub": "alice", "team": "ml"})), SECRET);
        let got = verify_hs256(&token, SECRET, NOW).unwrap();
        assert_eq!(got["sub"], "alice");
        assert_eq!(got["team"], "ml");
    }

    #[test]
    fn tampered_payload_fails_signature() {
        let token = sign_hs256(&claims(serde_json::json!({"sub": "alice"})), SECRET);
        let forged_payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"mallory"}"#);
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = &forged_payload;
        let forged = parts.join(".");
        assert_eq!(verify_hs256(&forged, SECRET, NOW), Err(JwtError::BadSignature));
    }

    #[test]
    fn wrong_secret_fails_signature() {
        let token = sign_hs256(&claims(serde_json::json!({"sub": "alice"})), SECRET);
        assert_eq!(verify_hs256(&token, b"other", NOW), Err(JwtError::BadSignature));
    }

    #[test]
    fn alg_none_is_rejected_before_signature_check() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"mallory"}"#);
        let token = format!("{header}.{payload}.");
        assert_eq!(
            verify_hs256(&token, SECRET, NOW),
            Err(JwtError::WrongAlg("none".to_string()))
        );
    }

    #[test]
    fn expired_token_is_rejected_and_future_exp_accepted() {
        let expired = sign_hs256(&claims(serde_json::json!({"sub": "a", "exp": NOW - 1})), SECRET);
        assert_eq!(verify_hs256(&expired, SECRET, NOW), Err(JwtError::Expired));
        let live = sign_hs256(&claims(serde_json::json!({"sub": "a", "exp": NOW + 60})), SECRET);
        assert!(verify_hs256(&live, SECRET, NOW).is_ok());
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        assert!(matches!(verify_hs256("", SECRET, NOW), Err(JwtError::Malformed(_))));
        assert!(matches!(verify_hs256("a.b", SECRET, NOW), Err(JwtError::Malformed(_))));
        assert!(matches!(verify_hs256("a.b.c.d", SECRET, NOW), Err(JwtError::Malformed(_))));
        assert!(matches!(verify_hs256("!!.??.@@", SECRET, NOW), Err(JwtError::Malformed(_))));
    }
}
