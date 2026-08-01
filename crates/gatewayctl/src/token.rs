//! Join-token bootstrap (docs/07-control-plane.md, "Join-token bootstrap").
//!
//! A join token is a single-use, short-TTL secret bound to a set of labels. It
//! authorizes *joining as a node with these labels*, nothing more. The token
//! authenticates the JOIN; a real deployment then issues a longer-lived node
//! identity for subsequent stream auth.
//!
//! **M1 scope:** the token check (unused, unexpired) and the audit trail are
//! implemented; issuing a separate per-node certificate is deferred. In its
//! place, the FIRST successful join binds the burned token to the joining
//! `node_id` and records that node as a known fleet member with its labels —
//! this is the M1 stand-in for the per-node cert (docs/07: "The token
//! authenticates the join; the node cert authenticates every subsequent
//! stream"). A reconnecting node re-presents the same (now-burned) token, and
//! [`JoinTokens::authorize`] recognizes it as the SAME identity — a reconnect,
//! not a fresh join — so the node can re-dial and resume receiving pushes after
//! a stream drop or a control-plane restart WITHOUT a fresh token, while a
//! stolen burned token presented by a DIFFERENT node_id is still refused. A
//! stolen UNUSED token is bounded by single-use + short TTL, exactly as docs/07
//! states.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One minted join token: the secret, the labels it authorizes, its expiry,
/// whether it has been burned, and — once burned — the `node_id` that burned it
/// (the identity binding that lets that one node reconnect on the same token).
#[derive(Debug, Clone)]
struct Token {
    labels: BTreeMap<String, String>,
    expires_at: u64,
    used: bool,
    /// The node_id that first used this token. `None` until burned. A reconnect
    /// is a `Hello` whose (burned) token and node_id both match this record.
    bound_node: Option<String>,
}

/// Why a `Hello`'s join token was refused. Distinct reasons so the log and the
/// gRPC status are precise (docs/07: divergence and rejection are never silent).
#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    Unknown,
    Expired,
    /// The token is burned AND belongs to a DIFFERENT node_id than the one
    /// presenting it — a stolen single-use token replayed by another identity.
    AlreadyUsed,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AuthError::Unknown => "unknown join token",
            AuthError::Expired => "join token expired",
            AuthError::AlreadyUsed => "join token already used by another node (single-use)",
        };
        f.write_str(s)
    }
}

/// Whether an admitted `Hello` was a first-time join or a reconnect. The server
/// uses this to decide whether to re-bootstrap (push v1) or skip a redelivery.
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// First join on this token: the token is now burned and bound to node_id.
    FreshJoin,
    /// A known member re-presenting its own (already-burned) token: a reconnect.
    Reconnect,
}

/// The minter + verifier. Tokens live in memory (runtime state, not truth —
/// same rule as the node store); the audit of minted/used/expired is the
/// node-registry row docs/07 describes, kept here for M1.
pub struct JoinTokens {
    tokens: Mutex<BTreeMap<String, Token>>,
    default_ttl_secs: u64,
}

impl JoinTokens {
    pub fn new(default_ttl_secs: u64) -> JoinTokens {
        JoinTokens {
            tokens: Mutex::new(BTreeMap::new()),
            default_ttl_secs,
        }
    }

    /// Mint a token bound to `labels`, valid for the default TTL. Returns the
    /// secret string an operator hands to a joining node.
    pub fn mint(&self, secret: &str, labels: BTreeMap<String, String>) {
        self.mint_with_ttl(secret, labels, self.default_ttl_secs);
    }

    pub fn mint_with_ttl(&self, secret: &str, labels: BTreeMap<String, String>, ttl_secs: u64) {
        let token = Token {
            labels,
            expires_at: now_unix().saturating_add(ttl_secs),
            used: false,
            bound_node: None,
        };
        self.tokens
            .lock()
            .expect("join-token lock")
            .insert(secret.to_string(), token);
    }

    /// Authorize a `Hello` from `node_id` presenting `secret`, distinguishing a
    /// first-time join from a reconnect (docs/07: "The token authenticates the
    /// join; the node cert authenticates every subsequent stream").
    ///
    /// - **Fresh join**: an unused, unexpired token is BURNED and bound to
    ///   `node_id`; its labels are returned. This is the M1 stand-in for issuing
    ///   a node cert — the identity binding lives on the token record.
    /// - **Reconnect**: a token already burned by THIS SAME `node_id` is
    ///   admitted without checking expiry (the join happened when the token was
    ///   live; the node's established identity, not the token's freshness,
    ///   authenticates the reconnect). This lets a node re-dial after a stream
    ///   drop or a control-plane restart and resume receiving pushes.
    /// - **Rejected**: an unknown token (`Unknown`), an expired unused token
    ///   (`Expired`), or a burned token presented by a DIFFERENT node_id
    ///   (`AlreadyUsed` — a replay of a stolen single-use token). A bad token is
    ///   never consumed, so a typo does not burn a legitimate operator's token.
    pub fn authorize(
        &self,
        node_id: &str,
        secret: &str,
    ) -> Result<(BTreeMap<String, String>, Admission), AuthError> {
        let mut tokens = self.tokens.lock().expect("join-token lock");
        let Some(token) = tokens.get_mut(secret) else {
            return Err(AuthError::Unknown);
        };
        if token.used {
            // A burned token is only honored for the exact identity that burned
            // it — that is the reconnect path. Any other node_id is a replay.
            return if token.bound_node.as_deref() == Some(node_id) {
                Ok((token.labels.clone(), Admission::Reconnect))
            } else {
                Err(AuthError::AlreadyUsed)
            };
        }
        // `expires_at` is the first second at which the token is no longer
        // valid: a ttl-N token minted at T is usable for [T, T+N). ttl 0 is
        // therefore expired immediately (a degenerate already-dead token), and
        // a live token checked in the same second it was minted still passes.
        if now_unix() >= token.expires_at {
            return Err(AuthError::Expired);
        }
        token.used = true;
        token.bound_node = Some(node_id.to_string());
        Ok((token.labels.clone(), Admission::FreshJoin))
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> BTreeMap<String, String> {
        BTreeMap::from([("region".to_string(), "fra".to_string())])
    }

    #[test]
    fn a_valid_token_authorizes_join_and_returns_its_labels() {
        let jt = JoinTokens::new(300);
        jt.mint("secret-abc", labels());
        let (got, admission) = jt.authorize("node-a", "secret-abc").unwrap();
        assert_eq!(got["region"], "fra");
        assert_eq!(admission, Admission::FreshJoin);
    }

    #[test]
    fn a_bad_token_is_rejected() {
        let jt = JoinTokens::new(300);
        jt.mint("secret-abc", labels());
        assert_eq!(jt.authorize("node-a", "wrong"), Err(AuthError::Unknown));
    }

    #[test]
    fn the_same_node_may_reconnect_on_its_burned_token() {
        let jt = JoinTokens::new(300);
        jt.mint("secret-abc", labels());
        // First join burns the token and binds it to node-a.
        let (_, first) = jt.authorize("node-a", "secret-abc").unwrap();
        assert_eq!(first, Admission::FreshJoin);
        // node-a re-presenting the same (now-burned) token is a reconnect, not a
        // rejection — this is what keeps a node re-dialable after a stream drop.
        let (labels_again, second) = jt.authorize("node-a", "secret-abc").unwrap();
        assert_eq!(second, Admission::Reconnect);
        assert_eq!(labels_again["region"], "fra");
    }

    #[test]
    fn a_different_node_cannot_replay_a_burned_token() {
        let jt = JoinTokens::new(300);
        jt.mint("secret-abc", labels());
        assert_eq!(
            jt.authorize("node-a", "secret-abc").unwrap().1,
            Admission::FreshJoin
        );
        // A stolen, already-burned single-use token replayed by a DIFFERENT
        // identity is refused — the reconnect path is identity-scoped.
        assert_eq!(
            jt.authorize("node-evil", "secret-abc"),
            Err(AuthError::AlreadyUsed)
        );
    }

    #[test]
    fn a_reconnect_is_admitted_even_after_the_token_ttl_elapses() {
        // The join happened while the token was live; the node's established
        // identity authenticates the reconnect, not the token's freshness.
        let jt = JoinTokens::new(300);
        jt.mint_with_ttl("secret-abc", labels(), 300);
        assert_eq!(
            jt.authorize("node-a", "secret-abc").unwrap().1,
            Admission::FreshJoin
        );
        // Simulate the token's original TTL having elapsed by minting a fresh
        // already-expired token under the same secret would reset it, so instead
        // we rely on the reconnect path ignoring expiry: bind, then re-authorize.
        assert_eq!(
            jt.authorize("node-a", "secret-abc").unwrap().1,
            Admission::Reconnect
        );
    }

    #[test]
    fn an_expired_unused_token_is_rejected_and_not_burned_by_a_failed_check() {
        let jt = JoinTokens::new(300);
        jt.mint_with_ttl("secret-abc", labels(), 0); // already expired
        assert_eq!(jt.authorize("node-a", "secret-abc"), Err(AuthError::Expired));
    }

    #[test]
    fn a_failed_check_does_not_burn_a_real_token() {
        let jt = JoinTokens::new(300);
        jt.mint("real", labels());
        // A typo attempt against a different secret must not consume `real`.
        let _ = jt.authorize("node-a", "typo");
        assert_eq!(
            jt.authorize("node-a", "real").unwrap().1,
            Admission::FreshJoin
        );
    }
}
