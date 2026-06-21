# coco-utils-stream-parser

Incremental parsers that process chunked LLM output into structured segments.

## Key Types

| Parser | Purpose |
|--------|---------|
| `StreamTextParser` / `StreamTextChunk` | Generic chunked text |
| `AssistantTextStreamParser` / `AssistantTextChunk` | Assistant message text |
| `CitationStreamParser` + `strip_citations` | Anthropic citations |
| `InlineHiddenTagParser` / `InlineTagSpec` / `ExtractedInlineTag` | Named inline tags |
| `ProposedPlanParser` / `ProposedPlanSegment` + `extract_proposed_plan_text` / `strip_proposed_plan_blocks` | Plan-mode `<proposed_plan>` blocks |
| `Utf8StreamParser` / `Utf8StreamParserError` | Boundary-safe UTF-8 byte stream |
