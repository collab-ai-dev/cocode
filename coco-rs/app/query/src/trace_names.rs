pub(crate) const PROMPT_CONTEXT_LOADING: &str = "coco.query.prompt_context_loading";
pub(crate) const TOOL_CALL_PREPARATION: &str = "coco.query.tool_call_preparation";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_span_names_are_stable() {
        assert_eq!(PROMPT_CONTEXT_LOADING, "coco.query.prompt_context_loading");
        assert_eq!(TOOL_CALL_PREPARATION, "coco.query.tool_call_preparation");
    }
}
