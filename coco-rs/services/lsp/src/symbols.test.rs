use super::*;

#[test]
fn test_symbol_kind_from_str() {
    assert_eq!(
        SymbolKind::from_str_loose("function"),
        Some(SymbolKind::Function)
    );
    assert_eq!(SymbolKind::from_str_loose("fn"), Some(SymbolKind::Function));
    assert_eq!(
        SymbolKind::from_str_loose("STRUCT"),
        Some(SymbolKind::Struct)
    );
    assert_eq!(
        SymbolKind::from_str_loose("trait"),
        Some(SymbolKind::Interface)
    );
    assert_eq!(SymbolKind::from_str_loose("unknown"), None);
}

#[test]
fn test_symbol_kind_display_name() {
    assert_eq!(SymbolKind::Function.display_name(), "function");
    assert_eq!(SymbolKind::Struct.display_name(), "struct");
    assert_eq!(SymbolKind::Other.display_name(), "symbol");
}

#[test]
fn test_find_matching_symbols() {
    let symbols = vec![
        ResolvedSymbol {
            name: "process_data".to_string(),
            kind: SymbolKind::Function,
            position: Position {
                line: 10,
                character: 0,
            },
            range_start_line: 10,
            range_end_line: 20,
        },
        ResolvedSymbol {
            name: "ProcessData".to_string(),
            kind: SymbolKind::Struct,
            position: Position {
                line: 5,
                character: 0,
            },
            range_start_line: 5,
            range_end_line: 8,
        },
        ResolvedSymbol {
            name: "other_func".to_string(),
            kind: SymbolKind::Function,
            position: Position {
                line: 30,
                character: 0,
            },
            range_start_line: 30,
            range_end_line: 35,
        },
    ];

    // Exact match with kind filter
    let matches = find_matching_symbols(&symbols, "process_data", Some(SymbolKind::Function));
    assert_eq!(matches.len(), 1);
    assert!(matches[0].exact_name_match);
    assert_eq!(matches[0].symbol.kind, SymbolKind::Function);

    // Case-insensitive matching without kind filter
    // "process_data" matches exactly, but "ProcessData" (lowercased "processdata")
    // does NOT contain "process_data" as substring due to underscore
    let matches = find_matching_symbols(&symbols, "PROCESS_DATA", None);
    assert_eq!(matches.len(), 1);
    assert!(matches[0].exact_name_match);

    // Searching for "process" should match both (substring match)
    let matches = find_matching_symbols(&symbols, "process", None);
    assert_eq!(matches.len(), 2);

    // Kind filter narrows results
    let matches = find_matching_symbols(&symbols, "process", Some(SymbolKind::Struct));
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].symbol.name, "ProcessData");

    // No matches
    let matches = find_matching_symbols(&symbols, "nonexistent", None);
    assert!(matches.is_empty());
}

#[test]
fn test_find_matching_symbols_exact_first() {
    let symbols = vec![
        ResolvedSymbol {
            name: "Config".to_string(),
            kind: SymbolKind::Struct,
            position: Position {
                line: 1,
                character: 0,
            },
            range_start_line: 1,
            range_end_line: 5,
        },
        ResolvedSymbol {
            name: "ConfigBuilder".to_string(),
            kind: SymbolKind::Struct,
            position: Position {
                line: 10,
                character: 0,
            },
            range_start_line: 10,
            range_end_line: 20,
        },
    ];

    let matches = find_matching_symbols(&symbols, "config", None);
    assert_eq!(matches.len(), 2);
    // Exact match should be first
    assert!(matches[0].exact_name_match);
    assert_eq!(matches[0].symbol.name, "Config");
}
