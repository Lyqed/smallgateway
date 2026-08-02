//! Streaming-tap enforcement helpers pulled out of `proxy.rs`'s
//! `response_body_filter` (kept there: the pingora hook plumbing). These are
//! free functions over explicit params — no `ReqCtx` coupling — so the proxy
//! module stays focused and under the file-size budget.

use bytes::Bytes;
use log::{error, info};

use gateway_core::attribution::Tag;
use gateway_core::budget::{CapId, CapTerms, Verdict};
use gateway_core::config::StreamingRejection;
use gateway_core::metering::MeterReport;
use gateway_core::scope::EffectivePolicy;

use crate::budget::{MeterOutcome, NodeBudgets};

/// A GB-5 admission denial: the spender that was already at its cap, for the
/// hook's GB-4 rejection (which owns the async response).
pub(crate) struct CapDenial {
    pub id: CapId,
    pub cap: u64,
    pub spent: u64,
}

/// GB-5 request admission: for each resolved tag this policy caps, admit
/// against the node-local budget BEFORE the upstream is reached. Returns the
/// admitted caps this request bills, or `Err(CapDenial)` for the FIRST spender
/// already at its cap (the hook rejects it with the GB-4 template). Extracted
/// from the request hook so it stays focused; the common path is one in-memory
/// check per capped tag, no control-plane hop.
pub(crate) fn admit_caps(
    budgets: &NodeBudgets,
    policy: &EffectivePolicy,
    tags: &[Tag],
    route_prefix: &str,
    cfg_version: u64,
    now_unix: u64,
) -> Result<Vec<(CapId, CapTerms)>, CapDenial> {
    let mut caps = Vec::new();
    for tag in tags {
        let Some(terms) = policy.terms_for(&tag.key, &tag.value) else {
            continue;
        };
        let id = CapId::new(&tag.key, &tag.value);
        match budgets.admit(&id, Some(&terms), now_unix) {
            Verdict::Deny { cap } => {
                let spent = budgets.snapshot(&id).map(|(_, _, s)| s).unwrap_or(cap);
                return Err(CapDenial { id, cap, spent });
            }
            Verdict::Escalate => {
                info!(
                    "[gb5 {route_prefix}] {id} at/above ~90% of local share; will escalate \
                     cfg=v{cfg_version}"
                );
                caps.push((id, terms));
            }
            Verdict::Allow => caps.push((id, terms)),
        }
    }
    Ok(caps)
}

/// The end-of-stream `[meter]` line: the attribution→spend join on ONE line, so
/// "who spent what" is a grep. `cfg=vN` names the version that metered THIS
/// stream — the bounded-staleness evidence during a drain overlap. Extracted
/// from the response tap to keep it focused.
#[allow(clippy::too_many_arguments)]
pub(crate) fn log_meter_report(
    route_prefix: &str,
    cfg_version: u64,
    provider: &str,
    provider_kind: &str,
    tag_summary: &str,
    event_summary: &str,
    chunks: usize,
    bytes: usize,
    report: &MeterReport,
) {
    let err = report
        .error_pct
        .map(|p| format!("{p:+.1}%"))
        .unwrap_or_else(|| "n/a".to_string());
    info!(
        "[meter {route_prefix}] cfg=v{cfg_version} provider={provider}({provider_kind}) \
         attribution{{{tag_summary}}} events{{{event_summary}}} chunks={chunks} bytes={bytes} \
         est_output_tokens={} auth_input_tokens={} auth_output_tokens={} est_err={err}",
        report.estimated_output_tokens,
        crate::proxy_support::opt(report.authoritative_input_tokens),
        crate::proxy_support::opt(report.authoritative_output_tokens),
    );
}

/// GB-5 mid-stream charge + cut. Charges `delta` estimated output tokens
/// against every capped spender in `caps` and returns the operator's GB-4
/// terminal-event bytes for the FIRST spender that crosses its bound (else
/// `None`). A cap tightened mid-stream does NOT retroactively apply — the
/// caller passes the caps the request BOUND (docs/03 limitation 2).
///
/// `streaming` is the bound snapshot's `missing_attribution.streaming`
/// template; `cfg_version` and `route_prefix` are for the cut log line.
#[allow(clippy::too_many_arguments)]
pub(crate) fn charge_caps_and_cut(
    budgets: &NodeBudgets,
    caps: &[(CapId, CapTerms)],
    delta: u64,
    streaming: Option<&StreamingRejection>,
    route_prefix: &str,
    cfg_version: u64,
    now_unix: u64,
) -> Option<Bytes> {
    if delta == 0 {
        return None;
    }
    for (id, terms) in caps {
        if let MeterOutcome::Cut { id, cap } = budgets.meter(id, Some(terms), delta, now_unix) {
            let spent = budgets.snapshot(&id).map(|(_, _, s)| s).unwrap_or(cap);
            error!(
                "[gb5 {route_prefix}] {id} EXCEEDED mid-stream: spent {spent}/{cap} tokens; \
                 CUTTING the stream with the GB-4 terminal event cfg=v{cfg_version}"
            );
            return Some(crate::proxy_support::render_cut_event(
                streaming,
                &id,
                cap,
                spent,
                route_prefix,
            ));
        }
    }
    None
}
