//! Generate the user-facing template for `~/.coco/keybindings.json`.
//!
//! Derives the template from the live default-bindings table and filters out
//! [`crate::reserved::NON_REBINDABLE`] entries so users don't see (and
//! get a `/doctor` warning for) shortcuts they cannot rebind.

use crate::KeybindingsConfig;
use crate::defaults::default_blocks;
use crate::reserved::NON_REBINDABLE;
use crate::reserved::normalize_key_for_comparison;

/// Generate the template content for `~/.coco/keybindings.json`.
///
/// The result is a JSON object with `$schema`, `$docs`, and a
/// `bindings` array. Always ends with a trailing newline (Unix
/// convention). Returns an error only if
/// serde_json fails — which it cannot for our typed shape, but the
/// surface is still `Result`-typed for consistency with [`KeybindingsConfig::to_json_pretty`].
pub fn generate_template() -> Result<String, serde_json::Error> {
    let mut config = KeybindingsConfig {
        // Re-point if/when coco-rs publishes its own SchemaStore entry.
        schema: Some("https://www.schemastore.org/claude-code-keybindings.json".to_string()),
        docs: Some("https://code.claude.com/docs/en/keybindings".to_string()),
        bindings: default_blocks(),
    };

    // Filter out NON_REBINDABLE keys. We compare against the canonical
    // normalized form so user spelling (`Ctrl+C`, `control+c`, `cmd+c`)
    // all collapse to the same key.
    let reserved_canonicals: Vec<String> = NON_REBINDABLE
        .iter()
        .map(|r| normalize_key_for_comparison(r.key))
        .collect();

    for block in &mut config.bindings {
        block
            .bindings
            .retain(|chord, _| !reserved_canonicals.contains(&normalize_key_for_comparison(chord)));
    }
    // Drop now-empty blocks.
    config.bindings.retain(|b| !b.bindings.is_empty());

    config.to_json_pretty()
}

#[cfg(test)]
#[path = "template.test.rs"]
mod tests;
