pub(crate) const COORDINATOR_DISPATCH: &str = "coco.retrieval.coordinator_dispatch";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_span_names_are_stable() {
        assert_eq!(COORDINATOR_DISPATCH, "coco.retrieval.coordinator_dispatch");
    }
}
