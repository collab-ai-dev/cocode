use super::*;

// ── CLM type analysis tests ──

#[test]
fn test_normalize_type_name_basic() {
    assert_eq!(normalize_type_name("String"), "string");
    assert_eq!(normalize_type_name("System.Int32"), "system.int32");
}

#[test]
fn test_normalize_type_name_array() {
    assert_eq!(normalize_type_name("String[]"), "string");
    assert_eq!(normalize_type_name("System.Byte[]"), "system.byte");
}

#[test]
fn test_normalize_type_name_generic() {
    assert_eq!(normalize_type_name("List[int]"), "list");
}

#[test]
fn test_clm_allowed_safe_types() {
    assert!(is_clm_allowed_type("string"));
    assert!(is_clm_allowed_type("String"));
    assert!(is_clm_allowed_type("int"));
    assert!(is_clm_allowed_type("System.Int32"));
    assert!(is_clm_allowed_type("hashtable"));
    assert!(is_clm_allowed_type("DateTime"));
    assert!(is_clm_allowed_type("regex"));
    assert!(is_clm_allowed_type("String[]")); // Array of allowed type
}

#[test]
fn test_clm_blocked_unsafe_types() {
    // These were explicitly removed from CLM allowlist for security
    assert!(!is_clm_allowed_type("adsi"));
    assert!(!is_clm_allowed_type("adsisearcher"));
    assert!(!is_clm_allowed_type("wmi"));
    assert!(!is_clm_allowed_type("wmiclass"));
    assert!(!is_clm_allowed_type("wmisearcher"));
    assert!(!is_clm_allowed_type("cimsession"));
    // General dangerous types
    assert!(!is_clm_allowed_type("System.Diagnostics.Process"));
    assert!(!is_clm_allowed_type("System.Reflection.Assembly"));
    assert!(!is_clm_allowed_type("System.IO.StreamWriter"));
}

#[test]
fn test_find_unsafe_type_references() {
    let cmd = "$p = [System.Diagnostics.Process]::Start('calc')";
    let unsafe_types = find_unsafe_type_references(cmd);
    assert_eq!(unsafe_types.len(), 1);
    assert_eq!(unsafe_types[0], "System.Diagnostics.Process");
}

#[test]
fn test_find_unsafe_type_references_safe_command() {
    let cmd = "$x = [int]42; $s = [string]'hello'";
    let unsafe_types = find_unsafe_type_references(cmd);
    assert!(unsafe_types.is_empty());
}

#[test]
fn test_find_unsafe_type_references_mixed() {
    let cmd = "[string]$name = 'test'; [System.Net.WebClient]::new()";
    let unsafe_types = find_unsafe_type_references(cmd);
    assert_eq!(unsafe_types.len(), 1);
    assert_eq!(unsafe_types[0], "System.Net.WebClient");
}

// ── Security analysis tests ──

#[test]
fn test_analyze_safe_command() {
    let result = analyze_ps_security("Get-Content -Path ./file.txt");
    assert!(result.is_safe);
    assert!(result.reason.is_none());
}

#[test]
fn test_analyze_unsafe_type_cast() {
    let result = analyze_ps_security("[System.Diagnostics.Process]::Start('calc')");
    assert!(!result.is_safe);
    assert!(result.reason.as_ref().unwrap().contains("CLM allowlist"));
}

#[test]
fn test_analyze_git_internal_path_write() {
    let result = analyze_ps_security("Set-Content -Path .git/hooks/pre-commit -Value 'malicious'");
    assert!(!result.is_safe);
    assert!(result.reason.as_ref().unwrap().contains("git-internal"));
}

// ── Command classification tests ──

#[test]
fn test_classify_ps_search() {
    let (is_search, _) = classify_ps_command("Select-String -Pattern 'TODO' -Path *.rs");
    assert!(is_search);
}

#[test]
fn test_classify_ps_read() {
    let (_, is_read) = classify_ps_command("Get-Content file.txt");
    assert!(is_read);
}

#[test]
fn test_classify_ps_non_collapsible() {
    let (is_search, is_read) = classify_ps_command("Install-Module -Name Az");
    assert!(!is_search);
    assert!(!is_read);
}

// ── UNC path validation tests ──

#[test]
fn test_unc_path_detection() {
    assert!(is_vulnerable_unc_path("\\\\evil-server\\share"));
    assert!(is_vulnerable_unc_path("//evil-server/share"));
    // Extended-length path prefix is safe
    assert!(!is_vulnerable_unc_path("\\\\?\\C:\\path"));
    // Device path prefix is safe
    assert!(!is_vulnerable_unc_path("\\\\.\\COM1"));
    // Regular paths are safe
    assert!(!is_vulnerable_unc_path("C:\\Users\\file.txt"));
    assert!(!is_vulnerable_unc_path("./relative/path"));
}

// ── Output encoding tests ──

#[test]
fn test_decode_utf8_output() {
    let bytes = b"Hello, World!";
    assert_eq!(decode_ps_output(bytes), "Hello, World!");
}

#[test]
fn test_decode_utf16_le_output() {
    // UTF-16 LE BOM + "Hi"
    let bytes: Vec<u8> = vec![
        0xFF, 0xFE, // BOM
        b'H', 0x00, // 'H'
        b'i', 0x00, // 'i'
    ];
    assert_eq!(decode_ps_output(&bytes), "Hi");
}

#[test]
fn test_decode_utf16_be_output() {
    // UTF-16 BE BOM + "Hi"
    let bytes: Vec<u8> = vec![
        0xFE, 0xFF, // BOM
        0x00, b'H', // 'H'
        0x00, b'i', // 'i'
    ];
    assert_eq!(decode_ps_output(&bytes), "Hi");
}
