//! Cross-provider conformance: the same message streamed over three wire
//! formats must normalize to the same canonical shape, at any chunking.
//!
//! Ported from `spikes/event-model/tests/conformance.rs` (Phase 0, Spike A);
//! the fixtures — including the 17-transcript real corpus — stay in the
//! frozen spike and are referenced by relative path, not copied.

use gateway_core::adapters::anthropic::AnthropicAdapter;
use gateway_core::adapters::bedrock::{encode_jsonl_fixture, BedrockAdapter};
use gateway_core::adapters::openai::OpenAiAdapter;
use gateway_core::adapters::Adapter;
use gateway_core::event::Event;
use gateway_core::metering::Meter;

const EXPECTED_TEXT: &str = "The Gateway Project keeps truth in Git.";

/// The spike's fixture corpus, from crates/gateway-core two levels up.
const FIXTURES: &str = "../../spikes/event-model/fixtures";

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/{FIXTURES}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn replay(mut adapter: Box<dyn Adapter>, wire: &[u8], chunk_size: usize) -> Vec<Event> {
    let mut events = Vec::new();
    for chunk in wire.chunks(chunk_size) {
        events.extend(adapter.feed(chunk));
    }
    events
}

fn assert_canonical_shape(events: &[Event]) {
    assert!(
        matches!(events.first(), Some(Event::MessageStart { .. })),
        "first event must be MessageStart, got {:?}",
        events.first()
    );
    assert!(
        matches!(events.last(), Some(Event::MessageEnd { .. })),
        "last event must be MessageEnd, got {:?}",
        events.last()
    );

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            Event::ContentDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, EXPECTED_TEXT);

    // The ordering contract: the terminal usage frame precedes MessageEnd.
    let last_usage = events
        .iter()
        .rposition(|e| matches!(e, Event::UsageDelta { .. }))
        .expect("stream must carry a usage frame");
    let end = events
        .iter()
        .position(|e| matches!(e, Event::MessageEnd { .. }))
        .unwrap();
    assert!(
        last_usage < end,
        "terminal usage frame must precede MessageEnd"
    );

    let mut meter = Meter::new();
    events.iter().for_each(|e| meter.observe(e));
    let report = meter.report();
    assert_eq!(report.authoritative_input_tokens, Some(14));
    assert_eq!(report.authoritative_output_tokens, Some(9));
    // 39 chars / 4 = 10 estimated vs 9 authoritative: the heuristic's error
    // is visible and measured, not hidden.
    assert_eq!(report.estimated_output_tokens, 10);
    let err = report.error_pct.unwrap();
    assert!((err - 11.11).abs() < 0.1, "error_pct = {err}");
}

// The provider-table type is the spike's shape, ported as-is.
#[allow(clippy::type_complexity)]
fn providers() -> Vec<(&'static str, fn() -> Box<dyn Adapter>, Vec<u8>)> {
    vec![
        (
            "openai",
            || Box::new(OpenAiAdapter::new()) as Box<dyn Adapter>,
            fixture("openai.sse"),
        ),
        (
            "anthropic",
            || Box::new(AnthropicAdapter::new()) as Box<dyn Adapter>,
            fixture("anthropic.sse"),
        ),
        (
            "bedrock",
            || Box::new(BedrockAdapter::new()) as Box<dyn Adapter>,
            encode_jsonl_fixture(&String::from_utf8(fixture("bedrock.jsonl")).unwrap())
                .unwrap(),
        ),
    ]
}

#[test]
fn all_providers_normalize_to_the_canonical_shape() {
    for (name, make, wire) in providers() {
        let events = replay(make(), &wire, wire.len().max(1));
        assert!(!events.is_empty(), "{name}: no events");
        assert_canonical_shape(&events);
    }
}

#[test]
fn chunking_never_changes_the_event_stream() {
    for (name, make, wire) in providers() {
        let whole = replay(make(), &wire, wire.len().max(1));
        for chunk_size in [1, 7, 64] {
            let chunked = replay(make(), &wire, chunk_size);
            assert_eq!(
                chunked, whole,
                "{name}: chunk_size={chunk_size} diverged from whole-buffer replay"
            );
        }
    }
}

/// Every recorded real transcript under fixtures/real/ must replay to a
/// stream that honors the canonical contract — authoritative usage frame
/// present and preceding a terminal MessageEnd — at any chunking.
#[test]
fn real_transcripts_replay_with_usage_and_ordering_contract() {
    let real_dir = format!("{}/{FIXTURES}/real", env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0usize;
    for (provider, make) in [
        ("openai", (|| Box::new(OpenAiAdapter::new()) as Box<dyn Adapter>)
            as fn() -> Box<dyn Adapter>),
        ("anthropic", || Box::new(AnthropicAdapter::new()) as Box<dyn Adapter>),
        ("bedrock", || Box::new(BedrockAdapter::new()) as Box<dyn Adapter>),
    ] {
        let dir = format!("{real_dir}/{provider}");
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|e| e.unwrap().path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("sse") | Some("jsonl")
                )
            })
            .collect();
        paths.sort();
        for path in paths {
            let raw = std::fs::read(&path).unwrap();
            let wire = if provider == "bedrock" {
                encode_jsonl_fixture(&String::from_utf8(raw).unwrap()).unwrap()
            } else {
                raw
            };
            let name = path.display();

            let whole = replay(make(), &wire, wire.len().max(1));
            for chunk_size in [1, 17] {
                let chunked = replay(make(), &wire, chunk_size);
                assert_eq!(
                    chunked, whole,
                    "{name}: chunk_size={chunk_size} diverged from whole-buffer replay"
                );
            }

            let end = whole
                .iter()
                .position(|e| matches!(e, Event::MessageEnd { .. }))
                .unwrap_or_else(|| panic!("{name}: no MessageEnd"));
            assert_eq!(end, whole.len() - 1, "{name}: MessageEnd must be terminal");
            let last_usage = whole
                .iter()
                .rposition(|e| matches!(e, Event::UsageDelta { .. }))
                .unwrap_or_else(|| panic!("{name}: no usage frame"));
            assert!(
                last_usage < end,
                "{name}: terminal usage frame must precede MessageEnd"
            );

            let mut meter = Meter::new();
            whole.iter().for_each(|e| meter.observe(e));
            let report = meter.report();
            assert!(
                report.authoritative_output_tokens.is_some(),
                "{name}: no authoritative output tokens"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 15,
        "expected the full real-transcript corpus, found {checked}"
    );
}

#[test]
fn tool_calls_normalize_across_providers() {
    let args = r#"{"location":"Tel Aviv"}"#;

    // OpenAI: tool call streamed as function-call argument fragments.
    let mut openai_wire = String::new();
    openai_wire.push_str("data: {\"id\":\"c1\",\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n");
    openai_wire.push_str("data: {\"id\":\"c1\",\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"location\\\":\"}}]},\"finish_reason\":null}]}\n\n");
    openai_wire.push_str("data: {\"id\":\"c1\",\"model\":\"gpt-4.1\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Tel Aviv\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n");
    openai_wire.push_str("data: [DONE]\n\n");

    // Anthropic: tool_use block start plus input_json_delta fragments.
    let mut anthropic_wire = String::new();
    anthropic_wire.push_str("data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"claude-sonnet-5\",\"usage\":{\"input_tokens\":8,\"output_tokens\":1}}}\n\n");
    anthropic_wire.push_str("data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}\n\n");
    anthropic_wire.push_str("data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"location\\\":\"}}\n\n");
    anthropic_wire.push_str("data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"Tel Aviv\\\"}\"}}\n\n");
    anthropic_wire.push_str("data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":6}}\n\n");
    anthropic_wire.push_str("data: {\"type\":\"message_stop\"}\n\n");

    // Bedrock: toolUse content block over event-stream frames.
    let bedrock_jsonl = concat!(
        "{\"event\":\"messageStart\",\"payload\":{\"role\":\"assistant\"}}\n",
        "{\"event\":\"contentBlockStart\",\"payload\":{\"contentBlockIndex\":0,\"start\":{\"toolUse\":{\"toolUseId\":\"t1\",\"name\":\"get_weather\"}}}}\n",
        "{\"event\":\"contentBlockDelta\",\"payload\":{\"contentBlockIndex\":0,\"delta\":{\"toolUse\":{\"input\":\"{\\\"location\\\":\"}}}}\n",
        "{\"event\":\"contentBlockDelta\",\"payload\":{\"contentBlockIndex\":0,\"delta\":{\"toolUse\":{\"input\":\"\\\"Tel Aviv\\\"}\"}}}}\n",
        "{\"event\":\"messageStop\",\"payload\":{\"stopReason\":\"tool_use\"}}\n",
        "{\"event\":\"metadata\",\"payload\":{\"usage\":{\"inputTokens\":8,\"outputTokens\":6}}}\n",
    );
    let bedrock_wire = encode_jsonl_fixture(bedrock_jsonl).unwrap();

    let runs: Vec<(&str, Vec<Event>)> = vec![
        (
            "openai",
            replay(Box::new(OpenAiAdapter::new()), openai_wire.as_bytes(), 1),
        ),
        (
            "anthropic",
            replay(
                Box::new(AnthropicAdapter::new()),
                anthropic_wire.as_bytes(),
                1,
            ),
        ),
        ("bedrock", replay(Box::new(BedrockAdapter::new()), &bedrock_wire, 1)),
    ];

    for (name, events) in runs {
        let first_tool = events
            .iter()
            .find_map(|e| match e {
                Event::ToolCallDelta { name, .. } => name.clone(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name}: no named ToolCallDelta"));
        assert_eq!(first_tool, "get_weather", "{name}");

        let assembled: String = events
            .iter()
            .filter_map(|e| match e {
                Event::ToolCallDelta {
                    arguments_delta, ..
                } => Some(arguments_delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(assembled, args, "{name}: reassembled tool arguments");

        assert!(
            matches!(events.last(), Some(Event::MessageEnd { .. })),
            "{name}: MessageEnd terminal"
        );
    }
}
