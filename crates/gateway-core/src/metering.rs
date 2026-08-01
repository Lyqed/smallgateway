//! Streaming token metering: an incremental estimate accumulated from
//! deltas, reconciled against the provider's terminal usage frame. The gap
//! between the two is the published error bound (design question Q3).
//!
//! Promoted unchanged from `spikes/event-model/src/metering.rs` (Phase 0,
//! Spike A); the measured bound lives in `spikes/event-model/README.md`.

use crate::event::Event;

/// Chars-per-token heuristic for the live estimate. Deliberately crude — the
/// spike measures how crude, and the reconciled number is always the
/// provider's own.
const CHARS_PER_TOKEN: f64 = 4.0;

#[derive(Debug, Default)]
pub struct Meter {
    output_chars: u64,
    authoritative_input: Option<u64>,
    authoritative_output: Option<u64>,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::ContentDelta { text, .. } => {
                self.output_chars += text.chars().count() as u64;
            }
            Event::ToolCallDelta {
                arguments_delta, ..
            } => {
                self.output_chars += arguments_delta.chars().count() as u64;
            }
            Event::UsageDelta {
                input_tokens,
                output_tokens,
            } => {
                if input_tokens.is_some() {
                    self.authoritative_input = *input_tokens;
                }
                if output_tokens.is_some() {
                    self.authoritative_output = *output_tokens;
                }
            }
            _ => {}
        }
    }

    /// The live estimate available mid-stream, before any usage frame lands.
    /// This is what mid-stream enforcement meters against.
    pub fn estimated_output_tokens(&self) -> u64 {
        (self.output_chars as f64 / CHARS_PER_TOKEN).ceil() as u64
    }

    pub fn report(&self) -> MeterReport {
        let estimated = self.estimated_output_tokens();
        let error_pct = self.authoritative_output.map(|auth| {
            if auth == 0 {
                0.0
            } else {
                (estimated as f64 - auth as f64) / auth as f64 * 100.0
            }
        });
        MeterReport {
            estimated_output_tokens: estimated,
            authoritative_input_tokens: self.authoritative_input,
            authoritative_output_tokens: self.authoritative_output,
            error_pct,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct MeterReport {
    pub estimated_output_tokens: u64,
    pub authoritative_input_tokens: Option<u64>,
    pub authoritative_output_tokens: Option<u64>,
    /// Signed relative error of the estimate vs the terminal frame, percent.
    pub error_pct: Option<f64>,
}

impl std::fmt::Display for MeterReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "--- metering report ---")?;
        writeln!(
            f,
            "estimated output tokens (chars/{CHARS_PER_TOKEN}): {}",
            self.estimated_output_tokens
        )?;
        match self.authoritative_input_tokens {
            Some(n) => writeln!(f, "authoritative input tokens:  {n}")?,
            None => writeln!(f, "authoritative input tokens:  (no usage frame)")?,
        }
        match self.authoritative_output_tokens {
            Some(n) => writeln!(f, "authoritative output tokens: {n}")?,
            None => writeln!(f, "authoritative output tokens: (no usage frame)")?,
        }
        match self.error_pct {
            Some(pct) => writeln!(f, "estimate error vs terminal frame: {pct:+.1}%"),
            None => writeln!(f, "estimate error vs terminal frame: n/a"),
        }
    }
}
