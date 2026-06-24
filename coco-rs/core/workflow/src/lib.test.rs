use std::fs;

use pretty_assertions::assert_eq;

use crate::WorkflowError;
use crate::WorkflowMeta;
use crate::WorkflowPhaseMeta;
use crate::WorkflowSourceInput;
use crate::WorkflowSourceKind;
use crate::list_workflows;
use crate::parse_workflow_meta;
use crate::parse_workflow_script;
use crate::resolve_workflow_source;

#[test]
fn test_resolve_workflow_source_uses_inline_script_when_script_path_also_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("explicit.ts");
    fs::write(&script_path, "workflow({ name: 'explicit' })").expect("write explicit");
    let coco_workflows = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("workflows");
    fs::create_dir_all(&coco_workflows).expect("mkdir workflows");
    fs::write(
        coco_workflows.join("named.ts"),
        "workflow({ name: 'named' })",
    )
    .expect("write named");

    let spec = resolve_workflow_source(WorkflowSourceInput {
        script_path: Some(script_path.clone()),
        name: Some("named".to_string()),
        script: Some("workflow({ name: 'inline' })".to_string()),
        cwd: Some(dir.path().to_path_buf()),
    })
    .expect("resolve");

    assert_eq!(
        spec.kind,
        WorkflowSourceKind::ScriptPath(script_path.clone())
    );
    assert_eq!(spec.source_path, Some(script_path));
    assert!(spec.source.contains("inline"));
}

#[test]
fn test_resolve_workflow_source_uses_coco_before_claude_for_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let coco_workflows = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("workflows");
    fs::create_dir_all(&coco_workflows).expect("mkdir coco");
    fs::create_dir_all(dir.path().join(".claude/workflows")).expect("mkdir claude");
    // Named lookup matches the parsed `meta.name`, so both files declare name
    // "build"; the `.cocode` copy is found first.
    fs::write(
        coco_workflows.join("build.ts"),
        r#"export const meta = { name: "build", description: "coco-build" };"#,
    )
    .expect("write coco");
    fs::write(
        dir.path().join(".claude/workflows/build.ts"),
        r#"export const meta = { name: "build", description: "claude-build" };"#,
    )
    .expect("write claude");

    let spec = resolve_workflow_source(WorkflowSourceInput {
        name: Some("build".to_string()),
        cwd: Some(dir.path().to_path_buf()),
        ..WorkflowSourceInput::default()
    })
    .expect("resolve");

    assert_eq!(spec.kind, WorkflowSourceKind::Name("build".to_string()));
    assert!(spec.source.contains("coco-build"));
}

#[test]
fn test_list_workflows_uses_precedence_and_meta_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let coco_workflows = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("workflows");
    let claude_workflows = dir.path().join(".claude").join("workflows");
    fs::create_dir_all(&coco_workflows).expect("mkdir coco");
    fs::create_dir_all(&claude_workflows).expect("mkdir claude");
    fs::write(
        coco_workflows.join("release.ts"),
        r#"export const meta = { name: "Release", description: "Ship it" };"#,
    )
    .expect("write coco");
    fs::write(
        claude_workflows.join("release.js"),
        r#"export const meta = { name: "Release", description: "Old copy" };"#,
    )
    .expect("write duplicate");
    fs::write(
        claude_workflows.join("audit.js"),
        r#"export const meta = { name: "Audit", description: "Check it" };"#,
    )
    .expect("write audit");

    let entries = list_workflows(Some(dir.path().to_path_buf()));

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "Release");
    assert_eq!(entries[0].description, "Ship it");
    assert_eq!(entries[1].name, "Audit");
    assert_eq!(entries[1].description, "Check it");
}

#[test]
fn test_resolve_named_workflow_matches_meta_name_not_filename_stem() {
    let dir = tempfile::tempdir().expect("tempdir");
    let coco_workflows = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("workflows");
    fs::create_dir_all(&coco_workflows).expect("mkdir coco");
    // Filename stem `my-build` differs from the declared `meta.name` "My Build".
    fs::write(
        coco_workflows.join("my-build.ts"),
        r#"export const meta = { name: "My Build", description: "slugged" };"#,
    )
    .expect("write");

    let spec = resolve_workflow_source(WorkflowSourceInput {
        name: Some("My Build".to_string()),
        cwd: Some(dir.path().to_path_buf()),
        ..WorkflowSourceInput::default()
    })
    .expect("resolve by meta.name");
    assert_eq!(spec.kind, WorkflowSourceKind::Name("My Build".to_string()));

    // The filename stem is NOT a valid handle.
    let miss = resolve_workflow_source(WorkflowSourceInput {
        name: Some("my-build".to_string()),
        cwd: Some(dir.path().to_path_buf()),
        ..WorkflowSourceInput::default()
    })
    .expect_err("stem is not a name");
    assert!(matches!(miss, WorkflowError::NamedWorkflowNotFound { .. }));
}

#[test]
fn test_resolve_named_workflow_inline_script_overrides_registry_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let coco_workflows = dir
        .path()
        .join(coco_utils_common::COCO_CONFIG_DIR_NAME)
        .join("workflows");
    fs::create_dir_all(&coco_workflows).expect("mkdir coco");
    let path = coco_workflows.join("ship.ts");
    fs::write(
        &path,
        r#"export const meta = { name: "ship", description: "on disk" };"#,
    )
    .expect("write");

    let spec = resolve_workflow_source(WorkflowSourceInput {
        name: Some("ship".to_string()),
        script: Some(
            "export const meta = { name: 'ship', description: 'inline override' };".into(),
        ),
        cwd: Some(dir.path().to_path_buf()),
        ..WorkflowSourceInput::default()
    })
    .expect("resolve");

    // Inline body wins, but provenance (the registry path) is retained.
    assert!(spec.source.contains("inline override"));
    assert_eq!(spec.source_path, Some(path));
}

#[test]
fn test_resolve_workflow_source_rejects_unc_and_large_inline_source() {
    let unc = resolve_workflow_source(WorkflowSourceInput {
        script_path: Some("//server/share/workflow.ts".into()),
        ..WorkflowSourceInput::default()
    })
    .expect_err("unc rejected");
    assert!(matches!(unc, WorkflowError::UncPath { .. }));

    // Backslash-UNC is rejected from the RAW input even on Linux, where it is
    // not absolute and would otherwise be hidden by the cwd join.
    let backslash_unc = resolve_workflow_source(WorkflowSourceInput {
        script_path: Some(r"\\server\share\workflow.ts".into()),
        cwd: Some(std::path::PathBuf::from("/tmp")),
        ..WorkflowSourceInput::default()
    })
    .expect_err("backslash unc rejected");
    assert!(matches!(backslash_unc, WorkflowError::UncPath { .. }));

    let large = resolve_workflow_source(WorkflowSourceInput {
        script: Some("x".repeat(crate::MAX_WORKFLOW_SOURCE_BYTES + 1)),
        ..WorkflowSourceInput::default()
    })
    .expect_err("large rejected");
    assert!(matches!(large, WorkflowError::SourceTooLarge { .. }));
}

#[test]
fn test_parse_workflow_meta_extracts_name_and_description_from_ast_pairs() {
    let meta = parse_workflow_meta(
        r#"
        export const meta = {
            name: "release",
            description: 'prepare release notes',
            title: "Release",
            whenToUse: "shipping",
            phases: ["Plan", { title: "Ship" }],
        };
        workflow({ name: "runtime" });
        "#,
    )
    .expect("parse");

    assert_eq!(
        meta,
        WorkflowMeta {
            name: "release".to_string(),
            description: "prepare release notes".to_string(),
            title: Some("Release".to_string()),
            when_to_use: Some("shipping".to_string()),
            // The string entry "Plan" is dropped (TS keeps only object entries
            // with a string title); only `{ title: "Ship" }` survives.
            phases: vec![WorkflowPhaseMeta {
                title: "Ship".to_string(),
                detail: None,
                model: None,
            }],
        }
    );
}

#[test]
fn test_parse_workflow_meta_cooks_js_string_escapes() {
    // A single-quote escape `\'` and `\x41` are valid JS but invalid JSON —
    // they must cook to `'` and `A` rather than being rejected.
    let meta =
        parse_workflow_meta(r#"export const meta = { name: 'it\'s', description: "a\x41b" };"#)
            .expect("parse");
    assert_eq!(meta.name, "it's");
    assert_eq!(meta.description, "aAb");

    // Backtick fields cook escapes too (no interpolation present).
    let templated =
        parse_workflow_meta("export const meta = { name: `a\\tb`, description: `c\\u0041d` };")
            .expect("parse template");
    assert_eq!(templated.name, "a\tb");
    assert_eq!(templated.description, "cAd");
}

#[test]
fn test_parse_workflow_meta_normalizes_phase_detail_and_model() {
    let meta = parse_workflow_meta(
        r#"export const meta = {
            name: "x",
            description: "y",
            phases: [{ title: "Plan", detail: "scout", model: "fast" }, { notitle: 1 }],
        };"#,
    )
    .expect("parse");
    assert_eq!(
        meta.phases,
        vec![WorkflowPhaseMeta {
            title: "Plan".to_string(),
            detail: Some("scout".to_string()),
            model: Some("fast".to_string()),
        }]
    );
}

#[test]
fn test_parse_workflow_meta_determinism_catches_optional_chaining() {
    // Optional chaining and whitespace forms escape a raw-text comparison but
    // are flagged by the name-based AST check.
    for source in [
        r#"export const meta = { name: "x", description: "y" }; const t = Date?.now();"#,
        r#"export const meta = { name: "x", description: "y" }; const t = Date . now();"#,
    ] {
        assert!(
            matches!(
                parse_workflow_meta(source),
                Err(WorkflowError::NondeterministicApi { .. })
            ),
            "expected determinism rejection for: {source}"
        );
    }

    // Computed/subscript access is deterministic-looking and must NOT be flagged.
    let subscript = parse_workflow_meta(
        r#"export const meta = { name: "x", description: "y" }; const d = Date["now"];"#,
    );
    assert!(subscript.is_ok(), "subscript access must not be flagged");
}

#[test]
fn test_parse_workflow_meta_rejects_non_const_meta_declaration() {
    let err = parse_workflow_meta(r#"export let meta = { name: "x", description: "y" };"#)
        .expect_err("rejected");
    assert!(matches!(err, WorkflowError::MissingMeta { .. }));
}

#[test]
fn test_parse_workflow_meta_rejects_nondeterministic_apis() {
    let err = parse_workflow_meta(
        r#"export const meta = { name: "x", description: "y" }; const seed = Math.random();"#,
    )
    .expect_err("rejected");
    assert!(matches!(err, WorkflowError::NondeterministicApi { .. }));
}

#[test]
fn test_parse_workflow_meta_requires_first_statement_export_const_meta() {
    let err =
        parse_workflow_meta(r#"const x = 1; export const meta = { name: "x", description: "y" };"#)
            .expect_err("rejected");
    assert!(matches!(err, WorkflowError::MissingMeta { .. }));
}

#[test]
fn test_parse_workflow_meta_rejects_reserved_keys_and_non_literals() {
    let reserved = parse_workflow_meta(
        r#"export const meta = { name: "x", description: "y", __proto__: {} };"#,
    )
    .expect_err("reserved");
    assert!(matches!(reserved, WorkflowError::InvalidMeta { .. }));

    let computed =
        parse_workflow_meta(r#"export const meta = { name: "x", description: getDescription() };"#)
            .expect_err("computed");
    assert!(matches!(computed, WorkflowError::InvalidMeta { .. }));
}

#[test]
fn test_parse_workflow_script_splits_meta_from_body() {
    let parsed = parse_workflow_script(
        r#"export const meta = { name: "x", description: "y" };
log("body");"#,
        true,
    )
    .expect("parse");

    assert_eq!(parsed.meta.name, "x");
    assert!(!parsed.script_body.contains("export const meta"));
    assert!(parsed.script_body.contains("log(\"body\")"));
}
