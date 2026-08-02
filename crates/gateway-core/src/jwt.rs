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
            JwtError::WrongAlg(alg) => write!(f, "unsupported alg {alg:?}"),
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

// ------------------------------------------------------------- RS256 / JWKS

use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::signature::Verifier;
use rsa::{BigUint, RsaPublicKey};

/// A parsed JWKS: the RSA public keys of a real IdP, with their `kid`s.
/// Built at CONFIG LOAD from the inline `auth.jwt.jwks` document, so a bad
/// key set is a load-time error, never a per-request surprise, and key
/// rotation rides the same hot-swap as every other rule.
#[derive(Debug, Clone)]
pub struct Jwks {
    keys: Vec<(Option<String>, RsaPublicKey)>,
}

impl Jwks {
    /// Parse a standard JWKS document. Only RSA keys are read (`kty: RSA`,
    /// base64url `n`/`e`); other key types are skipped. Zero usable keys is
    /// an error — an empty verifier that rejects everything is a
    /// misconfiguration, not a policy.
    pub fn parse(json: &str) -> Result<Jwks, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("jwks is not JSON: {e}"))?;
        let keys_json = v["keys"]
            .as_array()
            .ok_or("jwks has no \"keys\" array")?;
        let mut keys = Vec::new();
        for (i, k) in keys_json.iter().enumerate() {
            if k["kty"].as_str() != Some("RSA") {
                continue;
            }
            let n = k["n"]
                .as_str()
                .ok_or_else(|| format!("jwks key {i}: no \"n\""))?;
            let e = k["e"]
                .as_str()
                .ok_or_else(|| format!("jwks key {i}: no \"e\""))?;
            let n = URL_SAFE_NO_PAD
                .decode(n)
                .map_err(|_| format!("jwks key {i}: \"n\" is not base64url"))?;
            let e = URL_SAFE_NO_PAD
                .decode(e)
                .map_err(|_| format!("jwks key {i}: \"e\" is not base64url"))?;
            let key = RsaPublicKey::new(BigUint::from_bytes_be(&n), BigUint::from_bytes_be(&e))
                .map_err(|err| format!("jwks key {i}: invalid RSA key: {err}"))?;
            keys.push((k["kid"].as_str().map(str::to_string), key));
        }
        if keys.is_empty() {
            return Err("jwks contains no usable RSA keys".to_string());
        }
        Ok(Jwks { keys })
    }

    /// The candidate keys for a token's `kid`: the matching key when one
    /// exists, else every key (a JWKS without kids still verifies).
    fn candidates(&self, kid: Option<&str>) -> Vec<&RsaPublicKey> {
        if let Some(kid) = kid {
            let matched: Vec<&RsaPublicKey> = self
                .keys
                .iter()
                .filter(|(k, _)| k.as_deref() == Some(kid))
                .map(|(_, key)| key)
                .collect();
            if !matched.is_empty() {
                return matched;
            }
        }
        self.keys.iter().map(|(_, key)| key).collect()
    }
}

/// Verify an RS256 token against the parsed JWKS and return its claims.
/// The same shape and pinning discipline as [`verify_hs256`]: `alg` is
/// checked BEFORE the signature (an `alg: none` or HS256-confusion token
/// is refused outright), `exp` honored.
pub fn verify_rs256(
    token: &str,
    jwks: &Jwks,
    now_unix: u64,
) -> Result<serde_json::Map<String, Value>, JwtError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(sig_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed("expected three dot-separated parts"));
    };

    let header: Value = decode_json(header_b64, "header")?;
    match header["alg"].as_str() {
        Some("RS256") => {}
        Some(other) => return Err(JwtError::WrongAlg(other.to_string())),
        None => return Err(JwtError::Malformed("header has no alg")),
    }

    let sig = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| JwtError::Malformed("signature is not base64url"))?;
    let sig = Signature::try_from(sig.as_slice())
        .map_err(|_| JwtError::Malformed("signature is not an RSA signature"))?;
    let message = format!("{header_b64}.{payload_b64}");
    let kid = header["kid"].as_str();
    let verified = jwks.candidates(kid).into_iter().any(|key| {
        VerifyingKey::<Sha256>::new(key.clone())
            .verify(message.as_bytes(), &sig)
            .is_ok()
    });
    if !verified {
        return Err(JwtError::BadSignature);
    }

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

#[cfg(test)]
mod rs256_tests {
    use super::*;
    use rsa::pkcs1v15::SigningKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;

    fn keypair() -> (RsaPrivateKey, RsaPublicKey) {
        let mut rng = rand::thread_rng();
        let private = RsaPrivateKey::new(&mut rng, 2048).expect("keygen");
        let public = private.to_public_key();
        (private, public)
    }

    fn jwks_json(keys: &[(&str, &RsaPublicKey)]) -> String {
        let entries: Vec<String> = keys
            .iter()
            .map(|(kid, key)| {
                format!(
                    "{{\"kty\":\"RSA\",\"kid\":\"{kid}\",\"n\":\"{}\",\"e\":\"{}\"}}",
                    URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
                    URL_SAFE_NO_PAD.encode(key.e().to_bytes_be()),
                )
            })
            .collect();
        format!("{{\"keys\":[{}]}}", entries.join(","))
    }

    fn mint(private: &RsaPrivateKey, kid: &str, claims: &str) -> String {
        let header = URL_SAFE_NO_PAD
            .encode(format!("{{\"alg\":\"RS256\",\"kid\":\"{kid}\"}}"));
        let payload = URL_SAFE_NO_PAD.encode(claims);
        let message = format!("{header}.{payload}");
        let sig = SigningKey::<Sha256>::new(private.clone())
            .sign(message.as_bytes())
            .to_bytes();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(sig))
    }

    #[test]
    fn rs256_round_trip_with_kid_selection_and_rejections() {
        let (priv_a, pub_a) = keypair();
        let (priv_b, pub_b) = keypair();
        let jwks =
            Jwks::parse(&jwks_json(&[("a", &pub_a), ("b", &pub_b)])).expect("jwks parses");

        // kid-selected verification, both keys.
        let ta = mint(&priv_a, "a", r#"{"sub":"alice","exp":4102444800}"#);
        let claims = verify_rs256(&ta, &jwks, 1_000).expect("verifies");
        assert_eq!(claims["sub"], "alice");
        let tb = mint(&priv_b, "b", r#"{"sub":"bob"}"#);
        assert_eq!(verify_rs256(&tb, &jwks, 1_000).unwrap()["sub"], "bob");

        // A signature from a key NOT in the set is refused, even with a
        // known kid claimed in the header.
        let (priv_evil, _) = keypair();
        let forged = mint(&priv_evil, "a", r#"{"sub":"intruder"}"#);
        assert_eq!(verify_rs256(&forged, &jwks, 1_000), Err(JwtError::BadSignature));

        // Expired refused; HS256-confusion refused before any crypto.
        let expired = mint(&priv_a, "a", r#"{"sub":"late","exp":10}"#);
        assert_eq!(verify_rs256(&expired, &jwks, 1_000), Err(JwtError::Expired));
        let confused = ta.replace(
            &URL_SAFE_NO_PAD.encode("{\"alg\":\"RS256\",\"kid\":\"a\"}"),
            &URL_SAFE_NO_PAD.encode("{\"alg\":\"HS256\",\"kid\":\"a\"}"),
        );
        assert!(matches!(
            verify_rs256(&confused, &jwks, 1_000),
            Err(JwtError::WrongAlg(_)) | Err(JwtError::BadSignature)
        ));
    }

    #[test]
    fn jwks_parse_refuses_junk_and_empty_sets() {
        assert!(Jwks::parse("not json").is_err());
        assert!(Jwks::parse("{\"keys\":[]}").is_err());
        // A non-RSA key alone is an empty usable set.
        assert!(Jwks::parse("{\"keys\":[{\"kty\":\"EC\",\"crv\":\"P-256\"}]}").is_err());
    }
}
