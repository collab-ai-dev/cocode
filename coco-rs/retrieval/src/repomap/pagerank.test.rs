use super::*;
use crate::tags::extractor::TagKind;

#[test]
fn test_empty_graph() {
    let ranker = PageRanker::default();
    let graph: DiGraph<String, EdgeData> = DiGraph::new();
    let pers = HashMap::new();

    let ranks = ranker.rank(&graph, &pers).unwrap();
    assert!(ranks.is_empty());
}

#[test]
fn test_single_node() {
    let ranker = PageRanker::default();
    let mut graph: DiGraph<String, EdgeData> = DiGraph::new();
    graph.add_node("a.rs".to_string());

    let ranks = ranker.rank(&graph, &HashMap::new()).unwrap();
    assert_eq!(ranks.len(), 1);
    assert!((ranks["a.rs"] - 1.0).abs() < 0.001);
}

#[test]
fn test_two_nodes_with_edge() {
    let ranker = PageRanker::default();
    let mut graph: DiGraph<String, EdgeData> = DiGraph::new();

    let a = graph.add_node("a.rs".to_string());
    let b = graph.add_node("b.rs".to_string());

    // a references b (edge a -> b)
    graph.add_edge(
        a,
        b,
        EdgeData {
            weight: 1.0,
            symbol: "foo".to_string(),
        },
    );

    let ranks = ranker.rank(&graph, &HashMap::new()).unwrap();

    // b should have higher rank (it's referenced)
    assert!(ranks["b.rs"] > ranks["a.rs"]);
}

#[test]
fn test_personalization_boost() {
    let ranker = PageRanker::default();
    let mut graph: DiGraph<String, EdgeData> = DiGraph::new();

    graph.add_node("a.rs".to_string());
    graph.add_node("b.rs".to_string());

    // Personalize to boost a.rs
    let mut pers = HashMap::new();
    pers.insert("a.rs".to_string(), 0.9);
    pers.insert("b.rs".to_string(), 0.1);

    let ranks = ranker.rank(&graph, &pers).unwrap();

    // a.rs should have higher rank due to personalization
    assert!(ranks["a.rs"] > ranks["b.rs"]);
}

#[test]
fn test_distribute_to_definitions() {
    let ranker = PageRanker::default();

    let mut file_ranks = HashMap::new();
    file_ranks.insert("a.rs".to_string(), 0.6);
    file_ranks.insert("b.rs".to_string(), 0.4);

    let mut definitions = HashMap::new();
    definitions.insert(
        "foo".to_string(),
        vec![(
            "a.rs".to_string(),
            CodeTag {
                name: "foo".to_string(),
                kind: TagKind::Function,
                start_line: 10,
                end_line: 20,
                start_byte: 100,
                end_byte: 200,
                signature: Some("fn foo()".to_string()),
                docs: None,
                is_definition: true,
            },
        )],
    );
    definitions.insert(
        "bar".to_string(),
        vec![(
            "b.rs".to_string(),
            CodeTag {
                name: "bar".to_string(),
                kind: TagKind::Function,
                start_line: 5,
                end_line: 15,
                start_byte: 50,
                end_byte: 150,
                signature: Some("fn bar()".to_string()),
                docs: None,
                is_definition: true,
            },
        )],
    );

    // Each file has 1 definition
    let mut file_def_counts = HashMap::new();
    file_def_counts.insert("a.rs".to_string(), 1);
    file_def_counts.insert("b.rs".to_string(), 1);

    let ranked = ranker.distribute_to_definitions(&file_ranks, &definitions, &file_def_counts);

    assert_eq!(ranked.len(), 2);
    // foo (from a.rs with 0.6 rank) should be first
    assert_eq!(ranked[0].tag.name, "foo");
    assert_eq!(ranked[1].tag.name, "bar");
}

#[test]
fn test_distribute_with_multiple_defs_per_file() {
    let ranker = PageRanker::default();

    let mut file_ranks = HashMap::new();
    file_ranks.insert("a.rs".to_string(), 0.6);

    // File a.rs has 3 definitions: foo, bar, baz
    let mut definitions = HashMap::new();
    definitions.insert(
        "foo".to_string(),
        vec![(
            "a.rs".to_string(),
            CodeTag {
                name: "foo".to_string(),
                kind: TagKind::Function,
                start_line: 10,
                end_line: 20,
                start_byte: 100,
                end_byte: 200,
                signature: None,
                docs: None,
                is_definition: true,
            },
        )],
    );
    definitions.insert(
        "bar".to_string(),
        vec![(
            "a.rs".to_string(),
            CodeTag {
                name: "bar".to_string(),
                kind: TagKind::Function,
                start_line: 30,
                end_line: 40,
                start_byte: 300,
                end_byte: 400,
                signature: None,
                docs: None,
                is_definition: true,
            },
        )],
    );
    definitions.insert(
        "baz".to_string(),
        vec![(
            "a.rs".to_string(),
            CodeTag {
                name: "baz".to_string(),
                kind: TagKind::Function,
                start_line: 50,
                end_line: 60,
                start_byte: 500,
                end_byte: 600,
                signature: None,
                docs: None,
                is_definition: true,
            },
        )],
    );

    // File a.rs has 3 definitions total
    let mut file_def_counts = HashMap::new();
    file_def_counts.insert("a.rs".to_string(), 3);

    let ranked = ranker.distribute_to_definitions(&file_ranks, &definitions, &file_def_counts);

    assert_eq!(ranked.len(), 3);

    // Each symbol should get 0.6 / 3 = 0.2 rank
    for sym in &ranked {
        assert!(
            (sym.rank - 0.2).abs() < 0.001,
            "Expected rank ~0.2, got {}",
            sym.rank
        );
    }
}
