pub(crate) const MODEL_RUNTIME_FALLBACK_TRANSITION: &str =
    "coco.inference.model_runtime_fallback_transition";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_span_names_are_stable() {
        assert_eq!(
            MODEL_RUNTIME_FALLBACK_TRANSITION,
            "coco.inference.model_runtime_fallback_transition"
        );
    }
}
