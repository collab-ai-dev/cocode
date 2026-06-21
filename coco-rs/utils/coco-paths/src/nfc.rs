//! Unicode NFC normalization wrapper.
//!
//! TS calls `.normalize('NFC')` at every path computation site
//! (`memdir/paths.ts:232`, `utils/sessionStoragePortable.ts:341-343`,
//! `getWorktreePathsPortable.ts:23`). Without NFC, the same logical
//! project path can produce two different on-disk directories on
//! filesystems that don't normalise themselves (Linux ext4 stores
//! bytes verbatim; macOS APFS compares NFC-equivalently but stores
//! verbatim too — so a decomposed input creates a decomposed
//! directory that compares equal under APFS but not under ext4 or
//! when copied across volumes).
//!
//! This thin wrapper exists so callers don't import the
//! `unicode-normalization` crate directly — keeping the dependency
//! surface visible at one well-known boundary.

use unicode_normalization::UnicodeNormalization;

/// NFC-normalise `s` and return the resulting `String`.
pub fn normalize_nfc(s: &str) -> String {
    s.nfc().collect()
}

/// NFKC-normalise `s` (compatibility decomposition + canonical
/// composition). Differs from [`normalize_nfc`]: fullwidth glyphs
/// (`U+FF0E` "．"), one/two-dot leaders (`U+2024` "․" / `U+2025`
/// "‥"), and small full stop (`U+FE52` "﹒") all collapse into ASCII
/// `.`. Used by path validators to short-circuit Unicode-traversal
/// attacks that would otherwise pass a literal-codepoint check.
/// TS `String.prototype.normalize('NFKC')` parity.
pub fn normalize_nfkc(s: &str) -> String {
    s.nfkc().collect()
}

#[cfg(test)]
#[path = "nfc.test.rs"]
mod tests;
