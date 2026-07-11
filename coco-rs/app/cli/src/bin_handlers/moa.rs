//! `coco moa <action>` — manage MoA presets in user settings.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use coco_cli::MoaAction;
use coco_cli::MoaFanoutArg;
use coco_config::MoaFanout;
use coco_config::MoaPresetSettings;
use coco_config::MoaSettings;
use coco_config::global_config;
use coco_types::ProviderModelSelection;
use serde_json::Map;
use serde_json::Value;

pub fn handle_moa(action: &MoaAction, cwd: &Path) -> Result<()> {
    match action {
        MoaAction::List => list_presets(cwd),
        MoaAction::Configure {
            name,
            aggregator,
            references,
            fanout,
            reference_max_tokens,
            reference_temperature,
            aggregator_temperature,
            make_default,
            enable,
            disable,
        } => configure_preset(
            name,
            aggregator,
            references,
            *fanout,
            *reference_max_tokens,
            *reference_temperature,
            *aggregator_temperature,
            *make_default,
            *enable,
            *disable,
        ),
        MoaAction::Delete { name } => delete_preset(name),
    }
}

#[allow(clippy::too_many_arguments)]
fn configure_preset(
    name: &str,
    aggregator: &str,
    references: &[String],
    fanout: MoaFanoutArg,
    reference_max_tokens: Option<i64>,
    reference_temperature: Option<f32>,
    aggregator_temperature: Option<f32>,
    make_default: bool,
    enable: bool,
    disable: bool,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("MoA preset name must be non-empty");
    }
    if reference_max_tokens.is_some_and(|tokens| tokens <= 0) {
        bail!("--reference-max-tokens must be positive");
    }
    if make_default && disable && !enable {
        bail!("a disabled MoA preset cannot be saved as the default");
    }
    validate_temperature("--reference-temperature", reference_temperature)?;
    validate_temperature("--aggregator-temperature", aggregator_temperature)?;

    let aggregator = parse_real_selection("aggregator", aggregator)?;
    let reference_models = dedupe_references(
        references
            .iter()
            .enumerate()
            .map(|(idx, reference)| parse_real_selection(&format!("reference[{idx}]"), reference))
            .collect::<Result<Vec<_>>>()?,
    );
    if reference_models.is_empty() {
        bail!("configure at least one --reference provider/model");
    }
    if reference_models.len() > coco_config::model::MAX_REFERENCE_MODELS {
        bail!(
            "MoA preset `{name}` has {} reference models after dedupe; maximum is {}",
            reference_models.len(),
            coco_config::model::MAX_REFERENCE_MODELS
        );
    }

    let mut root = read_user_settings_value()?;
    let mut settings = read_user_moa_settings(&root)?;
    settings.presets.insert(
        name.to_string(),
        MoaPresetSettings {
            enabled: !disable || enable,
            aggregator: Some(aggregator),
            reference_models,
            fanout: match fanout {
                MoaFanoutArg::PerIteration => MoaFanout::PerIteration,
                MoaFanoutArg::UserTurn => MoaFanout::UserTurn,
            },
            reference_max_tokens,
            reference_temperature,
            aggregator_temperature,
        },
    );
    if make_default {
        settings.default_preset = Some(name.to_string());
    }
    write_user_moa_settings(&mut root, Some(settings))?;
    println!("Configured MoA preset `{name}` in user settings.");
    Ok(())
}

fn delete_preset(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("MoA preset name must be non-empty");
    }

    let mut root = read_user_settings_value()?;
    let mut settings = read_user_moa_settings(&root)?;
    if settings.presets.remove(name).is_none() {
        println!("MoA preset `{name}` was not present in user settings.");
        return Ok(());
    }
    if settings.default_preset.as_deref() == Some(name) {
        settings.default_preset = None;
    }
    let next = if settings.presets.is_empty() && settings.default_preset.is_none() {
        None
    } else {
        Some(settings)
    };
    write_user_moa_settings(&mut root, next)?;
    println!("Deleted MoA preset `{name}` from user settings.");
    Ok(())
}

fn list_presets(cwd: &Path) -> Result<()> {
    let roots = coco_agent_host::paths::settings_roots_for_cwd(cwd);
    let settings = coco_config::settings::load_settings_for_roots(&roots, None)?;
    let moa = &settings.merged.moa;
    if moa.presets.is_empty() {
        println!("No MoA presets configured.");
        println!("Default preset name: {}", moa.default_preset_name());
        return Ok(());
    }

    println!("MoA presets:");
    for (name, preset) in &moa.presets {
        let marker = if Some(name.as_str()) == moa.default_preset.as_deref()
            || (moa.default_preset.is_none() && name == "default")
        {
            " default"
        } else {
            ""
        };
        let state = if preset.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let aggregator = preset
            .aggregator
            .as_ref()
            .map(format_selection)
            .unwrap_or_else(|| "<missing>".to_string());
        let references = preset
            .reference_models
            .iter()
            .map(format_selection)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {name}{marker} ({state})");
        println!("    aggregator: {aggregator}");
        println!("    references: {references}");
        println!("    fanout: {}", format_fanout(preset.fanout));
    }
    Ok(())
}

fn validate_temperature(flag: &str, value: Option<f32>) -> Result<()> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        bail!("{flag} must be a finite non-negative number");
    }
    Ok(())
}

fn parse_real_selection(field: &str, value: &str) -> Result<ProviderModelSelection> {
    let selection = ProviderModelSelection::from_slash_str(value)
        .map_err(|message| anyhow::anyhow!("{field}: {message}"))?;
    if selection.provider == coco_config::MOA_PROVIDER {
        bail!("{field}: MoA presets must use real provider/model members, not moa/*");
    }
    Ok(selection)
}

fn dedupe_references(references: Vec<ProviderModelSelection>) -> Vec<ProviderModelSelection> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for selection in references {
        if seen.insert((selection.provider.clone(), selection.model_id.clone())) {
            out.push(selection);
        }
    }
    out
}

fn format_selection(selection: &ProviderModelSelection) -> String {
    format!("{}/{}", selection.provider, selection.model_id)
}

fn format_fanout(fanout: MoaFanout) -> &'static str {
    match fanout {
        MoaFanout::PerIteration => "per_iteration",
        MoaFanout::UserTurn => "user_turn",
    }
}

fn read_user_moa_settings(root: &Value) -> Result<MoaSettings> {
    match root.get("moa") {
        Some(value) => serde_json::from_value(value.clone()).context("invalid user settings.moa"),
        None => Ok(MoaSettings::default()),
    }
}

fn read_user_settings_value() -> Result<Value> {
    let path = global_config::user_settings_path();
    match fs::read_to_string(&path) {
        Ok(contents) if contents.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(contents) => parse_jsonc_value(&contents)
            .with_context(|| format!("could not parse user settings file `{}`", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(error)
            .with_context(|| format!("could not read user settings file `{}`", path.display())),
    }
}

fn write_user_moa_settings(root: &mut Value, settings: Option<MoaSettings>) -> Result<()> {
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("user settings root must be a JSON object"))?;
    match settings {
        Some(settings) => {
            root_obj.insert("moa".to_string(), serde_json::to_value(settings)?);
        }
        None => {
            root_obj.remove("moa");
        }
    }
    write_user_settings_value(root)
}

fn parse_jsonc_value(contents: &str) -> Result<Value> {
    jsonc_parser::parse_to_serde_value(
        contents,
        &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: true,
        },
    )
    .map(|value| value.unwrap_or_else(|| Value::Object(Map::new())))
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn write_user_settings_value(value: &Value) -> Result<()> {
    let path = global_config::user_settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "could not create user settings directory `{}`",
                parent.display()
            )
        })?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(value)?;
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("could not create `{}`", tmp.display()))?;
        file.write_all(&body)
            .with_context(|| format!("could not write `{}`", tmp.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("could not write `{}`", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("could not sync `{}`", tmp.display()))?;
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("could not replace user settings file `{}`", path.display()))
}
