//! `config/read` + `config/value/write` handlers.
//!
//! Reads expose merged persisted settings from the active runtime snapshot. Writes use the
//! canonical JSONC-aware atomic settings mutator on the blocking thread pool.

use tracing::info;

use super::{HandlerContext, HandlerResult};

/// `config/read` — return the merged persisted settings layers plus a
/// per-source breakdown keyed by source name.
///
/// Runtime-only CLI/env overrides remain in `RuntimeConfig` and are not
/// misrepresented as persisted settings values on this wire endpoint.
pub(crate) async fn handle_config_read(
    params: coco_types::ConfigReadParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let loaded = match params.target {
        coco_types::ConfigReadTarget::Process => match load_process_settings(ctx).await {
            Ok(loaded) => loaded,
            Err(error) => return error,
        },
        coco_types::ConfigReadTarget::Session(_) => {
            let Some(runtime) = ctx.resolve_runtime().await else {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "config/read requires a live target session".to_string(),
                    data: None,
                };
            };
            runtime.runtime_publisher().current().settings.clone()
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

    info!(sources = sources.len(), "AppServerHost: config/read");
    HandlerResult::ok(coco_types::ConfigReadResult {
        config: merged_json,
        sources,
    })
}

async fn load_process_settings(
    ctx: &HandlerContext,
) -> Result<coco_config::settings::SettingsWithSource, HandlerResult> {
    let cwd = ctx.state.process_cwd().await?;
    let replacement = ctx.state.runtime_replacement_snapshot().await;
    let flag = replacement
        .as_ref()
        .and_then(|replacement| replacement.runtime_factory.flag_settings_path());
    let enabled = replacement
        .as_ref()
        .map(|replacement| replacement.runtime_factory.enabled_setting_sources())
        .unwrap_or_else(|| coco_config::parse_enabled_setting_sources(None));
    tokio::task::spawn_blocking(move || {
        load_process_settings_from_disk(
            &cwd,
            flag.as_deref(),
            &enabled,
            &coco_config::CatalogPaths::default(),
        )
    })
    .await
    .map_err(|error| HandlerResult::Err {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("config/read task panicked: {error}"),
        data: None,
    })?
    .map_err(|error| HandlerResult::Err {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("config/read: {error}"),
        data: None,
    })
}

fn load_process_settings_from_disk(
    cwd: &std::path::Path,
    flag: Option<&std::path::Path>,
    enabled: &std::collections::HashSet<coco_config::settings::SettingSource>,
    catalogs: &coco_config::CatalogPaths,
) -> coco_config::Result<coco_config::settings::SettingsWithSource> {
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    coco_config::settings::load_settings_with_roots(
        &roots,
        flag,
        &catalogs.user_settings,
        &catalogs.managed_settings,
        enabled,
    )
}

/// `config/value/write` — persist a single setting to the user,
/// project, or local settings file.
///
/// Supports dotted key paths like `"permissions.default_mode"` which
/// are navigated as nested JSON objects (intermediate objects are
/// created as needed).
///
/// Scope defaults to `"user"` (`config home/settings.json`) if not
/// specified. Valid scopes: `"user"`, `"project"`, `"local"`.
///
/// Errors:
/// - `INVALID_PARAMS` if scope is not one of user/project/local
/// - `INTERNAL_ERROR` on filesystem or JSON serialization failure
pub(crate) async fn handle_config_write(
    params: coco_types::ConfigWriteParams,
    ctx: &HandlerContext,
) -> HandlerResult {
    let (scope, target_path) = match params.target {
        coco_types::ConfigWriteTarget::User => {
            ("user", coco_config::global_config::user_settings_path())
        }
        coco_types::ConfigWriteTarget::Project(_) => {
            let Some(runtime) = ctx.resolve_runtime().await else {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "config/value/write requires a live target session".to_string(),
                    data: None,
                };
            };
            let cwd = runtime.original_cwd();
            let roots = crate::paths::settings_roots_for_cwd(cwd);
            (
                "project",
                coco_config::global_config::project_settings_path(roots.project_root()),
            )
        }
        coco_types::ConfigWriteTarget::Local(_) => {
            let Some(runtime) = ctx.resolve_runtime().await else {
                return HandlerResult::Err {
                    code: coco_types::error_codes::INVALID_REQUEST,
                    message: "config/value/write requires a live target session".to_string(),
                    data: None,
                };
            };
            let cwd = runtime.original_cwd();
            let roots = crate::paths::settings_roots_for_cwd(cwd);
            (
                "local",
                coco_config::global_config::local_settings_path(roots.local_root()),
            )
        }
    };
    let publish_targets = affected_runtime_publish_targets(ctx, scope, &target_path).await;
    let flag_settings = ctx
        .state
        .runtime_replacement_snapshot()
        .await
        .and_then(|replacement| replacement.runtime_factory.flag_settings_path());

    // Run the entire read/modify/write sequence on the blocking pool —
    // it's three sequential sync I/O calls on the same file so splitting
    // them across spawn_blocking boundaries would add latency without
    // freeing the worker any earlier.
    let key = params.key.clone();
    let value = params.value.clone();
    let path = target_path.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        coco_config::settings::writer::mutate_settings_and_republish(
            &path,
            move |doc| set_nested_json_key(doc, &key, value),
            flag_settings.as_deref(),
            &coco_config::CatalogPaths::default(),
            publish_targets,
        )
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            info!(
                key = %params.key,
                scope = %scope,
                path = %target_path.display(),
                "AppServerHost: config/value/write"
            );
            HandlerResult::ok_empty()
        }
        Ok(Err(coco_config::settings::writer::SettingsWriteError::Mutation { message })) => {
            HandlerResult::Err {
                code: coco_types::error_codes::INVALID_PARAMS,
                message: format!("config/value/write: {message}"),
                data: None,
            }
        }
        Ok(Err(error)) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("config/value/write: {error}"),
            data: None,
        },
        Err(join_err) => HandlerResult::Err {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: format!("config/value/write task panicked: {join_err}"),
            data: None,
        },
    }
}

async fn affected_runtime_publish_targets(
    ctx: &HandlerContext,
    scope: &str,
    target_path: &std::path::Path,
) -> Vec<coco_config::settings::writer::RuntimePublishTarget> {
    let mut targets = Vec::new();

    if let Some(runtime) = ctx.resolve_runtime().await {
        push_publish_target_if_affected(
            &mut targets,
            scope,
            target_path,
            runtime.original_cwd(),
            runtime.runtime_publisher(),
        );
    }

    if let Some(app_server) = &ctx.app_server {
        for session_id in app_server.registry().list_live() {
            let Some(handle) = app_server.registry().get(&session_id) else {
                continue;
            };
            let runtime = handle.into_session();
            push_publish_target_if_affected(
                &mut targets,
                scope,
                target_path,
                runtime.original_cwd(),
                runtime.runtime_publisher(),
            );
        }
    }
    targets
}

fn push_publish_target_if_affected(
    targets: &mut Vec<coco_config::settings::writer::RuntimePublishTarget>,
    scope: &str,
    target_path: &std::path::Path,
    cwd: &std::path::Path,
    publisher: std::sync::Arc<coco_config::RuntimePublisher>,
) {
    if targets
        .iter()
        .any(|target| std::sync::Arc::ptr_eq(&target.publisher, &publisher))
    {
        return;
    }
    let roots = crate::paths::settings_roots_for_cwd(cwd);
    let runtime_path = match scope {
        "user" => coco_config::global_config::user_settings_path(),
        "project" => coco_config::global_config::project_settings_path(roots.project_root()),
        "local" => coco_config::global_config::local_settings_path(roots.local_root()),
        _ => return,
    };
    if runtime_path == target_path {
        targets.push(coco_config::settings::writer::RuntimePublishTarget { roots, publisher });
    }
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
        return Err("settings document root is not an object".to_string());
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
        // Descend, creating an empty object only when the segment is missing.
        // Replacing an existing scalar would silently discard user settings.
        let entry = obj
            .entry((*segment).to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !entry.is_object() {
            return Err(format!("path segment {segment:?} is not an object"));
        }
        cursor = entry;
    }
    unreachable!("segments vec is non-empty, loop returns on last iteration")
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
