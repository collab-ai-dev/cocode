use serde::Deserialize;
use serde::Serialize;

/// User category used for ant-only feature gates.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserType {
    Human,
    Api,
    /// Internal Anthropic user — unlocks experimental skills and prompts.
    Ant,
}

impl UserType {
    /// Whether this user type is the internal Anthropic role.
    pub fn is_ant(self) -> bool {
        matches!(self, UserType::Ant)
    }

    /// Read from the `COCO_USER_TYPE` env var (legacy: `USER_TYPE`).
    /// Returns `Human` if neither is set.
    pub fn from_env() -> Self {
        let raw = std::env::var("COCO_USER_TYPE")
            .or_else(|_| std::env::var("USER_TYPE"))
            .unwrap_or_default();
        match raw.as_str() {
            "ant" => UserType::Ant,
            "api" => UserType::Api,
            _ => UserType::Human,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entrypoint {
    Cli,
    SdkTs,
    SdkPy,
    Vscode,
    Jetbrains,
    Web,
}

// `SerializedMessage` (which embeds `Message`) lives in `coco-messages`;
// log persistence is a Core-layer concern, not a foundational types one.

/// Log file option for session listing.
pub struct LogOption {
    pub date: String,
    pub path: String,
}
