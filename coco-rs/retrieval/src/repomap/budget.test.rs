use super::*;
use crate::tags::extractor::CodeTag;
use crate::tags::extractor::TagKind;

fn make_symbol(name: &str, line: i32) -> RankedSymbol {
    RankedSymbol {
        tag: CodeTag {
            name: name.to_string(),
            kind: TagKind::Function,
            start_line: line,
            end_line: line + 10,
            start_byte: line * 100,
            end_byte: (line + 10) * 100,
            signature: Some(format!("fn {}() -> Result<()>", name)),
            docs: None,
            is_definition: true,
        },
        rank: 1.0 / (line as f64),
        filepath: format!("src/file_{}.rs", line / 100),
    }
}

#[test]
fn test_count_tokens() {
    let budgeter = TokenBudgeter::new().unwrap();

    let tokens = budgeter.count_tokens("Hello, world!");
    assert!(tokens > 0);
    assert!(tokens < 10);

    let long_text = "fn process_request(req: Request) -> Response { /* ... */ }";
    let long_tokens = budgeter.count_tokens(long_text);
    assert!(long_tokens > tokens);
}

#[test]
fn test_find_optimal_count_empty() {
    let budgeter = TokenBudgeter::new().unwrap();
    let renderer = TreeRenderer::new();

    let count = budgeter.find_optimal_count(&[], &renderer, 100);
    assert_eq!(count, 0);
}

#[test]
fn test_find_optimal_count_fits_all() {
    let budgeter = TokenBudgeter::new().unwrap();
    let renderer = TreeRenderer::new();

    let symbols = vec![make_symbol("foo", 10), make_symbol("bar", 20)];

    // Large budget should fit all symbols
    let count = budgeter.find_optimal_count(&symbols, &renderer, 10000);
    assert_eq!(count, 2);
}

#[test]
fn test_find_optimal_count_limited() {
    let budgeter = TokenBudgeter::new().unwrap();
    let renderer = TreeRenderer::new();

    // Create many symbols
    let symbols: Vec<RankedSymbol> = (1..=50)
        .map(|i| make_symbol(&format!("function_{}", i), i * 10))
        .collect();

    // Small budget should limit symbols
    let count = budgeter.find_optimal_count(&symbols, &renderer, 100);
    assert!(count > 0);
    assert!(count < 50);
}

#[test]
fn test_binary_search_convergence() {
    let budgeter = TokenBudgeter::new().unwrap();
    let renderer = TreeRenderer::new();

    // Create a reasonable number of symbols
    let symbols: Vec<RankedSymbol> = (1..=20)
        .map(|i| make_symbol(&format!("func_{}", i), i * 10))
        .collect();

    // Various budget sizes should all converge
    for budget in [50, 100, 200, 500, 1000] {
        let count = budgeter.find_optimal_count(&symbols, &renderer, budget);

        // Verify the result is valid
        if count > 0 {
            let content = renderer.render_symbols(&symbols, count);
            let tokens = budgeter.count_tokens(&content);
            assert!(
                tokens <= budget,
                "budget={} count={} tokens={}",
                budget,
                count,
                tokens
            );
        }
    }
}
