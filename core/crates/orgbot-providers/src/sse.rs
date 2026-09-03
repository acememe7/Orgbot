//! Server-Sent Events framing.
//!
//! Both backends stream with SSE, and the bytes arrive split at arbitrary
//! points — a chunk boundary can land in the middle of a UTF-8 character, in
//! the middle of a JSON payload, or between the `data:` line and the blank line
//! that terminates the event. This decoder buffers until a complete event is
//! available, so callers only ever see whole payloads.

/// Incrementally reassembles SSE events from a byte stream.
#[derive(Debug, Default)]
pub struct SseDecoder {
    /// Bytes received but not yet forming a complete event.
    buffer: Vec<u8>,
    /// `data:` lines collected for the event currently being assembled.
    pending: Vec<String>,
    /// The `event:` name for the event being assembled, if the server sent one.
    event_name: Option<String>,
}

/// One fully-received SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, when present.
    pub name: Option<String>,
    /// The concatenated `data:` lines.
    pub data: String,
}

impl SseDecoder {
    /// A decoder with an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes; returns every event completed by it.
    ///
    /// Partial UTF-8 at the end of a chunk is retained rather than replaced
    /// with the substitution character, so multi-byte text streamed one byte at
    /// a time still decodes correctly.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        // Consume whole lines only; anything after the last newline stays.
        loop {
            let Some(newline) = self.buffer.iter().position(|&b| b == b'\n') else {
                break;
            };
            let mut line = self.buffer.drain(..=newline).collect::<Vec<u8>>();
            line.pop(); // the '\n'
            if line.last() == Some(&b'\r') {
                line.pop(); // tolerate CRLF
            }
            // A line is only ever emitted once complete, so it is valid UTF-8
            // unless the server sent malformed bytes; drop those rather than
            // failing the whole stream.
            let Ok(line) = String::from_utf8(line) else {
                continue;
            };

            if line.is_empty() {
                if let Some(event) = self.take_event() {
                    events.push(event);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                self.pending.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.event_name = Some(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // `id:`, `retry:`, and comment lines (`:`) are not used here.
        }
        events
    }

    /// Emit any event still buffered when the stream ends without a final
    /// blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let trailing = std::mem::take(&mut self.buffer);
            if let Ok(line) = String::from_utf8(trailing) {
                if let Some(rest) = line.strip_prefix("data:") {
                    self.pending
                        .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                }
            }
        }
        self.take_event()
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        if self.pending.is_empty() {
            self.event_name = None;
            return None;
        }
        Some(SseEvent {
            name: self.event_name.take(),
            data: std::mem::take(&mut self.pending).join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_event_arriving_at_once_is_decoded() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(
            events,
            vec![SseEvent {
                name: Some("message_start".into()),
                data: "{\"a\":1}".into(),
            }]
        );
    }

    #[test]
    fn an_event_split_across_chunks_is_reassembled() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"event: content_block").is_empty());
        assert!(decoder.push(b"_delta\ndata: {\"text\":\"hel").is_empty());
        let events = decoder.push(b"lo\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\"text\":\"hello\"}");
    }

    #[test]
    fn several_events_in_one_chunk_all_come_out_in_order() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: one\n\ndata: two\n\ndata: three\n\n");
        let payloads: Vec<&str> = events.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(payloads, ["one", "two", "three"]);
    }

    #[test]
    fn multiline_data_fields_are_joined_with_newlines() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: line one\ndata: line two\n\n");
        assert_eq!(events[0].data, "line one\nline two");
    }

    /// A chunk boundary inside a multi-byte character must not corrupt the text.
    #[test]
    fn a_split_multibyte_character_survives() {
        let mut decoder = SseDecoder::new();
        let payload = "data: 日本語\n\n".as_bytes().to_vec();
        let mut events = Vec::new();
        // Feed one byte at a time — the worst case for UTF-8 boundaries.
        for byte in &payload {
            events.extend(decoder.push(&[*byte]));
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "日本語");
    }

    #[test]
    fn crlf_line_endings_are_tolerated() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event: ping\r\ndata: {}\r\n\r\n");
        assert_eq!(events[0].name.as_deref(), Some("ping"));
        assert_eq!(events[0].data, "{}");
    }

    #[test]
    fn a_missing_space_after_the_colon_is_accepted() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data:{\"tight\":true}\n\n");
        assert_eq!(events[0].data, "{\"tight\":true}");
    }

    #[test]
    fn comment_and_id_lines_are_ignored() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b": keep-alive\nid: 42\nretry: 100\ndata: real\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "real");
    }

    #[test]
    fn a_stream_ending_without_a_blank_line_still_yields_its_last_event() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: trailing").is_empty());
        assert_eq!(decoder.finish().unwrap().data, "trailing");
    }

    #[test]
    fn finish_on_a_drained_decoder_yields_nothing() {
        let mut decoder = SseDecoder::new();
        decoder.push(b"data: done\n\n");
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn the_event_name_does_not_leak_into_the_next_event() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event: first\ndata: a\n\ndata: b\n\n");
        assert_eq!(events[0].name.as_deref(), Some("first"));
        assert_eq!(events[1].name, None, "the name must not carry over");
    }
}
