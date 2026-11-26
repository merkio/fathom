//! Incremental decoders for streamed HTTP bodies, shared by every LLM adapter.
//!
//! Network chunks split lines (and even UTF-8 characters) anywhere, so both decoders buffer raw
//! bytes and only emit complete lines.

use std::collections::VecDeque;
use std::pin::Pin;

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};

use crate::ports::{LlmChunk, LlmError};

/// Splits a byte stream into lines (`\n` or `\r\n`).
#[derive(Debug, Default)]
pub struct LineDecoder {
    buffer: Vec<u8>,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes and returns every line completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }

    /// A final line without a trailing newline, if any.
    pub fn finish(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let line = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        Some(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, if the server sent one.
    pub event: Option<String>,
    /// The `data:` lines joined with `\n`.
    pub data: String,
}

/// Decodes Server-Sent Events.
#[derive(Debug, Default)]
pub struct SseDecoder {
    lines: LineDecoder,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let lines = self.lines.push(bytes);
        self.handle(lines)
    }

    /// Flushes an event the server left unterminated at the end of the stream.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut lines: Vec<String> = self.lines.finish().into_iter().collect();
        lines.push(String::new());
        self.handle(lines)
    }

    fn handle(&mut self, lines: Vec<String>) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for line in lines {
            if line.is_empty() {
                if self.event.is_some() || !self.data.is_empty() {
                    events.push(SseEvent {
                        event: self.event.take(),
                        data: std::mem::take(&mut self.data).join("\n"),
                    });
                }
            } else if line.starts_with(':') {
                // comment / keep-alive
            } else {
                let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
                let value = value.strip_prefix(' ').unwrap_or(value);
                match field {
                    "event" => self.event = Some(value.to_owned()),
                    "data" => self.data.push(value.to_owned()),
                    _ => {}
                }
            }
        }
        events
    }
}

/// Wire-format decoder of one provider: turns response bytes into normalized chunks.
///
/// Implementations are pure state machines, so they are unit-tested without any HTTP.
pub trait StreamDecoder: Send {
    /// Feeds bytes; returns the chunks (or errors) they complete.
    fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmChunk, LlmError>>;

    /// The byte stream ended. A decoder that has not seen the provider's end marker must return
    /// an error here, so a dropped connection is never mistaken for a complete reply.
    fn finish(&mut self) -> Vec<Result<LlmChunk, LlmError>>;
}

/// Drives a byte stream through a decoder. The stream ends after `Done` or the first error, and
/// stops reading the connection at that point. Dropping it closes the connection, which is how
/// requests are cancelled.
pub fn llm_stream<B, T, D>(bytes: B, decoder: D) -> BoxStream<'static, Result<LlmChunk, LlmError>>
where
    B: Stream<Item = Result<T, LlmError>> + Send + 'static,
    T: AsRef<[u8]> + Send + 'static,
    D: StreamDecoder + 'static,
{
    struct State<B, D> {
        bytes: Pin<Box<B>>,
        decoder: D,
        queue: VecDeque<Result<LlmChunk, LlmError>>,
        finished: bool,
    }
    let state = State {
        bytes: Box::pin(bytes),
        decoder,
        queue: VecDeque::new(),
        finished: false,
    };

    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                if matches!(item, Ok(LlmChunk::Done) | Err(_)) {
                    st.queue.clear();
                    st.finished = true;
                }
                return Some((item, st));
            }
            if st.finished {
                return None;
            }
            match st.bytes.next().await {
                Some(Ok(chunk)) => st.queue.extend(st.decoder.push(chunk.as_ref())),
                Some(Err(e)) => st.queue.push_back(Err(e)),
                None => {
                    st.queue.extend(st.decoder.finish());
                    // Whatever `finish` produced is the last word.
                    st.queue
                        .push_back(Err(LlmError::Malformed("stream ended unexpectedly".into())));
                    if st.queue.iter().any(|i| matches!(i, Ok(LlmChunk::Done))) {
                        st.queue.pop_back();
                    }
                }
            }
        }
    })
    .boxed()
}

/// Maps an HTTP error status to an `LlmError`. `message` is the provider's own error text.
pub fn error_for_status(status: u16, message: Option<&str>) -> LlmError {
    match status {
        401 | 403 => LlmError::Authentication,
        429 => LlmError::RateLimited,
        _ => {
            let detail = message
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .unwrap_or("no details");
            let detail: String = detail.chars().take(300).collect();
            LlmError::Provider(format!("HTTP {status}: {detail}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_survive_arbitrary_chunk_boundaries() {
        let mut d = LineDecoder::new();
        assert!(d.push(b"ab").is_empty());
        assert_eq!(d.push(b"c\r\nde"), ["abc"]);
        assert_eq!(d.push(b"f\n\nlast"), ["def", ""]);
        assert_eq!(d.finish().as_deref(), Some("last"));
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn multibyte_characters_split_across_chunks_are_not_corrupted() {
        let bytes = "héllo ✓\n".as_bytes();
        let mut d = LineDecoder::new();
        let mut out = Vec::new();
        for b in bytes {
            out.extend(d.push(std::slice::from_ref(b)));
        }
        assert_eq!(out, ["héllo ✓"]);
    }

    #[test]
    fn sse_events_with_names_comments_and_multiple_data_lines() {
        let mut d = SseDecoder::new();
        let input = ": ping\n\nevent: message_start\ndata: {\"a\":1}\n\ndata: one\ndata: two\n\n";
        let events = d.push(input.as_bytes());
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: Some("message_start".into()),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: None,
                    data: "one\ntwo".into()
                },
            ]
        );
    }

    #[test]
    fn sse_event_split_over_many_chunks() {
        let mut d = SseDecoder::new();
        let mut events = Vec::new();
        for chunk in ["da", "ta: hel", "lo\n", "\n", "data: [DONE]", "\n\n"] {
            events.extend(d.push(chunk.as_bytes()));
        }
        assert_eq!(
            events.iter().map(|e| e.data.as_str()).collect::<Vec<_>>(),
            ["hello", "[DONE]"]
        );
    }

    #[test]
    fn unterminated_final_event_is_flushed_on_finish() {
        let mut d = SseDecoder::new();
        assert!(d.push(b"data: tail").is_empty());
        assert_eq!(
            d.finish(),
            vec![SseEvent {
                event: None,
                data: "tail".into()
            }]
        );
    }

    #[test]
    fn colon_without_space_and_unknown_fields() {
        let mut d = SseDecoder::new();
        let events = d.push(b"data:x\nid: 7\nretry: 5\n\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "x".into()
            }]
        );
    }

    struct Echo {
        done: bool,
    }

    impl StreamDecoder for Echo {
        fn push(&mut self, bytes: &[u8]) -> Vec<Result<LlmChunk, LlmError>> {
            let text = String::from_utf8_lossy(bytes).into_owned();
            match text.as_str() {
                "END" => {
                    self.done = true;
                    vec![Ok(LlmChunk::Done)]
                }
                "BAD" => vec![Err(LlmError::Provider("bad".into()))],
                other => vec![Ok(LlmChunk::Text(other.to_owned()))],
            }
        }

        fn finish(&mut self) -> Vec<Result<LlmChunk, LlmError>> {
            Vec::new()
        }
    }

    fn collect(chunks: Vec<Result<&'static str, LlmError>>) -> Vec<Result<LlmChunk, LlmError>> {
        let bytes = futures::stream::iter(chunks);
        futures::executor::block_on(llm_stream(bytes, Echo { done: false }).collect())
    }

    #[test]
    fn driver_stops_after_done_and_ignores_the_rest() {
        let out = collect(vec![Ok("a"), Ok("b"), Ok("END"), Ok("late")]);
        assert_eq!(
            out,
            vec![
                Ok(LlmChunk::Text("a".into())),
                Ok(LlmChunk::Text("b".into())),
                Ok(LlmChunk::Done)
            ]
        );
    }

    #[test]
    fn driver_stops_after_the_first_error() {
        let out = collect(vec![Ok("a"), Ok("BAD"), Ok("b")]);
        assert_eq!(
            out,
            vec![
                Ok(LlmChunk::Text("a".into())),
                Err(LlmError::Provider("bad".into()))
            ]
        );
    }

    #[test]
    fn a_connection_that_ends_early_is_an_error_not_a_silent_success() {
        let out = collect(vec![Ok("a")]);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[1], Err(LlmError::Malformed(_))));
    }

    #[test]
    fn transport_errors_are_passed_through() {
        let out = collect(vec![Ok("a"), Err(LlmError::Network("reset".into()))]);
        assert_eq!(out[1], Err(LlmError::Network("reset".into())));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn status_mapping() {
        assert_eq!(error_for_status(401, None), LlmError::Authentication);
        assert_eq!(error_for_status(403, Some("x")), LlmError::Authentication);
        assert_eq!(error_for_status(429, None), LlmError::RateLimited);
        assert_eq!(
            error_for_status(500, Some("  boom  ")),
            LlmError::Provider("HTTP 500: boom".into())
        );
        assert_eq!(
            error_for_status(502, None),
            LlmError::Provider("HTTP 502: no details".into())
        );
        let long = "x".repeat(1000);
        let LlmError::Provider(m) = error_for_status(500, Some(&long)) else {
            unreachable!()
        };
        assert!(m.len() < 330);
    }
}
