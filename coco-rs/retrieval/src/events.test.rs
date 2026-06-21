use super::*;

#[test]
fn test_event_serialization() {
    let event = RetrievalEvent::SearchStarted {
        query_id: "q-123".to_string(),
        query: "test query".to_string(),
        mode: SearchMode::Hybrid,
        limit: 10,
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("search_started"));
    assert!(json.contains("test query"));
}

#[test]
fn test_event_to_json_line() {
    let event = RetrievalEvent::SearchCompleted {
        query_id: "q-123".to_string(),
        results: vec![],
        total_duration_ms: 100,
        filter: None,
    };

    let line = event.to_json_line();
    assert!(line.contains("timestamp"));
    assert!(line.contains("search_completed"));
}

#[test]
fn test_event_type() {
    let event = RetrievalEvent::IndexBuildStarted {
        workspace: "test".to_string(),
        mode: RebuildModeInfo::Clean,
        estimated_files: 100,
        filter: None,
    };

    assert_eq!(event.event_type(), "index_build_started");
}

#[test]
fn test_search_mode_display() {
    assert_eq!(format!("{}", SearchMode::Hybrid), "hybrid");
    assert_eq!(format!("{}", SearchMode::Bm25), "bm25");
    assert_eq!(format!("{}", SearchMode::Vector), "vector");
    assert_eq!(format!("{}", SearchMode::Snippet), "snippet");
}

#[test]
fn test_generate_query_id() {
    let id1 = generate_query_id();
    let id2 = generate_query_id();
    assert_ne!(id1, id2);
    assert!(id1.starts_with("q-"));
}

#[test]
fn test_json_lines_consumer() {
    let mut output = Vec::new();
    {
        let mut consumer = JsonLinesConsumer::new(&mut output);
        consumer.on_event(&RetrievalEvent::SessionStarted {
            session_id: "s-123".to_string(),
            config: ConfigSummary {
                enabled: true,
                data_dir: "/tmp".to_string(),
                bm25_enabled: true,
                vector_enabled: false,
                query_rewrite_enabled: false,
                reranker_backend: None,
            },
        });
        consumer.flush();
    }

    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("session_started"));
    assert!(output_str.ends_with('\n'));
}
