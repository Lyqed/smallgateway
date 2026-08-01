//! Multi-wave rollout grouped by failure domain (docs/07-control-plane.md,
//! "Partial application: all-or-nothing waves, chosen").
//!
//! A rollout config defines an ORDERED list of waves, each a label selector over
//! node labels (region/cluster/cloud). A node belongs to the FIRST wave whose
//! selector it matches; a node matching no wave falls into an implicit final
//! wave (docs/07: "nodes matching no wave are their own implicit final wave").
//! Applying a commit walks the waves in order: push to every node in wave k,
//! wait for every node to ack the exact render_hash within a per-wave timeout,
//! and only then proceed to wave k+1. On any Nack or timeout the wave HALTS —
//! wave k and ALL LATER waves stay on their prior committed version, while
//! earlier waves that already acked stay advanced.
//!
//! This module is the PURE substrate: selectors, node-to-wave assignment, and
//! the ordered plan. The sequencing that pushes and awaits acks lives in
//! `server.rs` (`roll_out_plan`); the per-wave committed-state bookkeeping lives
//! in `fleet.rs`. Selectors are simple label equality / set-membership (docs/07:
//! "not a full expression language"), so this is plain Rust, no new dependency.
//!
//! ## The degenerate one-wave case
//!
//! A plan with no configured waves is a SINGLE implicit wave over every node —
//! byte-for-byte the milestone-1/2 behavior. `WavePlan::single()` builds exactly
//! that, so the existing single-wave semantics are the degenerate one-wave case
//! and keep passing their tests.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One selector term over a node label. A node matches the term when the value
/// of `label` on the node is one of `in_values` (set membership; a single value
/// is the equality case). Absent the label, the term does not match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorTerm {
    pub label: String,
    /// The accepted values — membership. One value is plain equality.
    pub in_values: Vec<String>,
}

impl SelectorTerm {
    /// Equality: `label == value`.
    pub fn eq(label: &str, value: &str) -> SelectorTerm {
        SelectorTerm {
            label: label.to_string(),
            in_values: vec![value.to_string()],
        }
    }

    /// Set membership: `label in {values…}`.
    pub fn in_set(label: &str, values: &[&str]) -> SelectorTerm {
        SelectorTerm {
            label: label.to_string(),
            in_values: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Whether this term matches a node's labels.
    fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        match labels.get(&self.label) {
            Some(v) => self.in_values.iter().any(|accepted| accepted == v),
            None => false,
        }
    }
}

/// A label selector: the conjunction (AND) of its terms. An EMPTY selector
/// matches every node (the "everything" selector), which is how the degenerate
/// single wave and a catch-all final wave are expressed. Simple equality /
/// set-membership only — no expression language (docs/07).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selector {
    pub terms: Vec<SelectorTerm>,
}

impl Selector {
    /// The match-everything selector (no terms).
    pub fn everything() -> Selector {
        Selector { terms: Vec::new() }
    }

    /// Build from terms.
    pub fn of(terms: Vec<SelectorTerm>) -> Selector {
        Selector { terms }
    }

    /// Whether every term matches the node's labels (empty selector = always).
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.terms.iter().all(|t| t.matches(labels))
    }
}

/// One wave: a human name (for logs and the surfaced committed-state) plus the
/// selector that decides which nodes belong to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wave {
    pub name: String,
    pub selector: Selector,
}

/// What to do with a node that matches NO configured wave (docs/07 open question
/// "or a configurable default"). The MVP default follows docs/07: unmatched
/// nodes are their own implicit FINAL wave, rolled last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnmatchedPolicy {
    /// Unmatched nodes form an implicit final wave, rolled after every named
    /// wave (docs/07's stated default).
    #[default]
    ImplicitFinalWave,
}

/// The ordered wave plan: the named waves in rollout order plus the policy for
/// nodes matching none of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavePlan {
    pub waves: Vec<Wave>,
    pub unmatched: UnmatchedPolicy,
}

/// The reserved name for the implicit final wave that catches nodes matching no
/// configured selector.
pub const IMPLICIT_FINAL_WAVE: &str = "implicit-final";

impl WavePlan {
    /// The degenerate single wave: one wave named "all" that matches every node.
    /// This reproduces the milestone-1/2 single-wave behavior exactly.
    pub fn single() -> WavePlan {
        WavePlan {
            waves: vec![Wave {
                name: "all".to_string(),
                selector: Selector::everything(),
            }],
            unmatched: UnmatchedPolicy::ImplicitFinalWave,
        }
    }

    /// Build an ordered plan from named waves. `unmatched` governs nodes that
    /// match none of them.
    pub fn new(waves: Vec<Wave>, unmatched: UnmatchedPolicy) -> WavePlan {
        WavePlan { waves, unmatched }
    }

    /// Assign a set of nodes (id -> labels) to waves IN ORDER. Returns the
    /// ordered list of `(wave_name, node_ids)` — every configured wave in order,
    /// then (if any unmatched nodes exist) the implicit final wave. A node
    /// belongs to the FIRST wave whose selector matches (docs/07). Empty waves
    /// are retained in the ordering so the surfaced state names them; the caller
    /// skips pushing to an empty wave but still reports it.
    pub fn assign<'a, I>(&self, nodes: I) -> Vec<AssignedWave>
    where
        I: IntoIterator<Item = (&'a str, &'a BTreeMap<String, String>)>,
    {
        // Bucket per configured wave index; unmatched collected separately.
        let mut buckets: Vec<Vec<String>> = vec![Vec::new(); self.waves.len()];
        let mut unmatched: Vec<String> = Vec::new();

        // Sort node ids for a deterministic assignment order.
        let mut node_vec: Vec<(&str, &BTreeMap<String, String>)> = nodes.into_iter().collect();
        node_vec.sort_by_key(|(a, _)| *a);

        for (id, labels) in node_vec {
            match self
                .waves
                .iter()
                .position(|w| w.selector.matches(labels))
            {
                Some(idx) => buckets[idx].push(id.to_string()),
                None => unmatched.push(id.to_string()),
            }
        }

        let mut out: Vec<AssignedWave> = Vec::new();
        for (idx, wave) in self.waves.iter().enumerate() {
            out.push(AssignedWave {
                name: wave.name.clone(),
                node_ids: std::mem::take(&mut buckets[idx]),
            });
        }
        // The implicit final wave, only if it caught anyone.
        if !unmatched.is_empty() {
            out.push(AssignedWave {
                name: IMPLICIT_FINAL_WAVE.to_string(),
                node_ids: unmatched,
            });
        }
        out
    }

    /// The wave index (0-based) a node with `labels` belongs to under this plan,
    /// where the implicit final wave is `self.waves.len()`. Used by the
    /// reconciler to decide whether a node is in a not-yet-applied LATER wave
    /// (legitimately on the old version, pending — not drifted). A node matching
    /// no configured wave is in the implicit final wave.
    pub fn wave_index_for(&self, labels: &BTreeMap<String, String>) -> usize {
        self.waves
            .iter()
            .position(|w| w.selector.matches(labels))
            .unwrap_or(self.waves.len())
    }
}

/// One wave's assignment: its name and the node ids that landed in it, in
/// rollout order relative to the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedWave {
    pub name: String,
    pub node_ids: Vec<String>,
}

// --- Parsing the plan from the config repo ---------------------------------

/// The on-disk shape of `waves.yaml` in the config repo: an ordered list of
/// `{ name, selector: { region: eu } | { region: [eu, us] } }`. Kept in the Git
/// config repo so the rollout order is itself reviewed and reproducible (docs/07
/// "Truth in Git"). Deserialized with serde; no live templating.
#[derive(Debug, Deserialize)]
struct WavesFile {
    #[serde(default)]
    waves: Vec<WaveEntry>,
}

#[derive(Debug, Deserialize)]
struct WaveEntry {
    name: String,
    /// `label -> value` (equality) or `label -> [values]` (membership).
    #[serde(default)]
    selector: BTreeMap<String, SelectorValue>,
}

/// A selector value is either a single string (equality) or a list of strings
/// (set membership).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SelectorValue {
    One(String),
    Many(Vec<String>),
}

/// A malformed `waves.yaml`.
#[derive(Debug)]
pub struct WavePlanError(pub String);

impl std::fmt::Display for WavePlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid waves.yaml: {}", self.0)
    }
}

impl std::error::Error for WavePlanError {}

/// Parse a `waves.yaml` document into a [`WavePlan`]. Absent or empty file →
/// the degenerate single plan (so a repo without a rollout config keeps the
/// milestone-1/2 single-wave behavior). A wave with an empty selector matches
/// everything, which — placed before others — would starve later waves; that is
/// the operator's stated intent (a global first wave), not an error.
pub fn parse_wave_plan(yaml: &str) -> Result<WavePlan, WavePlanError> {
    if yaml.trim().is_empty() {
        return Ok(WavePlan::single());
    }
    let parsed: WavesFile =
        serde_yaml::from_str(yaml).map_err(|e| WavePlanError(e.to_string()))?;
    if parsed.waves.is_empty() {
        return Ok(WavePlan::single());
    }
    let mut waves = Vec::new();
    for entry in parsed.waves {
        if entry.name.trim().is_empty() {
            return Err(WavePlanError("a wave has an empty name".to_string()));
        }
        if entry.name == IMPLICIT_FINAL_WAVE {
            return Err(WavePlanError(format!(
                "the wave name {IMPLICIT_FINAL_WAVE:?} is reserved for the implicit final wave"
            )));
        }
        let mut terms = Vec::new();
        for (label, val) in entry.selector {
            let in_values = match val {
                SelectorValue::One(v) => vec![v],
                SelectorValue::Many(vs) => {
                    if vs.is_empty() {
                        return Err(WavePlanError(format!(
                            "wave {:?} selector {label:?} has an empty value list",
                            entry.name
                        )));
                    }
                    vs
                }
            };
            terms.push(SelectorTerm { label, in_values });
        }
        waves.push(Wave {
            name: entry.name,
            selector: Selector::of(terms),
        });
    }
    // Reject a duplicate wave name (the surfaced committed-state keys on it).
    let mut seen = std::collections::BTreeSet::new();
    for w in &waves {
        if !seen.insert(w.name.clone()) {
            return Err(WavePlanError(format!("duplicate wave name {:?}", w.name)));
        }
    }
    Ok(WavePlan::new(waves, UnmatchedPolicy::ImplicitFinalWave))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // --- Selector matching ---------------------------------------------------

    #[test]
    fn equality_selector_matches_only_the_exact_value() {
        let s = Selector::of(vec![SelectorTerm::eq("region", "eu")]);
        assert!(s.matches(&labels(&[("region", "eu")])));
        assert!(!s.matches(&labels(&[("region", "us")])));
        assert!(!s.matches(&labels(&[("cluster", "eu")])), "wrong label");
    }

    #[test]
    fn set_membership_matches_any_listed_value() {
        let s = Selector::of(vec![SelectorTerm::in_set("region", &["eu", "us"])]);
        assert!(s.matches(&labels(&[("region", "eu")])));
        assert!(s.matches(&labels(&[("region", "us")])));
        assert!(!s.matches(&labels(&[("region", "ap")])));
    }

    #[test]
    fn multi_term_selector_is_a_conjunction() {
        let s = Selector::of(vec![
            SelectorTerm::eq("region", "eu"),
            SelectorTerm::eq("cloud", "aws"),
        ]);
        assert!(s.matches(&labels(&[("region", "eu"), ("cloud", "aws")])));
        assert!(!s.matches(&labels(&[("region", "eu"), ("cloud", "gcp")])));
    }

    #[test]
    fn empty_selector_matches_everything() {
        let s = Selector::everything();
        assert!(s.matches(&labels(&[])));
        assert!(s.matches(&labels(&[("region", "anything")])));
    }

    // --- Node-to-wave assignment (first-match, implicit final) ---------------

    #[test]
    fn a_node_belongs_to_the_first_matching_wave() {
        let plan = WavePlan::new(
            vec![
                Wave {
                    name: "canary".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "canary")]),
                },
                Wave {
                    name: "eu".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "eu")]),
                },
                Wave {
                    name: "us".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "us")]),
                },
            ],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        let l_canary = labels(&[("region", "canary")]);
        let l_eu = labels(&[("region", "eu")]);
        let l_us = labels(&[("region", "us")]);
        let assigned = plan.assign([
            ("n-eu", &l_eu),
            ("n-canary", &l_canary),
            ("n-us", &l_us),
        ]);
        assert_eq!(assigned.len(), 3);
        assert_eq!(assigned[0].name, "canary");
        assert_eq!(assigned[0].node_ids, vec!["n-canary"]);
        assert_eq!(assigned[1].name, "eu");
        assert_eq!(assigned[1].node_ids, vec!["n-eu"]);
        assert_eq!(assigned[2].name, "us");
        assert_eq!(assigned[2].node_ids, vec!["n-us"]);
    }

    #[test]
    fn a_node_matching_a_later_wave_first_is_not_stolen_by_a_broader_earlier_one() {
        // wave 1 is a broad set membership; a node in it must not also appear
        // in wave 2. First-match wins.
        let plan = WavePlan::new(
            vec![
                Wave {
                    name: "early".to_string(),
                    selector: Selector::of(vec![SelectorTerm::in_set("region", &["eu", "us"])]),
                },
                Wave {
                    name: "late".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "us")]),
                },
            ],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        let l_us = labels(&[("region", "us")]);
        let assigned = plan.assign([("n-us", &l_us)]);
        assert_eq!(assigned[0].name, "early");
        assert_eq!(assigned[0].node_ids, vec!["n-us"], "first match wins");
        assert!(assigned[1].node_ids.is_empty(), "not double-counted");
    }

    #[test]
    fn a_node_matching_no_wave_lands_in_the_implicit_final_wave() {
        let plan = WavePlan::new(
            vec![Wave {
                name: "eu".to_string(),
                selector: Selector::of(vec![SelectorTerm::eq("region", "eu")]),
            }],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        let l_eu = labels(&[("region", "eu")]);
        let l_orphan = labels(&[("region", "antarctica")]);
        let assigned = plan.assign([("n-eu", &l_eu), ("n-orphan", &l_orphan)]);
        assert_eq!(assigned.len(), 2);
        assert_eq!(assigned[0].name, "eu");
        assert_eq!(assigned[1].name, IMPLICIT_FINAL_WAVE);
        assert_eq!(assigned[1].node_ids, vec!["n-orphan"]);
    }

    #[test]
    fn no_implicit_wave_appears_when_every_node_matches() {
        let plan = WavePlan::new(
            vec![Wave {
                name: "all".to_string(),
                selector: Selector::everything(),
            }],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        let l = labels(&[("region", "eu")]);
        let assigned = plan.assign([("n1", &l)]);
        assert_eq!(assigned.len(), 1, "no orphan wave when all matched");
        assert_eq!(assigned[0].name, "all");
    }

    #[test]
    fn the_single_plan_is_one_everything_wave() {
        let plan = WavePlan::single();
        assert_eq!(plan.waves.len(), 1);
        let l = labels(&[("region", "eu")]);
        let assigned = plan.assign([("n1", &l), ("n2", &l)]);
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].node_ids, vec!["n1", "n2"]);
    }

    #[test]
    fn wave_index_for_places_orphans_after_the_last_named_wave() {
        let plan = WavePlan::new(
            vec![
                Wave {
                    name: "eu".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "eu")]),
                },
                Wave {
                    name: "us".to_string(),
                    selector: Selector::of(vec![SelectorTerm::eq("region", "us")]),
                },
            ],
            UnmatchedPolicy::ImplicitFinalWave,
        );
        assert_eq!(plan.wave_index_for(&labels(&[("region", "eu")])), 0);
        assert_eq!(plan.wave_index_for(&labels(&[("region", "us")])), 1);
        assert_eq!(
            plan.wave_index_for(&labels(&[("region", "ap")])),
            2,
            "orphan is the implicit final wave index"
        );
    }

    // --- Parsing waves.yaml --------------------------------------------------

    #[test]
    fn parse_ordered_waves_with_equality_and_membership() {
        let yaml = "\
waves:
  - name: canary
    selector: { region: canary }
  - name: europe
    selector: { region: [eu-west, eu-central] }
  - name: us
    selector: { region: us }
";
        let plan = parse_wave_plan(yaml).unwrap();
        assert_eq!(plan.waves.len(), 3);
        assert_eq!(plan.waves[0].name, "canary");
        assert_eq!(
            plan.waves[0].selector.terms[0],
            SelectorTerm::eq("region", "canary")
        );
        assert_eq!(
            plan.waves[1].selector.terms[0],
            SelectorTerm::in_set("region", &["eu-west", "eu-central"])
        );
    }

    #[test]
    fn empty_or_absent_waves_file_is_the_single_plan() {
        assert_eq!(parse_wave_plan("").unwrap(), WavePlan::single());
        assert_eq!(parse_wave_plan("waves: []").unwrap(), WavePlan::single());
    }

    #[test]
    fn a_reserved_or_duplicate_wave_name_is_rejected() {
        let reserved = format!("waves:\n  - name: {IMPLICIT_FINAL_WAVE}\n    selector: {{}}\n");
        assert!(parse_wave_plan(&reserved).is_err());
        let dup = "\
waves:
  - name: eu
    selector: { region: eu }
  - name: eu
    selector: { region: us }
";
        assert!(parse_wave_plan(dup).is_err());
    }

    #[test]
    fn a_multi_term_selector_parses_as_a_conjunction() {
        let yaml = "\
waves:
  - name: eu-aws
    selector: { region: eu, cloud: aws }
";
        let plan = parse_wave_plan(yaml).unwrap();
        assert_eq!(plan.waves[0].selector.terms.len(), 2);
        // Both terms must match.
        let l_match = labels(&[("region", "eu"), ("cloud", "aws")]);
        let l_miss = labels(&[("region", "eu"), ("cloud", "gcp")]);
        assert!(plan.waves[0].selector.matches(&l_match));
        assert!(!plan.waves[0].selector.matches(&l_miss));
    }
}
