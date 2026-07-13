use std::sync::Arc;

use crate::session_runtime::SessionHandle;

pub struct SkillOverridesUpdate {
    pub result: coco_types::SkillOverridesSaveResult,
    pub commands: Option<Vec<coco_types::SlashCommandInfo>>,
}

pub async fn write_skill_overrides(
    session: &SessionHandle,
    patch: serde_json::Value,
    runtime_publisher: Option<&Arc<coco_config::RuntimePublisher>>,
    cwd: &std::path::Path,
    flag_settings: Option<&std::path::Path>,
) -> SkillOverridesUpdate {
    let Some(publisher) = runtime_publisher else {
        return SkillOverridesUpdate {
            result: coco_types::SkillOverridesSaveResult::Err {
                kind: coco_types::SkillOverridesSaveErrorKind::NoPublisher,
                message: "settings hot-reload disabled; restart the process to pick up changes"
                    .to_string(),
            },
            commands: None,
        };
    };

    let catalogs = coco_config::CatalogPaths::default();
    let roots = coco_config::SettingsRoots::new(session.project_root().clone(), cwd.to_path_buf());
    let write_result = coco_config::write_local_settings_with_roots(
        roots,
        flag_settings.map(std::path::Path::to_path_buf),
        catalogs,
        Arc::clone(publisher),
        patch,
    )
    .await;

    match write_result {
        Ok(()) => {
            // Use the freshly-republished RuntimeConfig so rebuilt registries
            // and per-turn engine config see the new skill override tiers.
            let fresh = publisher.current();
            session
                .set_skill_overrides(Arc::new(fresh.skill_overrides.clone()))
                .await;
            let _ = session.reload_plugins_with(cwd, &fresh).await;
            let commands = crate::session_dialogs::build_available_commands_payload(session).await;
            SkillOverridesUpdate {
                result: coco_types::SkillOverridesSaveResult::Ok,
                commands: Some(commands),
            }
        }
        Err(error) => SkillOverridesUpdate {
            result: coco_types::SkillOverridesSaveResult::Err {
                kind: skill_override_save_error_kind(&error),
                message: error.to_string(),
            },
            commands: None,
        },
    }
}

fn skill_override_save_error_kind(
    error: &coco_config::SettingsWriteError,
) -> coco_types::SkillOverridesSaveErrorKind {
    use coco_config::SettingsWriteError as Error;
    use coco_types::SkillOverridesSaveErrorKind as Kind;
    match error {
        Error::Io { .. } => Kind::Io,
        Error::Parse { .. } => Kind::Parse,
        Error::Rebuild { .. } => Kind::Rebuild,
    }
}
