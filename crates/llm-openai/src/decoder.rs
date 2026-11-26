//! Decodes Chat Completions streaming chunks into normalized chunks.
//!
//! Each SSE `data:` line is a JSON chunk (text in `choices[0].delta.content`); the final chunk
//! may carry `usage`; `data: [DONE]` ends the stream. Some compatible servers omit `[DONE]`, so a
//! chunk with a `finish_reason` also counts as the end when the connection closes.

use application::ports::{LlmChunk, LlmError, Usage};
use application::streaming::{SseDecoder, SseEvent, StreamDecoder};
use serde_json::Value;

pub(crate) struct OpenAiDecoder {
    sse: SseDecoder,
    usage: Option<Usage>,
    saw_finish_reason: bool,
    done: bool,
}

impl OpenAiDecoder {
    pub(crate) fn new() -> Self {
        Self {
            sse: SseDecoder::new(),
            usage: None,
            saw_finish_reason: false,
            done: false,
        }
    }

    fn complete(&mut self) -> Vec<Result<LlmChunk, LlmError>> {
        self.done = true;
        let mut out = Vec::new();
        if let Some(u) = self.usage {
            out.push(Ok(LlmChunk::Usage(u)));
        }
        out.push(Ok(LlmChunk::Done));
        out
    }

    fn event(&mut self, event: SseEvent) -> Vec<Result<LlmChunk, LlmError>> {
        let data = event.data.trim();
        if data == "[DONE]" {
            return self.complete();
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::Malformed(format!("invalid chunk: {e}")))],
        };
        if let Some(error) = value.get("error") {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("");
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .or_else(|| error.get("type").and_then(Value::as_str))
                .unwrap_or("");
            return vec![Err(match code {
                "invalid_api_key" | "authentication_error" => LlmError::Authentication,
                "rate_limit_exceeded" | "rate_limit_error" => LlmError::RateLimited,
                _ => LlmError::Provider(format!("{code}: {message}")),
            })];
        }
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            let n = |k: &str| {
                usage
                    .get(k)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            self.usage = Some(Usage {
                input_tokens: n("prompt_tokens").unwrap_or(0),
                output_tokens: n("completion_tokens").unwrap_or(0),
            });
        }
        let Some(choice) = value.pointer("/choices/0") else {
            return Vec::new();
        };
        if choice.get("finish_reason").is_some_and(|f| !f.is_null()) {
            self.saw_finish_reason = true;
        }
        match choice.pointer("/delta/content").and_then(Value::as_str) {
            Some(text) if !text.is_empty() => vec![Ok(LlmChunk::Text(text.to_owned()))],
            _ => Vec::new(),
        }
    }
}

impl StreamDecoder for OpenAiDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmChunk, LlmError>> {
        if self.done {
            return Vec::new();
        }
        let events = self.sse.push(bytes);
        events.into_iter().flat_map(|e| self.event(e)).collect()
    }

    fn finish(&mut self) -> Vec<Result<LlmChunk, LlmError>> {
        let events = self.sse.finish();
        let mut out: Vec<_> = events.into_iter().flat_map(|e| self.event(e)).collect();
        let terminal = out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_)));
        if !terminal {
            if self.saw_finish_reason {
                out.extend(self.complete());
            } else {
                out.push(Err(LlmError::Malformed(
                    "stream ended before [DONE]".into(),
                )));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Vec<Result<LlmChunk, LlmError>> {
        let mut d = OpenAiDecoder::new();
        let mut out = d.push(input.as_bytes());
        if !out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_))) {
            out.extend(d.finish());
        }
        out
    }

    const FULL: &str = "\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"id\":\"c1\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}\n\n\
data: [DONE]\n\n";

    #[test]
    fn full_reply_with_usage() {
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
    fn replies_without_usage_still_complete() {
        let input = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";
        assert_eq!(
            run(input),
            vec![Ok(LlmChunk::Text("hi".into())), Ok(LlmChunk::Done)]
        );
    }

    #[test]
    fn servers_that_omit_done_are_accepted_after_a_finish_reason() {
        let input = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
        assert_eq!(
            run(input),
            vec![Ok(LlmChunk::Text("hi".into())), Ok(LlmChunk::Done)]
        );
    }

    #[test]
    fn a_cut_stream_is_an_error() {
        let input = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        let out = run(input);
        assert!(
            matches!(out.last(), Some(Err(LlmError::Malformed(_)))),
            "{out:?}"
        );
    }

    #[test]
    fn error_chunks_map_to_error_kinds() {
        let e = |code: &str| {
            format!("data: {{\"error\":{{\"message\":\"m\",\"code\":\"{code}\"}}}}\n\n")
        };
        assert_eq!(
            run(&e("invalid_api_key")),
            vec![Err(LlmError::Authentication)]
        );
        assert_eq!(
            run(&e("rate_limit_exceeded")),
            vec![Err(LlmError::RateLimited)]
        );
        assert_eq!(
            run(&e("server_error")),
            vec![Err(LlmError::Provider("server_error: m".into()))]
        );
    }

    #[test]
    fn non_json_data_is_malformed() {
        assert!(matches!(
            run("data: nope\n\n")[0],
            Err(LlmError::Malformed(_))
        ));
    }

    #[test]
    fn bytes_split_anywhere_decode_the_same() {
        let mut d = OpenAiDecoder::new();
        let mut out = Vec::new();
        for b in FULL.as_bytes() {
            out.extend(d.push(std::slice::from_ref(b)));
        }
        assert_eq!(out, run(FULL));
    }
}
