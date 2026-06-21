use super::Utf8StreamParser;
use super::Utf8StreamParserError;
use crate::CitationStreamParser;
use crate::StreamTextChunk;
use crate::StreamTextParser;

use pretty_assertions::assert_eq;

fn collect_bytes(
    parser: &mut Utf8StreamParser<CitationStreamParser>,
    chunks: &[&[u8]],
) -> Result<StreamTextChunk<String>, Utf8StreamParserError> {
    let mut all = StreamTextChunk::default();
    for chunk in chunks {
        let next = parser.push_bytes(chunk)?;
        all.visible_text.push_str(&next.visible_text);
        all.extracted.extend(next.extracted);
    }
    let tail = parser.finish()?;
    all.visible_text.push_str(&tail.visible_text);
    all.extracted.extend(tail.extracted);
    Ok(all)
}

#[test]
fn utf8_stream_parser_handles_split_code_points_across_chunks() {
    let chunks: [&[u8]; 3] = [
        b"A\xC3",
        b"\xA9<oai-mem-citation>\xE4",
        b"\xB8\xAD</oai-mem-citation>Z",
    ];

    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());
    let out = match collect_bytes(&mut parser, &chunks) {
        Ok(out) => out,
        Err(err) => panic!("valid UTF-8 stream should parse: {err}"),
    };

    assert_eq!(out.visible_text, "AéZ");
    assert_eq!(out.extracted, vec!["中".to_string()]);
}

#[test]
fn utf8_stream_parser_rolls_back_on_invalid_utf8_chunk() {
    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());

    let first = match parser.push_bytes(&[0xC3]) {
        Ok(out) => out,
        Err(err) => panic!("leading byte may be buffered until next chunk: {err}"),
    };
    assert!(first.is_empty());

    let err = match parser.push_bytes(&[0x28]) {
        Ok(out) => panic!("invalid continuation byte should error, got output: {out:?}"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        Utf8StreamParserError::InvalidUtf8 {
            valid_up_to: 0,
            error_len: 1,
        }
    );

    let second = match parser.push_bytes(&[0xA9, b'x']) {
        Ok(out) => out,
        Err(err) => panic!("state should still allow a valid continuation: {err}"),
    };
    let tail = match parser.finish() {
        Ok(out) => out,
        Err(err) => panic!("stream should finish: {err}"),
    };

    assert_eq!(second.visible_text, "éx");
    assert!(second.extracted.is_empty());
    assert!(tail.is_empty());
}

#[test]
fn utf8_stream_parser_rolls_back_entire_chunk_when_invalid_byte_follows_valid_prefix() {
    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());

    let err = match parser.push_bytes(b"ok\xFF") {
        Ok(out) => panic!("invalid byte should error, got output: {out:?}"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        Utf8StreamParserError::InvalidUtf8 {
            valid_up_to: 2,
            error_len: 1,
        }
    );

    let next = match parser.push_bytes(b"!") {
        Ok(out) => out,
        Err(err) => panic!("parser should recover after rollback: {err}"),
    };

    assert_eq!(next.visible_text, "!");
    assert!(next.extracted.is_empty());
}

#[test]
fn utf8_stream_parser_errors_on_incomplete_code_point_at_eof() {
    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());

    let out = match parser.push_bytes(&[0xE2, 0x82]) {
        Ok(out) => out,
        Err(err) => panic!("partial code point should be buffered: {err}"),
    };
    assert!(out.is_empty());

    let err = match parser.finish() {
        Ok(out) => panic!("unfinished code point should error, got output: {out:?}"),
        Err(err) => err,
    };
    assert_eq!(err, Utf8StreamParserError::IncompleteUtf8AtEof);
}

#[test]
fn utf8_stream_parser_into_inner_errors_when_partial_code_point_is_buffered() {
    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());

    let out = match parser.push_bytes(&[0xC3]) {
        Ok(out) => out,
        Err(err) => panic!("partial code point should be buffered: {err}"),
    };
    assert!(out.is_empty());

    let err = match parser.into_inner() {
        Ok(_) => panic!("buffered partial code point should be rejected"),
        Err(err) => err,
    };
    assert_eq!(err, Utf8StreamParserError::IncompleteUtf8AtEof);
}

#[test]
fn utf8_stream_parser_into_inner_lossy_drops_buffered_partial_code_point() {
    let mut parser = Utf8StreamParser::new(CitationStreamParser::new());

    let out = match parser.push_bytes(&[0xC3]) {
        Ok(out) => out,
        Err(err) => panic!("partial code point should be buffered: {err}"),
    };
    assert!(out.is_empty());

    let mut inner = parser.into_inner_lossy();
    let tail = inner.finish();
    assert!(tail.is_empty());
}
