//! `config/read` + `config/value/write` handlers.
//!
//! Both walk the coco settings layers rooted at the active session's
//! cwd (or the process cwd when no session is active) and read/write
//! JSON settings files via the blocking thread pool.

use tracing::info;

use super::HandlerContext;
use super::HandlerResult;

/// `config/read` — return the merged effective configuration plus a
/// per-source breakdown keyed by source name.
///
/// Delegates to [`coco_config::settings::load_settings`] with the
/// session's cwd (if a session is active) or the CLI's cwd as the
/// project root. Returns the JSON-serialized merged view and a
/// per-source map suitable for clients that want to display or
/// override specific layers.
pub(super) async fn handle_config_read(ctx: &HandlerContext) -> HandlerResult {
    // Resolve cwd — prefer active session's cwd, fall back to
    // process cwd. Project/local settings live under cwd, so this
    // matters for clients that have multiple repos open.
    let cwd = {
        let slot = ctx.state.session.read().await;
        slot.as_ref()
            .map(|s| std::path::PathBuf::from(&s.cwd))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    };

    // `load_settings` reads up to 6 layered JSON files synchronously; run
    // it on the blocking pool so frequent `config/read` polls don't stall
    // the tokio worker.
    let cwd_for_load = cwd.clone();
    let load_result = tokio::task::spawn_blocking(move || {
        coco_config::settings::load_settings(&cwd_for_load, None)
    })
    .await;
    let loaded = match load_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("config/read: failed to load settings: {e}"),
                data: None,
            };
        }
        Err(join_err) => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("config/read task panicked: {join_err}"),
                data: None,
            };
        }
    };

    // Serialize the merged settings as JSON for the wire.
    let merged_json = match serde_json::to_value(&loaded.merged) {
        Ok(v) => v,
        Err(e) => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("config/read: failed to serialize settings: {e}"),
                data: None,
            };
        }
    };

    // Flatten the per-source map to string keys for the wire format.
    let mut sources: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    for (source, value) in &loaded.per_source {
        sources.insert(source.to_string(), value.clone());
    }

    info!(sources = sources.len(), "SdkServer: config/read");
    HandlerResult::ok(coco_types::ConfigReadResult {
        config: merged_json,
        sources,
    })
}

/// `config/value/write` — persist a single setting to the user,
/// project, or local settings file.
///
/// Supports dotted key paths like `"permissions.default_mode"` which
/// are navigated as nested JSON objects (intermediate objects are
/// created as needed).
///
/// Scope defaults to `"user"` (`~/.coco/settings.json`) if not
/// specified. Valid scopes: `"user"`, `"project"`, `"local"`.
///
/// Errors:
/// - `INVALID_PARAMS` if scope is not one of user/project/local
/// - `INTERNAL_ERROR` on filesystem or JSON serialization failure
pub(super) async fn handle_config_write(
    params: coco_types::ConfigWriteParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let scope = params.scope.as_deref().unwrap_or("user");
    let cwd = {
        let slot = ctx.state.session.read().await;
        slot.as_ref()
            .map(|s| std::path::PathBuf::from(&s.cwd))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    };

    let target_path = match scope {
        "user" => coco_config::global_config::user_settings_path(),
        "project" => coco_config::global_config::project_settings_path(&cwd),
        "local" => coco_config::global_config::local_settings_path(&cwd),
        other => {
            return HandlerResult::Err {
                code: coco_types::error_codes::INVALID_PARAMS,
                message: format!(
                    "config/value/write: invalid scope {other:?}; expected user|project|local"
                ),
                data: None,
            };
        }
    };

    // Run the entire read/modify/write sequence on the blocking pool —
    // it's three sequential sync I/O calls on the same file so splitting
    // them across spawn_blocking boundaries would add latency without
    // freeing the worker any earlier.
    let key = params.key.clone();
    let value = params.value.clone();
    let path = target_path.clone();
    let write_result = tokio::task::spawn_blocking(move || -> Result<(), ConfigWriteError> {
        let mut doc: serde_json::Value = if path.exists() {
            let contents = std::fs::read_to_string(&path).map_err(|e| {
                ConfigWriteError::Io(format!("failed to read {}: {e}", path.display()))
            })?;
            serde_json::from_str(&contents).map_err(|e| {
                ConfigWriteError::InvalidExisting(format!(
                    "existing file at {} is not valid JSON: {e}",
                    path.display()
                ))
            })?
        } else {
            serde_json::Value::Object(serde_json::Map::new())
        };

        set_nested_json_key(&mut doc, &key, value).map_err(ConfigWriteError::InvalidKey)?;
        validate_settings_document(&doc).map_err(ConfigWriteError::InvalidKey)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigWriteError::Io(format!("failed to create parent dir: {e}")))?;
        }

        let serialized = serde_json::to_string_pretty(&doc)
            .map_err(|e| ConfigWriteError::Io(format!("failed to serialize: {e}")))?;
        std::fs::write(&path, serialized).map_err(|e| {
            ConfigWriteError::Io(format!("failed to write {}: {e}", path.display()))
        })?;
        Ok(())
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            info!(
                key = %params.key,
                scope = %scope,
                path = %target_path.display(),
                "SdkServer: config/value/write"
            );
            HandlerResult::ok_empty()
        }
        Ok(Err(ConfigWriteError::InvalidKey(msg))) => HandlerResult::Err {
            code: coco_types::error_codes::INVALID_PARAMS,
            message: format!("config/value/write: {msg}"),
            data: None,
        },
        Ok(Err(ConfigWriteError::InvalidExisting(msg) | ConfigWriteError::Io(msg))) => {
            HandlerResult::Err {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("config/value/write: {msg}"),
                data: None,
            }
        }
        Err(join_err) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("config/value/write task panicked: {join_err}"),
            data: None,
        },
    }
}

/// Internal error tag for the `config/value/write` blocking task. Mapped
/// to JSON-RPC error codes at the await boundary.
enum ConfigWriteError {
    /// Bad dotted-path key — caller error.
    InvalidKey(String),
    /// Existing settings file is not valid JSON.
    InvalidExisting(String),
    /// File I/O failure (read/write/mkdir/serialize).
    Io(String),
}

/// Set a dotted-path key on a JSON object, creating intermediate
/// objects as needed. Used by `config/value/write` so clients can
/// target nested settings like `"permissions.default_mode"`.
///
/// Errors if an intermediate path segment exists but is not an object
/// (e.g. `a.b.c` where `a.b` is a string).
fn set_nested_json_key(
    doc: &mut serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    if !doc.is_object() {
        *doc = serde_json::Value::Object(serde_json::Map::new());
    }
    let segments: Vec<&str> = key.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return Err(format!("invalid key path {key:?}"));
    }
    let mut cursor = doc;
    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let obj = cursor
            .as_object_mut()
            .ok_or_else(|| format!("path segment {segment:?} is not an object"))?;
        if is_last {
            obj.insert((*segment).to_string(), value);
            return Ok(());
        }
        // Descend, creating an empty object if the intermediate is
        // missing OR not an object.
        let entry = obj
            .entry((*segment).to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !entry.is_object() {
            *entry = serde_json::Value::Object(serde_json::Map::new());
        }
        cursor = entry;
    }
    unreachable!("segments vec is non-empty, loop returns on last iteration")
}

fn validate_settings_document(doc: &serde_json::Value) -> Result<(), String> {
    let body = serde_json::to_string(doc).map_err(|e| format!("failed to serialize: {e}"))?;
    coco_config::settings::parse_settings(&body)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
