//! Decodes the Messages API event stream into normalized chunks.
//!
//! Events used: `message_start` (input tokens), `content_block_delta` (text), `message_delta`
//! (output tokens), `message_stop` (end), `error`. Everything else (`ping`, block start/stop,
//! non-text deltas) is ignored.

use application::ports::{LlmChunk, LlmError, Usage};
use application::streaming::{SseDecoder, SseEvent, StreamDecoder};
use serde_json::Value;

pub(crate) struct AnthropicDecoder {
    sse: SseDecoder,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    stopped: bool,
}

impl AnthropicDecoder {
    pub(crate) fn new() -> Self {
        Self {
            sse: SseDecoder::new(),
            input_tokens: None,
            output_tokens: None,
            stopped: false,
        }
    }

    fn tokens(value: &Value, pointer: &str) -> Option<u32> {
        value
            .pointer(pointer)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    }

    fn event(&mut self, event: SseEvent) -> Vec<Result<LlmChunk, LlmError>> {
        let value: Value = match serde_json::from_str(&event.data) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::Malformed(format!("invalid event data: {e}")))],
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or("");
        match kind {
            "message_start" => {
                self.input_tokens = Self::tokens(&value, "/message/usage/input_tokens");
                self.output_tokens =
                    Self::tokens(&value, "/message/usage/output_tokens").or(self.output_tokens);
                Vec::new()
            }
            "content_block_delta" => {
                let is_text =
                    value.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta");
                match value.pointer("/delta/text").and_then(Value::as_str) {
                    Some(text) if is_text && !text.is_empty() => {
                        vec![Ok(LlmChunk::Text(text.to_owned()))]
                    }
                    _ => Vec::new(),
                }
            }
            "message_delta" => {
                self.output_tokens =
                    Self::tokens(&value, "/usage/output_tokens").or(self.output_tokens);
                Vec::new()
            }
            "message_stop" => {
                self.stopped = true;
                let mut out = Vec::new();
                if self.input_tokens.is_some() || self.output_tokens.is_some() {
                    out.push(Ok(LlmChunk::Usage(Usage {
                        input_tokens: self.input_tokens.unwrap_or(0),
                        output_tokens: self.output_tokens.unwrap_or(0),
                    })));
                }
                out.push(Ok(LlmChunk::Done));
                out
            }
            "error" => {
                let kind = value
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                vec![Err(match kind {
                    "authentication_error" | "permission_error" => LlmError::Authentication,
                    "rate_limit_error" => LlmError::RateLimited,
                    _ => LlmError::Provider(format!("{kind}: {message}")),
                })]
            }
            _ => Vec::new(),
        }
    }
}

impl StreamDecoder for AnthropicDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmChunk, LlmError>> {
        if self.stopped {
            return Vec::new();
        }
        let events = self.sse.push(bytes);
        events.into_iter().flat_map(|e| self.event(e)).collect()
    }

    fn finish(&mut self) -> Vec<Result<LlmChunk, LlmError>> {
        let events = self.sse.finish();
        let mut out: Vec<_> = events.into_iter().flat_map(|e| self.event(e)).collect();
        if !self.stopped && !out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_))) {
            out.push(Err(LlmError::Malformed(
                "stream ended before message_stop".into(),
            )));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Vec<Result<LlmChunk, LlmError>> {
        let mut d = AnthropicDecoder::new();
        let mut out = d.push(input.as_bytes());
        // Like the real driver: nothing follows a terminal item, and `finish` runs only when
        // the bytes end without one.
        if !out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_))) {
            out.extend(d.finish());
        }
        out
    }

    const FULL: &str = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: ping\n\
data: {\"type\":\"ping\"}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn full_reply() {
        assert_eq!(
            run(FULL),
            vec![
                Ok(LlmChunk::Text("Hello".into())),
                Ok(LlmChunk::Text(" world".into())),
                Ok(LlmChunk::Usage(Usage {
                    input_tokens: 11,
                    output_tokens: 7
                })),
                Ok(LlmChunk::Done),
            ]
        );
    }

    #[test]
    fn bytes_split_anywhere_decode_the_same() {
        let mut d = AnthropicDecoder::new();
        let mut out = Vec::new();
        for b in FULL.as_bytes() {
            out.extend(d.push(std::slice::from_ref(b)));
        }
        out.extend(d.finish());
        assert_eq!(out, run(FULL));
    }

    #[test]
    fn truncated_stream_is_an_error() {
        let cut = FULL
            .split("event: message_delta")
            .next()
            .unwrap_or_default();
        let out = run(cut);
        assert!(
            matches!(out.last(), Some(Err(LlmError::Malformed(_)))),
            "{out:?}"
        );
    }

    #[test]
    fn non_text_deltas_are_ignored() {
        let input = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
data: {\"type\":\"message_stop\"}\n\n";
        assert_eq!(run(input), vec![Ok(LlmChunk::Done)]);
    }

    #[test]
    fn error_events_map_to_error_kinds() {
        let ev = |t: &str| {
            format!(
                "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"{t}\",\"message\":\"m\"}}}}\n\n"
            )
        };
        assert_eq!(
            run(&ev("authentication_error")),
            vec![Err(LlmError::Authentication)]
        );
        assert_eq!(
            run(&ev("rate_limit_error")),
            vec![Err(LlmError::RateLimited)]
        );
        assert_eq!(
            run(&ev("overloaded_error")),
            vec![Err(LlmError::Provider("overloaded_error: m".into()))]
        );
    }

    #[test]
    fn invalid_json_is_malformed() {
        let out = run("data: {not json}\n\n");
        assert!(matches!(out[0], Err(LlmError::Malformed(_))));
    }

    #[test]
    fn missing_usage_still_completes() {
        let out = run("data: {\"type\":\"message_stop\"}\n\n");
        assert_eq!(out, vec![Ok(LlmChunk::Done)]);
    }
}
