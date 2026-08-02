//! The data plane's GB-5 budget bridge: the node-local counters the proxy taps
//! for enforcement, shared with the control-plane client that reports spend and
//! applies share grants (docs/01 Q4; docs/04 Phase 3).
//!
//! The proxy runs on pingora's threads; the control-plane client loop runs on
//! its own tokio runtime. They share one `Arc<NodeBudgets>` so the enforcement
//! decision and the telemetry/rebalance are the same counters. Everything here
//! is in-memory for this milestone (durable counters are deferred; docs/03
//! limitation 3).
//!
//! ## Enforcement (the proxy's view)
//!
//! Per request, once attribution resolves, the proxy calls [`NodeBudgets::admit`]
//! with each capped `key=value` and its composed cap. That returns a
//! [`gateway_core::budget::Verdict`]: `Allow` to proceed, `Escalate` to keep
//! serving but flag the near-limit path, or `Deny` to reject with the GB-4
//! template. During the stream the proxy calls [`NodeBudgets::meter`] as the
//! Meter tallies tokens; when the running tally would cross the bound it returns
//! a cut signal and the proxy terminates the stream with the GB-4 terminal
//! event. At stream end [`NodeBudgets::settle`] reconciles the live estimate to
//! the authoritative usage frame.
//!
//! ## Partition (bounded overspend)
//!
//! [`NodeBudgets::set_partitioned`] flips the node into partition mode: the
//! admit/meter decisions use [`LocalBudget::check_partitioned`], so the node
//! spends only up to its currently-held share and then hard-denies — the
//! documented, MEASURABLE bounded-overspend policy. [`NodeBudgets::overspend`]
//! reports the number.

use std::collections::BTreeMap;
use std::sync::Mutex;

use gateway_core::budget::{
    Alert, AlertLatch, AlertSink, CapId, CapTerms, LocalBudget, Verdict, Window,
    ESCALATION_FRACTION,
};

/// One capped spender's node-local state: the counter, its GB-6 alert latch.
struct Entry {
    budget: LocalBudget,
    latch: AlertLatch,
    /// The billing window this cap rolls on, from the last-seen config
    /// terms (`None` → lifetime, never resets).
    window: Option<Window>,
    /// The window id the counters currently belong to (0 for lifetime).
    window_id: u64,
    /// The GB-6 soft-alert fraction from the last-seen terms.
    alert_fraction: f64,
}

impl Entry {
    /// Lazy rollover: if the wall clock left the window the counters belong
    /// to, the spend starts over and the alert latch re-arms. Lazy (checked
    /// on every touch) instead of scheduled: it needs no timers, survives
    /// restarts, and every node computes the same UTC-aligned window id
    /// from its own clock — residual error is bounded by clock skew.
    fn roll(&mut self, now_unix: u64) {
        let Some(window) = self.window else { return };
        let id = window.id_at(now_unix);
        if id != self.window_id {
            self.budget.reset_window_spend();
            self.latch = AlertLatch::new();
            self.window_id = id;
        }
    }

    /// Adopt the request's composed terms (config may have hot-swapped), then
    /// roll. A CHANGED window kind resets the counters: yesterday's spend has
    /// no meaning against a new window shape.
    fn sync_terms(&mut self, terms: Option<&CapTerms>, now_unix: u64) {
        if let Some(t) = terms {
            self.alert_fraction = t.alert_fraction;
            if self.window != t.window {
                self.window = t.window;
                self.window_id = t.window.map(|w| w.id_at(now_unix)).unwrap_or(0);
                self.budget.reset_window_spend();
                self.latch = AlertLatch::new();
                return;
            }
        }
        self.roll(now_unix);
    }
}

/// The node's whole GB-5 state: a [`LocalBudget`] + alert latch per capped
/// `CapId`, the partition flag, the node id (for alert context), and the alert
/// sink alerts fire into AT the enforcement point.
pub struct NodeBudgets {
    node_id: String,
    inner: Mutex<BTreeMap<CapId, Entry>>,
    partitioned: std::sync::atomic::AtomicBool,
    alerts: Box<dyn AlertSink>,
}

/// What the proxy should do about a running stream after metering more tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeterOutcome {
    /// Keep streaming.
    Continue,
    /// The bound is crossed: cut the stream with the GB-4 terminal event. Carries
    /// the cap for the operator template.
    Cut { id: CapId, cap: u64 },
}

impl NodeBudgets {
    pub fn new(node_id: impl Into<String>, alerts: Box<dyn AlertSink>) -> NodeBudgets {
        NodeBudgets {
            node_id: node_id.into(),
            inner: Mutex::new(BTreeMap::new()),
            partitioned: std::sync::atomic::AtomicBool::new(false),
            alerts,
        }
    }

    /// Flip partition mode on/off (the control-plane client sets it on a stream
    /// loss and clears it on reconnect). While partitioned the admit/meter
    /// decisions cannot escalate and enforce bounded overspend.
    pub fn set_partitioned(&self, on: bool) {
        self.partitioned
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_partitioned(&self) -> bool {
        self.partitioned.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Ensure a budget exists for `id` with fleet cap `cap`. On first sight the
    /// node holds the WHOLE cap as its share (single-node-safe: it can enforce
    /// alone until the control plane rebalances it a smaller slice). A later
    /// grant shrinks the share; the spend is preserved.
    fn ensure<'a>(
        inner: &'a mut BTreeMap<CapId, Entry>,
        id: &CapId,
        cap: Option<u64>,
    ) -> &'a mut Entry {
        inner.entry(id.clone()).or_insert_with(|| Entry {
            // Initial share = the whole cap: correct for a single node, and a
            // safe ceiling until the control plane grants a fleet-aware slice.
            budget: LocalBudget::new(id.clone(), cap, cap.unwrap_or(0)),
            latch: AlertLatch::new(),
            window: None,
            window_id: 0,
            alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION,
        })
    }

    /// Admit (or refuse) a request for a capped `key=value` BEFORE it reaches
    /// the upstream. `terms` is the composed cap+window+alert for this value
    /// from the request's policy (`None` → uncapped, always allowed). Returns
    /// the verdict; the proxy rejects with the GB-4 template on `Deny`. Does
    /// not record spend — that happens as the stream meters.
    pub fn admit(&self, id: &CapId, terms: Option<&CapTerms>, now_unix: u64) -> Verdict {
        let mut inner = self.inner.lock().expect("budget lock");
        let entry = Self::ensure(&mut inner, id, terms.map(|t| t.cap));
        entry.sync_terms(terms, now_unix);
        // Probe with one token: a request that reaches the upstream will spend
        // at least something, so a value already AT its cap (no room for even
        // one token) denies. Below the cap the probe reports Allow/Escalate
        // exactly as the next real spend will.
        if self.is_partitioned() {
            entry.budget.check_partitioned(1)
        } else {
            entry.budget.check(1)
        }
    }

    /// Record `tokens` of live-metered spend for a capped value mid-stream and
    /// decide whether the stream must be cut. Fires GB-6 alerts (soft/hard) at
    /// this enforcement point, at the operator's alert_at threshold. Returns
    /// [`MeterOutcome::Cut`] when the running tally crosses the bound (the
    /// cap, or — partitioned — the held share).
    pub fn meter(
        &self,
        id: &CapId,
        terms: Option<&CapTerms>,
        tokens: u64,
        now_unix: u64,
    ) -> MeterOutcome {
        let mut inner = self.inner.lock().expect("budget lock");
        let entry = Self::ensure(&mut inner, id, terms.map(|t| t.cap));
        entry.sync_terms(terms, now_unix);
        entry.budget.commit(tokens);

        // GB-6: fire soft/hard alerts from the enforcement layer itself.
        if let Some(t) = terms {
            let fraction = entry.alert_fraction;
            for alert in entry
                .latch
                .cross(id, t.cap, entry.budget.spent(), &self.node_id, fraction)
            {
                self.alerts.emit(&alert);
            }
        }

        // The bound: the cap when connected, the held share when partitioned
        // (bounded overspend — a partitioned node cannot grow its share).
        let Some(cap) = terms.map(|t| t.cap) else {
            return MeterOutcome::Continue; // uncapped
        };
        let bound = if self.is_partitioned() {
            entry.budget.share().min(cap)
        } else {
            cap
        };
        if entry.budget.spent() > bound {
            MeterOutcome::Cut {
                id: id.clone(),
                cap,
            }
        } else {
            MeterOutcome::Continue
        }
    }

    /// Whether a capped value is at/above the ~90% escalation band of its local
    /// share, so the proxy/client should escalate to a synchronous check. Reads
    /// the current spend; safe to call after `meter`. (The client loop escalates
    /// via [`NodeBudgets::escalating`] over all values; this single-value probe
    /// is the enforcement-point counterpart, exercised by the tests.)
    #[allow(dead_code)]
    pub fn should_escalate(&self, id: &CapId) -> bool {
        let inner = self.inner.lock().expect("budget lock");
        match inner.get(id) {
            Some(e) => e.budget.consumed_fraction() >= ESCALATION_FRACTION,
            None => false,
        }
    }

    /// Reconcile a stream's live estimate to the provider's authoritative
    /// terminal usage frame at stream end (docs/01 Q3). No-op when they agree.
    /// Rolls the window first: a stream that started in window N and settles
    /// in N+1 reconciles into the NEW window's counter (saturating), a
    /// boundary fuzz bounded by one stream's tokens — the same honesty class
    /// as clock skew.
    pub fn settle(&self, id: &CapId, estimated: u64, authoritative: u64, now_unix: u64) {
        let mut inner = self.inner.lock().expect("budget lock");
        if let Some(e) = inner.get_mut(id) {
            e.roll(now_unix);
            e.budget.reconcile(estimated, authoritative);
        }
    }

    /// GB-5 end-of-stream settlement: reconcile each capped spender's live
    /// estimate for this stream to the provider's authoritative terminal frame
    /// (docs/01 Q3) when one landed, then log its post-reconcile state. When no
    /// usage frame arrived the estimate stands as the charge (its error bound is
    /// the published Q3 number). `caps` is the request's `(CapId, cap)` list.
    #[allow(clippy::too_many_arguments)] // one call site; a params struct adds noise
    pub fn settle_and_log(
        &self,
        caps: &[(CapId, CapTerms)],
        estimated: u64,
        authoritative: Option<u64>,
        route: &str,
        version: u64,
        cut: bool,
        now_unix: u64,
    ) {
        let cut_note = if cut { " [CUT]" } else { "" };
        for (id, terms) in caps {
            let cap = terms.cap;
            if let Some(auth) = authoritative {
                self.settle(id, estimated, auth, now_unix);
                if let Some((_, share, spent)) = self.snapshot(id) {
                    log::info!(
                        "[budget {route}] {id} reconciled est={estimated}->auth={auth}; \
                         spent={spent}/{cap} tokens (share={share}){cut_note} cfg=v{version}"
                    );
                }
            } else if let Some((_, share, spent)) = self.snapshot(id) {
                log::info!(
                    "[budget {route}] {id} no usage frame; spent={spent}/{cap} tokens \
                     (share={share}, estimate stands){cut_note} cfg=v{version}"
                );
            }
        }
    }

    /// Non-streaming end-of-stream settlement: a JSON response was metered as
    /// ONE terminal message (docs/11 D1), so no per-chunk `meter` ran during
    /// the body. Charge the authoritative total through the SAME `meter`
    /// enforcement path a stream uses — so GB-6 alerts fire, the cut bound is
    /// checked, and partition is honored — then log. `charged` is the token
    /// count to bill (authoritative input+output; the billable unit for
    /// embeddings is input, which output-only settlement dropped). Returns the
    /// per-cap `MeterOutcome`, so the caller can note a would-cut on a
    /// non-streaming response (the body already completed; there is nothing
    /// left to cut, but the crossing is real and logged).
    ///
    /// Unlike `settle_and_log`, this does NOT call `reconcile`: `meter`
    /// already committed the exact authoritative number (the estimate was 0
    /// for a JSON body — nothing to reconcile against).
    pub fn charge_terminal_and_log(
        &self,
        caps: &[(CapId, CapTerms)],
        charged: u64,
        route: &str,
        version: u64,
        now_unix: u64,
    ) -> Vec<MeterOutcome> {
        let mut outcomes = Vec::with_capacity(caps.len());
        for (id, terms) in caps {
            let cap = terms.cap;
            let outcome = self.meter(id, Some(terms), charged, now_unix);
            let crossed = if matches!(outcome, MeterOutcome::Cut { .. }) {
                " [OVER CAP]"
            } else {
                ""
            };
            if let Some((_, share, spent)) = self.snapshot(id) {
                log::info!(
                    "[budget {route}] {id} non-streaming charge={charged}; \
                     spent={spent}/{cap} tokens (share={share}){crossed} cfg=v{version}"
                );
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    /// Apply a control-plane `ShareGrant`: (re)set each named budget's share to
    /// the granted tokens without losing its running spend. A grant for a value
    /// not yet seen creates the budget at the granted share.
    pub fn apply_shares(&self, grants: &[(CapId, u64, u64)]) {
        let mut inner = self.inner.lock().expect("budget lock");
        for (id, cap, share) in grants {
            let entry = Self::ensure(&mut inner, id, Some(*cap));
            entry.budget.set_share(*share);
        }
    }

    /// The observed spend to report up the stream: `(id, cap, spent)` for
    /// every capped value with nonzero spend in the CURRENT window (entries
    /// roll lazily first). The control plane rebalances shares from this
    /// telemetry; because its ledger OVERWRITES per-node spend, a node's
    /// post-rollover (smaller) report propagates the new window fleet-wide
    /// with no extra protocol.
    pub fn spend_report(&self, now_unix: u64) -> Vec<(CapId, u64, u64)> {
        let mut inner = self.inner.lock().expect("budget lock");
        inner
            .iter_mut()
            .filter_map(|(id, e)| {
                e.roll(now_unix);
                let cap = e.budget.cap()?;
                if e.budget.spent() == 0 {
                    return None;
                }
                Some((id.clone(), cap, e.budget.spent()))
            })
            .collect()
    }

    /// The set of capped values at/above the escalation band — the spenders a
    /// synchronous `SyncCheck` should carry. Rolls lazily first, so a spender
    /// that crossed into a new window stops escalating.
    pub fn escalating(&self, now_unix: u64) -> Vec<(CapId, u64, u64)> {
        let mut inner = self.inner.lock().expect("budget lock");
        inner
            .iter_mut()
            .filter_map(|(id, e)| {
                e.roll(now_unix);
                let cap = e.budget.cap()?;
                if e.budget.consumed_fraction() >= ESCALATION_FRACTION {
                    Some((id.clone(), cap, e.budget.spent()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The MEASURED overspend for one capped value: tokens spent beyond the
    /// node's held share (the partition bound). Zero on the common path. This is
    /// the number the partition demo/test reports against the configured cap.
    #[allow(dead_code)] // exercised by the partition test + budget-demo.sh
    pub fn overspend(&self, id: &CapId) -> u64 {
        self.inner
            .lock()
            .expect("budget lock")
            .get(id)
            .map(|e| e.budget.overspend())
            .unwrap_or(0)
    }

    /// One value's `(cap, share, spent)` — for logging and tests.
    pub fn snapshot(&self, id: &CapId) -> Option<(Option<u64>, u64, u64)> {
        self.inner
            .lock()
            .expect("budget lock")
            .get(id)
            .map(|e| (e.budget.cap(), e.budget.share(), e.budget.spent()))
    }
}

/// The production data-plane alert sink: a structured `log::warn!` line at the
/// enforcement point (so it cannot be missed) plus a webhook-shaped body a
/// deployment can route. The webhook body is logged too rather than POSTed —
/// wiring a real HTTP delivery is a small, deliberately-deferred edge.
#[derive(Default)]
pub struct LogWebhookSink;

impl AlertSink for LogWebhookSink {
    fn emit(&self, alert: &Alert) {
        log::warn!("{alert}");
        let body = gateway_core::budget::WebhookAlertSink::<fn(String)>::body(alert);
        log::info!("[gb6] webhook body (no webhook configured): {body}");
    }
}

/// The production sink: the guaranteed structured log line at the
/// enforcement point, PLUS — once `configure` is called with the config's
/// `alerts.webhook` — a fire-and-forget POST of the documented JSON body
/// to the fleet owner's chosen receiver. Delivery is best-effort with a
/// short timeout on its own thread (alerts are rare; enforcement never
/// waits on a pager).
#[derive(Default)]
pub struct ConfigurableSink {
    webhook: std::sync::OnceLock<gateway_core::config::WebhookTarget>,
}

impl ConfigurableSink {
    pub fn new() -> std::sync::Arc<ConfigurableSink> {
        std::sync::Arc::new(ConfigurableSink::default())
    }

    /// Set the webhook target (once, after the bootstrap config loads).
    pub fn configure(&self, target: gateway_core::config::WebhookTarget) {
        let addr = format!("{}:{}", target.endpoint.host, target.endpoint.port);
        if self.webhook.set(target).is_ok() {
            log::info!("[gb6] alert webhook configured: {addr}");
        }
    }
}

impl AlertSink for ConfigurableSink {
    fn emit(&self, alert: &Alert) {
        log::warn!("{alert}");
        let body = gateway_core::budget::WebhookAlertSink::<fn(String)>::body(alert);
        match self.webhook.get() {
            None => log::info!("[gb6] webhook body (no webhook configured): {body}"),
            Some(target) => {
                let target = target.clone();
                std::thread::spawn(move || {
                    if let Err(e) = post_alert_blocking(&target, &body) {
                        log::error!("[gb6] alert webhook delivery failed: {e}");
                    }
                });
            }
        }
    }
}

/// A minimal blocking POST for alert delivery: rare, small, and off the
/// enforcement path on its own thread. 5s whole-call timeout.
fn post_alert_blocking(
    target: &gateway_core::config::WebhookTarget,
    body: &str,
) -> Result<(), String> {
    use std::io::{Read, Write};
    let addr = format!("{}:{}", target.endpoint.host, target.endpoint.port);
    let timeout = std::time::Duration::from_secs(5);
    let stream = std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .or_else(|_| {
                use std::net::ToSocketAddrs;
                addr.to_socket_addrs()
                    .map_err(|e| format!("resolve {addr}: {e}"))?
                    .next()
                    .ok_or_else(|| format!("resolve {addr}: no address"))
            })
            .map_err(|e: String| e)?,
        timeout,
    )
    .map_err(|e| format!("connect {addr}: {e}"))?;
    stream.set_write_timeout(Some(timeout)).ok();
    stream.set_read_timeout(Some(timeout)).ok();
    let mut stream = stream;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        target.path,
        body.len(),
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|e| format!("read: {e}"))?;
    let head = String::from_utf8_lossy(&response);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if (200..300).contains(&status) {
        log::info!("[gb6] alert delivered to webhook ({status})");
        Ok(())
    } else {
        Err(format!("webhook returned {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::budget::AlertKind;
    use std::sync::Arc;

    fn id() -> CapId {
        CapId::new("team", "ml-research")
    }

    /// A test alert sink capturing every alert the enforcement layer raised.
    #[derive(Default)]
    struct CaptureSink(Mutex<Vec<Alert>>);
    impl CaptureSink {
        fn alerts(&self) -> Vec<Alert> {
            self.0.lock().unwrap().clone()
        }
    }
    impl AlertSink for CaptureSink {
        fn emit(&self, alert: &Alert) {
            self.0.lock().unwrap().push(alert.clone());
        }
    }

    fn budgets() -> (Arc<NodeBudgets>, Arc<CaptureSink>) {
        // Share the sink so a test can read the alerts it captured. NodeBudgets
        // takes a Box<dyn AlertSink>; wrap the Arc in a thin forwarding sink.
        let sink = Arc::new(CaptureSink::default());
        let fwd = ForwardSink(sink.clone());
        (Arc::new(NodeBudgets::new("n1", Box::new(fwd))), sink)
    }

    struct ForwardSink(Arc<CaptureSink>);
    impl AlertSink for ForwardSink {
        fn emit(&self, alert: &Alert) {
            self.0.emit(alert);
        }
    }

    #[test]
    fn admit_allows_under_cap_and_denies_when_exhausted() {
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        assert_eq!(b.admit(&id(), cap, 0), Verdict::Allow);
        // Spend it to the cap via metering.
        assert_eq!(b.meter(&id(), cap, 100_000, 0), MeterOutcome::Continue);
        // A new request for the same value is now denied at admission.
        assert_eq!(b.admit(&id(), cap, 0), Verdict::Deny { cap: 100_000 });
    }

    #[test]
    fn meter_cuts_the_stream_when_the_running_tally_crosses_the_cap() {
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        assert_eq!(b.meter(&id(), cap, 90_000, 0), MeterOutcome::Continue);
        // The next chunk crosses the cap -> cut.
        match b.meter(&id(), cap, 20_000, 0) {
            MeterOutcome::Cut { id: cid, cap: c } => {
                assert_eq!(cid, id());
                assert_eq!(c, 100_000);
            }
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn alerts_fire_from_the_meter_at_soft_then_hard() {
        let (b, sink) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        b.meter(&id(), cap, 79_000, 0); // below soft
        assert!(sink.alerts().is_empty());
        b.meter(&id(), cap, 5_000, 0); // crosses 80% (84k)
        assert_eq!(sink.alerts().len(), 1);
        assert!(matches!(sink.alerts()[0].kind, AlertKind::SoftThreshold { .. }));
        b.meter(&id(), cap, 20_000, 0); // crosses cap (104k)
        assert!(sink.alerts().iter().any(|a| a.kind == AlertKind::HardCap));
    }

    #[test]
    fn uncapped_value_never_cuts_or_alerts() {
        let (b, sink) = budgets();
        assert_eq!(b.admit(&id(), None, 0), Verdict::Allow);
        assert_eq!(b.meter(&id(), None, 10_000_000, 0), MeterOutcome::Continue);
        assert!(sink.alerts().is_empty());
    }

    #[test]
    fn a_share_grant_shrinks_the_share_without_losing_spend() {
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        b.meter(&id(), cap, 30_000, 0);
        // The control plane grants this node a 40k slice (it is not the only
        // node). Spend is preserved; escalation now measures against 40k.
        b.apply_shares(&[(id(), 100_000, 40_000)]);
        let (c, share, spent) = b.snapshot(&id()).unwrap();
        assert_eq!(c, Some(100_000));
        assert_eq!(share, 40_000);
        assert_eq!(spent, 30_000);
        assert!(!b.should_escalate(&id())); // 30k of 40k = 75% < 90%
        b.meter(&id(), cap, 8_000, 0); // 38k of 40k = 95%
        assert!(b.should_escalate(&id()));
    }

    // --- Bounded overspend under partition -----------------------------------

    #[test]
    fn under_partition_a_stream_is_cut_at_the_held_share_and_overspend_is_measured() {
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        // The control plane granted a 40k slice, then the stream partitions.
        b.apply_shares(&[(id(), 100_000, 40_000)]);
        b.set_partitioned(true);
        // A new stream is admitted (room under the 40k share)...
        assert_eq!(b.admit(&id(), cap, 0), Verdict::Allow);
        // ...it meters up to and just past the share before the cut fires.
        assert_eq!(b.meter(&id(), cap, 39_500, 0), MeterOutcome::Continue);
        // The next chunk pushes past the 40k share -> cut (bound is the share,
        // not the 100k cap, because the node cannot reach the control plane).
        match b.meter(&id(), cap, 1_200, 0) {
            MeterOutcome::Cut { cap: c, .. } => assert_eq!(c, 100_000),
            other => panic!("expected Cut at the share, got {other:?}"),
        }
        // The MEASURED overspend past the 40k share is the running stream's tail.
        assert_eq!(b.overspend(&id()), 700, "40_700 spent, 40_000 share");
        // And no new stream can start under partition.
        assert_eq!(b.admit(&id(), cap, 0), Verdict::Deny { cap: 100_000 });
    }

    #[test]
    fn terminal_charge_fires_gb6_alerts_like_the_meter_path() {
        // docs/11 D1: a non-streaming JSON body charges its whole
        // authoritative total through the enforcement path, so GB-6 alerts
        // fire on the crossing exactly as they do for a stream — the bypass
        // the review found is closed.
        let (b, sink) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let caps = vec![(id(), terms)];
        // First non-streaming request bills 84k tokens: crosses the 80% soft
        // threshold in one shot.
        b.charge_terminal_and_log(&caps, 84_000, "r", 1, 0);
        assert_eq!(sink.alerts().len(), 1);
        assert!(matches!(sink.alerts()[0].kind, AlertKind::SoftThreshold { .. }));
        // A second non-streaming request pushes past the hard cap: HardCap
        // alert AND the outcome reports the crossing (nothing to cut on an
        // already-complete body, but the ledger and alert are correct).
        let outcomes = b.charge_terminal_and_log(&caps, 20_000, "r", 1, 0);
        assert!(sink.alerts().iter().any(|a| a.kind == AlertKind::HardCap));
        assert!(matches!(outcomes[0], MeterOutcome::Cut { .. }));
        // And a subsequent request is denied at admission — the cap moved.
        let t = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        assert_eq!(b.admit(&id(), Some(&t), 0), Verdict::Deny { cap: 100_000 });
    }

    #[test]
    fn terminal_charge_honors_partition_bound() {
        // Under partition the non-streaming charge cannot exceed the held
        // share any more than a stream can (bounded overspend holds for
        // stream:false traffic too).
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let caps = vec![(id(), terms)];
        b.apply_shares(&[(id(), 100_000, 40_000)]);
        b.set_partitioned(true);
        let outcomes = b.charge_terminal_and_log(&caps, 41_000, "r", 1, 0);
        assert!(matches!(outcomes[0], MeterOutcome::Cut { .. }), "past the 40k share -> over bound");
        assert_eq!(b.overspend(&id()), 1_000);
    }

    #[test]
    fn settle_reconciles_the_estimate_to_the_authoritative_frame() {
        let (b, _) = budgets();
        let terms = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let cap = Some(&terms);
        b.meter(&id(), cap, 1_000, 0); // live estimate
        b.settle(&id(), 1_000, 1_250, 0); // provider frame says 1_250
        let (_, _, spent) = b.snapshot(&id()).unwrap();
        assert_eq!(spent, 1_250);
    }

    /// The MEASURED bounded-overspend number the Phase 3 exit criterion asks
    /// for, printed for the demo to capture (run with `--nocapture`). A node
    /// holds a 40k share of a 100k cap; the control plane goes unreachable
    /// (partition). One in-flight stream is admitted just under the share and
    /// runs 1_800 tokens before the mid-stream cut stops it. The overspend past
    /// the held share is reported as an absolute number AND as a fraction of the
    /// configured cap — bounded by one stream's tail, never unbounded.
    #[test]
    fn measured_bounded_overspend_under_partition() {
        let (b, _) = budgets();
        let cap: u64 = 100_000;
        let share: u64 = 40_000;
        let pterms = CapTerms { cap, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        b.apply_shares(&[(id(), cap, share)]);
        b.set_partitioned(true);

        // Admit one last stream just under the share, then let it run past.
        assert_eq!(b.admit(&id(), Some(&pterms), 0), Verdict::Allow);
        b.meter(&id(), Some(&pterms), 39_800, 0); // still under the 40k share
        let outcome = b.meter(&id(), Some(&pterms), 1_800, 0); // crosses the share -> cut
        assert!(matches!(outcome, MeterOutcome::Cut { .. }));

        let overspend = b.overspend(&id());
        let (_, held_share, spent) = b.snapshot(&id()).unwrap();
        let pct_of_cap = overspend as f64 / cap as f64 * 100.0;
        println!(
            "[MEASURED] partition bounded-overspend: cap={cap} tokens, held_share={held_share}, \
             spent={spent}, overspend={overspend} tokens ({pct_of_cap:.2}% of the cap); \
             the node was UNREACHABLE and stopped at its share + one stream's tail, \
             never unbounded"
        );
        // The bound: the overspend is exactly the tail the one admitted stream
        // produced past the share (1_600 here), and strictly less than one
        // stream's worth — never the unbounded local-bucket failure.
        assert_eq!(spent, 41_600);
        assert_eq!(overspend, 1_600);
        assert!(overspend < cap, "overspend is bounded well under the cap");
    }

    #[test]
    fn spend_report_carries_only_capped_nonzero_spenders() {
        let (b, _) = budgets();
        let t100 = CapTerms { cap: 100_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        let t50 = CapTerms { cap: 50_000, window: None, alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION };
        b.meter(&id(), Some(&t100), 5_000, 0);
        b.meter(&CapId::new("region", "eu"), None, 9_000, 0); // uncapped
        b.meter(&CapId::new("team", "idle"), Some(&t50), 0, 0); // zero spend
        let report = b.spend_report(0);
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].0, id());
        assert_eq!(report[0].2, 5_000);
    }

    // --- Billing windows (lazy rollover) -------------------------------------

    fn minute_terms(cap: u64) -> CapTerms {
        CapTerms {
            cap,
            window: Some(Window::Minute),
            alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION,
        }
    }

    #[test]
    fn a_minute_window_rolls_the_counter_and_readmits() {
        let (b, _) = budgets();
        let t = minute_terms(100_000);
        // Exhaust the cap inside window 0.
        b.meter(&id(), Some(&t), 100_000, 10);
        assert_eq!(b.admit(&id(), Some(&t), 30), Verdict::Deny { cap: 100_000 });
        // 61s: a new minute window — the counter starts over, spend admitted.
        assert_eq!(b.admit(&id(), Some(&t), 61), Verdict::Allow);
        let (_, _, spent) = b.snapshot(&id()).unwrap();
        assert_eq!(spent, 0, "window rollover resets the spend counter");
    }

    #[test]
    fn a_lifetime_cap_never_rolls() {
        let (b, _) = budgets();
        let t = CapTerms {
            cap: 100_000,
            window: None,
            alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION,
        };
        b.meter(&id(), Some(&t), 100_000, 0);
        // A year later: still denied. Lifetime = the original behavior.
        assert_eq!(
            b.admit(&id(), Some(&t), 31_536_000),
            Verdict::Deny { cap: 100_000 }
        );
    }

    #[test]
    fn alerts_re_arm_each_window_and_honor_alert_at() {
        let (b, sink) = budgets();
        // alert_at 50%: the operator's threshold, not the default 80.
        let t = CapTerms {
            cap: 100_000,
            window: Some(Window::Minute),
            alert_fraction: 0.5,
        };
        b.meter(&id(), Some(&t), 55_000, 0); // crosses 50% -> soft fires
        assert_eq!(sink.alerts().len(), 1);
        assert!(matches!(
            sink.alerts()[0].kind,
            AlertKind::SoftThreshold { fraction } if (fraction - 0.5).abs() < 1e-9
        ));
        b.meter(&id(), Some(&t), 1_000, 5); // same window: latched, no re-fire
        assert_eq!(sink.alerts().len(), 1);
        // Next minute: the latch re-armed; crossing fires again.
        b.meter(&id(), Some(&t), 60_000, 70);
        assert_eq!(sink.alerts().len(), 2);
    }

    #[test]
    fn a_changed_window_kind_resets_the_counters() {
        let (b, _) = budgets();
        let day = CapTerms {
            cap: 100_000,
            window: Some(Window::Day),
            alert_fraction: gateway_core::budget::SOFT_ALERT_FRACTION,
        };
        b.meter(&id(), Some(&day), 90_000, 100);
        // The operator hot-swaps the cap to a minute window: yesterday's
        // day-spend has no meaning against the new shape.
        let minute = minute_terms(100_000);
        assert_eq!(b.admit(&id(), Some(&minute), 100), Verdict::Allow);
        let (_, _, spent) = b.snapshot(&id()).unwrap();
        assert_eq!(spent, 0);
    }

    #[test]
    fn spend_report_rolls_before_reporting() {
        let (b, _) = budgets();
        let t = minute_terms(100_000);
        b.meter(&id(), Some(&t), 5_000, 0);
        assert_eq!(b.spend_report(30).len(), 1, "same window: spend reported");
        // Next window: the rolled counter reports empty — the control plane's
        // overwrite ledger then propagates the reset fleet-wide.
        assert!(b.spend_report(90).is_empty());
    }
}
