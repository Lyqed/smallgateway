//! Per-request provider selection: `x-spike-provider` header first, then a
//! route prefix (`/openai/...`, `/anthropic/...`, `/bedrock/...`), default
//! OpenAI. Pure and pingora-free so it stays unit-testable.

use spike_event_model::adapters::{
    anthropic::AnthropicAdapter, bedrock::BedrockAdapter, openai::OpenAiAdapter, Adapter,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
    Bedrock,
}

impl Provider {
    /// Header wins over path prefix; anything unrecognized falls back to
    /// OpenAI (the spike default, matching the task spec).
    pub fn select(header: Option<&str>, path: &str) -> Self {
        if let Some(p) = header.and_then(|h| Self::parse(h.trim())) {
            return p;
        }
        let first_segment = path
            .trim_start_matches('/')
            .split(['/', '?'])
            .next()
            .unwrap_or("");
        Self::parse(first_segment).unwrap_or(Provider::OpenAi)
    }

    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai" => Some(Provider::OpenAi),
            "anthropic" => Some(Provider::Anthropic),
            "bedrock" => Some(Provider::Bedrock),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Bedrock => "bedrock",
        }
    }

    /// Fresh adapter for one response stream. `Send + Sync` because pingora
    /// requires `ProxyHttp::CTX: Send + Sync`; the adapters are plain-data
    /// push parsers so both auto-derive.
    pub fn new_adapter(self) -> Box<dyn Adapter + Send + Sync> {
        match self {
            Provider::OpenAi => Box::new(OpenAiAdapter::new()),
            Provider::Anthropic => Box::new(AnthropicAdapter::new()),
            Provider::Bedrock => Box::new(BedrockAdapter::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_wins_over_path() {
        assert_eq!(
            Provider::select(Some("anthropic"), "/openai/v1/chat"),
            Provider::Anthropic
        );
    }

    #[test]
    fn header_is_case_insensitive_and_trimmed() {
        assert_eq!(Provider::select(Some(" Bedrock "), "/"), Provider::Bedrock);
    }

    #[test]
    fn route_prefix_used_when_no_header() {
        assert_eq!(
            Provider::select(None, "/anthropic/v1/messages"),
            Provider::Anthropic
        );
        assert_eq!(
            Provider::select(None, "/bedrock/model/x/converse-stream"),
            Provider::Bedrock
        );
    }

    #[test]
    fn defaults_to_openai() {
        assert_eq!(Provider::select(None, "/v1/chat/completions"), Provider::OpenAi);
        assert_eq!(Provider::select(Some("mystery"), "/nope"), Provider::OpenAi);
        assert_eq!(Provider::select(None, "/"), Provider::OpenAi);
    }
}
