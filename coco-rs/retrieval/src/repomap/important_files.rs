//! Important files filter for repo map.
//!
//! Identifies and prioritizes important metadata files like README, Cargo.toml,
//! etc. that should appear at the top of the repo map output.
//!
//! Inspired by Aider's 177+ patterns for important files.

use std::path::Path;

/// Root-level important file patterns (README, configs, etc.)
pub const ROOT_IMPORTANT_FILES: &[&str] = &[
    // Version Control
    ".gitignore",
    ".gitattributes",
    // Documentation
    "README",
    "README.md",
    "README.txt",
    "README.rst",
    "CONTRIBUTING",
    "CONTRIBUTING.md",
    "LICENSE",
    "LICENSE.md",
    "LICENSE.txt",
    "CHANGELOG",
    "CHANGELOG.md",
    "CODEOWNERS",
    "SECURITY",
    "SECURITY.md",
    // Rust Package Management
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    // Node.js
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "tsconfig.json",
    "jsconfig.json",
    ".npmrc",
    // Python
    "requirements.txt",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "Pipfile",
    "Pipfile.lock",
    "poetry.lock",
    "tox.ini",
    "pyrightconfig.json",
    ".python-version",
    // Go
    "go.mod",
    "go.sum",
    // Java/Kotlin/Gradle
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradlew",
    // Ruby
    "Gemfile",
    "Gemfile.lock",
    // PHP
    "composer.json",
    "composer.lock",
    // .NET
    "*.csproj",
    "*.sln",
    "nuget.config",
    // Elixir
    "mix.exs",
    "rebar.config",
    // Clojure
    "project.clj",
    // iOS/macOS
    "Podfile",
    "Podfile.lock",
    // Docker
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    ".dockerignore",
    // CI/CD
    ".travis.yml",
    ".gitlab-ci.yml",
    "Jenkinsfile",
    "azure-pipelines.yml",
    ".circleci/config.yml",
    ".github/dependabot.yml",
    // Configuration
    ".env.example",
    ".editorconfig",
    ".prettierrc",
    ".prettierrc.json",
    ".eslintrc",
    ".eslintrc.json",
    ".babelrc",
    "babel.config.js",
    ".pylintrc",
    ".flake8",
    "mypy.ini",
    // Build
    "webpack.config.js",
    "rollup.config.js",
    "vite.config.js",
    "vite.config.ts",
    "gulpfile.js",
    "Gruntfile.js",
    "Makefile",
    "justfile",
    "CMakeLists.txt",
    "MANIFEST.in",
    // Testing
    "pytest.ini",
    "phpunit.xml",
    "jest.config.js",
    "karma.conf.js",
    ".nycrc",
    ".nycrc.json",
    "vitest.config.ts",
    // Cloud
    "serverless.yml",
    "firebase.json",
    "netlify.toml",
    "vercel.json",
    "terraform.tf",
    "main.tf",
    "kubernetes.yaml",
    "k8s.yaml",
    // API
    "swagger.yaml",
    "openapi.yaml",
    "openapi.json",
];

/// Check if a file path is an important metadata file.
///
/// Returns true if the file should be prioritized in repo map output.
pub fn is_important(rel_path: &str) -> bool {
    let path = Path::new(rel_path);

    // Get the file name
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    // Check for exact matches in ROOT_IMPORTANT_FILES
    for pattern in ROOT_IMPORTANT_FILES {
        if pattern.starts_with("*.") {
            // Wildcard extension match (e.g., "*.csproj")
            let ext = &pattern[1..]; // ".csproj"
            if file_name.ends_with(ext) {
                return true;
            }
        } else if pattern.contains('/') {
            // Path pattern (e.g., ".cargo/config.toml", ".circleci/config.yml")
            if rel_path.ends_with(pattern) || rel_path == *pattern {
                return true;
            }
        } else {
            // Exact file name match
            if file_name == *pattern {
                return true;
            }
        }
    }

    // Special case: GitHub Actions workflows
    if rel_path.starts_with(".github/workflows/") && file_name.ends_with(".yml") {
        return true;
    }
    if rel_path.starts_with(".github/workflows/") && file_name.ends_with(".yaml") {
        return true;
    }

    false
}

/// Filter a list of file paths to return only important files.
///
/// Returns important files sorted by their original order.
pub fn filter_important_files(files: &[String]) -> Vec<String> {
    files.iter().filter(|f| is_important(f)).cloned().collect()
}

#[cfg(test)]
#[path = "important_files.test.rs"]
mod tests;
