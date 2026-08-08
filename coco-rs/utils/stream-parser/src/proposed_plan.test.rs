use super::ProposedPlanParser;
use super::ProposedPlanSegment;
use super::extract_proposed_plan_text;
use super::strip_proposed_plan_blocks;
use crate::StreamTextChunk;
use crate::StreamTextParser;
use pretty_assertions::assert_eq;

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

/// Segment lists differ by chunking only in how runs are split
/// (`Delta("ab")` vs `Delta("a") + Delta("b")`); coalesce adjacent runs and
/// drop empties so the comparison is over semantic content.
fn coalesced(segments: Vec<ProposedPlanSegment>) -> Vec<ProposedPlanSegment> {
    let mut out: Vec<ProposedPlanSegment> = Vec::new();
    for seg in segments {
        match (out.last_mut(), seg) {
            (Some(ProposedPlanSegment::Normal(acc)), ProposedPlanSegment::Normal(next)) => {
                acc.push_str(&next);
            }
            (
                Some(ProposedPlanSegment::ProposedPlanDelta(acc)),
                ProposedPlanSegment::ProposedPlanDelta(next),
            ) => acc.push_str(&next),
            (_, seg) => out.push(seg),
        }
    }
    out.retain(|seg| match seg {
        ProposedPlanSegment::Normal(text) | ProposedPlanSegment::ProposedPlanDelta(text) => {
            !text.is_empty()
        }
        ProposedPlanSegment::ProposedPlanStart | ProposedPlanSegment::ProposedPlanEnd => true,
    });
    out
}

/// Differential invariant: any chunking must yield the same visible text and
/// the same coalesced segment sequence as feeding the input whole — including
/// tags split mid-token, tag-adjacent indentation, and multi-byte chars on
/// the boundary.
#[test]
fn proposed_plan_parser_is_chunking_invariant() {
    let input = "开场白\n<proposed_plan>\n- 步骤一\n- step 2\n</proposed_plan>\n结尾\n\
                 <proposed_plan> not a tag line\n";
    let whole = collect_chunks(&mut ProposedPlanParser::new(), &[input]);
    let chars: Vec<char> = input.chars().collect();
    for size in [1usize, 2, 3, 7] {
        let chunks: Vec<String> = chars.chunks(size).map(|c| c.iter().collect()).collect();
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let out = collect_chunks(&mut ProposedPlanParser::new(), &refs);
        assert_eq!(
            out.visible_text, whole.visible_text,
            "chunked-by-{size}-chars visible text diverged"
        );
        assert_eq!(
            coalesced(out.extracted),
            coalesced(whole.extracted.clone()),
            "chunked-by-{size}-chars segments diverged"
        );
    }
}

#[test]
fn streams_proposed_plan_segments_and_visible_text() {
    let mut parser = ProposedPlanParser::new();
    let out = collect_chunks(
        &mut parser,
        &[
            "Intro text\n<prop",
            "osed_plan>\n- step 1\n",
            "</proposed_plan>\nOutro",
        ],
    );

    assert_eq!(out.visible_text, "Intro text\nOutro");
    assert_eq!(
        out.extracted,
        vec![
            ProposedPlanSegment::Normal("Intro text\n".to_string()),
            ProposedPlanSegment::ProposedPlanStart,
            ProposedPlanSegment::ProposedPlanDelta("- step 1\n".to_string()),
            ProposedPlanSegment::ProposedPlanEnd,
            ProposedPlanSegment::Normal("Outro".to_string()),
        ]
    );
}

#[test]
fn preserves_non_tag_lines() {
    let mut parser = ProposedPlanParser::new();
    let out = collect_chunks(&mut parser, &["  <proposed_plan> extra\n"]);

    assert_eq!(out.visible_text, "  <proposed_plan> extra\n");
    assert_eq!(
        out.extracted,
        vec![ProposedPlanSegment::Normal(
            "  <proposed_plan> extra\n".to_string()
        )]
    );
}

#[test]
fn closes_unterminated_plan_block_on_finish() {
    let mut parser = ProposedPlanParser::new();
    let out = collect_chunks(&mut parser, &["<proposed_plan>\n- step 1\n"]);

    assert_eq!(out.visible_text, "");
    assert_eq!(
        out.extracted,
        vec![
            ProposedPlanSegment::ProposedPlanStart,
            ProposedPlanSegment::ProposedPlanDelta("- step 1\n".to_string()),
            ProposedPlanSegment::ProposedPlanEnd,
        ]
    );
}

#[test]
fn strips_proposed_plan_blocks_from_text() {
    let text = "before\n<proposed_plan>\n- step\n</proposed_plan>\nafter";
    assert_eq!(strip_proposed_plan_blocks(text), "before\nafter");
}

#[test]
fn extracts_proposed_plan_text() {
    let text = "before\n<proposed_plan>\n- step\n</proposed_plan>\nafter";
    assert_eq!(
        extract_proposed_plan_text(text),
        Some("- step\n".to_string())
    );
}
