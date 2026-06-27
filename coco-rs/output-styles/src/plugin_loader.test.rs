use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn no_plugins_returns_empty() {
    assert!(load_plugin_output_styles(&[]).is_empty());
}

#[test]
fn namespaces_styles_with_plugin_prefix() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    write(&styles_dir, "concise.md", "# Concise\nBe brief.\n");
    write(
        &styles_dir,
        "verbose.md",
        "---\nname: Verbose Override\n---\nBe verbose.\n",
    );

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "alpha".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![],
    }];
    let mut styles = load_plugin_output_styles(&plugins);
    styles.sort_by(|a, b| a.name.cmp(&b.name));

    assert_eq!(styles.len(), 2);
    assert_eq!(styles[0].name, "alpha:Verbose Override");
    assert_eq!(styles[0].source, OutputStyleSource::Plugin);
    assert_eq!(styles[1].name, "alpha:concise");
    assert_eq!(styles[1].source, OutputStyleSource::Plugin);
}

#[test]
fn parses_force_for_plugin_only_on_plugin_styles() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    write(
        &styles_dir,
        "forced.md",
        "---\nforce-for-plugin: true\n---\nbody\n",
    );

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "p".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![],
    }];
    let styles = load_plugin_output_styles(&plugins);
    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].force_for_plugin, Some(true));
    // Plugin styles must NOT inherit keep_coding_instructions even when
    // present in frontmatter — that flag is dir-style-only.
    assert!(styles[0].keep_coding_instructions.is_none());
}

#[test]
fn force_for_plugin_rejects_non_ts_bool_strings() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    write(
        &styles_dir,
        "forced.md",
        "---\nforce-for-plugin: \"TRUE\"\n---\nbody\n",
    );

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "p".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![],
    }];
    let styles = load_plugin_output_styles(&plugins);

    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].force_for_plugin, None);
}

#[test]
fn handles_extra_paths_directory_and_file() {
    let plugin_dir = tempdir().unwrap();
    let extras_dir = plugin_dir.path().join("more-styles");
    std::fs::create_dir_all(&extras_dir).unwrap();
    write(&extras_dir, "extra-dir.md", "# From dir extra\n");
    let extra_file = plugin_dir.path().join("solo.md");
    std::fs::write(&extra_file, "# Solo file\n").unwrap();

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "p".into(),
        default_dir: None,
        extra_paths: vec![extras_dir, extra_file],
    }];
    let mut styles = load_plugin_output_styles(&plugins);
    styles.sort_by(|a, b| a.name.cmp(&b.name));

    assert_eq!(styles.len(), 2);
    assert_eq!(styles[0].name, "p:extra-dir");
    assert_eq!(styles[1].name, "p:solo");
}

#[test]
fn loads_nested_plugin_markdown_files_recursively() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    let nested = styles_dir.join("nested").join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    write(&nested, "focused.md", "# Focused\n");

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "p".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![],
    }];
    let styles = load_plugin_output_styles(&plugins);

    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].name, "p:focused");
}

#[test]
fn keeps_same_names_from_distinct_plugin_files() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    write(&styles_dir, "dup.md", "# First\n");

    let extra_dir = plugin_dir.path().join("dup-extra");
    std::fs::create_dir_all(&extra_dir).unwrap();
    write(&extra_dir, "dup.md", "# Second\n");

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "p".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![extra_dir],
    }];
    let styles = load_plugin_output_styles(&plugins);
    assert_eq!(styles.len(), 2);
    assert!(styles.iter().any(|style| style.prompt.contains("First")));
    assert!(styles.iter().any(|style| style.prompt.contains("Second")));
}

#[test]
fn prefixes_already_namespaced_name_like_ts() {
    let plugin_dir = tempdir().unwrap();
    let styles_dir = plugin_dir.path().join("output-styles");
    std::fs::create_dir_all(&styles_dir).unwrap();
    write(
        &styles_dir,
        "x.md",
        "---\nname: alpha:already-namespaced\n---\nbody\n",
    );

    let plugins = vec![PluginOutputStyleSource {
        plugin_name: "alpha".into(),
        default_dir: Some(styles_dir),
        extra_paths: vec![],
    }];
    let styles = load_plugin_output_styles(&plugins);
    assert_eq!(styles[0].name, "alpha:alpha:already-namespaced");
}
