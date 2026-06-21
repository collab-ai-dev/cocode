use serde::Deserialize;
use serde::Serialize;

/// Built-in plugin that ships with the CLI (can be enabled/disabled by users).
///
/// NOTE: To avoid L1→L4 dependency on coco-plugins, the manifest field uses
/// serde_json::Value. The consuming crate (coco-plugins) deserializes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinPluginDefinition {
    pub name: String,
    pub display_name: String,
    pub description: String,
    /// PluginManifest — deserialized by coco-plugins, kept as Value here.
    pub manifest: serde_json::Value,
}
