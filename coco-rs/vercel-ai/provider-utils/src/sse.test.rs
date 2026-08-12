use super::*;
use pretty_assertions::assert_eq;

fn drain(decoder: &mut SseDecoder) -> Vec<SseEvent> {
    let mut events = Vec::new();
    while let Some(event) = decoder.next_event().expect("decode event") {
        events.push(event);
    }
    events
}

#[test]
fn test_sse_decoder_frames_events_and_preserves_event_type() {
    let mut decoder = SseDecoder::new();
    decoder
        .push(b": comment\nevent: message\ndata: {\"a\":1}\n\ndata:{\"b\":2}\r\n\r\n")
        .expect("push");

    assert_eq!(
        drain(&mut decoder),
        vec![
            SseEvent {
                event: Some("message".into()),
                data: "{\"a\":1}".into(),
            },
            SseEvent {
                event: None,
                data: "{\"b\":2}".into(),
            },
        ]
    );
}

#[test]
fn test_sse_decoder_joins_multiline_data() {
    let mut decoder = SseDecoder::new();
    decoder
        .push(b"event: content\ndata: first\ndata: second\n\n")
        .expect("push");

    assert_eq!(
        drain(&mut decoder),
        vec![SseEvent {
            event: Some("content".into()),
            data: "first\nsecond".into(),
        }]
    );
}

#[test]
fn test_sse_decoder_preserves_utf8_at_every_chunk_boundary() {
    let full = "data: {\"text\":\"世界🚀\"}\n\n".as_bytes();
    for split_at in 0..=full.len() {
        let mut decoder = SseDecoder::new();
        decoder.push(&full[..split_at]).expect("first chunk");
        let mut events = drain(&mut decoder);
        decoder.push(&full[split_at..]).expect("second chunk");
        events.extend(drain(&mut decoder));
        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "{\"text\":\"世界🚀\"}".into(),
            }],
            "split at byte {split_at}"
        );
    }
}

#[test]
fn test_sse_decoder_rejects_invalid_utf8() {
    let mut decoder = SseDecoder::new();
    decoder.push(b"data: \xff\n\n").expect("push");
    assert!(matches!(
        decoder.next_event(),
        Err(SseDecodeError::InvalidUtf8 { .. })
    ));
}

#[test]
fn test_sse_decoder_rejects_oversized_unterminated_event() {
    let mut decoder = SseDecoder::with_max_event_bytes(8);
    let error = decoder
        .push(b"data: 123")
        .expect_err("unterminated event must be bounded");
    assert!(matches!(error, SseDecodeError::EventTooLarge { limit: 8 }));
}

#[test]
fn test_sse_decoder_rejects_oversized_complete_event() {
    let mut decoder = SseDecoder::with_max_event_bytes(8);
    let error = decoder
        .push(b"data: 123\n\n")
        .expect_err("complete buffered event must be bounded");
    assert!(matches!(error, SseDecodeError::EventTooLarge { limit: 8 }));
}

#[test]
fn test_sse_decoder_flushes_event_at_eof() {
    let mut decoder = SseDecoder::new();
    decoder.push(b"event: done\ndata: final").expect("push");
    assert_eq!(
        decoder.finish().expect("finish"),
        Some(SseEvent {
            event: Some("done".into()),
            data: "final".into(),
        })
    );
}
