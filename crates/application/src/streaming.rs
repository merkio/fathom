//! Incremental decoders for streamed HTTP bodies, shared by every LLM adapter.
//!
//! Network chunks split lines (and even UTF-8 characters) anywhere, so both decoders buffer raw
//! bytes and only emit complete lines.

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
}
