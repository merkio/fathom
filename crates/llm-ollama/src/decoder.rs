//! Decodes Ollama's newline-delimited JSON chat stream into normalized chunks.
//!
//! Each line is a JSON object; text is in `message.content`; the last line has `done: true` and
//! the token counts (`prompt_eval_count`, `eval_count`). A line with `error` reports a failure.

use application::ports::{LlmChunk, LlmError, Usage};
use application::streaming::{LineDecoder, StreamDecoder};
use serde_json::Value;

pub(crate) struct OllamaDecoder {
    lines: LineDecoder,
    done: bool,
}

impl OllamaDecoder {
    pub(crate) fn new() -> Self {
        Self {
            lines: LineDecoder::new(),
            done: false,
        }
    }

    fn line(&mut self, line: &str) -> Vec<Result<LlmChunk, LlmError>> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::Malformed(format!("invalid line: {e}")))],
        };
        if let Some(error) = value.get("error") {
            let message = error
                .as_str()
                .map_or_else(|| error.to_string(), str::to_owned);
            return vec![Err(LlmError::Provider(message))];
        }
        let mut out = Vec::new();
        if let Some(text) = value.pointer("/message/content").and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push(Ok(LlmChunk::Text(text.to_owned())));
        }
        if value.get("done").and_then(Value::as_bool) == Some(true) {
            self.done = true;
            let n = |k: &str| {
                value
                    .get(k)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            if n("prompt_eval_count").is_some() || n("eval_count").is_some() {
                out.push(Ok(LlmChunk::Usage(Usage {
                    input_tokens: n("prompt_eval_count").unwrap_or(0),
                    output_tokens: n("eval_count").unwrap_or(0),
                })));
            }
            out.push(Ok(LlmChunk::Done));
        }
        out
    }
}

impl StreamDecoder for OllamaDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmChunk, LlmError>> {
        if self.done {
            return Vec::new();
        }
        let lines = self.lines.push(bytes);
        lines.iter().flat_map(|l| self.line(l)).collect()
    }

    fn finish(&mut self) -> Vec<Result<LlmChunk, LlmError>> {
        let mut out: Vec<_> = self
            .lines
            .finish()
            .iter()
            .flat_map(|l| self.line(l))
            .collect();
        if !out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_))) {
            out.push(Err(LlmError::Malformed(
                "stream ended before the final `done` line".into(),
            )));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Vec<Result<LlmChunk, LlmError>> {
        let mut d = OllamaDecoder::new();
        let mut out = d.push(input.as_bytes());
        if !out.iter().any(|i| matches!(i, Ok(LlmChunk::Done) | Err(_))) {
            out.extend(d.finish());
        }
        out
    }

    const FULL: &str = "\
{\"model\":\"llama3\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"done\":false}\n\
{\"model\":\"llama3\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\" world\"},\"done\":false}\n\
{\"model\":\"llama3\",\"created_at\":\"t\",\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done_reason\":\"stop\",\"done\":true,\"prompt_eval_count\":11,\"eval_count\":7}\n";

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
    fn missing_counts_and_missing_final_newline() {
        let input = "{\"message\":{\"content\":\"hi\"},\"done\":false}\n{\"done\":true}";
        assert_eq!(
            run(input),
            vec![Ok(LlmChunk::Text("hi".into())), Ok(LlmChunk::Done)]
        );
    }

    #[test]
    fn an_error_line_is_a_provider_error() {
        let out = run("{\"error\":\"model 'x' not found\"}\n");
        assert_eq!(
            out,
            vec![Err(LlmError::Provider("model 'x' not found".into()))]
        );
    }

    #[test]
    fn a_cut_stream_is_an_error() {
        let out = run("{\"message\":{\"content\":\"hi\"},\"done\":false}\n");
        assert!(
            matches!(out.last(), Some(Err(LlmError::Malformed(_)))),
            "{out:?}"
        );
    }

    #[test]
    fn garbage_is_malformed() {
        assert!(matches!(run("not json\n")[0], Err(LlmError::Malformed(_))));
    }

    #[test]
    fn bytes_split_anywhere_and_multibyte_text_survive() {
        let input = "{\"message\":{\"content\":\"wörld ✓\"},\"done\":false}\n{\"done\":true}\n";
        let mut d = OllamaDecoder::new();
        let mut out = Vec::new();
        for b in input.as_bytes() {
            out.extend(d.push(std::slice::from_ref(b)));
        }
        assert_eq!(
            out,
            vec![Ok(LlmChunk::Text("wörld ✓".into())), Ok(LlmChunk::Done)]
        );
    }
}
