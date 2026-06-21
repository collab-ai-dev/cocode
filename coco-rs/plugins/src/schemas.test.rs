use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_plugin_id_parse_valid() {
    let id = PluginId::parse("my-plugin@my-marketplace").expect("should parse");
    assert_eq!(id.name, "my-plugin");
    assert_eq!(id.marketplace, "my-marketplace");
    assert_eq!(id.as_str(), "my-plugin@my-marketplace");
}

#[test]
fn test_plugin_id_parse_invalid() {
    assert!(PluginId::parse("no-at-sign").is_none());
    assert!(PluginId::parse("@marketplace").is_none());
    assert!(PluginId::parse("name@").is_none());
    assert!(PluginId::parse("").is_none());
}

#[test]
fn test_plugin_id_serde_roundtrip() {
    let id = PluginId {
        name: "test".to_string(),
        marketplace: "mkt".to_string(),
    };
    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, r#""test@mkt""#);

    let deserialized: PluginId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, id);
}

#[test]
fn test_plugin_id_deserialize_invalid() {
    let result: Result<PluginId, _> = serde_json::from_str(r#""no-at""#);
    assert!(result.is_err());
}

#[test]
fn test_manifest_v2_from_toml_full() {
    let toml_str = r#"
name = "example-plugin"
version = "1.0.0"
description = "An example plugin"
homepage = "https://example.com"
license = "MIT"
keywords = ["test", "example"]

[author]
name = "Test Author"
email = "test@example.com"
url = "https://test.com"
"#;
    let manifest: PluginManifestV2 = toml::from_str(toml_str).expect("should parse");
    assert_eq!(manifest.name, "example-plugin");
    assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
    assert_eq!(manifest.description.as_deref(), Some("An example plugin"));
    assert_eq!(manifest.homepage.as_deref(), Some("https://example.com"));
    assert_eq!(manifest.license.as_deref(), Some("MIT"));
    assert_eq!(
        manifest.keywords,
        Some(vec!["test".to_string(), "example".to_string()])
    );
    let author = manifest.author.as_ref().expect("author present");
    assert_eq!(author.name, "Test Author");
    assert_eq!(author.email.as_deref(), Some("test@example.com"));
}

#[test]
fn test_manifest_v2_minimal() {
    let toml_str = r#"name = "minimal""#;
    let manifest: PluginManifestV2 = toml::from_str(toml_str).expect("should parse");
    assert_eq!(manifest.name, "minimal");
    assert!(manifest.version.is_none());
    assert!(manifest.description.is_none());
    assert!(manifest.author.is_none());
    assert!(manifest.dependencies.is_none());
    assert!(manifest.skills.is_none());
    assert!(manifest.hooks.is_none());
}

#[test]
fn test_manifest_paths_single() {
    let paths: ManifestPaths = serde_json::from_str(r#""./skills""#).expect("parse");
    assert_eq!(paths.to_vec(), vec!["./skills"]);
}

#[test]
fn test_manifest_paths_multiple() {
    let paths: ManifestPaths =
        serde_json::from_str(r#"["./skills", "./more-skills"]"#).expect("parse");
    assert_eq!(paths.to_vec(), vec!["./skills", "./more-skills"]);
}

#[test]
fn test_validate_manifest_valid() {
    let manifest = PluginManifestV2 {
        name: "valid-plugin".to_string(),
        version: Some("1.0.0".to_string()),
        description: Some("A valid plugin".to_string()),
        author: None,
        homepage: Some("https://example.com".to_string()),
        repository: None,
        license: None,
        keywords: None,
        dependencies: None,
        skills: None,
        hooks: None,
        agents: None,
        commands: None,
        mcp_servers: None,
        lsp_servers: None,
        output_styles: None,
        channels: None,
        user_config: None,
        settings: None,
        env_vars: None,
        min_version: None,
        max_version: None,
    };
    let errors = validate_manifest(&manifest);
    assert!(errors.is_empty(), "expected no errors, got: {errors:?}");
}

#[test]
fn test_validate_manifest_empty_name() {
    let mut manifest = make_minimal_manifest("");
    manifest.name = String::new();
    let errors = validate_manifest(&manifest);
    assert!(!errors.is_empty());
    assert!(errors.iter().any(|e| e.field == "name"));
}

#[test]
fn test_validate_manifest_name_with_spaces() {
    let manifest = make_minimal_manifest("bad name");
    let errors = validate_manifest(&manifest);
    assert!(
        errors
            .iter()
            .any(|e| e.field == "name" && e.message.contains("spaces"))
    );
}

#[test]
fn test_validate_manifest_bad_homepage() {
    let mut manifest = make_minimal_manifest("test");
    manifest.homepage = Some("not-a-url".to_string());
    let errors = validate_manifest(&manifest);
    assert!(errors.iter().any(|e| e.field == "homepage"));
}

#[test]
fn test_validate_marketplace_name_valid() {
    assert!(validate_marketplace_name("my-marketplace").is_none());
    assert!(validate_marketplace_name("company-internal").is_none());
}

#[test]
fn test_validate_marketplace_name_empty() {
    assert!(validate_marketplace_name("").is_some());
}

#[test]
fn test_validate_marketplace_name_spaces() {
    let err = validate_marketplace_name("bad name").expect("should fail");
    assert!(err.contains("spaces"));
}

#[test]
fn test_validate_marketplace_name_reserved() {
    assert!(validate_marketplace_name("inline").is_some());
    assert!(validate_marketplace_name("INLINE").is_some());
    assert!(validate_marketplace_name("builtin").is_some());
}

#[test]
fn test_is_blocked_official_name() {
    // Allowed official names are not blocked.
    assert!(!is_blocked_official_name("claude-plugins-official"));
    assert!(!is_blocked_official_name("anthropic-marketplace"));

    // Impersonation attempts are blocked.
    assert!(is_blocked_official_name("official-claude-plugins-v2"));
    assert!(is_blocked_official_name("anthropic-official"));
    assert!(is_blocked_official_name("claude-marketplace-fake"));

    // Normal third-party names are not blocked.
    assert!(!is_blocked_official_name("my-cool-plugins"));
    assert!(!is_blocked_official_name("company-internal"));
}

#[test]
fn test_validate_official_name_source_github_official() {
    let source = MarketplaceSource::Github {
        repo: "anthropics/claude-plugins".to_string(),
        git_ref: None,
        path: None,
        sparse_paths: None,
    };
    assert!(validate_official_name_source("claude-plugins-official", &source).is_none());
}

#[test]
fn test_validate_official_name_source_github_wrong_org() {
    let source = MarketplaceSource::Github {
        repo: "evil-org/claude-plugins".to_string(),
        git_ref: None,
        path: None,
        sparse_paths: None,
    };
    assert!(validate_official_name_source("claude-plugins-official", &source).is_some());
}

#[test]
fn test_validate_official_name_source_non_reserved() {
    let source = MarketplaceSource::Npm {
        package: "whatever".to_string(),
    };
    // Non-reserved names should pass regardless of source.
    assert!(validate_official_name_source("my-marketplace", &source).is_none());
}

#[test]
fn test_marketplace_source_serde_github() {
    let source = MarketplaceSource::Github {
        repo: "owner/repo".to_string(),
        git_ref: Some("main".to_string()),
        path: None,
        sparse_paths: None,
    };
    let json = serde_json::to_string(&source).expect("serialize");
    let deserialized: MarketplaceSource = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, source);
}

#[test]
fn test_plugin_scope_serde() {
    let scope = PluginScope::Project;
    let json = serde_json::to_string(&scope).expect("serialize");
    assert_eq!(json, r#""project""#);

    let deserialized: PluginScope = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, scope);
}

#[test]
fn test_installed_plugins_file_v2_default() {
    let file = InstalledPluginsFileV2::default();
    assert_eq!(file.version, 2);
    assert!(file.plugins.is_empty());
}

#[test]
fn test_installed_plugins_file_v2_roundtrip() {
    let mut file = InstalledPluginsFileV2::default();
    file.plugins.insert(
        "test@mkt".to_string(),
        vec![PluginInstallationEntry {
            scope: PluginScope::User,
            project_path: None,
            install_path: "/home/user/.coco/plugins/cache/mkt/test/1.0.0".to_string(),
            version: Some("1.0.0".to_string()),
            installed_at: Some("2024-01-15T10:30:00Z".to_string()),
            last_updated: None,
            git_commit_sha: None,
        }],
    );

    let json = serde_json::to_string_pretty(&file).expect("serialize");
    let deserialized: InstalledPluginsFileV2 = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, file);
}

#[test]
fn test_plugin_installation_record() {
    let record = PluginInstallationRecord {
        name: "my-plugin".to_string(),
        version: "1.0.0".to_string(),
        installed_at: "2024-01-15T10:30:00Z".to_string(),
        source_url: Some("https://github.com/org/repo".to_string()),
        scope: PluginScope::User,
    };
    let json = serde_json::to_string(&record).expect("serialize");
    let deserialized: PluginInstallationRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, record);
}

#[test]
fn test_marketplace_entry_strict_default() {
    let json = r#"{
        "name": "test-plugin",
        "source": "./plugins/test"
    }"#;
    let entry: PluginMarketplaceEntry = serde_json::from_str(json).expect("parse");
    assert!(entry.strict, "strict should default to true");
}

#[test]
fn test_is_local_plugin_source() {
    assert!(is_local_plugin_source(&PluginSource::RelativePath(
        "./foo".to_string()
    )));
    assert!(!is_local_plugin_source(&PluginSource::RelativePath(
        "foo".to_string()
    )));
    assert!(!is_local_plugin_source(&PluginSource::Remote(
        RemotePluginSource::Npm {
            package: "test".to_string(),
            version: None,
            registry: None,
        }
    )));
}

#[test]
fn test_is_local_marketplace_source() {
    assert!(is_local_marketplace_source(&MarketplaceSource::File {
        path: "/foo".to_string()
    }));
    assert!(is_local_marketplace_source(&MarketplaceSource::Directory {
        path: "/foo".to_string()
    }));
    assert!(!is_local_marketplace_source(&MarketplaceSource::Github {
        repo: "owner/repo".to_string(),
        git_ref: None,
        path: None,
        sparse_paths: None,
    }));
}

#[test]
fn test_user_config_option_serde() {
    let json = r#"{
        "type": "string",
        "title": "API Key",
        "description": "Your API key",
        "required": true,
        "sensitive": true
    }"#;
    let option: UserConfigOption = serde_json::from_str(json).expect("parse");
    assert_eq!(option.config_type, UserConfigType::String);
    assert_eq!(option.title, "API Key");
    assert_eq!(option.required, Some(true));
    assert_eq!(option.sensitive, Some(true));
}

#[test]
fn test_env_var_declaration_serde() {
    let decl = EnvVarDeclaration {
        name: "MY_API_KEY".to_string(),
        required: true,
        description: Some("API key for the service".to_string()),
        default: None,
    };
    let json = serde_json::to_string(&decl).expect("serialize");
    let deserialized: EnvVarDeclaration = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, decl);
}

#[test]
fn test_validate_semver_min_max_version() {
    let mut manifest = make_minimal_manifest("ver-test");
    manifest.min_version = Some("1.2.3".to_string());
    manifest.max_version = Some("2.0.0".to_string());
    let errors = validate_manifest(&manifest);
    assert!(errors.is_empty(), "valid semver should pass: {errors:?}");

    manifest.min_version = Some("bad".to_string());
    let errors = validate_manifest(&manifest);
    assert!(errors.iter().any(|e| e.field == "min_version"));
}

#[test]
fn test_validate_author_empty_name() {
    let author = PluginAuthor {
        name: String::new(),
        email: None,
        url: None,
    };
    let errors = validate_author(&author);
    assert!(errors.iter().any(|e| e.field == "author.name"));
}

#[test]
fn test_validate_author_bad_url() {
    let author = PluginAuthor {
        name: "Valid Name".to_string(),
        email: None,
        url: Some("not-a-url".to_string()),
    };
    let errors = validate_author(&author);
    assert!(errors.iter().any(|e| e.field == "author.url"));
}

#[test]
fn test_validate_env_var_declarations_valid() {
    let decls = vec![EnvVarDeclaration {
        name: "MY_API_KEY".to_string(),
        required: true,
        description: None,
        default: None,
    }];
    let errors = validate_env_var_declarations(&decls);
    assert!(errors.is_empty());
}

#[test]
fn test_validate_env_var_declarations_bad_name() {
    let decls = vec![EnvVarDeclaration {
        name: "lower-case-bad".to_string(),
        required: false,
        description: None,
        default: None,
    }];
    let errors = validate_env_var_declarations(&decls);
    assert!(!errors.is_empty());
    assert!(errors[0].message.contains("uppercase"));
}

#[test]
fn test_validate_marketplace_entry_valid() {
    let entry = PluginMarketplaceEntry {
        name: "my-plugin".to_string(),
        source: PluginSource::RelativePath("./plugins/my-plugin".to_string()),
        version: Some("1.0.0".to_string()),
        description: Some("A plugin".to_string()),
        author: None,
        category: None,
        tags: None,
        strict: true,
        homepage: None,
        license: None,
        keywords: None,
        dependencies: None,
    };
    let errors = validate_marketplace_entry(&entry);
    assert!(errors.is_empty());
}

#[test]
fn test_validate_marketplace_entry_empty_name() {
    let entry = PluginMarketplaceEntry {
        name: String::new(),
        source: PluginSource::RelativePath("./plugins/x".to_string()),
        version: None,
        description: None,
        author: None,
        category: None,
        tags: None,
        strict: true,
        homepage: None,
        license: None,
        keywords: None,
        dependencies: None,
    };
    let errors = validate_marketplace_entry(&entry);
    assert!(errors.iter().any(|e| e.field == "name"));
}

#[test]
fn test_manifest_v2_min_max_version_serde() {
    let toml_str = r#"
name = "version-bounded"
min_version = "1.0.0"
max_version = "3.0.0"
"#;
    let manifest: PluginManifestV2 = toml::from_str(toml_str).expect("should parse");
    assert_eq!(manifest.min_version.as_deref(), Some("1.0.0"));
    assert_eq!(manifest.max_version.as_deref(), Some("3.0.0"));
}

/// Helper to build a minimal valid manifest for test scenarios.
fn make_minimal_manifest(name: &str) -> PluginManifestV2 {
    PluginManifestV2 {
        name: name.to_string(),
        version: None,
        description: None,
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: None,
        dependencies: None,
        skills: None,
        hooks: None,
        agents: None,
        commands: None,
        mcp_servers: None,
        lsp_servers: None,
        output_styles: None,
        channels: None,
        user_config: None,
        settings: None,
        env_vars: None,
        min_version: None,
        max_version: None,
    }
}
