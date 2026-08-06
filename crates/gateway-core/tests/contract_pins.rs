//! Byte-exact pins for the gateway-owned rejection shapes frozen by the
//! rejection-shape contract (docs/13). Operator templates are rendered
//! verbatim by construction; these tests pin the few shapes the GATEWAY
//! owns, so any accidental change to a default fails CI instead of a
//! caller's parser. Changing any expectation here is a contract change
//! and requires a docs/13 entry.

use gateway_core::config::{default_model_not_allowed, StreamingRejection};
use gateway_core::eventstream::FrameDecoder;
use gateway_core::template::{render_terminal_event, render_terminal_frame_bedrock};

/// The one gateway-invented 4xx body (the model gate is opt-in per scope,
/// so this default exists; every other reason is operator-mandatory).
#[test]
fn default_model_not_allowed_shape_is_frozen() {
    let t = default_model_not_allowed();
    assert_eq!(t.status, 403);
    assert_eq!(t.content_type, "application/json");
    assert_eq!(
        t.body,
        r#"{"error":"model_not_allowed","model":"{{model}}","route":"{{route}}"}"#
    );
    assert!(t.streaming.is_none(), "the default carries no streaming half");
}

/// The SSE terminal-event framing: event line, one `data:` prefix per
/// payload line, blank-line terminator. Byte-exact.
#[test]
fn sse_terminal_event_framing_is_frozen() {
    let s = StreamingRejection {
        event: Some("cap".to_string()),
        data: "{\"who\":\"{{key}}\"}".to_string(),
    };
    let out = render_terminal_event(&s, &[("key", "team=ml")]);
    assert_eq!(out, "event: cap\ndata: {\"who\":\"team=ml\"}\n\n");

    // No event name: a bare data frame, still blank-line terminated.
    let bare = StreamingRejection {
        event: None,
        data: "x".to_string(),
    };
    assert_eq!(render_terminal_event(&bare, &[]), "data: x\n\n");
}

/// The Bedrock cut frame: one exception frame whose `:exception-type`
/// defaults to `stream_cut` when the operator names no event. The default
/// name is contract; SDK decoders surface it as the typed error.
#[test]
fn bedrock_default_exception_type_is_frozen() {
    let s = StreamingRejection {
        event: None,
        data: "{\"why\":\"cap\"}".to_string(),
    };
    let frame_bytes = render_terminal_frame_bedrock(&s, &[]);
    let mut dec = FrameDecoder::new();
    let frames = dec.feed(&frame_bytes).expect("well-formed frame");
    assert_eq!(frames.len(), 1, "exactly one terminal frame");
    let f = &frames[0];
    assert_eq!(f.headers.get(":message-type").map(String::as_str), Some("exception"));
    assert_eq!(f.headers.get(":exception-type").map(String::as_str), Some("stream_cut"));
    assert_eq!(f.headers.get(":content-type").map(String::as_str), Some("application/json"));
    assert_eq!(f.payload, b"{\"why\":\"cap\"}");

    // An operator-named event replaces the exception type verbatim.
    let named = StreamingRejection {
        event: Some("route-cap".to_string()),
        data: "{}".to_string(),
    };
    let frames = FrameDecoder::new()
        .feed(&render_terminal_frame_bedrock(&named, &[]))
        .expect("well-formed frame");
    assert_eq!(
        frames[0].headers.get(":exception-type").map(String::as_str),
        Some("route-cap")
    );
}
