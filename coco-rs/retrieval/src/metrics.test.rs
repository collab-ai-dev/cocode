use super::*;

#[test]
fn test_valid_rust_code() {
    let code = r#"
fn main() {
    println!("Hello, world!");
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    assert!(is_valid_file(code));
}

#[test]
fn test_empty_file() {
    assert!(!is_valid_file(""));
}

#[test]
fn test_minified_code() {
    // Long single line (>300 chars)
    let minified = "a".repeat(500);
    assert!(!is_valid_file(&minified));
}

#[test]
fn test_binary_content() {
    // Low alphanumeric fraction
    let binary = "\x00\x01\x02\x03\x04\x05".repeat(100);
    assert!(!is_valid_file(&binary));
}

#[test]
fn test_data_file() {
    // High number fraction (>50% digits)
    // "123456789\n" has 9 digits / 10 chars = 90%
    let data = "123456789\n".repeat(100);
    assert!(!is_valid_file(&data));
}

#[test]
fn test_validate_file_reasons() {
    assert_eq!(validate_file(""), Err("empty file"));
    assert_eq!(
        validate_file(&"a".repeat(500)),
        Err("line too long (>300 chars, likely minified)")
    );
}
