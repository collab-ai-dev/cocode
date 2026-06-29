//! Shell `coco mcp login/logout` helpers.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::io::Write;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use coco_mcp::McpConfigLoader;
use coco_mcp::McpServerConfig;
use coco_mcp::ScopedMcpServerConfig;
use coco_rmcp_client::OAuthCredentialsStoreMode;
use coco_rmcp_client::OauthLoginHandle;
use coco_rmcp_client::OauthRedirectUrlSubmitter;
use tokio::io::AsyncBufReadExt;

pub async fn run_login(name: &str, no_browser: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_home = coco_config::global_config::config_home();
    let configs = McpConfigLoader::load(&cwd, &config_home);
    let server = find_server(&configs, name)?;
    let Some(target) = oauth_target(server).await? else {
        println!("{}", no_oauth_message(name, &server.config));
        return Ok(());
    };

    let removed = coco_rmcp_client::delete_oauth_tokens(
        name,
        &target.url,
        OAuthCredentialsStoreMode::Auto,
        &config_home,
    )?;
    if removed {
        println!("Cleared existing OAuth credentials for \"{name}\".");
    }

    if no_browser {
        let handle = coco_rmcp_client::perform_oauth_login_return_url(
            name,
            &target.url,
            OAuthCredentialsStoreMode::Auto,
            Some(target.headers),
            /*env_http_headers*/ None,
            &[],
            /*timeout_secs*/ None,
            /*callback_port*/ None,
            config_home,
        )
        .await?;
        println!(
            "Visit this URL to authorize:\n{}\n\nWaiting for authorization...",
            handle.authorization_url()
        );
        wait_for_no_browser_login(name, handle).await?;
    } else {
        coco_rmcp_client::perform_oauth_login(
            name,
            &target.url,
            OAuthCredentialsStoreMode::Auto,
            Some(target.headers),
            /*env_http_headers*/ None,
            &[],
            /*callback_port*/ None,
            config_home,
        )
        .await?;
    }

    println!("Authenticated with \"{name}\". Its tools are now available in Coco.");
    Ok(())
}

async fn wait_for_no_browser_login(name: &str, handle: OauthLoginHandle) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!(
            "Couldn't complete authentication for \"{name}\": stdin isn't a terminal, so authentication can't be completed here. Re-run in an interactive terminal, for example `ssh -t`, and paste the redirect URL when prompted."
        ));
    }

    let submitter = handle.redirect_url_submitter();
    let wait = handle.wait();
    tokio::pin!(wait);
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        print!("Or paste the redirect URL here: ");
        std::io::stdout().flush()?;
        tokio::select! {
            result = &mut wait => {
                return result.map_err(Into::into);
            }
            line = lines.next_line() => {
                let Some(line) = line? else {
                    return Err(anyhow!(
                        "Couldn't complete authentication for \"{name}\": stdin closed before a redirect URL was provided."
                    ));
                };
                if submit_redirect_url(&submitter, &line) {
                    println!("Received redirect URL; completing authentication...");
                    return wait.await.map_err(Into::into);
                }
                if !line.trim().is_empty() {
                    println!(
                        "That doesn't look like a redirect URL; paste the full address from your browser's address bar."
                    );
                }
            }
        }
    }
}

fn submit_redirect_url(submitter: &OauthRedirectUrlSubmitter, line: &str) -> bool {
    submitter.submit(line.trim())
}

pub async fn run_logout(name: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_home = coco_config::global_config::config_home();
    let configs = McpConfigLoader::load(&cwd, &config_home);
    let server = find_server(&configs, name)?;
    let Some((url, can_login_again)) = logout_target(&server.config) else {
        println!("\"{name}\" doesn't use OAuth; there are no stored credentials to clear.");
        return Ok(());
    };

    let removed = coco_rmcp_client::delete_oauth_tokens(
        name,
        &url,
        OAuthCredentialsStoreMode::Auto,
        &config_home,
    )?;
    if removed {
        if can_login_again {
            println!(
                "Signed out of \"{name}\". Run `coco mcp login {name}` to authenticate again."
            );
        } else {
            println!("Cleared local credentials for \"{name}\".");
        }
    } else {
        println!("No stored OAuth credentials found for \"{name}\".");
    }
    Ok(())
}

struct OAuthTarget {
    url: String,
    headers: HashMap<String, String>,
}

async fn oauth_target(server: &ScopedMcpServerConfig) -> Result<Option<OAuthTarget>> {
    match &server.config {
        McpServerConfig::Sse(config) => {
            if has_static_authorization(&config.headers) {
                return Ok(None);
            }
            let headers = resolve_http_headers(
                &server.name,
                &config.url,
                &config.headers,
                config.headers_helper.as_deref(),
            )
            .await?;
            Ok(Some(OAuthTarget {
                url: config.url.clone(),
                headers,
            }))
        }
        McpServerConfig::Http(config) => {
            if has_static_authorization(&config.headers) {
                return Ok(None);
            }
            let headers = resolve_http_headers(
                &server.name,
                &config.url,
                &config.headers,
                config.headers_helper.as_deref(),
            )
            .await?;
            Ok(Some(OAuthTarget {
                url: config.url.clone(),
                headers,
            }))
        }
        _ => Ok(None),
    }
}

fn logout_target(config: &McpServerConfig) -> Option<(String, bool)> {
    match config {
        McpServerConfig::Sse(config) => Some((
            config.url.clone(),
            !has_static_authorization(&config.headers),
        )),
        McpServerConfig::Http(config) => Some((
            config.url.clone(),
            !has_static_authorization(&config.headers),
        )),
        _ => None,
    }
}

fn no_oauth_message(name: &str, config: &McpServerConfig) -> String {
    match config {
        McpServerConfig::Sse(config) if has_static_authorization(&config.headers) => {
            format!(
                "\"{name}\" is configured with a static Authorization header; no OAuth login is needed."
            )
        }
        McpServerConfig::Http(config) if has_static_authorization(&config.headers) => {
            format!(
                "\"{name}\" is configured with a static Authorization header; no OAuth login is needed."
            )
        }
        McpServerConfig::ClaudeAiProxy(_) => {
            format!("\"{name}\" is a claude.ai connector; authorize it from claude.ai.")
        }
        _ => format!(
            "\"{name}\" doesn't support OAuth login; OAuth is only available for HTTP and SSE MCP servers."
        ),
    }
}

fn has_static_authorization(headers: &HashMap<String, String>) -> bool {
    headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case(reqwest::header::AUTHORIZATION.as_str()))
}

async fn resolve_http_headers(
    server_name: &str,
    server_url: &str,
    static_headers: &HashMap<String, String>,
    helper: Option<&str>,
) -> Result<HashMap<String, String>> {
    let mut headers = static_headers.clone();
    if let Some(helper) = helper {
        let dynamic = run_headers_helper(server_name, server_url, helper).await?;
        headers.extend(dynamic);
    }
    Ok(headers)
}

async fn run_headers_helper(
    server_name: &str,
    server_url: &str,
    helper: &str,
) -> Result<HashMap<String, String>> {
    let mut cmd = shell_command(helper);
    cmd.env("CLAUDE_CODE_MCP_SERVER_NAME", server_name)
        .env("CLAUDE_CODE_MCP_SERVER_URL", server_url);
    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .with_context(|| format!("headersHelper timed out for MCP server '{server_name}'"))?
        .with_context(|| format!("headersHelper failed for MCP server '{server_name}'"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "headersHelper exited with status {} for MCP server '{}'",
            output.status,
            server_name
        ));
    }
    let stdout = String::from_utf8(output.stdout).context("headersHelper output was not UTF-8")?;
    parse_headers_helper_output(server_name, &stdout)
}

fn shell_command(helper: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(helper);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(helper);
        cmd
    }
}

fn parse_headers_helper_output(server_name: &str, stdout: &str) -> Result<HashMap<String, String>> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("headersHelper returned invalid JSON for '{server_name}'"))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("headersHelper for '{server_name}' must return a JSON object"))?;

    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        let Some(value) = value.as_str() else {
            return Err(anyhow!(
                "headersHelper for '{server_name}' returned non-string value for '{key}'"
            ));
        };
        out.insert(key.clone(), value.to_string());
    }
    Ok(out)
}

fn find_server<'a>(
    configs: &'a [ScopedMcpServerConfig],
    name: &str,
) -> Result<&'a ScopedMcpServerConfig> {
    configs
        .iter()
        .find(|server| server.name == name)
        .ok_or_else(|| anyhow!("{}", suggest_server_not_found(name, configs)))
}

fn suggest_server_not_found(name: &str, configs: &[ScopedMcpServerConfig]) -> String {
    let mut names: Vec<&str> = configs.iter().map(|server| server.name.as_str()).collect();
    names.sort_unstable();
    if let Some(closest) = closest_name(name, &names, 2) {
        return format!(
            "No MCP server named \"{name}\". Did you mean \"{closest}\"? Run `coco mcp list` to see all."
        );
    }
    if names.is_empty() {
        return format!("No MCP server named \"{name}\". Run `coco mcp add` to add one.");
    }
    const MAX_SHOWN: usize = 8;
    let shown = names
        .iter()
        .take(MAX_SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let more = if names.len() > MAX_SHOWN {
        format!(
            " (and {} more; run `coco mcp list` to see all)",
            names.len() - MAX_SHOWN
        )
    } else {
        String::new()
    };
    format!("No MCP server named \"{name}\". Configured servers: {shown}{more}")
}

fn closest_name<'a>(needle: &str, names: &'a [&str], max_distance: usize) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter_map(|name| {
            let distance = levenshtein(needle, name);
            (distance <= max_distance).then_some((distance, name))
        })
        .min_by_key(|(distance, name)| (*distance, *name))
        .map(|(_, name)| name)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0; b_chars.len() + 1];

    for (i, a_char) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != *b_char);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}

#[cfg(test)]
#[path = "mcp_cli.test.rs"]
mod tests;
