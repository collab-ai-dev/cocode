//! `harness = false` test target that runs a real `SdkServer` on stdio.
//!
//! This is the lightweight bootstrap the Python coco-sdk e2e suite
//! attaches to instead of building the full `coco` binary (~8 min cold
//! vs. ~30 s for this test target on top of an already-warm `target/`).
//!
//! Why a `harness = false` *test target* and not an example or a
//! libtest `#[test]`:
//!
//! * Examples in `coco-tests-live` can't see `[dev-dependencies]`, so
//!   they'd have no access to `coco_cli::sdk_server` / `coco_query` /
//!   `coco_session` / etc. Test targets get the dev-dep graph for free.
//! * libtest `#[test]` wraps stdout (`running 1 test`, `test result: ok`),
//!   which would corrupt the NDJSON stream Python parses. With
//!   `harness = false` cargo invokes our binary directly — `fn main()`
//!   owns stdio cleanly.
//!
//! Self-skips when `COCO_SDK_STDIO_RUN != "1"` so plain
//! `cargo test -p coco-tests-live` doesn't hang waiting for stdin
//! when this target is swept up alongside the rest of the suite. The
//! Python wrapper (`scripts/coco-sdk-via-cargo.sh`) sets the env var
//! and forwards `--provider` / `--model` after `--`.
//!
//! Invocation (what the wrapper expands to):
//!
//! ```text
//! COCO_SDK_STDIO_RUN=1 cargo test -p coco-tests-live \
//!     --test sdk_server_stdio -- \
//!     --provider deepseek-openai --model deepseek-v4-flash
//! ```

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use coco_cli::Cli;
use coco_cli::headless;
use coco_cli::sdk_server::CliInitializeBootstrap;
use coco_cli::sdk_server::QueryEngineRunner;
use coco_cli::sdk_server::SdkServer;
use coco_cli::sdk_server::StdioTransport;
use coco_cli::session_runtime::SessionRuntime;
use coco_cli::session_runtime::SessionRuntimeBuildOpts;
use coco_commands::CommandRegistry;
use coco_commands::register_extended_builtins;
use coco_session::SessionManager;
use coco_tool_runtime::ToolRegistry;

const SKIP_ENV: &str = "COCO_SDK_STDIO_RUN";

fn main() {
    if std::env::var(SKIP_ENV).ok().as_deref() != Some("1") {
        eprintln!(
            "[sdk_server_stdio] skipping (set {SKIP_ENV}=1 to run; \
             this target is for the Python coco-sdk e2e wrapper)"
        );
        return;
    }
    if let Err(e) = run() {
        eprintln!("[sdk_server_stdio] fatal: {e:#}");
        std::process::exit(1);
    }
}

struct Args {
    provider: String,
    model: String,
}

/// Parse `--provider X` / `--model Y` (or `--provider=X` / `--model=Y`)
/// out of argv. Falls back to `COCO_SDK_STDIO_PROVIDER` /
/// `COCO_SDK_STDIO_MODEL` env vars so callers that can't easily pass
/// args after `--` can still wire it up.
fn parse_args() -> Result<Args> {
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix("--provider=") {
            provider = Some(rest.to_string());
        } else if let Some(rest) = arg.strip_prefix("--model=") {
            model = Some(rest.to_string());
        } else if arg == "--provider" {
            provider = iter.next();
        } else if arg == "--model" {
            model = iter.next();
        }
        // Silently ignore anything else — keeps us robust against
        // libtest leftover args if someone runs us via `cargo test`
        // without `--no-fail-fast` / explicit filter.
    }
    let provider = provider
        .or_else(|| std::env::var("COCO_SDK_STDIO_PROVIDER").ok())
        .ok_or_else(|| anyhow!("missing --provider (or COCO_SDK_STDIO_PROVIDER)"))?;
    let model = model
        .or_else(|| std::env::var("COCO_SDK_STDIO_MODEL").ok())
        .ok_or_else(|| anyhow!("missing --model (or COCO_SDK_STDIO_MODEL)"))?;
    Ok(Args { provider, model })
}

fn run() -> Result<()> {
    // Best-effort `.env` load so DEEPSEEK_API_KEY etc. flow through
    // when the wrapper is launched outside a shell that already
    // sourced them. Mirrors `common::env::ensure_env_loaded`.
    let _ = dotenvy::dotenv();

    let args = parse_args()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(serve(args))
}

async fn serve(args: Args) -> Result<()> {
    use clap::Parser;
    let model_arg = format!("{}/{}", args.provider, args.model);
    let cli = Cli::parse_from(["coco", "--models.main", &model_arg, "sdk"]);

    let cwd = std::env::current_dir().context("read cwd")?;
    let sessions_dir = tempfile::tempdir().context("create sessions tmpdir")?;

    let runtime_config = headless::build_runtime_config_for_cli(&cli, &cwd)?;
    let main_model = headless::resolve_main_model(&runtime_config);
    let model_id = main_model.model_id.clone();
    if model_id == "mock-model" {
        return Err(anyhow!(
            "no live provider configured (api key for `{}` missing); \
             cannot start SDK server against the mock model",
            args.provider
        ));
    }

    // Curated tool subset matches the `sdk_server_deepseek` harness:
    // some builtins emit non-strict schemas DeepSeek rejects.
    let registry = ToolRegistry::new();
    registry.register(Arc::new(coco_tools::BashTool));
    registry.register(Arc::new(coco_tools::ReadTool));
    registry.register(Arc::new(coco_tools::WriteTool));
    registry.register(Arc::new(coco_tools::EditTool));
    registry.register(Arc::new(coco_tools::GlobTool));
    let tools = Arc::new(registry);

    let system_prompt = headless::build_system_prompt_for_model(
        &cwd,
        &runtime_config,
        &main_model.provider,
        &model_id,
        None,
        &[],
    );
    let session_manager = Arc::new(SessionManager::new(sessions_dir.path().to_path_buf()));
    let startup =
        headless::resolve_startup_permission_state(&cli, &runtime_config.settings.merged)?;

    let mut command_registry = CommandRegistry::new();
    register_extended_builtins(&mut command_registry);
    let command_registry = Arc::new(tokio::sync::RwLock::new(Arc::new(command_registry)));

    let skill_manager = coco_skills::SkillManager::new();
    skill_manager.load_from_dirs(&[cwd.join(".coco").join("skills")]);
    let skill_manager = Arc::new(skill_manager);

    let session_runtime = SessionRuntime::build(SessionRuntimeBuildOpts {
        cli: &cli,
        runtime_config: Arc::new(runtime_config),
        cwd: cwd.clone(),
        model_id: model_id.clone(),
        system_prompt: system_prompt.clone(),
        bypass_permissions_available: startup.bypass_available,
        permission_mode: startup.mode,
        model_runtimes: None,
        tools,
        session_manager: session_manager.clone(),
        fast_model_spec: None,
        permission_bridge: None,
        command_registry: command_registry.clone(),
        skill_manager,
        agent_search_paths: coco_subagent::definition_store::AgentSearchPaths::empty(),
        builtin_agent_catalog: coco_subagent::BuiltinAgentCatalog::interactive(),
        session_id_override: None,
        is_non_interactive: true,
    })
    .await
    .with_context(|| format!("build SessionRuntime for {}/{model_id}", args.provider))?;

    session_runtime.fire_session_start_hooks("startup").await;

    let bootstrap = Arc::new(
        CliInitializeBootstrap::new("default".to_string()).with_command_registry(command_registry),
    );

    let transport = StdioTransport::new();
    let file_history_for_server = session_runtime.file_history.clone().unwrap_or_else(|| {
        Arc::new(tokio::sync::RwLock::new(
            coco_context::FileHistoryState::new(),
        ))
    });
    let server = SdkServer::new(transport)
        .with_session_manager(session_manager)
        .with_initialize_bootstrap(bootstrap)
        .with_file_history(file_history_for_server, std::env::temp_dir())
        .with_session_runtime(session_runtime.clone());

    let runner = Arc::new(QueryEngineRunner::new(
        session_runtime.clone(),
        cli.max_tokens.unwrap_or(2_048),
        cli.max_turns.or(Some(8)),
        Some(system_prompt),
    ));
    server.set_turn_runner(runner).await;

    eprintln!(
        "[sdk_server_stdio] ready (provider={} model={model_id}); reading NDJSON from stdin",
        args.provider
    );
    server
        .run()
        .await
        .map_err(|e| anyhow!("SdkServer.run: {e:?}"))?;
    eprintln!("[sdk_server_stdio] stdin EOF — shutting down cleanly");
    Ok(())
}
