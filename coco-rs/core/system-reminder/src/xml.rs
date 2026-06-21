//! XML wrapping for reminder content.
//!
//! Format: `<system-reminder>\n{content}\n</system-reminder>`.
//!
//! ```text
//! <system-reminder>
//! {content}
//! </system-reminder>
//! ```
//!
//! Callers of this module should treat the wrapping as idempotent at the API
//! layer: [`ensure_wrapped`] checks for an existing prefix and skips
//! re-wrapping.

use regex_lite::Regex;

use crate::types::XmlTag;

/// Wrap `content` with the given XML tag.
///
/// For [`XmlTag::None`] the content is returned unchanged. For any other tag
/// the returned string is `<tag>\n{content}\n</tag>`.
pub fn wrap_with_tag(content: &str, tag: XmlTag) -> String {
    match tag.tag_name() {
        Some(name) => format!("<{name}>\n{content}\n</{name}>"),
        None => content.to_string(),
    }
}

/// Shorthand for `wrap_with_tag(content, XmlTag::SystemReminder)`.
///
/// Delegates to [`coco_messages::wrapping::wrap_in_system_reminder`] so the
/// canonical format string lives in one place — fixes propagate without drift
/// between the system-reminder pipeline and post-compact / plan-mode / CLI
/// paths that wrap text directly.
pub fn wrap_system_reminder(content: &str) -> String {
    coco_messages::wrapping::wrap_in_system_reminder(content)
}

/// Wrap content only if it isn't already wrapped in a `<system-reminder>` tag.
///
/// Used by the normalizer to avoid double-wrapping reminders that passed
/// through more than one mutation.
pub fn ensure_wrapped(content: &str) -> String {
    if content.starts_with("<system-reminder>") {
        content.to_string()
    } else {
        wrap_system_reminder(content)
    }
}

/// Extract content from a `<system-reminder>` block, if present. Returns the
/// inner content without the enclosing newlines.
pub fn extract_system_reminder(text: &str) -> Option<&str> {
    extract_tag_content(text, "system-reminder")
}

/// Extract content for an arbitrary XML tag by name.
fn extract_tag_content<'a>(text: &'a str, tag_name: &str) -> Option<&'a str> {
    // Non-greedy inner match so nested same-named tags extract the first block.
    let pattern = format!(r"<{tag_name}>\n?([\s\S]*?)\n?</{tag_name}>");
    let re = Regex::new(&pattern).ok()?;
    re.captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

/// Check whether the text contains a complete system-reminder tag pair.
pub fn has_system_reminder(text: &str) -> bool {
    text.contains("<system-reminder>") && text.contains("</system-reminder>")
}

/// Check whether the text contains a complete pair for the given tag.
/// [`XmlTag::None`] always returns `false`.
pub fn has_tag(text: &str, tag: XmlTag) -> bool {
    match tag.tag_name() {
        Some(name) => text.contains(&format!("<{name}>")) && text.contains(&format!("</{name}>")),
        None => false,
    }
}

#[cfg(test)]
#[path = "xml.test.rs"]
mod tests;
