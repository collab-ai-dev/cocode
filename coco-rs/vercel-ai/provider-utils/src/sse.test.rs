use super::*;
use pretty_assertions::assert_eq;

fn drain(dec: &mut SseDecoder) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(line) = dec.next_data_line() {
        out.push(line);
    }
    out
}

#[test]
fn parses_data_lines_and_skips_done() {
    let mut dec = SseDecoder::new();
    dec.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\ndata: [DONE]\n\n");
    assert_eq!(drain(&mut dec), vec!["{\"a\":1}", "{\"b\":2}"]);
}

#[test]
fn accepts_data_without_space_and_trims_cr() {
    let mut dec = SseDecoder::new();
    dec.push(b"data:{\"a\":1}\r\n");
    assert_eq!(drain(&mut dec), vec!["{\"a\":1}"]);
}

#[test]
fn skips_non_data_lines() {
    let mut dec = SseDecoder::new();
    dec.push(b": comment\nevent: ping\ndata: {\"ok\":true}\n");
    assert_eq!(drain(&mut dec), vec!["{\"ok\":true}"]);
}

#[test]
fn buffers_partial_line_until_newline() {
    let mut dec = SseDecoder::new();
    dec.push(b"data: {\"a\":");
    assert!(dec.next_data_line().is_none()); // no newline yet
    dec.push(b"1}\n");
    assert_eq!(drain(&mut dec), vec!["{\"a\":1}"]);
}

#[test]
fn multibyte_char_split_across_chunks_is_not_corrupted() {
    // "世界" — each char is 3 UTF-8 bytes. Split the first char across two
    // pushes, mid-sequence, to reproduce a network chunk boundary.
    let full = "data: {\"t\":\"世界\"}\n".as_bytes().to_vec();
    let split_at = full.iter().position(|&b| b == 0xE4).unwrap() + 1; // mid "世"
    let mut dec = SseDecoder::new();
    dec.push(&full[..split_at]);
    assert!(dec.next_data_line().is_none());
    dec.push(&full[split_at..]);
    assert_eq!(drain(&mut dec), vec!["{\"t\":\"世界\"}"]);
}

#[test]
fn emoji_split_across_three_chunks() {
    // "🚀" is 4 UTF-8 bytes; feed one byte at a time.
    let full = "data: \"🚀\"\n".as_bytes().to_vec();
    let mut dec = SseDecoder::new();
    for b in &full[..full.len() - 1] {
        dec.push(&[*b]);
        assert!(dec.next_data_line().is_none());
    }
    dec.push(&[*full.last().unwrap()]);
    assert_eq!(drain(&mut dec), vec!["\"🚀\""]);
}
