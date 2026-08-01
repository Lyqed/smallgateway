//! Replay a recorded provider stream through its adapter and print the
//! canonical events plus the metering reconciliation report.
//!
//! Usage:
//!   spike-event-model --provider openai    --file fixtures/openai.sse
//!   spike-event-model --provider anthropic --file fixtures/anthropic.sse
//!   spike-event-model --provider bedrock   --file fixtures/bedrock.jsonl
//!
//! `--chunk-size N` controls the replay chunking (default 17, deliberately
//! odd so frame boundaries never line up with feed boundaries).

use std::env;
use std::fs;
use std::process::exit;

use spike_event_model::adapters::anthropic::AnthropicAdapter;
use spike_event_model::adapters::bedrock::{encode_jsonl_fixture, BedrockAdapter};
use spike_event_model::adapters::openai::OpenAiAdapter;
use spike_event_model::adapters::Adapter;
use spike_event_model::metering::Meter;

fn usage() -> ! {
    eprintln!(
        "usage: spike-event-model --provider <openai|anthropic|bedrock> --file <path> [--chunk-size N]"
    );
    exit(2);
}

fn main() {
    let mut provider: Option<String> = None;
    let mut file: Option<String> = None;
    let mut chunk_size: usize = 17;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--provider" => provider = args.next(),
            "--file" => file = args.next(),
            "--chunk-size" => {
                chunk_size = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n > 0)
                    .unwrap_or_else(|| usage())
            }
            _ => usage(),
        }
    }
    let provider = provider.unwrap_or_else(|| usage());
    let file = file.unwrap_or_else(|| usage());

    let (mut adapter, wire): (Box<dyn Adapter>, Vec<u8>) = match provider.as_str() {
        "openai" => (Box::new(OpenAiAdapter::new()), read(&file)),
        "anthropic" => (Box::new(AnthropicAdapter::new()), read(&file)),
        "bedrock" => {
            let jsonl = String::from_utf8(read(&file)).unwrap_or_else(|e| die(&e.to_string()));
            let wire = encode_jsonl_fixture(&jsonl).unwrap_or_else(|e| die(&e));
            (Box::new(BedrockAdapter::new()), wire)
        }
        _ => usage(),
    };

    let mut meter = Meter::new();
    for chunk in wire.chunks(chunk_size) {
        for event in adapter.feed(chunk) {
            meter.observe(&event);
            println!("{event:?}");
        }
    }
    println!();
    print!("{}", meter.report());
}

fn read(path: &str) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")))
}

fn die(message: &str) -> ! {
    eprintln!("error: {message}");
    exit(1);
}
