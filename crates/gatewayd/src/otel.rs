//! OTLP telemetry export: one span per request, carrying the ADJUDICATED
//! attribution — the observability face of the invoice thesis, shipped to
//! the collector the operator already owns. No dashboard of ours.
//!
//! Deliberately SDK-free (defer, no bloat): the exporter speaks OTLP/HTTP
//! with the protobuf-JSON encoding, which every collector's OTLP receiver
//! accepts on `/v1/traces` — spans are hand-assembled with serde_json and
//! POSTed with the same minimal HTTP client the STS/GCP chains use. No
//! OpenTelemetry dependency tree, no global tracer, no context
//! propagation (a follow-up if a fleet asks).
//!
//! Best-effort BY DESIGN: the proxy's hot path does one bounded
//! `try_send`; a slow or absent collector fills the queue and further
//! spans are DROPPED AND COUNTED (logged at flush), never buffered
//! unbounded and never blocking enforcement. Telemetry is an observer
//! here; the GB checks are the product.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use serde_json::json;

use gateway_core::aws::sha256_hex;
use gateway_core::config::OtlpConfig;

/// Bounded queue depth: at most this many spans wait for a flush.
const QUEUE_DEPTH: usize = 4096;
/// Flush early when this many spans are pending.
const BATCH_CAP: usize = 512;

/// Everything one request-span carries. Plain data; JSON assembly happens
/// on the flusher thread, never on the hot path.
#[derive(Debug)]
pub struct SpanRecord {
    pub start_unix_nanos: u64,
    pub end_unix_nanos: u64,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub route: Option<String>,
    pub provider: Option<String>,
    /// Adjudicated attribution tags — the point of the span.
    pub attribution: Vec<(String, String)>,
    pub tokens_estimated: u64,
    pub tokens_authoritative: Option<u64>,
    pub cut: bool,
    pub config_version: u64,
    /// Infra-failure detail (e.g. the STS error code on a 502) — the span's
    /// answer to "why did this request fail", debuggable from the collector.
    pub error: Option<String>,
}

/// The hot-path handle: a bounded try_send plus a dropped counter.
pub struct OtelExporter {
    tx: SyncSender<SpanRecord>,
    dropped: Arc<AtomicU64>,
}

impl OtelExporter {
    /// Spawn the flusher thread and return the handle. The thread owns a
    /// tiny current-thread tokio runtime for the HTTP posts.
    pub fn spawn(cfg: OtlpConfig, node_id: String) -> OtelExporter {
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_reader = dropped.clone();
        info!(
            "[otel] exporting request spans to {}:{} /v1/traces (service={}, flush={}s)",
            cfg.endpoint.host, cfg.endpoint.port, cfg.service_name, cfg.flush_interval_secs,
        );
        std::thread::Builder::new()
            .name("otel-flusher".into())
            .spawn(move || flusher(cfg, node_id, rx, dropped_reader))
            .expect("spawn otel flusher");
        OtelExporter { tx, dropped }
    }

    /// Queue a span. Never blocks: a full queue drops the span and counts.
    pub fn record(&self, span: SpanRecord) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(span) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn flusher(cfg: OtlpConfig, node_id: String, rx: Receiver<SpanRecord>, dropped: Arc<AtomicU64>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("otel runtime");
    let interval = Duration::from_secs(cfg.flush_interval_secs);
    let mut seq: u64 = 0;
    loop {
        // Collect one batch: block for the first span (no busy loop), then
        // drain up to the cap or until the flush interval elapses.
        let first = match rx.recv_timeout(interval) {
            Ok(s) => Some(s),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let mut batch = Vec::new();
        if let Some(s) = first {
            batch.push(s);
            let deadline = std::time::Instant::now() + interval;
            while batch.len() < BATCH_CAP {
                match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                {
                    Ok(s) => batch.push(s),
                    Err(_) => break,
                }
            }
        }
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            warn!("[otel] dropped {lost} span(s): queue full (collector slow or absent)");
        }
        if batch.is_empty() {
            continue;
        }
        let body = otlp_traces_json(&cfg, &node_id, &batch, &mut seq).to_string();
        let n = batch.len();
        let result = rt.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                crate::aws_auth::http_post(
                    &cfg.endpoint,
                    "/v1/traces",
                    "application/json",
                    &body,
                    &[],
                ),
            )
            .await
        });
        match result {
            Ok(Ok((200, _))) => {
                info!("[otel] flushed {n} span(s)");
            }
            Ok(Ok((status, resp))) => error!(
                "[otel] collector returned {status}: {} ({n} span(s) lost)",
                resp.chars().take(120).collect::<String>()
            ),
            Ok(Err(e)) => error!("[otel] flush failed: {e} ({n} span(s) lost)"),
            Err(_) => error!("[otel] flush timed out ({n} span(s) lost)"),
        }
    }
}

/// Assemble the OTLP/JSON ExportTraceServiceRequest. Field spellings and
/// value envelopes follow the proto3 JSON mapping the collector's OTLP
/// receiver parses (camelCase keys, int64 as strings, attribute values
/// wrapped in typed envelopes).
pub fn otlp_traces_json(
    cfg: &OtlpConfig,
    node_id: &str,
    batch: &[SpanRecord],
    seq: &mut u64,
) -> serde_json::Value {
    let spans: Vec<serde_json::Value> = batch
        .iter()
        .map(|s| {
            *seq += 1;
            // Unique ids derived, not random: sha256 over (node, seq, start).
            let digest = sha256_hex(
                format!("{node_id}|{seq}|{}", s.start_unix_nanos).as_bytes(),
            );
            let trace_id = &digest[..32];
            let span_id = &digest[32..48];
            let mut attributes = vec![
                str_attr("http.request.method", &s.method),
                str_attr("url.path", &s.path),
                int_attr("gateway.config.version", s.config_version),
                int_attr("gateway.tokens.estimated", s.tokens_estimated),
                bool_attr("gateway.stream.cut", s.cut),
            ];
            if let Some(code) = s.status {
                attributes.push(int_attr("http.response.status_code", u64::from(code)));
            }
            if let Some(route) = &s.route {
                attributes.push(str_attr("gateway.route", route));
            }
            if let Some(provider) = &s.provider {
                attributes.push(str_attr("gateway.provider", provider));
            }
            if let Some(auth_tokens) = s.tokens_authoritative {
                attributes.push(int_attr("gateway.tokens.authoritative", auth_tokens));
            }
            if let Some(error) = &s.error {
                attributes.push(str_attr("gateway.error", error));
            }
            for (key, value) in &s.attribution {
                attributes.push(str_attr(&format!("gateway.attribution.{key}"), value));
            }
            let status_code = match s.status {
                Some(c) if c >= 500 => 2, // ERROR
                _ => 1,                   // OK
            };
            json!({
                "traceId": trace_id,
                "spanId": span_id,
                "name": "gateway.request",
                "kind": 2, // SERVER
                "startTimeUnixNano": s.start_unix_nanos.to_string(),
                "endTimeUnixNano": s.end_unix_nanos.to_string(),
                "attributes": attributes,
                "status": { "code": status_code },
            })
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    str_attr("service.name", &cfg.service_name),
                    str_attr("service.instance.id", node_id),
                ],
            },
            "scopeSpans": [{
                "scope": { "name": "gatewayd" },
                "spans": spans,
            }],
        }],
    })
}

fn str_attr(key: &str, value: &str) -> serde_json::Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn int_attr(key: &str, value: u64) -> serde_json::Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

fn bool_attr(key: &str, value: bool) -> serde_json::Value {
    json!({ "key": key, "value": { "boolValue": value } })
}

/// Wall-clock unix nanos (u64 holds until 2554).
pub fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::config::Upstream;

    fn cfg() -> OtlpConfig {
        OtlpConfig {
            endpoint: Upstream { host: "127.0.0.1".into(), port: 4318, tls: false, sni: None },
            service_name: "gatewayd".into(),
            flush_interval_secs: 5,
        }
    }

    #[test]
    fn otlp_json_carries_the_adjudicated_attribution() {
        let span = SpanRecord {
            start_unix_nanos: 1_000,
            end_unix_nanos: 2_000,
            method: "POST".into(),
            path: "/openai/v1/chat".into(),
            status: Some(200),
            route: Some("/openai".into()),
            provider: Some("openai-main".into()),
            attribution: vec![("ward".into(), "peds-3".into())],
            tokens_estimated: 42,
            tokens_authoritative: Some(40),
            cut: false,
            config_version: 7,
            error: Some("sts: STS returned 403 code=AccessDenied".into()),
        };
        let mut seq = 0;
        let v = otlp_traces_json(&cfg(), "node-1", &[span], &mut seq);

        let resource = &v["resourceSpans"][0]["resource"]["attributes"];
        assert!(resource.as_array().unwrap().iter().any(|a| a["key"] == "service.name"));
        let s = &v["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(s["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(s["spanId"].as_str().unwrap().len(), 16);
        assert_eq!(s["startTimeUnixNano"], "1000");
        let attrs = s["attributes"].as_array().unwrap();
        let find = |k: &str| attrs.iter().find(|a| a["key"] == k).cloned();
        assert_eq!(find("gateway.attribution.ward").unwrap()["value"]["stringValue"], "peds-3");
        assert_eq!(find("gateway.tokens.authoritative").unwrap()["value"]["intValue"], "40");
        assert_eq!(find("http.response.status_code").unwrap()["value"]["intValue"], "200");
        assert_eq!(
            find("gateway.error").unwrap()["value"]["stringValue"],
            "sts: STS returned 403 code=AccessDenied"
        );
        assert_eq!(s["status"]["code"], 1);
    }

    #[test]
    fn span_ids_are_unique_across_a_batch() {
        let mk = |n: u64| SpanRecord {
            start_unix_nanos: n,
            end_unix_nanos: n + 1,
            method: "POST".into(),
            path: "/x".into(),
            status: None,
            route: None,
            provider: None,
            attribution: vec![],
            tokens_estimated: 0,
            tokens_authoritative: None,
            cut: false,
            config_version: 1,
            error: None,
        };
        let mut seq = 0;
        let v = otlp_traces_json(&cfg(), "n", &[mk(5), mk(5)], &mut seq);
        let spans = v["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap();
        assert_ne!(spans[0]["spanId"], spans[1]["spanId"]);
        assert_ne!(spans[0]["traceId"], spans[1]["traceId"]);
    }
}
