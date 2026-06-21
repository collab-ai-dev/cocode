use super::InlineHiddenTagParser;
use super::InlineTagSpec;
use crate::StreamTextChunk;
use crate::StreamTextParser;
use pretty_assertions::assert_eq;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    A,
    B,
}

fn collect_chunks<P>(parser: &mut P, chunks: &[&str]) -> StreamTextChunk<P::Extracted>
where
    P: StreamTextParser,
{
    let mut all = StreamTextChunk::default();
    for chunk in chunks {
        let next = parser.push_str(chunk);
        all.visible_text.push_str(&next.visible_text);
        all.extracted.extend(next.extracted);
    }
    let tail = parser.finish();
    all.visible_text.push_str(&tail.visible_text);
    all.extracted.extend(tail.extracted);
    all
}

#[test]
fn generic_inline_parser_supports_multiple_tag_types() {
    let mut parser = InlineHiddenTagParser::new(vec![
        InlineTagSpec {
            tag: Tag::A,
            open: "<a>",
            close: "</a>",
        },
        InlineTagSpec {
            tag: Tag::B,
            open: "<b>",
            close: "</b>",
        },
    ]);

    let out = collect_chunks(&mut parser, &["1<a>x</a>2<b>y</b>3"]);

    assert_eq!(out.visible_text, "123");
    assert_eq!(out.extracted.len(), 2);
    assert_eq!(out.extracted[0].tag, Tag::A);
    assert_eq!(out.extracted[0].content, "x");
    assert_eq!(out.extracted[1].tag, Tag::B);
    assert_eq!(out.extracted[1].content, "y");
}

#[test]
fn generic_inline_parser_supports_non_ascii_tag_delimiters() {
    let mut parser = InlineHiddenTagParser::new(vec![InlineTagSpec {
        tag: Tag::A,
        open: "<é>",
        close: "</é>",
    }]);

    let out = collect_chunks(&mut parser, &["a<", "é>中</", "é>b"]);

    assert_eq!(out.visible_text, "ab");
    assert_eq!(out.extracted.len(), 1);
    assert_eq!(out.extracted[0].tag, Tag::A);
    assert_eq!(out.extracted[0].content, "中");
}

#[test]
fn generic_inline_parser_prefers_longest_opener_at_same_offset() {
    let mut parser = InlineHiddenTagParser::new(vec![
        InlineTagSpec {
            tag: Tag::A,
            open: "<a>",
            close: "</a>",
        },
        InlineTagSpec {
            tag: Tag::B,
            open: "<ab>",
            close: "</ab>",
        },
    ]);

    let out = collect_chunks(&mut parser, &["x<ab>y</ab>z"]);

    assert_eq!(out.visible_text, "xz");
    assert_eq!(out.extracted.len(), 1);
    assert_eq!(out.extracted[0].tag, Tag::B);
    assert_eq!(out.extracted[0].content, "y");
}

#[test]
#[should_panic(expected = "non-empty open delimiters")]
fn generic_inline_parser_rejects_empty_open_delimiter() {
    let _ = InlineHiddenTagParser::new(vec![InlineTagSpec {
        tag: Tag::A,
        open: "",
        close: "</a>",
    }]);
}

#[test]
#[should_panic(expected = "non-empty close delimiters")]
fn generic_inline_parser_rejects_empty_close_delimiter() {
    let _ = InlineHiddenTagParser::new(vec![InlineTagSpec {
        tag: Tag::A,
        open: "<a>",
        close: "",
    }]);
}
