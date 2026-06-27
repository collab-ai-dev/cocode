use super::NoopSymbolCompletionSource;
use super::SymbolCompletionSource;

#[test]
fn noop_symbol_completion_source_returns_no_rows() {
    let source = NoopSymbolCompletionSource;

    assert!(source.search("main").is_empty());
}
