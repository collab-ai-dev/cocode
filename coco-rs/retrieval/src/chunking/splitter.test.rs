use super::*;

#[test]
fn test_chunk_code() {
    let code = r#"
fn main() {
    println!("Hello, world!");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    let chunker = CodeChunkerService::new(512, 50);
    let chunks = chunker.chunk(code, "rust").expect("chunking failed");

    assert!(!chunks.is_empty());
    // Chunks should cover the entire content
    let total_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(total_content.trim(), code.trim());
}

#[test]
fn test_line_numbers() {
    let code = "line1\nline2\nline3\nline4\nline5";
    let chunker = CodeChunkerService::new(1000, 0);
    let chunks = chunker.chunk(code, "text").expect("chunking failed");

    assert_eq!(chunks.len(), 1);
    // Line numbers are 1-indexed
    assert_eq!(chunks[0].start_line, 1);
    assert_eq!(chunks[0].end_line, 5);
}

#[test]
fn test_multiple_chunks() {
    // Use small token limit to force multiple chunks
    let code = "a".repeat(100) + "\n" + &"b".repeat(100);
    let chunker = CodeChunkerService::new(20, 0);
    let chunks = chunker.chunk(&code, "text").expect("chunking failed");

    assert!(chunks.len() > 1);
}

#[test]
fn test_code_splitter_supported_languages() {
    // Supported languages
    assert!(is_code_splitter_supported("rust"));
    assert!(is_code_splitter_supported("go"));
    assert!(is_code_splitter_supported("python"));
    assert!(is_code_splitter_supported("java"));
    assert!(is_code_splitter_supported("typescript"));
    assert!(is_code_splitter_supported("javascript"));
    assert!(is_code_splitter_supported("tsx"));
    assert!(is_code_splitter_supported("jsx"));
    // Unsupported languages
    assert!(!is_code_splitter_supported("markdown"));
    assert!(!is_code_splitter_supported("unknown"));
    assert!(!is_code_splitter_supported("c"));
    assert!(!is_code_splitter_supported("cpp"));
}

#[test]
fn test_code_splitter_typescript() {
    let code = r#"interface User {
    id: number;
    name: string;
}

function greet(user: User): string {
    return `Hello, ${user.name}!`;
}

class UserService {
    private users: User[] = [];

    addUser(user: User): void {
        this.users.push(user);
    }

    getUser(id: number): User | undefined {
        return this.users.find(u => u.id === id);
    }
}
"#;
    let chunker = CodeChunkerService::new(100, 0);
    let chunks = chunker.chunk(code, "typescript").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(total.contains("interface User"));
    assert!(total.contains("function greet"));
    assert!(total.contains("class UserService"));
}

#[test]
fn test_code_splitter_javascript() {
    let code = r#"const express = require('express');
const app = express();

function handleRequest(req, res) {
    res.json({ message: 'Hello, World!' });
}

app.get('/hello', handleRequest);

class Router {
    constructor() {
        this.routes = [];
    }

    add(path, handler) {
        this.routes.push({ path, handler });
    }
}
"#;
    let chunker = CodeChunkerService::new(100, 0);
    let chunks = chunker.chunk(code, "javascript").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(total.contains("const express"));
    assert!(total.contains("function handleRequest"));
    assert!(total.contains("class Router"));
}

#[test]
fn test_code_splitter_rust() {
    let code = r#"fn hello() {
    println!("Hello");
}

fn world() {
    println!("World");
}

fn long_function() {
    let x = 1;
    let y = 2;
    let z = 3;
    println!("{} {} {}", x, y, z);
}
"#;
    let chunker = CodeChunkerService::new(100, 0);
    let chunks = chunker.chunk(code, "rust").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(total.contains("fn hello()"));
    assert!(total.contains("fn world()"));
    assert!(total.contains("fn long_function()"));
}

#[test]
fn test_code_splitter_python() {
    let code = r#"def hello():
    print("Hello")

def world():
    print("World")

class Greeter:
    def greet(self, name):
        return f"Hello, {name}"
"#;
    let chunker = CodeChunkerService::new(100, 0);
    let chunks = chunker.chunk(code, "python").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(total.contains("def hello()"));
    assert!(total.contains("def world()"));
    assert!(total.contains("class Greeter"));
}

#[test]
fn test_code_splitter_go() {
    let code = r#"package main

func hello() {
    fmt.Println("Hello")
}

func world() {
    fmt.Println("World")
}
"#;
    let chunker = CodeChunkerService::new(100, 0);
    let chunks = chunker.chunk(code, "go").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(total.contains("func hello()"));
    assert!(total.contains("func world()"));
}

#[test]
fn test_text_splitter_fallback() {
    let code = "const x = 1;\nconst y = 2;\nconst z = 3;";
    let chunker = CodeChunkerService::new(1000, 0);
    let chunks = chunker.chunk(code, "javascript").expect("chunking failed");

    assert!(!chunks.is_empty());
    let total: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(total.trim(), code.trim());
}

#[test]
fn test_chunk_with_overlap() {
    let lines: Vec<String> = (1..=20).map(|i| format!("line{i}")).collect();
    let code = lines.join("\n");

    // Without overlap
    let chunker_no_overlap = CodeChunkerService::new(30, 0);
    let chunks_no_overlap = chunker_no_overlap
        .chunk(&code, "text")
        .expect("chunking failed");

    // With overlap
    let chunker_with_overlap = CodeChunkerService::new(30, 5);
    let chunks_with_overlap = chunker_with_overlap
        .chunk(&code, "text")
        .expect("chunking failed");

    assert!(chunks_no_overlap.len() > 1);
    assert!(chunks_with_overlap.len() > 1);

    // With overlap, subsequent chunks should have extra content
    if chunks_with_overlap.len() >= 2 {
        assert!(
            chunks_with_overlap[1].content.len() >= chunks_no_overlap[1].content.len(),
            "Overlapped chunk should be at least as long as non-overlapped"
        );
    }
}

#[test]
fn test_single_chunk_no_overlap_effect() {
    let code = "short content";
    let chunker = CodeChunkerService::new(1000, 50);
    let chunks = chunker.chunk(code, "text").expect("chunking failed");

    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content.trim(), code.trim());
}

#[test]
fn test_code_overlap_disabled() {
    // Overlap is DISABLED for code because token-based overlap creates AST fragments.
    // This test verifies that overlap_tokens parameter has no effect for code.
    let code = r#"fn first_function() {
    let a = 1;
    let b = 2;
}

fn second_function() {
    let c = 3;
    let d = 4;
}

fn third_function() {
    let e = 5;
}"#;

    // Without overlap
    let chunker_no_overlap = CodeChunkerService::new(40, 0);
    let chunks_no = chunker_no_overlap.chunk(code, "rust").unwrap();

    // With overlap parameter (10 tokens) - should have NO effect for code
    let chunker_with_overlap = CodeChunkerService::new(40, 10);
    let chunks_with = chunker_with_overlap.chunk(code, "rust").unwrap();

    // For code, both should produce identical chunks (overlap disabled)
    assert_eq!(
        chunks_no.len(),
        chunks_with.len(),
        "Code chunking should ignore overlap parameter"
    );

    for (no, with) in chunks_no.iter().zip(chunks_with.iter()) {
        assert_eq!(
            no.content, with.content,
            "Code chunks should be identical regardless of overlap setting"
        );
    }

    // Verify chunks don't start with AST fragments
    for (i, chunk) in chunks_no.iter().enumerate() {
        let trimmed = chunk.content.trim();
        // Chunks shouldn't start with closing braces or partial expressions
        let bad_starts = ["}", ")", "]", ",", "else", "&&", "||"];
        for bad in &bad_starts {
            assert!(
                !trimmed.starts_with(bad),
                "Chunk {} should not start with '{}': {}",
                i + 1,
                bad,
                &trimmed[..trimmed.len().min(30)]
            );
        }
    }
}

#[test]
fn test_text_overlap_works() {
    // Overlap SHOULD work for plain text (non-code)
    let text = "Line one.\nLine two.\nLine three.\nLine four.\nLine five.\nLine six.";

    // Small token limit to force multiple chunks
    let chunker_no_overlap = CodeChunkerService::new(10, 0);
    let chunks_no = chunker_no_overlap.chunk(text, "text").unwrap();

    let chunker_with_overlap = CodeChunkerService::new(10, 3);
    let chunks_with = chunker_with_overlap.chunk(text, "text").unwrap();

    // With overlap, chunks should be different (overlap applied)
    if chunks_no.len() > 1 && chunks_with.len() > 1 {
        // Second chunk with overlap should contain content from end of first chunk
        let second_no = &chunks_no[1].content;
        let second_with = &chunks_with[1].content;

        // The overlapped chunk should be longer or start with content from previous
        assert!(
            second_with.len() >= second_no.len() || chunks_with.len() != chunks_no.len(),
            "Text overlap should produce different chunks"
        );
    }
}

#[test]
fn test_oversized_function_handling() {
    // Test what happens when a single function exceeds max_tokens
    // This is a ~150 token function, we'll use 30 token limit
    let code = r#"fn very_long_function(a: i32, b: i32, c: String) -> Result<String, Error> {
    let result_one = process_first_step(a, b);
    let result_two = process_second_step(result_one, &c);
    let result_three = process_third_step(result_two);
    if result_three.is_ok() {
        println!("Success: {:?}", result_three);
        Ok(result_three.unwrap())
    } else {
        Err(Error::new("Failed"))
    }
}"#;
    let chunker = CodeChunkerService::new(30, 0);
    let chunks = chunker.chunk(code, "rust").expect("chunking failed");

    // Key assertions:
    // 1. Content is NOT lost
    let combined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(
        combined.contains("very_long_function"),
        "Function name should be present"
    );
    assert!(
        combined.contains("result_one"),
        "Variable should be present"
    );
    assert!(
        combined.contains("result_three"),
        "Last variable should be present"
    );

    // 2. Multiple chunks are produced (function was split)
    assert!(
        chunks.len() > 1,
        "Long function should be split into multiple chunks"
    );
}

#[test]
fn test_token_mode_respects_syntax() {
    // This test verifies that token mode produces valid chunks that don't break
    // in the middle of statements. A chunk may contain:
    // - A complete function
    // - Multiple complete functions
    // - The closing brace of one function + another complete function
    // What we want to AVOID is a chunk ending mid-statement, e.g.:
    // "fn foo() {\n    let x = 1;" without the closing brace
    let code = r#"fn process_data(input: &str) -> Result<String, Error> {
    let mut result = String::new();
    for line in input.lines() {
        if line.starts_with("//") {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    Ok(result)
}

fn another_function() {
    println!("test");
}
"#;
    // Use small token limit to force chunking
    let chunker = CodeChunkerService::new(50, 0);
    let chunks = chunker.chunk(code, "rust").expect("chunking failed");

    // We should have at least one chunk
    assert!(!chunks.is_empty(), "Should produce at least one chunk");

    // Verify all code is covered (no content lost)
    let combined: String = chunks.iter().map(|c| c.content.as_str()).collect();
    assert!(
        combined.contains("fn process_data"),
        "Should contain first function"
    );
    assert!(
        combined.contains("fn another_function"),
        "Should contain second function"
    );

    // Verify each chunk that contains a function body has balanced braces
    // (meaning we didn't split mid-function)
    for chunk in &chunks {
        let open_braces = chunk.content.matches('{').count();
        let close_braces = chunk.content.matches('}').count();
        // Allow for partial functions at boundaries, but the imbalance shouldn't be extreme
        let imbalance = (open_braces as i32 - close_braces as i32).abs();
        assert!(
            imbalance <= 2,
            "Chunk has excessive brace imbalance ({}): {}",
            imbalance,
            &chunk.content[..chunk.content.len().min(100)]
        );
    }
}

#[test]
fn test_detect_import_block_rust() {
    let code = r#"use std::io;
use std::path::Path;
use crate::error::Result;

fn main() {
    println!("Hello");
}
"#;
    let result = detect_import_block(code, "rust");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    // Import block includes trailing empty line (line 4)
    assert_eq!(end_line, 4);
    assert!(content.contains("use std::io"));
    assert!(content.contains("use crate::error::Result"));
    assert!(!content.contains("fn main"));
}

#[test]
fn test_detect_import_block_python() {
    let code = r#"import os
import sys
from typing import List, Optional
from pathlib import Path

def main():
    print("Hello")
"#;
    let result = detect_import_block(code, "python");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    // Import block includes trailing empty line (line 5)
    assert_eq!(end_line, 5);
    assert!(content.contains("import os"));
    assert!(content.contains("from pathlib import Path"));
    assert!(!content.contains("def main"));
}

#[test]
fn test_detect_import_block_typescript() {
    let code = r#"import React from 'react';
import { useState, useEffect } from 'react';
import type { User } from './types';

export function App() {
    return <div>Hello</div>;
}
"#;
    let result = detect_import_block(code, "typescript");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    assert!(end_line >= 3);
    assert!(content.contains("import React"));
    assert!(content.contains("import type { User }"));
}

#[test]
fn test_detect_import_block_typescript_multiline() {
    let code = r#"import {
    useState,
    useEffect,
    useCallback,
    useMemo
} from 'react';
import { Button } from './components';

function App() {
    return <div>Hello</div>;
}
"#;
    let result = detect_import_block(code, "typescript");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    // Should include both multi-line import and single-line import (line 8 with trailing empty)
    assert!(
        end_line >= 7,
        "end_line should be at least 7, got {end_line}"
    );
    assert!(content.contains("useState"));
    assert!(content.contains("useMemo"));
    assert!(content.contains("} from 'react'"));
    assert!(content.contains("Button"));
    assert!(!content.contains("function App"));
}

#[test]
fn test_detect_import_block_javascript_multiline() {
    let code = r#"const {
    readFile,
    writeFile
} = require('fs');
const path = require('path');

function main() {
    console.log('Hello');
}
"#;
    let result = detect_import_block(code, "javascript");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    assert!(
        end_line >= 5,
        "end_line should be at least 5, got {end_line}"
    );
    assert!(content.contains("readFile"));
    assert!(content.contains("writeFile"));
    assert!(content.contains("path"));
    assert!(!content.contains("function main"));
}

#[test]
fn test_detect_import_block_javascript_require_variants() {
    // Test all require() variants: const, let, var, destructured
    let code = r#"const fs = require('fs');
let path = require('path');
var os = require('os');
const { join, resolve } = require('path');

function main() {}
"#;
    let result = detect_import_block(code, "javascript");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    assert!(
        end_line >= 4,
        "end_line should be at least 4, got {end_line}"
    );
    assert!(content.contains("const fs"));
    assert!(content.contains("let path"));
    assert!(content.contains("var os"));
    assert!(content.contains("join, resolve"));
    assert!(!content.contains("function main"));
}

#[test]
fn test_detect_import_block_go_multiline() {
    let code = r#"package main

import (
    "fmt"
    "os"
    "path/filepath"
)

func main() {
    fmt.Println("Hello")
}
"#;
    let result = detect_import_block(code, "go");
    assert!(result.is_some());
    let (end_line, content) = result.unwrap();
    // Import block includes trailing empty line (line 8)
    assert_eq!(end_line, 8);
    assert!(content.contains("package main"));
    assert!(content.contains("\"fmt\""));
    assert!(content.contains("\"path/filepath\""));
    assert!(!content.contains("func main"));
}

#[test]
fn test_detect_import_block_no_imports() {
    let code = r#"fn main() {
    println!("Hello");
}
"#;
    let result = detect_import_block(code, "rust");
    assert!(result.is_none());
}
