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
