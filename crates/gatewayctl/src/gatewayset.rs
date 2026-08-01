//! GatewaySets — ApplicationSets + generators for the fleet (docs/02
//! "ApplicationSets + generators: label selectors (region, env, tenant, cloud) x
//! generators stamp config across the fleet").
//!
//! A GatewaySet is a **label selector plus a config overlay** that stamps config
//! across every node whose labels match the selector. An operator writes one
//! GatewaySet instead of per-node config; adding or removing a node with
//! matching labels picks up or drops the stamped config on the next render, with
//! no per-node files edited.
//!
//! ## Git-native, rendered-manifest, no live templating (docs/07)
//!
//! A GatewaySet lives in the config repo (`gatewaysets.yaml`) and is composed
//! into the flat render at compile time, NOT interpreted in the data plane. The
//! overlay is a YAML mapping deep-merged into the assembled scoped-chain document
//! for a matching node, then validated by the same `Config::from_yaml` gate every
//! render passes. This keeps the six-month rule mechanical: the render is a pure
//! function of `(repo bytes, node labels)`, so the same repo plus the same node
//! labels always yields the same `render_hash`. There is no templating engine at
//! runtime — the diff a reviewer sees is the diff the node runs (docs/07's whole
//! point in banning in-gateway templating).
//!
//! ## Scope precedence
//!
//! The GatewaySet overlay composes as the OUTERMOST layer over the four scoped
//! chains (`fleet → project → route → app`): it is merged onto the already-
//! assembled document, so a GatewaySet value wins over a same-keyed base default
//! (the operator stamping a fleet-wide override is the intent), while keys the
//! GatewaySet does not mention are untouched. Deep merge, not replace: a
//! GatewaySet that stamps `fleet.attribution.pinned.tier` does not wipe a
//! sibling `fleet.attribution.pinned.env`.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::waves::{Selector, SelectorTerm};

/// One GatewaySet: a name, a label selector deciding which nodes it stamps, and
/// the config overlay it stamps onto them.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewaySet {
    pub name: String,
    pub selector: Selector,
    /// The overlay, as a YAML mapping deep-merged onto matching nodes' render.
    pub overlay: serde_yaml::Mapping,
}

impl GatewaySet {
    /// Whether this GatewaySet stamps a node with the given labels.
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.selector.matches(labels)
    }
}

/// The set of GatewaySets defined in the config repo, in a deterministic order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GatewaySets {
    /// Sorted by name so overlay application order is deterministic — two
    /// GatewaySets touching the same key resolve last-writer-wins in NAME order,
    /// stated and reproducible rather than incidental.
    pub sets: Vec<GatewaySet>,
}

impl GatewaySets {
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The GatewaySets that match a node's labels, in deterministic (name) order.
    pub fn matching(&self, labels: &BTreeMap<String, String>) -> Vec<&GatewaySet> {
        self.sets.iter().filter(|g| g.matches(labels)).collect()
    }

    /// Deep-merge every matching GatewaySet's overlay onto `doc`, in name order.
    /// Returns the number of GatewaySets stamped (for logging / determinism
    /// checks). The mutation is on the assembled flat document BEFORE validation.
    pub fn stamp(&self, doc: &mut serde_yaml::Mapping, labels: &BTreeMap<String, String>) -> usize {
        let matching = self.matching(labels);
        for g in &matching {
            deep_merge_mapping(doc, &g.overlay);
        }
        matching.len()
    }
}

/// Deep-merge `overlay` into `base`: for each key, if both sides hold a mapping,
/// recurse; otherwise the overlay value REPLACES the base value. This is the
/// stamping semantics — a GatewaySet wins on the keys it names, leaves the rest
/// alone, and never silently drops sibling keys.
pub fn deep_merge_mapping(base: &mut serde_yaml::Mapping, overlay: &serde_yaml::Mapping) {
    for (k, ov) in overlay {
        match (base.get_mut(k), ov) {
            (Some(serde_yaml::Value::Mapping(base_child)), serde_yaml::Value::Mapping(ov_child)) => {
                deep_merge_mapping(base_child, ov_child);
            }
            _ => {
                base.insert(k.clone(), ov.clone());
            }
        }
    }
}

// --- Parsing gatewaysets.yaml ----------------------------------------------

/// The on-disk shape of `gatewaysets.yaml`: a list of
/// `{ name, selector: { label: value | [values] }, overlay: { … } }`.
#[derive(Debug, Deserialize)]
struct GatewaySetsFile {
    #[serde(default)]
    gatewaysets: Vec<GatewaySetEntry>,
}

#[derive(Debug, Deserialize)]
struct GatewaySetEntry {
    name: String,
    #[serde(default)]
    selector: BTreeMap<String, SelectorValue>,
    #[serde(default)]
    overlay: serde_yaml::Mapping,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SelectorValue {
    One(String),
    Many(Vec<String>),
}

/// A malformed `gatewaysets.yaml`.
#[derive(Debug)]
pub struct GatewaySetError(pub String);

impl std::fmt::Display for GatewaySetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid gatewaysets.yaml: {}", self.0)
    }
}

impl std::error::Error for GatewaySetError {}

/// Parse a `gatewaysets.yaml` document into [`GatewaySets`]. Absent or empty →
/// no GatewaySets (every node renders from its scoped chains alone, unchanged).
/// The parsed sets are sorted by name for deterministic overlay ordering.
pub fn parse_gatewaysets(yaml: &str) -> Result<GatewaySets, GatewaySetError> {
    if yaml.trim().is_empty() {
        return Ok(GatewaySets::default());
    }
    let parsed: GatewaySetsFile =
        serde_yaml::from_str(yaml).map_err(|e| GatewaySetError(e.to_string()))?;
    let mut sets = Vec::new();
    for entry in parsed.gatewaysets {
        if entry.name.trim().is_empty() {
            return Err(GatewaySetError("a gatewayset has an empty name".to_string()));
        }
        let mut terms = Vec::new();
        for (label, val) in entry.selector {
            let in_values = match val {
                SelectorValue::One(v) => vec![v],
                SelectorValue::Many(vs) => {
                    if vs.is_empty() {
                        return Err(GatewaySetError(format!(
                            "gatewayset {:?} selector {label:?} has an empty value list",
                            entry.name
                        )));
                    }
                    vs
                }
            };
            terms.push(SelectorTerm { label, in_values });
        }
        sets.push(GatewaySet {
            name: entry.name,
            selector: Selector::of(terms),
            overlay: entry.overlay,
        });
    }
    // Deterministic order + duplicate-name rejection.
    sets.sort_by(|a, b| a.name.cmp(&b.name));
    let mut seen = std::collections::BTreeSet::new();
    for s in &sets {
        if !seen.insert(s.name.clone()) {
            return Err(GatewaySetError(format!("duplicate gatewayset name {:?}", s.name)));
        }
    }
    Ok(GatewaySets { sets })
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

    fn mapping(yaml: &str) -> serde_yaml::Mapping {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn deep_merge_wins_on_named_keys_and_keeps_siblings() {
        let mut base = mapping("fleet:\n  attribution:\n    pinned:\n      env: prod\n");
        let overlay = mapping("fleet:\n  attribution:\n    pinned:\n      tier: gold\n");
        deep_merge_mapping(&mut base, &overlay);
        let merged = serde_yaml::to_string(&serde_yaml::Value::Mapping(base)).unwrap();
        assert!(merged.contains("env: prod"), "sibling preserved: {merged}");
        assert!(merged.contains("tier: gold"), "overlay stamped: {merged}");
    }

    #[test]
    fn overlay_value_replaces_a_same_keyed_base_scalar() {
        let mut base = mapping("fleet:\n  attribution:\n    pinned:\n      env: prod\n");
        let overlay = mapping("fleet:\n  attribution:\n    pinned:\n      env: canary\n");
        deep_merge_mapping(&mut base, &overlay);
        let merged = serde_yaml::to_string(&serde_yaml::Value::Mapping(base)).unwrap();
        assert!(merged.contains("env: canary"), "overlay wins: {merged}");
        assert!(!merged.contains("env: prod"));
    }

    #[test]
    fn matching_selects_only_nodes_whose_labels_match() {
        let sets = parse_gatewaysets(
            "\
gatewaysets:
  - name: eu-tier
    selector: { region: eu }
    overlay:
      fleet:
        attribution:
          pinned: { tier: gold }
",
        )
        .unwrap();
        assert_eq!(sets.matching(&labels(&[("region", "eu")])).len(), 1);
        assert_eq!(sets.matching(&labels(&[("region", "us")])).len(), 0);
    }

    #[test]
    fn stamp_applies_matching_overlays_and_counts_them() {
        let sets = parse_gatewaysets(
            "\
gatewaysets:
  - name: eu-tier
    selector: { region: eu }
    overlay:
      fleet: { attribution: { pinned: { tier: gold } } }
",
        )
        .unwrap();
        let mut doc = mapping("fleet:\n  attribution:\n    pinned:\n      env: prod\n");
        let stamped = sets.stamp(&mut doc, &labels(&[("region", "eu")]));
        assert_eq!(stamped, 1);
        let out = serde_yaml::to_string(&serde_yaml::Value::Mapping(doc.clone())).unwrap();
        assert!(out.contains("tier: gold") && out.contains("env: prod"));

        // A non-matching node is untouched.
        let mut doc2 = mapping("fleet:\n  attribution:\n    pinned:\n      env: prod\n");
        assert_eq!(sets.stamp(&mut doc2, &labels(&[("region", "us")])), 0);
        assert_eq!(doc2, mapping("fleet:\n  attribution:\n    pinned:\n      env: prod\n"));
    }

    #[test]
    fn parsing_sorts_by_name_for_deterministic_overlay_order() {
        let sets = parse_gatewaysets(
            "\
gatewaysets:
  - name: z-set
    selector: {}
    overlay: {}
  - name: a-set
    selector: {}
    overlay: {}
",
        )
        .unwrap();
        assert_eq!(sets.sets[0].name, "a-set");
        assert_eq!(sets.sets[1].name, "z-set");
    }

    #[test]
    fn empty_or_absent_file_yields_no_sets() {
        assert!(parse_gatewaysets("").unwrap().is_empty());
        assert!(parse_gatewaysets("gatewaysets: []").unwrap().is_empty());
    }

    #[test]
    fn a_duplicate_gatewayset_name_is_rejected() {
        let dup = "\
gatewaysets:
  - name: dup
    selector: { region: eu }
    overlay: {}
  - name: dup
    selector: { region: us }
    overlay: {}
";
        assert!(parse_gatewaysets(dup).is_err());
    }

    #[test]
    fn an_empty_selector_gatewayset_stamps_every_node() {
        // A GatewaySet with no selector terms is fleet-wide (matches everything).
        let sets = parse_gatewaysets(
            "\
gatewaysets:
  - name: global
    selector: {}
    overlay:
      fleet: { attribution: { pinned: { global_flag: on } } }
",
        )
        .unwrap();
        assert_eq!(sets.matching(&labels(&[("region", "anything")])).len(), 1);
        assert_eq!(sets.matching(&labels(&[])).len(), 1);
    }
}
