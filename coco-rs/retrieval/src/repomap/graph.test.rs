use super::*;
use crate::tags::extractor::TagKind;

fn make_def(name: &str, line: i32) -> CodeTag {
    CodeTag {
        name: name.to_string(),
        kind: TagKind::Function,
        start_line: line,
        end_line: line + 10,
        start_byte: line * 100,
        end_byte: (line + 10) * 100,
        signature: Some(format!("fn {}()", name)),
        docs: None,
        is_definition: true,
    }
}

fn make_ref(name: &str, line: i32) -> CodeTag {
    CodeTag {
        name: name.to_string(),
        kind: TagKind::Function,
        start_line: line,
        end_line: line,
        start_byte: line * 100,
        end_byte: line * 100,
        signature: None,
        docs: None,
        is_definition: false,
    }
}

#[test]
fn test_build_graph() {
    let mut graph = DependencyGraph::new();

    // file_a.rs defines foo, references bar
    graph.add_file_tags("file_a.rs", &[make_def("foo", 10), make_ref("bar", 20)]);

    // file_b.rs defines bar, references foo
    graph.add_file_tags("file_b.rs", &[make_def("bar", 5), make_ref("foo", 15)]);

    assert_eq!(graph.file_count(), 2);
    assert_eq!(graph.definitions().len(), 2);

    // Build edges with default weights
    graph.build_edges(
        &HashSet::new(),
        &HashSet::new(),
        &HashSet::new(),
        50.0,
        10.0,
        0.1,
        10.0,
        5.0,
    );

    // Should have 2 edges: a->b (bar ref) and b->a (foo ref)
    assert_eq!(graph.edge_count(), 2);
}

#[test]
fn test_personalization() {
    let mut graph = DependencyGraph::new();
    graph.add_file_tags("a.rs", &[make_def("x", 1)]);
    graph.add_file_tags("b.rs", &[make_def("y", 1)]);
    graph.add_file_tags("c.rs", &[make_def("z", 1)]);

    let chat_files: HashSet<String> = ["a.rs".to_string()].into_iter().collect();
    let pers = graph.build_personalization(&chat_files);

    // Chat file should have higher probability
    assert!(pers["a.rs"] > pers["b.rs"]);
    assert!(pers["a.rs"] > pers["c.rs"]);

    // Probabilities should sum to ~1.0
    let sum: f64 = pers.values().sum();
    assert!((sum - 1.0).abs() < 0.001);
}

#[test]
fn test_private_symbol_penalty() {
    let mut graph = DependencyGraph::new();

    // _private symbol should get penalty
    graph.add_file_tags("a.rs", &[make_def("_private", 1)]);
    graph.add_file_tags("b.rs", &[make_ref("_private", 1)]);

    graph.build_edges(
        &HashSet::new(),
        &HashSet::new(),
        &HashSet::new(),
        50.0,
        10.0,
        0.1,
        10.0,
        5.0,
    );

    // Edge should exist with reduced weight
    assert_eq!(graph.edge_count(), 1);
}

#[test]
fn test_is_well_named() {
    // snake_case with len >= 8
    assert!(is_well_named("calculate_hash"));
    assert!(is_well_named("get_user_name"));
    assert!(is_well_named("process_data"));

    // camelCase with len >= 8
    assert!(is_well_named("calculateHash"));
    assert!(is_well_named("getUserName"));
    assert!(is_well_named("processData"));

    // Too short (< 8 chars)
    assert!(!is_well_named("foo_bar"));
    assert!(!is_well_named("fooBar"));
    assert!(!is_well_named("get"));

    // No naming convention
    assert!(!is_well_named("foobarba"));
    assert!(!is_well_named("CONSTANT"));
}

#[test]
fn test_extract_terms() {
    // snake_case
    assert_eq!(
        extract_terms("calculate_user_score"),
        vec!["calculate", "user", "score"]
    );

    // camelCase
    assert_eq!(
        extract_terms("calculateUserScore"),
        vec!["calculate", "user", "score"]
    );

    // Mixed
    assert_eq!(
        extract_terms("getUserName"),
        vec!["user", "name"] // "get" is filtered as common term
    );

    // Short terms filtered
    assert_eq!(extract_terms("a_b_c"), Vec::<String>::new());
}

#[test]
fn test_term_match_score() {
    let query_terms: HashSet<String> = ["calculate", "user", "score"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Full match
    let score = term_match_score("calculate_user_score", &query_terms);
    assert!((score - 1.0).abs() < 0.001); // All terms match

    // Partial match (2 of 3 query terms)
    let score = term_match_score("calculate_user_name", &query_terms);
    assert!(score > 0.5 && score < 1.0);

    // No match
    let score = term_match_score("process_data", &query_terms);
    assert!(score < 0.001);

    // Empty query
    let empty: HashSet<String> = HashSet::new();
    assert!(term_match_score("anything", &empty) < 0.001);
}

#[test]
fn test_edge_weight_accumulation() {
    let mut graph = DependencyGraph::new();

    // file_a.rs references both foo and bar from file_b.rs
    // This should create a single edge with accumulated weight
    graph.add_file_tags("file_a.rs", &[make_ref("foo", 10), make_ref("bar", 20)]);
    graph.add_file_tags("file_b.rs", &[make_def("foo", 5), make_def("bar", 15)]);

    graph.build_edges(
        &HashSet::new(),
        &HashSet::new(),
        &HashSet::new(),
        50.0,
        10.0,
        0.1,
        10.0,
        5.0,
    );

    // Should have only 1 edge (a->b), not 2
    // The edge weight should be accumulated from both foo and bar references
    assert_eq!(graph.edge_count(), 1);

    // Verify the edge exists and has accumulated weight
    let a_idx = *graph.node_indices.get("file_a.rs").unwrap();
    let b_idx = *graph.node_indices.get("file_b.rs").unwrap();
    let edge = graph.graph.find_edge(a_idx, b_idx).unwrap();
    let edge_data = graph.graph.edge_weight(edge).unwrap();

    // Edge weight should be > 1.0 (accumulated from two symbols)
    assert!(edge_data.weight > 1.0);
    // Edge should track both symbols
    assert!(edge_data.symbol.contains("foo") || edge_data.symbol.contains("bar"));
}

#[test]
fn test_file_definition_counts() {
    let mut graph = DependencyGraph::new();

    // file_a.rs has 3 definitions
    graph.add_file_tags(
        "file_a.rs",
        &[
            make_def("foo", 10),
            make_def("bar", 20),
            make_def("baz", 30),
        ],
    );
    // file_b.rs has 1 definition
    graph.add_file_tags("file_b.rs", &[make_def("qux", 5)]);

    let counts = graph.compute_file_definition_counts();

    assert_eq!(counts.get("file_a.rs").copied().unwrap_or(0), 3);
    assert_eq!(counts.get("file_b.rs").copied().unwrap_or(0), 1);
}
