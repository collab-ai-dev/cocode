use super::*;

#[test]
fn test_generated_file_new() {
    let file = GeneratedFile::new("test.txt", "Hello, world!", "text/plain");

    assert_eq!(file.name, "test.txt");
    assert_eq!(file.content, "Hello, world!");
    assert_eq!(file.media_type, "text/plain");
    assert!(file.is_text());
    assert!(!file.is_binary());
}

#[test]
fn test_generated_file_text() {
    let file = GeneratedFile::text("readme.txt", "This is a readme file");

    assert_eq!(file.name, "readme.txt");
    assert_eq!(file.media_type, "text/plain");
    assert!(file.is_text());
}

#[test]
fn test_generated_file_json() {
    let json = serde_json::json!({ "key": "value" });
    let file = GeneratedFile::json("config.json", &json);

    assert_eq!(file.name, "config.json");
    assert_eq!(file.media_type, "application/json");
    assert!(file.is_text());
}

#[test]
fn test_generated_file_markdown() {
    let file = GeneratedFile::markdown("README.md", "# Title\n\nContent");

    assert_eq!(file.name, "README.md");
    assert_eq!(file.media_type, "text/markdown");
}

#[test]
fn test_generated_file_code() {
    let file = GeneratedFile::code("main.rs", "fn main() {}", "rust");

    assert_eq!(file.name, "main.rs");
    assert!(file.media_type.contains("rust"));

    let file = GeneratedFile::code("app.py", "print('hello')", "python");
    assert!(file.media_type.contains("python"));
}

#[test]
fn test_generated_file_extension() {
    let file = GeneratedFile::text("test.txt", "content");
    assert_eq!(file.extension(), Some("txt"));

    let file = GeneratedFile::code("main.rs", "code", "rust");
    assert_eq!(file.extension(), Some("rs"));
}

#[test]
fn test_generated_file_base64() {
    let file = GeneratedFile::from_base64("image.png", "aGVsbG8=", "image/png");

    assert!(file.is_base64);
    assert!(file.is_binary());
}

#[test]
fn test_generated_files() {
    let mut files = GeneratedFiles::new();

    files.add(GeneratedFile::text("a.txt", "A"));
    files.add(GeneratedFile::text("b.txt", "B"));
    files.add(GeneratedFile::from_base64("img.png", "data", "image/png"));

    assert_eq!(files.len(), 3);
    assert!(files.contains("a.txt"));
    assert!(!files.contains("c.txt"));

    let text_files = files.text_files();
    assert_eq!(text_files.len(), 2);

    let binary_files = files.binary_files();
    assert_eq!(binary_files.len(), 1);
}

#[test]
fn test_generated_file_size() {
    let file = GeneratedFile::text("test.txt", "Hello, world!");
    assert_eq!(file.size(), 13);
}