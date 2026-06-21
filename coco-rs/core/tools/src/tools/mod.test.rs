use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use coco_types::Features;
use coco_types::ToolName;
use std::collections::HashSet;
use std::sync::Arc;

#[test]
fn test_register_all_tools_count() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);
    // 41 = 40 baseline + ApplyPatchTool (gated to gpt-5 family via
    // ToolOverrides; registered universally so the layer-2 filter
    // can surface it when the model declares it as extra).
    // `StructuredOutputTool` is intentionally **not** in the baseline:
    // it's conditionally injected via `register_structured_output_tool`
    // only when the non-interactive bootstrap parses `--json-schema`
    // (`specialTools` excludes it).
    assert_eq!(registry.len(), 41, "expected 41 tools registered");
}

#[test]
fn test_register_core_tools_count() {
    let registry = ToolRegistry::new();
    crate::register_core_tools(&registry);
    assert_eq!(registry.len(), 6, "expected 6 core tools");
}

#[test]
fn test_all_tools_have_unique_names() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);

    let names: Vec<String> = registry
        .all()
        .into_iter()
        .map(|t| t.name().to_string())
        .collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(names.len(), unique.len(), "duplicate tool names found");
}

#[test]
fn test_lookup_by_name() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);

    // Verify key tools can be found
    for name in [
        "Bash",
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "Agent",
        "WebFetch",
        "LSP",
        "Config",
        "TaskCreate",
        "EnterPlanMode",
    ] {
        assert!(
            registry.get_by_name(name).is_some(),
            "tool {name} not found"
        );
    }
}

#[test]
fn test_verify_plan_execution_is_not_registered_by_default() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);

    assert!(
        registry
            .get_by_name(ToolName::VerifyPlanExecution.as_str())
            .is_none(),
        "VerifyPlanExecution mirrors TS conditional import and must not be in the default registry"
    );
}

#[test]
fn kairos_brief_and_proactive_tools_are_hidden_by_default() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);

    let visible: HashSet<String> = registry
        .loaded_tools(&ToolUseContext::test_default())
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert!(
        !visible.contains(ToolName::SendUserMessage.as_str()),
        "SendUserMessage must require Feature::KairosBrief"
    );
    assert!(
        !visible.contains(ToolName::Sleep.as_str()),
        "Sleep must require Feature::Proactive"
    );
}

#[test]
fn kairos_brief_and_proactive_features_expose_their_tools() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);
    let mut features = Features::with_defaults();
    features.enable(Feature::KairosBrief);
    features.enable(Feature::Proactive);
    let mut ctx = ToolUseContext::test_default();
    ctx.features = Arc::new(features);

    let visible: HashSet<String> = registry
        .loaded_tools(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert!(
        visible.contains(ToolName::SendUserMessage.as_str()),
        "Feature::KairosBrief should expose SendUserMessage"
    );
    assert!(
        visible.contains(ToolName::Sleep.as_str()),
        "Feature::Proactive should expose Sleep"
    );
}

#[test]
fn task_tools_loaded_except_task_output() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);
    let ctx = ToolUseContext::test_default()
        .with_model_capabilities(false, true)
        .with_tool_search_candidates(true);

    let loaded: HashSet<String> = registry
        .loaded_tools(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();
    let deferred: HashSet<String> = registry
        .deferred_tools(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    for name in [
        ToolName::TaskCreate,
        ToolName::TaskGet,
        ToolName::TaskList,
        ToolName::TaskUpdate,
        ToolName::TaskStop,
    ] {
        assert!(
            loaded.contains(name.as_str()),
            "{name:?} should load eagerly"
        );
        assert!(
            !deferred.contains(name.as_str()),
            "{name:?} should not be deferred"
        );
    }
    assert!(!loaded.contains(ToolName::TaskOutput.as_str()));
    assert!(deferred.contains(ToolName::TaskOutput.as_str()));
}

#[test]
fn todo_write_loaded_in_v1_mode() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);
    let mut features = Features::with_defaults();
    features.disable(Feature::TaskV2);
    let ctx = ToolUseContext::test_default()
        .with_model_capabilities(false, true)
        .with_tool_search_candidates(true);
    let mut ctx = ctx;
    ctx.features = Arc::new(features);

    let loaded: HashSet<String> = registry
        .loaded_tools(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();
    let deferred: HashSet<String> = registry
        .deferred_tools(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert!(loaded.contains(ToolName::TodoWrite.as_str()));
    assert!(!deferred.contains(ToolName::TodoWrite.as_str()));
}

#[test]
fn repl_stub_is_hidden() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);

    let visible: HashSet<String> = registry
        .loaded_tools(&ToolUseContext::test_default())
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert!(!visible.contains(ToolName::Repl.as_str()));
}

/// Force-initialize every registered tool's runtime validation schema. The
/// schemas are `OnceLock`-lazy, so registering a tool does NOT compile them —
/// only calling `runtime_validation_schema()` does. This is the gate the schema
/// constructors rely on: a malformed Bucket-A (`from_input_type`) or hand-built
/// (`from_static_value`) schema panics HERE in CI, not on first production use.
#[test]
fn test_all_tool_schemas_force_initialize() {
    let all = ToolRegistry::new();
    crate::register_all_tools(&all);
    let core = ToolRegistry::new();
    crate::register_core_tools(&core);
    for registry in [&all, &core] {
        for tool in registry.all() {
            assert!(
                tool.runtime_validation_schema().as_value().is_object(),
                "{} runtime schema must compile to a root object",
                tool.name()
            );
        }
    }
}

/// `tool_spec()` is the single source of truth for a tool's model-facing wire
/// shape — `engine_prompt` builds the wire `description` from it. This guards
/// the gap where a tool ships with an *empty* description (Function via the
/// default `prompt()` path, or a hand-built Freeform spec).
#[tokio::test]
async fn test_all_registered_tools_have_nonempty_spec_description() {
    let registry = ToolRegistry::new();
    crate::register_all_tools(&registry);
    let prompt_opts = coco_tool_runtime::PromptOptions::default();
    let schema_ctx = coco_tool_runtime::SchemaContext::default();
    for tool in registry.all() {
        let spec = tool.tool_spec(&schema_ctx, &prompt_opts).await;
        assert!(
            !spec.description().trim().is_empty(),
            "tool `{}` has an empty model-facing tool_spec() description",
            tool.name()
        );
    }
}

/// Guard against orphan modules: every non-test `.rs` file and every
/// subdirectory module under `src/tools/` must be declared in `mod.rs`.
///
/// An un-`mod`'d file silently never compiles — taking its `*.test.rs`
/// companion down with it and yielding a false-green suite (this is exactly how
/// the dead `mcp_advanced` module hid). Mirrors codex's
/// `tui/tests/manager_dependency_regression.rs`: scan the source tree and
/// assert the wiring instead of trusting it.
#[test]
fn test_every_tools_module_is_declared_in_mod_rs() {
    use std::fs;
    use std::path::Path;

    let tools_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools");
    let mod_rs = fs::read_to_string(tools_dir.join("mod.rs")).expect("read tools/mod.rs");

    let mut expected: Vec<String> = Vec::new();
    for entry in fs::read_dir(&tools_dir).expect("read src/tools") {
        let entry = entry.expect("dir entry");
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            // A subdirectory is a module iff it carries a `mod.rs`.
            if entry.path().join("mod.rs").is_file() {
                expected.push(entry.file_name().to_string_lossy().into_owned());
            }
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip non-Rust, the module root itself, and `*.test.rs` companions
        // (those are wired via `#[path]` inside their sibling, not `mod.rs`).
        if !name.ends_with(".rs") || name == "mod.rs" || name.ends_with(".test.rs") {
            continue;
        }
        expected.push(name.trim_end_matches(".rs").to_string());
    }

    let missing: Vec<&String> = expected
        .iter()
        .filter(|stem| !mod_rs.contains(&format!("mod {stem};")))
        .collect();

    assert!(
        missing.is_empty(),
        "orphan module(s) under src/tools/ not declared in mod.rs: {missing:?}\n\
         An un-mod'd file never compiles and its *.test.rs companion is silently \
         dead. Add `mod <name>;` to mod.rs or delete the file."
    );
}
