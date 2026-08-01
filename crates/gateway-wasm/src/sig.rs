//! Signed modules only (docs/02, docs/04). A WASM policy module is admitted
//! only if it carries a valid signature against an operator-configured key;
//! an unsigned or bad-signature module is REJECTED — at config load in the
//! data plane, and at admission in the control plane (`gatewayctl`), which
//! is why this verifier lives in a shared library both binaries link.
//!
//! The primitive is HMAC-SHA256 over the exact module bytes — the same
//! symmetric primitive the GB-2 JWT path already verifies tokens with
//! (`gateway_core::jwt`), so this phase adds NO new crypto dependency
//! family (the hard rule: wasmtime is the only new one). The operator holds
//! a signing key; the module ships with `hmac_sha256(key, module_bytes)`
//! hex-encoded in its manifest. A different key, a flipped byte in the
//! module, or a missing signature all fail verification identically.
//!
//! HMAC (symmetric) rather than a public-key scheme is a deliberate,
//! stated edge: the operator who runs the fleet is the one who signs its
//! modules, so a shared secret held by the control plane fits the trust
//! model (docs/02, GB-7 already assumes operator-held secrets). Asymmetric
//! signing (a build system signs, the fleet verifies with a public key) is
//! a later phase and noted as such in the README — the verification SEAM
//! here does not change when the algorithm does.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Why a module's signature did not verify. Every variant is fail-closed:
/// the module is not loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigError {
    /// No signature present on the module at all.
    Missing,
    /// The signature field was not valid lowercase hex.
    Malformed(String),
    /// Hex-valid but does not match `hmac(key, bytes)`.
    Mismatch,
}

impl std::fmt::Display for SigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SigError::Missing => write!(f, "module carries no signature (unsigned)"),
            SigError::Malformed(e) => write!(f, "module signature is malformed: {e}"),
            SigError::Mismatch => {
                write!(f, "module signature does not match the configured signing key")
            }
        }
    }
}

impl std::error::Error for SigError {}

/// The operator's signature over `module_bytes`, lowercase hex — what a
/// module manifest carries and what [`verify`] checks. Computed by the
/// module author (or `gatewayctl wasm sign`, a thin wrapper over this) with
/// the operator's key.
pub fn sign(key: &[u8], module_bytes: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(module_bytes);
    let tag = mac.finalize().into_bytes();
    let mut out = String::with_capacity(tag.len() * 2);
    for byte in tag {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Verify a module's declared signature against the operator key. Constant-
/// time via `Mac::verify_slice`. `signature` is the lowercase-hex tag from
/// the manifest; `None`/empty is [`SigError::Missing`] — an UNSIGNED module
/// is a distinct, loud rejection, never a silent pass.
pub fn verify(key: &[u8], module_bytes: &[u8], signature: Option<&str>) -> Result<(), SigError> {
    let sig = match signature {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return Err(SigError::Missing),
    };
    let raw = decode_hex(sig).map_err(SigError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(module_bytes);
    mac.verify_slice(&raw).map_err(|_| SigError::Mismatch)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", s.len()));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        other => Err(format!("non-hex byte {:?}", other as char)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"operator-signing-key";
    const MODULE: &[u8] = b"\0asm\x01\0\0\0 pretend module bytes";

    #[test]
    fn a_correct_signature_verifies() {
        let sig = sign(KEY, MODULE);
        assert!(verify(KEY, MODULE, Some(&sig)).is_ok());
    }

    #[test]
    fn an_unsigned_module_is_rejected_as_missing_not_mismatch() {
        assert_eq!(verify(KEY, MODULE, None), Err(SigError::Missing));
        assert_eq!(verify(KEY, MODULE, Some("")), Err(SigError::Missing));
        assert_eq!(verify(KEY, MODULE, Some("   ")), Err(SigError::Missing));
    }

    #[test]
    fn a_flipped_module_byte_fails_verification() {
        let sig = sign(KEY, MODULE);
        let mut tampered = MODULE.to_vec();
        tampered[10] ^= 0x01;
        assert_eq!(verify(KEY, &tampered, Some(&sig)), Err(SigError::Mismatch));
    }

    #[test]
    fn the_wrong_key_fails_verification() {
        let sig = sign(KEY, MODULE);
        assert_eq!(verify(b"other-key", MODULE, Some(&sig)), Err(SigError::Mismatch));
    }

    #[test]
    fn a_malformed_hex_signature_is_rejected() {
        assert!(matches!(
            verify(KEY, MODULE, Some("not-hex-zz")),
            Err(SigError::Malformed(_))
        ));
        assert!(matches!(
            verify(KEY, MODULE, Some("abc")), // odd length
            Err(SigError::Malformed(_))
        ));
    }

    #[test]
    fn sign_is_stable_and_lowercase_hex() {
        let a = sign(KEY, MODULE);
        let b = sign(KEY, MODULE);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }
}
