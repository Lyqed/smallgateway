//! Run the published recipes through the real proxy with local mock upstreams.
use super::harness::*;

#[test]
fn caller_signed_bedrock_keeps_its_host_while_bearer_routes_use_the_upstream() {
    let p = ports(2);
    let cfg = super::base_cfg(p[0]).replace("kind: openai", "kind: bedrock");
    let _mock = spawn_mock(p[0], &spike_fixture("bedrock.jsonl"), "bedrock", false);
    let _gateway = spawn_gatewayd(&cfg, p[1], "host-signature");
    // This checks transport preservation, not cryptographic verification.
    for (auth, expected_port) in [
        ("AWS4-HMAC-SHA256 test-signed-request", p[1]),
        ("Bearer test-provider-key", p[0]),
    ] {
        let reply = http(
            p[1],
            "POST",
            "/openai/model/test/converse-stream",
            &[("authorization", auth), ("x-attr-team", "team")],
            b"{}",
        );
        assert_eq!(reply.status, 200);
        let authority = format!("127.0.0.1:{expected_port}");
        assert_eq!(reply.header("x-echo-host"), Some(authority.as_str()));
    }
}

#[test]
fn neocloud_recipes_preserve_provider_auth_path_host_and_stream() {
    let recipes = [
        (
            "nebius",
            "api.tokenfactory.nebius.com",
            include_str!("../../../../deploy/examples/nebius.yaml"),
        ),
        (
            "coreweave",
            "your-gateway.example.com",
            include_str!("../../../../deploy/examples/coreweave.yaml"),
        ),
    ];
    for (name, host, yaml) in recipes {
        let p = ports(2);
        // Only the transport points at a mock. The rest of the published
        // configuration stays intact; Host must name the upstream port.
        let yaml = yaml.replace(
            &format!("host: {host}, port: 443, tls: true"),
            &format!("host: 127.0.0.1, port: {}, tls: false", p[0]),
        );
        let fixture = spike_fixture("openai.sse");
        let _mock = spawn_mock_bearer(p[0], &fixture, "openai", "recipe-test-");
        let _gateway = spawn_gatewayd(&yaml, p[1], name);
        let reply = http(
            p[1], "POST", "/v1/chat/completions",
            &[("authorization", "Bearer recipe-test-token"), ("content-type", "application/json")],
            br#"{"model":"test-model","messages":[],"stream":true,"stream_options":{"include_usage":true}}"#,
        );
        assert_eq!(reply.status, 200, "{name}: {}", reply.body_text());
        let authority = format!("127.0.0.1:{}", p[0]);
        assert_eq!(reply.header("x-echo-host"), Some(authority.as_str()));
        assert_eq!(
            reply.header("x-echo-request-line"),
            Some("POST /v1/chat/completions HTTP/1.1")
        );
        assert_eq!(reply.header("x-echo-bearer"), Some("recipe-test-token"));
        assert_eq!(
            reply.body,
            std::fs::read(&fixture).unwrap(),
            "{name}: stream altered"
        );

        let denied = http(p[1], "POST", "/v1/chat/completions", &[], b"{}");
        assert_eq!(
            denied.status, 403,
            "{name}: provider rejection must pass through"
        );
    }
}
