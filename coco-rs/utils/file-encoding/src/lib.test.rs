use super::*;

#[test]
fn test_detect_encoding_utf8_no_bom() {
    let bytes = b"Hello World";
    assert_eq!(detect_encoding(bytes), Encoding::Utf8);
}

#[test]
fn test_detect_encoding_utf8_with_bom() {
    let bytes = [0xEF, 0xBB, 0xBF, b'H', b'e', b'l', b'l', b'o'];
    assert_eq!(detect_encoding(&bytes), Encoding::Utf8WithBom);
}

#[test]
fn test_detect_encoding_utf16le() {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("Hello".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(detect_encoding(&bytes), Encoding::Utf16Le);
}

#[test]
fn test_detect_encoding_utf16be() {
    let mut bytes = vec![0xFE, 0xFF];
    bytes.extend("Hello".encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(detect_encoding(&bytes), Encoding::Utf16Be);
}

#[test]
fn test_decode_utf8() {
    let bytes = b"Hello World";
    let content = Encoding::Utf8.decode(bytes).unwrap();
    assert_eq!(content, "Hello World");
}

#[test]
fn test_decode_utf8_with_bom() {
    let bytes = [0xEF, 0xBB, 0xBF, b'H', b'i'];
    let content = Encoding::Utf8.decode(&bytes).unwrap();
    assert_eq!(content, "Hi");
}

#[test]
fn test_decode_utf16le() {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("Hi".encode_utf16().flat_map(u16::to_le_bytes));
    let content = Encoding::Utf16Le.decode(&bytes).unwrap();
    assert_eq!(content, "Hi");
}

#[test]
fn test_decode_utf16be() {
    let mut bytes = vec![0xFE, 0xFF];
    bytes.extend("Hi".encode_utf16().flat_map(u16::to_be_bytes));
    let content = Encoding::Utf16Be.decode(&bytes).unwrap();
    assert_eq!(content, "Hi");
}

#[test]
fn test_detect_line_ending_lf() {
    let content = "line1\nline2\nline3";
    assert_eq!(detect_line_ending(content), LineEnding::Lf);
}

#[test]
fn test_detect_line_ending_crlf() {
    let content = "line1\r\nline2\r\nline3";
    assert_eq!(detect_line_ending(content), LineEnding::CrLf);
}

#[test]
fn test_detect_line_ending_cr_only_returns_lf() {
    // CR-only line endings (old Mac OS 9) are rare and not worth special casing
    // Simplified detection returns LF for CR-only content
    let content = "line1\rline2\rline3";
    assert_eq!(detect_line_ending(content), LineEnding::Lf);
}

#[test]
fn test_detect_line_ending_mixed_prefers_crlf() {
    let content = "line1\r\nline2\nline3\r\n";
    assert_eq!(detect_line_ending(content), LineEnding::CrLf);
}

#[test]
fn test_detect_line_ending_no_newlines() {
    let content = "no newlines here";
    assert_eq!(detect_line_ending(content), LineEnding::Lf);
}

#[test]
fn test_normalize_line_endings_to_crlf() {
    let content = "line1\nline2\nline3";
    let normalized = normalize_line_endings(content, LineEnding::CrLf);
    assert_eq!(normalized, "line1\r\nline2\r\nline3");
}

#[test]
fn test_normalize_line_endings_to_lf() {
    let content = "line1\r\nline2\r\nline3";
    let normalized = normalize_line_endings(content, LineEnding::Lf);
    assert_eq!(normalized, "line1\nline2\nline3");
}

#[test]
fn test_normalize_mixed_to_lf() {
    let content = "line1\r\nline2\rline3\nline4";
    let normalized = normalize_line_endings(content, LineEnding::Lf);
    assert_eq!(normalized, "line1\nline2\nline3\nline4");
}

#[test]
fn test_encode_utf16le() {
    let encoded = Encoding::Utf16Le.encode("Hi");
    // 'H' = 0x0048, 'i' = 0x0069 in UTF-16LE
    assert_eq!(encoded, vec![0x48, 0x00, 0x69, 0x00]);
}

#[test]
fn test_encode_utf16be() {
    let encoded = Encoding::Utf16Be.encode("Hi");
    // 'H' = 0x0048, 'i' = 0x0069 in UTF-16BE
    assert_eq!(encoded, vec![0x00, 0x48, 0x00, 0x69]);
}

#[test]
fn test_roundtrip_utf8_lf() {
    let original = "Hello\nWorld\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.txt");

    write_with_format(&path, original, Encoding::Utf8, LineEnding::Lf).unwrap();
    let (content, enc, le) = read_with_format(&path).unwrap();

    assert_eq!(content, original);
    assert_eq!(enc, Encoding::Utf8);
    assert_eq!(le, LineEnding::Lf);
}

#[test]
fn test_roundtrip_utf8_crlf() {
    let original = "Hello\nWorld\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.txt");

    write_with_format(&path, original, Encoding::Utf8, LineEnding::CrLf).unwrap();
    let (content, enc, le) = read_with_format(&path).unwrap();

    assert_eq!(content, "Hello\r\nWorld\r\n");
    assert_eq!(enc, Encoding::Utf8);
    assert_eq!(le, LineEnding::CrLf);
}

#[test]
fn test_roundtrip_utf16le() {
    let original = "Hello\nWorld\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.txt");

    write_with_format(&path, original, Encoding::Utf16Le, LineEnding::Lf).unwrap();
    let (content, enc, le) = read_with_format(&path).unwrap();

    assert_eq!(content, original);
    assert_eq!(enc, Encoding::Utf16Le);
    assert_eq!(le, LineEnding::Lf);
}

#[test]
fn test_roundtrip_utf16be_crlf() {
    let original = "Hello\nWorld\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.txt");

    write_with_format(&path, original, Encoding::Utf16Be, LineEnding::CrLf).unwrap();
    let (content, enc, le) = read_with_format(&path).unwrap();

    assert_eq!(content, "Hello\r\nWorld\r\n");
    assert_eq!(enc, Encoding::Utf16Be);
    assert_eq!(le, LineEnding::CrLf);
}

#[test]
fn test_bom_bytes() {
    assert!(Encoding::Utf8.bom().is_empty());
    assert_eq!(Encoding::Utf8WithBom.bom(), &[0xEF, 0xBB, 0xBF]);
    assert_eq!(Encoding::Utf16Le.bom(), &[0xFF, 0xFE]);
    assert_eq!(Encoding::Utf16Be.bom(), &[0xFE, 0xFF]);
}

#[test]
fn test_line_ending_as_str() {
    assert_eq!(LineEnding::Lf.as_str(), "\n");
    assert_eq!(LineEnding::CrLf.as_str(), "\r\n");
    assert_eq!(LineEnding::Cr.as_str(), "\r");
}

#[test]
fn test_has_trailing_newline() {
    assert!(has_trailing_newline("hello\n"));
    assert!(has_trailing_newline("hello\r\n"));
    assert!(!has_trailing_newline("hello"));
    assert!(!has_trailing_newline(""));
}

#[test]
fn test_preserve_trailing_newline_add() {
    // Original had trailing newline, modified doesn't - add it
    let preserved = preserve_trailing_newline("hello\n", "world");
    assert_eq!(preserved, "world\n");
}

#[test]
fn test_preserve_trailing_newline_remove() {
    // Original didn't have trailing newline, modified does - remove it
    let preserved = preserve_trailing_newline("hello", "world\n");
    assert_eq!(preserved, "world");
}

#[test]
fn test_preserve_trailing_newline_keep_both() {
    // Both have trailing newline - keep as is
    let preserved = preserve_trailing_newline("hello\n", "world\n");
    assert_eq!(preserved, "world\n");
}

#[test]
fn test_preserve_trailing_newline_keep_neither() {
    // Neither has trailing newline - keep as is
    let preserved = preserve_trailing_newline("hello", "world");
    assert_eq!(preserved, "world");
}

#[test]
fn test_roundtrip_utf8_with_bom() {
    let original = "Hello\nWorld\n";
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.txt");

    // Write with BOM
    write_with_format(&path, original, Encoding::Utf8WithBom, LineEnding::Lf).unwrap();

    // Verify BOM is written
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[0..3], &[0xEF, 0xBB, 0xBF]);

    // Read back and verify encoding is detected as Utf8WithBom
    let (content, enc, le) = read_with_format(&path).unwrap();
    assert_eq!(content, original);
    assert_eq!(enc, Encoding::Utf8WithBom);
    assert_eq!(le, LineEnding::Lf);
}
