use super::*;
use crate::config::RerankerConfig;

#[test]
fn test_create_rule_based_reranker() {
    let config = RerankerConfig::default();
    let reranker = create_rule_based_reranker(&config);
    assert_eq!(reranker.name(), "rule_based");
}

#[test]
fn test_chain_reranker_capabilities() {
    let rule_config = RerankerConfig::default();
    let rule_reranker = create_rule_based_reranker(&rule_config);

    let chain = ChainReranker::new(vec![rule_reranker]);

    assert!(!chain.capabilities().requires_network);
    assert!(!chain.capabilities().is_async);
}
