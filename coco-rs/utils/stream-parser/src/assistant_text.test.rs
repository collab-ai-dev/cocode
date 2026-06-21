use super::AssistantTextStreamParser;
use crate::ProposedPlanSegment;
use pretty_assertions::assert_eq;

#[test]
fn parses_citations_across_seed_and_delta_boundaries() {
    let mut parser = AssistantTextStreamParser::new(false);

    let seeded = parser.push_str("hello <oai-mem-citation>doc");
    let parsed = parser.push_str("1</oai-mem-citation> world");
    let tail = parser.finish();

    assert_eq!(seeded.visible_text, "hello ");
    assert_eq!(seeded.citations, Vec::<String>::new());
    assert_eq!(parsed.visible_text, " world");
    assert_eq!(parsed.citations, vec!["doc1".to_string()]);
    assert_eq!(tail.visible_text, "");
    assert_eq!(tail.citations, Vec::<String>::new());
}

#[test]
fn parses_plan_segments_after_citation_stripping() {
    let mut parser = AssistantTextStreamParser::new(true);

    let seeded = parser.push_str("Intro\n<proposed");
    let parsed = parser.push_str("_plan>\n- step <oai-mem-citation>doc</oai-mem-citation>\n");
    let tail = parser.push_str("</proposed_plan>\nOutro");
    let finish = parser.finish();

    assert_eq!(seeded.visible_text, "Intro\n");
    assert_eq!(
        seeded.plan_segments,
        vec![ProposedPlanSegment::Normal("Intro\n".to_string())]
    );
    assert_eq!(parsed.visible_text, "");
    assert_eq!(parsed.citations, vec!["doc".to_string()]);
    assert_eq!(
        parsed.plan_segments,
        vec![
            ProposedPlanSegment::ProposedPlanStart,
            ProposedPlanSegment::ProposedPlanDelta("- step \n".to_string()),
        ]
    );
    assert_eq!(tail.visible_text, "Outro");
    assert_eq!(
        tail.plan_segments,
        vec![
            ProposedPlanSegment::ProposedPlanEnd,
            ProposedPlanSegment::Normal("Outro".to_string()),
        ]
    );
    assert!(finish.is_empty());
}
