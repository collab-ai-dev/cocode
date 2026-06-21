//! Spawn utilities — CLI flag building and env var inheritance for teammates.

use coco_config::EnvKey;
use coco_config::env;

use crate::constants::AGENT_ID_ENV_VAR;
use crate::constants::AGENT_NAME_ENV_VAR;
use crate::constants::PLAN_MODE_REQUIRED_ENV_VAR;
use crate::constants::TEAM_NAME_ENV_VAR;
use crate::constants::TEAMMATE_COLOR_ENV_VAR;
use crate::pane::TeammateSpawnConfig;

/// Get the command used to spawn teammates.
///
/// Returns the `TEAMMATE_COMMAND` env var if set, otherwise the current process executable.
pub fn get_teammate_command() -> String {
    env::var(crate::constants::TEAMMATE_COMMAND_ENV_VAR).unwrap_or_else(|_| {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "claude".to_string())
    })
}

/// Build the full CLI command to spawn a pane-based teammate.
pub fn build_teammate_command(config: &TeammateSpawnConfig) -> String {
    let command = get_teammate_command();

    // Identity (agent-id / agent-name / team-name / parent-session-id /
    // color / plan-mode) rides `COCO_*` env (build_inherited_env_vars +
    // identity.rs tier-3), NOT CLI flags: the clap `Cli` defines none of
    // those flags and has no catch-all, so emitting them makes the spawned
    // `coco` child fail argument parsing on launch. Only real clap flags
    // belong here.
    let mut args: Vec<String> = Vec::new();
    if let Some(model) = &config.model {
        args.push(format!("--models.main={}", shell_quote(model)));
    }

    let env_vars = build_inherited_env_vars(config);
    let args_str = args.join(" ");
    let suffix = if args_str.is_empty() {
        String::new()
    } else {
        format!(" {args_str}")
    };

    // cd to working directory, then run with env
    format!(
        "cd {cwd} && {env_vars}{command}{suffix}",
        cwd = shell_quote(&config.cwd),
        command = shell_quote(&command),
    )
}

/// Build env vars to inherit into teammate processes.
///
/// Three categories:
/// 1. **Worker identity** — agent_id / agent_name / team_name / color /
///    plan-mode flag. Identity is also passed as CLI flags (so the
///    child can boot without env), but env duplication keeps tools
///    that read `COCO_*` directly (e.g. `crate::identity::*`)
///    coherent without depending on the CLI parser.
/// 2. **Coco runtime config** — `ANTHROPIC_BASE_URL`, `COCO_CONFIG_DIR`,
///    `COCO_REMOTE`, `COCO_REMOTE_MEMORY_DIR`, plus the Feature gate.
/// 3. **Third-party (non-COCO) passthroughs** — AWS / Google credentials,
///    HTTP proxy, TLS bundle paths. These keep their upstream names by
///    convention; coco doesn't shadow them.
pub fn build_inherited_env_vars(config: &TeammateSpawnConfig) -> String {
    let mut vars = Vec::new();

    // ── 1. Worker identity ──
    let agent_id = format!("{}@{}", config.name, config.team_name);
    vars.push(format!("{AGENT_ID_ENV_VAR}={agent_id}"));
    vars.push(format!("{AGENT_NAME_ENV_VAR}={}", config.name));
    vars.push(format!("{TEAM_NAME_ENV_VAR}={}", config.team_name));
    if let Some(color) = &config.color {
        vars.push(format!("{TEAMMATE_COLOR_ENV_VAR}={}", color.as_str()));
    }
    if config.plan_mode_required {
        vars.push(format!("{PLAN_MODE_REQUIRED_ENV_VAR}=1"));
    }

    // ── 2. Coco runtime + feature gates ──
    //
    // `COCO_FEATURE_AGENT_TEAMS=1` makes the child's `Features::resolve()`
    // pick up the gate even when settings.json doesn't enable it.
    vars.push("COCO_FEATURE_AGENT_TEAMS=1".to_string());
    // Bedrock / Vertex / Foundry routing vars are intentionally
    // omitted — coco-rs configures providers via `~/.coco.json`, not
    // env. Add specific keys back here only if a provider crate
    // grows env-driven runtime knobs.
    for var in &[
        EnvKey::AnthropicBaseUrl,
        EnvKey::CocoConfigDir,
        EnvKey::CocoRemote,
        EnvKey::CocoRemoteMemoryDir,
    ] {
        if let Ok(val) = env::var(*var) {
            vars.push(format!("{var}={}", shell_quote(&val)));
        }
    }

    // ── 3. Third-party env passthroughs ──
    //
    // These are not COCO_-prefixed because they belong to upstream
    // tools (AWS SDK, libcurl, Node, requests, …). Forwarding lets
    // the child's HTTP stack, TLS validation, and cloud auth pick up
    // the operator's environment.
    for var in &[
        // AWS
        "AWS_REGION",
        "AWS_PROFILE",
        "GOOGLE_CLOUD_PROJECT",
        // Proxy (preserve upper- and lower-case forms; tools differ).
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
        "ALL_PROXY",
        "all_proxy",
        // TLS certificates
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
    ] {
        if let Ok(val) = std::env::var(var) {
            vars.push(format!("{var}={}", shell_quote(&val)));
        }
    }

    if vars.is_empty() {
        String::new()
    } else {
        format!("{} ", vars.join(" "))
    }
}

fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_else(|_| "''".to_string())
}

#[cfg(test)]
#[path = "spawn.test.rs"]
mod tests;
