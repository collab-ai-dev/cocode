# coco-utils-json-to-toml

Convert `serde_json::Value` to semantically equivalent `toml::Value`.

## Key Types
- `json_to_toml(JsonValue) -> TomlValue` — single public function. Nulls map to empty strings; numbers prefer integer over float over string fallback.
