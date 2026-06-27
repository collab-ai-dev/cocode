//! PowerShell tool — advanced features ported from PowerShellTool/.
//!
//! Provides CLM (Constrained Language Mode) security analysis, PowerShell-specific
//! permission checking, command execution via pwsh, Windows path validation,
//! and output encoding handling (UTF-16 to UTF-8).

use std::collections::HashSet;
use std::sync::LazyLock;

// ── CLM (Constrained Language Mode) type allowlist ──
// Microsoft's CLM restricts .NET type usage to this allowlist
// when PS runs under AppLocker/WDAC system lockdown.

static CLM_ALLOWED_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Type accelerators (short names)
        "alias",
        "allowemptycollection",
        "allowemptystring",
        "allownull",
        "argumentcompleter",
        "argumentcompletions",
        "array",
        "bigint",
        "bool",
        "byte",
        "char",
        "cimclass",
        "cimconverter",
        "ciminstance",
        "cimtype",
        "cmdletbinding",
        "cultureinfo",
        "datetime",
        "decimal",
        "double",
        "dsclocalconfigurationmanager",
        "dscproperty",
        "dscresource",
        "experimentaction",
        "experimental",
        "experimentalfeature",
        "float",
        "guid",
        "hashtable",
        "int",
        "int16",
        "int32",
        "int64",
        "ipaddress",
        "ipendpoint",
        "long",
        "mailaddress",
        "norunspaceaffinity",
        "nullstring",
        "objectsecurity",
        "ordered",
        "outputtype",
        "parameter",
        "physicaladdress",
        "pscredential",
        "pscustomobject",
        "psdefaultvalue",
        "pslistmodifier",
        "psobject",
        "psprimitivedictionary",
        "pstypenameattribute",
        "ref",
        "regex",
        "sbyte",
        "securestring",
        "semver",
        "short",
        "single",
        "string",
        "supportswildcards",
        "switch",
        "timespan",
        "uint",
        "uint16",
        "uint32",
        "uint64",
        "ulong",
        "uri",
        "ushort",
        "validatecount",
        "validatedrive",
        "validatelength",
        "validatenotnull",
        "validatenotnullorempty",
        "validatenotnullorwhitespace",
        "validatepattern",
        "validaterange",
        "validatescript",
        "validateset",
        "validatetrusteddata",
        "validateuserdrive",
        "version",
        "void",
        "wildcardpattern",
        "x500distinguishedname",
        "x509certificate",
        "xml",
        "object",
        // Full qualified names
        "system.array",
        "system.boolean",
        "system.byte",
        "system.char",
        "system.datetime",
        "system.decimal",
        "system.double",
        "system.guid",
        "system.int16",
        "system.int32",
        "system.int64",
        "system.numerics.biginteger",
        "system.sbyte",
        "system.single",
        "system.string",
        "system.timespan",
        "system.uint16",
        "system.uint32",
        "system.uint64",
        "system.uri",
        "system.version",
        "system.void",
        "system.object",
        "system.collections.hashtable",
        "system.text.regularexpressions.regex",
        "system.globalization.cultureinfo",
        "system.net.ipaddress",
        "system.net.ipendpoint",
        "system.net.mail.mailaddress",
        "system.net.networkinformation.physicaladdress",
        "system.security.securestring",
        "system.security.cryptography.x509certificates.x509certificate",
        "system.security.cryptography.x509certificates.x500distinguishedname",
        "system.xml.xmldocument",
        "system.management.automation.pscredential",
        "system.management.automation.pscustomobject",
        "system.management.automation.pslistmodifier",
        "system.management.automation.psobject",
        "system.management.automation.psprimitivedictionary",
        "system.management.automation.psreference",
        "system.management.automation.semanticversion",
        "system.management.automation.switchparameter",
        "system.management.automation.wildcardpattern",
        "system.management.automation.language.nullstring",
        "microsoft.management.infrastructure.cimclass",
        "microsoft.management.infrastructure.cimconverter",
        "microsoft.management.infrastructure.ciminstance",
        "microsoft.management.infrastructure.cimtype",
        "system.collections.specialized.ordereddictionary",
        "system.security.accesscontrol.objectsecurity",
        "microsoft.powershell.commands.modulespecification",
    ]
    .into_iter()
    .collect()
});

/// Cmdlets that can write files at caller-specified paths.
/// Used to guard against git-internal path attacks.
static GIT_SAFETY_WRITE_CMDLETS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "new-item",
        "set-content",
        "add-content",
        "out-file",
        "copy-item",
        "move-item",
        "rename-item",
        "expand-archive",
        "invoke-webrequest",
        "invoke-restmethod",
    ]
    .into_iter()
    .collect()
});

/// PowerShell search commands for collapsible display.
static PS_SEARCH_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["select-string", "get-childitem", "findstr", "where.exe"]
        .into_iter()
        .collect()
});

/// PowerShell read/view commands for collapsible display.
static PS_READ_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "get-content",
        "get-item",
        "test-path",
        "resolve-path",
        "get-process",
        "get-service",
        "get-childitem",
        "get-location",
        "get-filehash",
        "get-acl",
        "format-hex",
    ]
    .into_iter()
    .collect()
});

/// PowerShell semantic-neutral commands.
static PS_SEMANTIC_NEUTRAL_COMMANDS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["write-output", "write-host"].into_iter().collect());

// ── CLM security analysis ──

/// Normalize a type name for CLM lookup.
/// Strips array suffix `[]` and generic brackets.
pub fn normalize_type_name(name: &str) -> String {
    let lower = name.to_lowercase();
    // Strip array suffix: "String[]" -> "string"
    let without_array = lower.strip_suffix("[]").unwrap_or(&lower);
    // Strip generic args: "List[int]" -> "list"
    let bracket_pos = without_array.find('[');
    match bracket_pos {
        Some(pos) => without_array[..pos].trim().to_string(),
        None => without_array.trim().to_string(),
    }
}

/// Check if a type name is in Microsoft's CLM allowlist.
/// Types NOT in this set are potentially unsafe (access system APIs that CLM blocks).
pub fn is_clm_allowed_type(type_name: &str) -> bool {
    let normalized = normalize_type_name(type_name);
    CLM_ALLOWED_TYPES.contains(normalized.as_str())
}

/// Analyze a PowerShell command for CLM security concerns.
/// Returns type names that are NOT in the CLM allowlist.
///
/// Simple heuristic: extract `[TypeName]` patterns from the command.
pub fn find_unsafe_type_references(command: &str) -> Vec<String> {
    let mut unsafe_types = Vec::new();
    let mut rest = command;

    while let Some(open) = rest.find('[') {
        let after_open = &rest[open + 1..];
        if let Some(close) = after_open.find(']') {
            let type_name = &after_open[..close];
            // Skip array indexing (numeric) and empty brackets
            if !type_name.is_empty()
                && !type_name
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c == ' ')
                && !is_clm_allowed_type(type_name)
            {
                unsafe_types.push(type_name.to_string());
            }
            rest = &after_open[close + 1..];
        } else {
            break;
        }
    }

    unsafe_types
}

// ── Permission checking ──

/// Result of a PowerShell security analysis.
#[derive(Debug, Clone)]
pub struct PsSecurityResult {
    /// Whether the command is safe to run without user approval.
    pub is_safe: bool,
    /// Reason for requiring approval (if not safe).
    pub reason: Option<String>,
    /// Unsafe type references found.
    pub unsafe_types: Vec<String>,
}

/// Analyze a PowerShell command for security concerns.
pub fn analyze_ps_security(command: &str) -> PsSecurityResult {
    // Check for unsafe CLM type references
    let unsafe_types = find_unsafe_type_references(command);
    if !unsafe_types.is_empty() {
        return PsSecurityResult {
            is_safe: false,
            reason: Some(format!(
                "Command uses types not in CLM allowlist: {}",
                unsafe_types.join(", ")
            )),
            unsafe_types,
        };
    }

    // Check for git-internal path writes
    let lower = command.to_lowercase();
    for cmdlet in GIT_SAFETY_WRITE_CMDLETS.iter() {
        if lower.contains(cmdlet) && has_git_internal_path(command) {
            return PsSecurityResult {
                is_safe: false,
                reason: Some(format!("Command writes to git-internal path via {cmdlet}")),
                unsafe_types: Vec::new(),
            };
        }
    }

    PsSecurityResult {
        is_safe: true,
        reason: None,
        unsafe_types: Vec::new(),
    }
}

/// Check if any argument references a git-internal path.
fn has_git_internal_path(command: &str) -> bool {
    let git_internal_patterns = [
        ".git/hooks/",
        ".git\\hooks\\",
        ".git/refs/",
        ".git\\refs\\",
        ".git/objects/",
        ".git\\objects\\",
        ".git/HEAD",
        ".git\\HEAD",
        ".git/config",
        ".git\\config",
    ];
    let lower = command.to_lowercase();
    git_internal_patterns
        .iter()
        .any(|p| lower.contains(&p.to_lowercase()))
}

// ── Command classification ──

/// Classify a PowerShell command as search or read.
pub fn classify_ps_command(command: &str) -> (bool, bool) {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return (false, false);
    }

    let parts: Vec<&str> = trimmed
        .split(&[';', '|'][..])
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return (false, false);
    }

    let mut has_search = false;
    let mut has_read = false;
    let mut has_non_neutral = false;

    for part in &parts {
        let base = part.split_whitespace().next().unwrap_or("").to_lowercase();
        if base.is_empty() {
            continue;
        }

        if PS_SEMANTIC_NEUTRAL_COMMANDS.contains(base.as_str()) {
            continue;
        }

        has_non_neutral = true;
        let is_search = PS_SEARCH_COMMANDS.contains(base.as_str());
        let is_read = PS_READ_COMMANDS.contains(base.as_str());

        if !is_search && !is_read {
            return (false, false);
        }

        if is_search {
            has_search = true;
        }
        if is_read {
            has_read = true;
        }
    }

    if !has_non_neutral {
        return (false, false);
    }

    (has_search, has_read)
}

// ── Windows path validation ──

/// Validate that a path is a valid Windows path (if running on Windows).
/// Checks for UNC path attacks.
pub fn is_vulnerable_unc_path(path: &str) -> bool {
    // UNC paths start with \\ or // and could be used for credential theft
    let trimmed = path.trim();
    (trimmed.starts_with("\\\\") || trimmed.starts_with("//"))
        && !trimmed.starts_with("\\\\?\\")
        && !trimmed.starts_with("\\\\.\\")
}

// ── Output encoding ──

/// Attempt to decode UTF-16 LE output to UTF-8.
/// PowerShell on Windows may produce UTF-16 LE output with BOM.
pub fn decode_ps_output(bytes: &[u8]) -> String {
    // Check for UTF-16 LE BOM (0xFF 0xFE)
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        // UTF-16 LE: each character is 2 bytes
        let u16_iter = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]));
        let decoded: String = char::decode_utf16(u16_iter)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        return decoded;
    }

    // Check for UTF-16 BE BOM (0xFE 0xFF)
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16_iter = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
        let decoded: String = char::decode_utf16(u16_iter)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        return decoded;
    }

    // Default: UTF-8 (lossy)
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
#[path = "powershell.test.rs"]
mod tests;
