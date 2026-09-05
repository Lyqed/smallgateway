//! Synthetic compatibility checks, not recordings from live provider accounts.
use gateway_core::adapters::{json_body::JsonBodyTap, Adapter};
use gateway_core::attribution::{resolve, Origin};
use gateway_core::config::Config;
use gateway_core::metering::Meter;

fn recipes() -> [(&'static str, &'static str); 2] {
    [
        (
            "nebius",
            include_str!("../../../deploy/examples/nebius.yaml"),
        ),
        (
            "coreweave",
            include_str!("../../../deploy/examples/coreweave.yaml"),
        ),
    ]
}

#[test]
fn neocloud_recipes_keep_operator_ownership_when_caller_spoofs_it() {
    for (name, yaml) in recipes() {
        let cfg = Config::from_yaml(yaml).unwrap();
        let policy = cfg.routes[0].policy();
        let resolved = resolve(policy, |_| Some("forged".into()), None, |_| None);
        assert!(resolved.ok(), "{name}");
        assert!(resolved.tags.iter().all(|t| t.origin == Origin::Assigned));
        assert_eq!(
            resolved
                .tags
                .iter()
                .find(|t| t.key == "team")
                .unwrap()
                .value,
            "team"
        );
        assert_eq!(
            resolved.tags.iter().find(|t| t.key == "app").unwrap().value,
            "app"
        );
    }
}

#[test]
fn neocloud_wire_recipes_meter_json_and_chunked_sse_and_keep_missing_usage_unknown() {
    // Shapes used by compatible chat endpoints, with deliberately synthetic counts.
    let json = br#"{"id":"test","model":"test-model","choices":[{"message":{"content":"Hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":24,"completion_tokens":80,"total_tokens":104},"kv_transfer_params":null}"#;
    let sse = b"data: {\"id\":\"test\",\"model\":\"test-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}],\"usage\":null}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":24,\"completion_tokens\":80,\"total_tokens\":104}}\n\ndata: [DONE]\n\n";
    for (name, yaml) in recipes() {
        let cfg = Config::from_yaml(yaml).unwrap();
        let kind = cfg.providers[name].kind;
        for chunk_size in [1, 7, 4096] {
            let mut taps: Vec<(Box<dyn Adapter + Send + Sync>, &[u8])> = vec![
                (kind.new_adapter(), sse),
                (Box::new(JsonBodyTap::new(kind)), json),
            ];
            for (tap, wire) in &mut taps {
                let mut meter = Meter::new();
                for chunk in wire.chunks(chunk_size) {
                    for event in tap.feed(chunk) {
                        meter.observe(&event);
                    }
                }
                for event in tap.finish() {
                    meter.observe(&event);
                }
                assert_eq!(
                    meter.report().authoritative_input_tokens,
                    Some(24),
                    "{name}"
                );
                assert_eq!(
                    meter.report().authoritative_output_tokens,
                    Some(80),
                    "{name}"
                );
            }
        }
        let mut tap = kind.new_adapter();
        let mut meter = Meter::new();
        for event in tap
            .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n")
        {
            meter.observe(&event);
        }
        assert_eq!(meter.report().authoritative_output_tokens, None, "{name}");
        assert!(meter.report().estimated_output_tokens > 0);
    }
}
