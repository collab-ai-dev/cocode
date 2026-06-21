//! Sandbox runtime state.
//!
//! `SandboxState` is the central coordination point for sandbox enforcement,
//! shared via `Arc` across the system (shell executor, tool context, etc.).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::Mutex;

use crate::config::EnforcementLevel;
use crate::config::SandboxBypass;
use crate::config::SandboxConfig;
use crate::config::SandboxSettings;
use crate::monitor::generate_session_tag;
use crate::platform::SandboxPlatform;
use crate::violation::ViolationStore;

/// Network proxy port configuration.
#[derive(Debug, Clone, Copy)]
pub struct ProxyPorts {
    /// HTTP proxy port (default: 3128).
    pub http_port: u16,
    /// SOCKS proxy port (default: 1080).
    pub socks_port: u16,
}

impl Default for ProxyPorts {
    fn default() -> Self {
        Self {
            http_port: 3128,
            socks_port: 1080,
        }
    }
}

/// Hot-reloadable sandbox configuration fields.
///
/// Wrapped in `RwLock` so the `/sandbox` command or settings changes
/// can update enforcement without re-creating the entire state.
struct MutableConfig {
    enforcement: EnforcementLevel,
    settings: SandboxSettings,
    config: SandboxConfig,
    network_active: bool,
    proxy_ports: Option<ProxyPorts>,
    /// Cached proxy environment variables (computed once at `activate_network()`).
    /// Avoids re-allocating 18+ strings on every command execution.
    cached_proxy_env: HashMap<String, String>,
    /// Extra paths to bind-mount into the sandbox (e.g., bridge UDS sockets).
    bridge_bind_paths: Vec<std::path::PathBuf>,
    /// The running egress proxy, owned here so it lives for the session and is
    /// shut down (via its `Drop`) when the state is dropped. `Some` once
    /// [`SandboxState::start_network_proxy`] has run.
    proxy_server: Option<crate::proxy::ProxyServer>,
    /// The running netns socat bridge (Linux only), owned here so its
    /// host-side socat tasks live for the session and tear down on `Drop`.
    /// `Some` once [`SandboxState::start_network_proxy_with_bridge`] has run.
    bridge: Option<crate::proxy::BridgeManager>,
}

/// Pre-computed snapshot for per-command sandbox decisions.
///
/// Produced by `SandboxState::command_snapshot()` with a single `RwLock` read,
/// then consumed by the shell executor without further lock acquisitions.
pub struct CommandSandboxSnapshot {
    /// Whether the command should be wrapped with platform enforcement.
    pub should_wrap: bool,
    /// Whether the managed network proxy is active.
    pub network_active: bool,
    /// Cached proxy environment variables (empty if proxy is not active).
    pub proxy_env: HashMap<String, String>,
    /// Full sandbox config (only populated when `should_wrap` is true).
    pub config: Option<SandboxConfig>,
    /// Whether network is allowed.
    pub allow_network: bool,
    /// Current enforcement level (for tracing).
    pub enforcement: EnforcementLevel,
    /// Shell prefix prepended to the inner command (Linux netns bridge: starts
    /// inner socat listeners forwarding `localhost:<inner_port>` → bind-mounted
    /// UDS → host proxy). `None` on macOS / when no bridge is active.
    pub inner_command_prefix: Option<String>,
}

/// Runtime sandbox state, shared via `Arc` across the system.
///
/// Immutable fields (platform, violations) live directly on the struct.
/// Mutable fields (enforcement, settings, config) are behind a `RwLock`
/// to support hot-reload when the user changes sandbox settings mid-session.
pub struct SandboxState {
    /// Whether platform enforcement is active (kernel-level).
    platform_active: bool,
    /// External sandbox mode: environment is already sandboxed (Docker, CI).
    /// Platform wrapping (bwrap/Seatbelt) is skipped, but env vars, proxy
    /// filtering, and violation tracking still apply.
    external_sandbox: bool,
    /// Violation store (ring buffer, max 100).
    violations: Arc<Mutex<ViolationStore>>,
    /// Hot-reloadable configuration.
    mutable: RwLock<MutableConfig>,
    /// Platform-specific sandbox implementation.
    platform: Box<dyn SandboxPlatform>,
    /// Session-unique tag for macOS log stream filtering and command correlation.
    session_tag: String,
    /// Optional interactive approval bridge, installed after construction by the
    /// runner (SDK: `SdkSandboxApprovalBridge`; TUI: `TuiSandboxApprovalBridge`).
    /// `RwLock` because it is set post-build; [`Self::permission_checker`]
    /// propagates it so the async `check_path_async` variant consults the bridge
    /// before a deny, and [`Self::build_network_ask_callback`] builds the egress
    /// proxy's network-approval callback from it.
    approval_bridge: std::sync::RwLock<Option<crate::bridge::SandboxApprovalBridgeRef>>,
    /// Take-once receiver of non-benign violation counts (the store is built
    /// `with_observer`). The runner drains it into `SandboxViolationsDetected`
    /// flash events; see [`Self::take_violation_observer`].
    violation_observer: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<i32>>>,
    /// Platform violation monitor (macOS `log stream`; passive on Linux/Windows),
    /// retained so it is cancelled when this state drops. Installed once via
    /// [`Self::start_violation_monitor`].
    monitor: std::sync::Mutex<Option<crate::monitor::ViolationMonitor>>,
}

impl std::fmt::Debug for SandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.debug_struct("SandboxState")
            .field("enforcement", &m.enforcement)
            .field("platform_active", &self.platform_active)
            .field("external_sandbox", &self.external_sandbox)
            .field("network_active", &m.network_active)
            .field("proxy_ports", &m.proxy_ports)
            .finish()
    }
}

impl SandboxState {
    /// Shared construction logic for all `SandboxState` variants.
    fn build(
        platform_active: bool,
        external_sandbox: bool,
        enforcement: EnforcementLevel,
        settings: SandboxSettings,
        config: SandboxConfig,
        platform: Box<dyn SandboxPlatform>,
    ) -> Self {
        let (mut store, violation_rx) = ViolationStore::with_observer();
        if !settings.ignore_violations.is_empty() {
            store.set_ignore_patterns(settings.ignore_violations.clone());
        }

        Self {
            platform_active,
            external_sandbox,
            violations: Arc::new(Mutex::new(store)),
            mutable: RwLock::new(MutableConfig {
                enforcement,
                network_active: false,
                proxy_ports: None,
                settings,
                config,
                cached_proxy_env: HashMap::new(),
                bridge_bind_paths: Vec::new(),
                proxy_server: None,
                bridge: None,
            }),
            platform,
            session_tag: generate_session_tag(),
            approval_bridge: std::sync::RwLock::new(None),
            violation_observer: std::sync::Mutex::new(Some(violation_rx)),
            monitor: std::sync::Mutex::new(None),
        }
    }

    /// Create a new sandbox state.
    ///
    /// Call `bootstrap::check_enable_gates()` first to determine if sandbox
    /// should be enabled, then construct the state accordingly.
    pub fn new(
        enforcement: EnforcementLevel,
        settings: SandboxSettings,
        config: SandboxConfig,
        platform: Box<dyn SandboxPlatform>,
    ) -> Self {
        let platform_active = enforcement != EnforcementLevel::Disabled && platform.available();
        Self::build(
            platform_active,
            /*external_sandbox*/ false,
            enforcement,
            settings,
            config,
            platform,
        )
    }

    /// Create a sandbox state for external sandbox mode (Docker, CI).
    ///
    /// Platform wrapping is skipped since the environment is already sandboxed,
    /// but env vars, proxy filtering, and violation tracking still apply.
    pub fn external(
        enforcement: EnforcementLevel,
        settings: SandboxSettings,
        config: SandboxConfig,
    ) -> Self {
        Self::build(
            /*platform_active*/ false,
            /*external_sandbox*/ true,
            enforcement,
            settings,
            config,
            crate::platform::create_platform(),
        )
    }

    /// Create a disabled (no-op) sandbox state.
    pub fn disabled() -> Self {
        Self::build(
            /*platform_active*/ false,
            /*external_sandbox*/ false,
            EnforcementLevel::Disabled,
            SandboxSettings::default(),
            SandboxConfig::default(),
            crate::platform::create_platform(),
        )
    }

    /// Whether sandbox enforcement is active.
    pub fn is_active(&self) -> bool {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.enforcement != EnforcementLevel::Disabled
            && (self.platform_active || self.external_sandbox)
    }

    /// Whether this is an external sandbox (Docker, CI).
    pub fn is_external_sandbox(&self) -> bool {
        self.external_sandbox
    }

    /// Current enforcement level.
    pub fn enforcement(&self) -> EnforcementLevel {
        self.mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enforcement
    }

    /// Whether a command should be sandboxed.
    pub fn should_sandbox_command(&self, command: &str, bypass: SandboxBypass) -> bool {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.enforcement != EnforcementLevel::Disabled
            && (self.platform_active || self.external_sandbox)
            && m.settings.is_sandboxed(command, bypass)
    }

    /// Whether auto-allow for bash commands is enabled.
    pub fn auto_allow_enabled(&self) -> bool {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.enforcement != EnforcementLevel::Disabled
            && (self.platform_active || self.external_sandbox)
            && m.settings.auto_allow_bash_if_sandboxed
    }

    /// Get a snapshot of the sandbox settings.
    pub fn settings(&self) -> SandboxSettings {
        self.mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .settings
            .clone()
    }

    /// Get a snapshot of the sandbox config with bridge bind paths merged.
    pub fn config(&self) -> SandboxConfig {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut config = m.config.clone();
        // Merge runtime state into the config snapshot (not persisted to disk).
        config.proxy_active = m.network_active;
        if !m.bridge_bind_paths.is_empty() {
            config
                .extra_bind_ro
                .extend(m.bridge_bind_paths.iter().cloned());
        }
        config
    }

    /// Build a fresh [`PermissionChecker`] from the live config snapshot.
    ///
    /// Used by tool pre-flight checks (Read/Write/Edit) so SDK consumers
    /// can intercept disallowed file accesses *before* the tool spawns
    /// any I/O — the platform sandboxes (bwrap/Seatbelt) catch the same
    /// violation at the kernel level, but only after the syscall has
    /// already issued. Constructing per-call (rather than caching) means
    /// the checker automatically observes hot-reloaded config changes
    /// without extra wiring; cost is negligible compared to file I/O.
    pub fn permission_checker(&self) -> crate::checker::PermissionChecker {
        let checker = crate::checker::PermissionChecker::new(self.config());
        // Propagate the installed approval bridge so the async check_*_async
        // variants can surface an interactive "allow?" prompt before denying.
        match self
            .approval_bridge
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            Some(bridge) => checker.with_approval_bridge(bridge),
            None => checker,
        }
    }

    /// Install (or replace) the interactive approval bridge. Called by the
    /// runner after [`SessionRuntime`](../../../app/cli) is built: SDK installs
    /// `SdkSandboxApprovalBridge`, interactive TUI installs a TUI bridge.
    /// Interior-mutable (`&self`) so it survives hot-reload on the persistent
    /// `Arc<SandboxState>`.
    pub fn set_approval_bridge(&self, bridge: crate::bridge::SandboxApprovalBridgeRef) {
        *self
            .approval_bridge
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bridge);
    }

    /// Record a sandbox violation into the store so it surfaces to the model
    /// via [`Self::format_violations_since`]. Producers: the Linux executor
    /// (SIGSYS on a seccomp kill) and the egress proxy (denied CONNECT).
    pub async fn record_violation(&self, violation: crate::violation::Violation) {
        self.violations.lock().await.push(violation);
    }

    /// Get the session-unique tag for log filtering and command correlation.
    pub fn session_tag(&self) -> &str {
        &self.session_tag
    }

    /// Get the platform sandbox implementation.
    pub fn platform(&self) -> &dyn SandboxPlatform {
        &*self.platform
    }

    /// Get the violation store.
    pub fn violations(&self) -> &Arc<Mutex<ViolationStore>> {
        &self.violations
    }

    /// Take the non-benign violation observer receiver (once). The runner drains
    /// it into `SandboxViolationsDetected { count }` events for the TUI. Returns
    /// `None` on a second call.
    pub fn take_violation_observer(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<i32>> {
        self.violation_observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Spawn the platform violation monitor (macOS `log stream`; passive on
    /// Linux/Windows) and retain it so it is cancelled when this state drops.
    /// Idempotent, and a no-op unless the platform sandbox is enforcing
    /// (`platform_active`) — there are no kernel violations to observe otherwise.
    /// Survives hot-reload (lives on the persistent `Arc<SandboxState>`).
    pub fn start_violation_monitor(&self) {
        if !self.platform_active {
            return;
        }
        let mut guard = self
            .monitor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_some() {
            return;
        }
        *guard = crate::monitor::ViolationMonitor::start(
            self.violations.clone(),
            tokio_util::sync::CancellationToken::new(),
            self.session_tag.clone(),
        );
    }

    /// Get the current violation count (non-benign).
    pub async fn violation_count(&self) -> i32 {
        self.violations.lock().await.non_benign_count()
    }

    /// Get proxy environment variables for injection into sandboxed commands.
    ///
    /// Returns the cached env vars computed at `activate_network()` time.
    /// Returns an empty map if the proxy is not active.
    pub fn proxy_env_vars(&self) -> HashMap<String, String> {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.cached_proxy_env.clone()
    }

    /// Build proxy environment variables for the given ports.
    ///
    /// Aligned with Claude Code's `f21()` function: sets HTTP, SOCKS, FTP,
    /// Docker, gcloud, gRPC proxy vars and GIT_SSH_COMMAND for SOCKS tunneling.
    fn build_proxy_env_vars(ports: ProxyPorts) -> HashMap<String, String> {
        let http_proxy = format!("http://localhost:{}", ports.http_port);
        // socks5h: DNS resolution via proxy (prevents DNS leaks in sandbox)
        let socks_proxy = format!("socks5h://localhost:{}", ports.socks_port);
        let no_proxy = "localhost,127.0.0.1,::1,*.local,.local,\
                         169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16";

        let mut vars = HashMap::from([
            ("HTTP_PROXY".to_string(), http_proxy.clone()),
            ("HTTPS_PROXY".to_string(), http_proxy.clone()),
            ("http_proxy".to_string(), http_proxy.clone()),
            ("https_proxy".to_string(), http_proxy.clone()),
            ("ALL_PROXY".to_string(), socks_proxy.clone()),
            ("all_proxy".to_string(), socks_proxy.clone()),
            ("NO_PROXY".to_string(), no_proxy.to_string()),
            ("no_proxy".to_string(), no_proxy.to_string()),
            // FTP proxy
            ("FTP_PROXY".to_string(), socks_proxy.clone()),
            ("ftp_proxy".to_string(), socks_proxy.clone()),
            (
                "RSYNC_PROXY".to_string(),
                format!("localhost:{}", ports.socks_port),
            ),
            // Docker proxy
            ("DOCKER_HTTP_PROXY".to_string(), http_proxy.clone()),
            ("DOCKER_HTTPS_PROXY".to_string(), http_proxy),
            // gRPC proxy
            ("GRPC_PROXY".to_string(), socks_proxy.clone()),
            ("grpc_proxy".to_string(), socks_proxy),
            // gcloud SDK proxy
            ("CLOUDSDK_PROXY_TYPE".to_string(), "https".to_string()),
            (
                "CLOUDSDK_PROXY_ADDRESS".to_string(),
                "localhost".to_string(),
            ),
            (
                "CLOUDSDK_PROXY_PORT".to_string(),
                ports.http_port.to_string(),
            ),
        ]);

        // GIT_SSH_COMMAND: route git SSH via SOCKS proxy (macOS/Linux)
        vars.insert(
            "GIT_SSH_COMMAND".to_string(),
            format!(
                "ssh -o ProxyCommand='nc -X 5 -x localhost:{} %h %p'",
                ports.socks_port
            ),
        );

        vars
    }

    /// Start the egress proxy and route the sandbox through it.
    ///
    /// Builds a [`crate::proxy::DomainFilter`] from the configured allow/deny
    /// domain lists, starts the HTTP-CONNECT + SOCKS5 proxy, then activates
    /// network isolation so commands run with the proxy env vars (and, on
    /// Linux, the `proxy_active` seccomp `ProxyRouted` mode). The proxy is
    /// owned by this state and shut down when the state is dropped.
    ///
    /// Idempotent: a no-op if the proxy is already running. Without this call,
    /// "network isolated" would degrade to "block all network" (Linux
    /// `--unshare-net`) because no egress path exists.
    pub async fn start_network_proxy(&self) -> anyhow::Result<()> {
        let (filter, fixed_http, fixed_socks) = {
            let m = self
                .mutable
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if m.network_active {
                return Ok(());
            }
            (
                std::sync::Arc::new(crate::proxy::DomainFilter::from_network_config(
                    &m.settings.network,
                )),
                m.settings.network.http_proxy_port,
                m.settings.network.socks_proxy_port,
            )
        };

        let cancel = tokio_util::sync::CancellationToken::new();
        let ask_cb = self.build_network_ask_callback();
        // `start_with_ports` honors the configured fixed http/socks ports
        // (auto-assigned when `None`).
        let proxy = crate::proxy::ProxyServer::start_with_ports(
            filter,
            cancel,
            fixed_http,
            fixed_socks,
            ask_cb,
        )
        .await?;
        let ports = ProxyPorts {
            http_port: proxy.http_port(),
            socks_port: proxy.socks_port(),
        };
        let env = Self::build_proxy_env_vars(ports);

        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.proxy_server = Some(proxy);
        m.proxy_ports = Some(ports);
        m.cached_proxy_env = env;
        m.network_active = true;
        Ok(())
    }

    /// Start the egress proxy AND a Linux netns socat bridge so a
    /// `--unshare-net` sandbox can reach the host proxy.
    ///
    /// The host proxy listens on `host_*_port` (loopback). A
    /// [`crate::proxy::BridgeManager`] spawns host-side socat processes that
    /// forward bind-mounted UDS sockets → those TCP ports. Inside the netns the
    /// wrapped command runs an inner socat (via
    /// [`CommandSandboxSnapshot::inner_command_prefix`]) that listens on the
    /// netns-local proxy ports and forwards to the UDS. The proxy env therefore
    /// points at the **inner** (netns-local) ports, not the host ports.
    ///
    /// Idempotent. Returns `Err` if the proxy or bridge fails to start; the
    /// caller logs and falls closed (network blocked).
    pub async fn start_network_proxy_with_bridge(
        &self,
        socat_path: std::path::PathBuf,
        session_id: &str,
    ) -> anyhow::Result<()> {
        let (filter, fixed_http, fixed_socks) = {
            let m = self
                .mutable
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if m.network_active {
                return Ok(());
            }
            (
                std::sync::Arc::new(crate::proxy::DomainFilter::from_network_config(
                    &m.settings.network,
                )),
                m.settings.network.http_proxy_port,
                m.settings.network.socks_proxy_port,
            )
        };

        let proxy_cancel = tokio_util::sync::CancellationToken::new();
        let ask_cb = self.build_network_ask_callback();
        let proxy = crate::proxy::ProxyServer::start_with_ports(
            filter,
            proxy_cancel,
            fixed_http,
            fixed_socks,
            ask_cb,
        )
        .await?;
        let host_http = proxy.http_port();
        let host_socks = proxy.socks_port();

        let bridge_cancel = tokio_util::sync::CancellationToken::new();
        let bridge = crate::proxy::BridgeManager::start(
            socat_path,
            host_http,
            host_socks,
            session_id,
            bridge_cancel,
        )
        .await?;
        let bind_paths: Vec<std::path::PathBuf> =
            bridge.socket_paths().iter().map(|p| (*p).clone()).collect();

        // Proxy env points at the netns-local inner ports (the inner socat
        // listeners), not the host proxy ports.
        let inner = crate::proxy::BridgePorts::default();
        let inner_ports = ProxyPorts {
            http_port: inner.http_port,
            socks_port: inner.socks_port,
        };
        let env = Self::build_proxy_env_vars(inner_ports);

        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.proxy_server = Some(proxy);
        m.bridge = Some(bridge);
        m.proxy_ports = Some(inner_ports);
        m.cached_proxy_env = env;
        m.bridge_bind_paths = bind_paths;
        m.network_active = true;
        Ok(())
    }

    /// Build a [`crate::proxy::NetworkAskCallback`] from the installed approval
    /// bridge, if any. On a denied CONNECT the proxy invokes it to surface
    /// "Allow network connection to {host}?" (TS `createSandboxAskCallback`)
    /// before refusing; an `Approved` decision lets the connection through.
    /// Returns `None` when no bridge is installed (fail-closed: static refuse),
    /// OR when `allow_managed_domains_only` is set — that policy forbids
    /// interactively widening past the managed allowlist, so denied hosts get a
    /// static refusal with no prompt (TS `sandbox-adapter.ts` wraps the callback
    /// to hard-deny under the same policy).
    fn build_network_ask_callback(&self) -> Option<crate::proxy::NetworkAskCallback> {
        if self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .settings
            .network
            .allow_managed_domains_only
        {
            return None;
        }
        let bridge = self
            .approval_bridge
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        Some(Arc::new(move |host: String| {
            let bridge = bridge.clone();
            Box::pin(async move {
                let request = crate::bridge::SandboxApprovalRequest {
                    operation: crate::bridge::SandboxOperation::Network,
                    path: host.clone(),
                    reason: format!("network connection to {host}"),
                };
                matches!(
                    bridge.request_approval(request).await,
                    crate::bridge::SandboxApprovalDecision::Approved
                )
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
        }))
    }

    /// Set network isolation as active with the given proxy ports.
    ///
    /// Pre-computes and caches the proxy environment variables so they
    /// don't need to be re-allocated on every command execution.
    pub fn activate_network(&self, ports: ProxyPorts) {
        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.network_active = true;
        m.proxy_ports = Some(ports);
        m.cached_proxy_env = Self::build_proxy_env_vars(ports);
    }

    /// Register bridge socket paths to be bind-mounted into the sandbox.
    pub fn set_bridge_bind_paths(&self, paths: Vec<std::path::PathBuf>) {
        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.bridge_bind_paths = paths;
    }

    /// Whether network isolation is active.
    pub fn network_active(&self) -> bool {
        self.mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .network_active
    }

    /// Take a single-lock snapshot of all fields needed for per-command sandbox
    /// decisions. Avoids 5+ separate `RwLock` acquisitions on the hot path.
    pub fn command_snapshot(&self, command: &str, bypass: SandboxBypass) -> CommandSandboxSnapshot {
        let m = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let has_enforcement_backing = self.platform_active || self.external_sandbox;
        let active = m.enforcement != EnforcementLevel::Disabled && has_enforcement_backing;
        let should_wrap = active && m.settings.is_sandboxed(command, bypass);

        // Linux netns bridge: when a bridge is running, the wrapped command must
        // start inner socat listeners forwarding the netns-local proxy ports to
        // the bind-mounted host UDS. `proxy_ports` carries the inner (netns-local)
        // ports on Linux (set by start_network_proxy_with_bridge).
        let inner_command_prefix = if should_wrap && m.network_active {
            m.bridge.as_ref().map(|b| {
                b.inner_bridge_prefix(&crate::proxy::BridgePorts {
                    http_port: m.proxy_ports.map(|p| p.http_port).unwrap_or(3128),
                    socks_port: m.proxy_ports.map(|p| p.socks_port).unwrap_or(1080),
                })
            })
        } else {
            None
        };

        CommandSandboxSnapshot {
            should_wrap,
            network_active: m.network_active,
            proxy_env: if m.network_active {
                m.cached_proxy_env.clone()
            } else {
                HashMap::new()
            },
            config: if should_wrap {
                let mut config = m.config.clone();
                config.proxy_active = m.network_active;
                if !m.bridge_bind_paths.is_empty() {
                    config
                        .extra_bind_ro
                        .extend(m.bridge_bind_paths.iter().cloned());
                }
                Some(config)
            } else {
                None
            },
            allow_network: m.config.allow_network,
            enforcement: m.enforcement,
            inner_command_prefix,
        }
    }

    /// JSON description of filesystem restrictions for the system prompt.
    ///
    /// Matches Claude Code's E9z format so the model sees structured
    /// restriction data: `{"read":{...},"write":{...}}`.
    pub fn describe_filesystem(&self) -> String {
        let cfg = self.config();

        // Collect all read-denied paths (denied_paths + denied_read_paths).
        let mut deny_read: Vec<String> = cfg
            .denied_paths
            .iter()
            .chain(cfg.denied_read_paths.iter())
            .map(|p| p.display().to_string())
            .collect();
        deny_read.dedup();

        // Collect allow_read carve-outs (re-allow within deny regions).
        let mut allow_read: Vec<String> = cfg
            .allowed_read_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        allow_read.dedup();

        // Collect all write-denied paths (denied_paths + deny_write_paths).
        let mut deny_write: Vec<String> = cfg
            .denied_paths
            .iter()
            .chain(cfg.deny_write_paths.iter())
            .map(|p| p.display().to_string())
            .collect();
        deny_write.dedup();

        // Collect writable roots with read-only subpath annotations.
        let allow_write: Vec<serde_json::Value> = cfg
            .writable_roots
            .iter()
            .map(|r| {
                if r.readonly_subpaths.is_empty() {
                    serde_json::Value::String(r.path.display().to_string())
                } else {
                    serde_json::json!({
                        "path": r.path.display().to_string(),
                        "readOnlySubpaths": r.readonly_subpaths,
                    })
                }
            })
            .collect();

        // Read block carries denyOnly always; allowOnly only when allow_read
        // carve-outs were configured (omits the empty key for compactness
        // and TS-shape compatibility).
        let read_block = if allow_read.is_empty() {
            serde_json::json!({ "denyOnly": deny_read })
        } else {
            serde_json::json!({ "denyOnly": deny_read, "allowOnly": allow_read })
        };

        let desc = serde_json::json!({
            "read": read_block,
            "write": { "allowOnly": allow_write, "denyOnly": deny_write },
        });
        desc.to_string()
    }

    /// JSON description of network restrictions for the system prompt.
    ///
    /// Matches Claude Code's E9z format: `{"allowedHosts":[...],"deniedHosts":[...]}`.
    pub fn describe_network(&self) -> String {
        let settings = self.settings();
        let net = &settings.network;

        if !self.network_active() {
            if !self.config().allow_network {
                return r#"{"status":"blocked"}"#.to_string();
            }
            return r#"{"status":"allowed","filtering":"none"}"#.to_string();
        }

        let desc = serde_json::json!({
            "allowedHosts": net.allowed_domains,
            "deniedHosts": net.denied_domains,
            "unixSockets": if net.allow_all_unix_sockets { "all" } else { "filtered" },
        });
        desc.to_string()
    }

    /// Add a writable root to the sandbox configuration.
    ///
    /// Used when the agent enters a new worktree or workspace that needs
    /// write access. The writable root uses default read-only subpath
    /// protections (.git, .coco, .agents).
    ///
    /// Uses `std::sync::RwLock::write()` (blocking). The lock is held briefly
    /// so this is safe to call from async contexts, but avoid calling while
    /// another task holds the read lock in a tight loop.
    pub fn add_writable_root(&self, path: std::path::PathBuf) {
        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Avoid duplicates
        if !m.config.writable_roots.iter().any(|r| r.path == path) {
            tracing::info!(path = %path.display(), "Adding writable root to sandbox config");
            m.config
                .writable_roots
                .push(crate::config::WritableRoot::new(path));
        }
    }

    /// Snapshot the current total violation count. Pair with
    /// [`Self::format_violations_since`] to summarize anything that
    /// landed during a single command's execution. TS parity:
    /// `annotateStderrWithSandboxFailures` (`sandbox-adapter.ts:961`).
    pub async fn violations_total_snapshot(&self) -> i32 {
        self.violations.lock().await.total_count()
    }

    /// Format violations recorded since `prev_total` as a single
    /// `<sandbox_violations>` block, ready to splice into a command's
    /// stderr. Returns `None` when no new non-benign violations occurred.
    ///
    /// Output shape (one violation per line, kept short for stderr):
    ///
    /// ```text
    /// <sandbox_violations>
    /// op=file-write-data path=/etc/passwd
    /// op=network-outbound path=evil.example.com
    /// </sandbox_violations>
    /// ```
    pub async fn format_violations_since(&self, prev_total: i32) -> Option<String> {
        let store = self.violations.lock().await;
        let current = store.total_count();
        if current <= prev_total {
            return None;
        }
        let new_count = (current - prev_total).min(store.count());
        // `recent(n)` returns the n most recently pushed entries; that's
        // the slice we want. `count()` caps at the ring size, so for very
        // bursty commands we may underreport — accept that vs. unbounded
        // memory.
        let recent = store.recent(new_count);
        let lines: Vec<String> = recent
            .iter()
            .filter(|v| !v.benign && !v.is_benign_pattern())
            .map(|v| match &v.path {
                Some(p) => format!("op={op} path={p}", op = v.operation),
                None => format!("op={op}", op = v.operation),
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        Some(format!(
            "<sandbox_violations>\n{}\n</sandbox_violations>",
            lines.join("\n")
        ))
    }

    /// Allocate a per-command sandbox tmpdir on the host.
    ///
    /// The directory is `mkdir`'d under the parent process's `$TMPDIR`
    /// (or `/tmp` fallback) with a unique `coco-sbx-XXXXXX` suffix.
    /// The returned [`tempfile::TempDir`] owns the cleanup — drop it
    /// after `child.wait_with_output()` returns to remove the dir.
    ///
    /// The path is then handed to:
    /// - [`SandboxPlatform::wrap_command`] via `extra_writable_binds`
    ///   so the inner process can write inside the sandbox.
    /// - The shell provider as `BuildExecOpts.sandbox_tmp_dir` so the
    ///   inner shell writes its cwd-tracking file there and the
    ///   provider can inject `TMPDIR` / `COCO_TMPDIR` / `TMPPREFIX`.
    ///
    /// Returns `None` if `tempfile::tempdir()` fails (extremely
    /// unlikely — would require `/tmp` itself being un-writable).
    pub fn allocate_command_tmp_dir() -> Option<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix("coco-sbx-")
            .tempdir()
            .map_err(|e| tracing::warn!("Failed to allocate sandbox tmpdir: {e}"))
            .ok()
    }

    /// Apply platform sandbox enforcement to a `tokio::process::Command`.
    ///
    /// One-shot helper that combines `command_snapshot` + platform-wrap so
    /// callers outside the shell crate (PowerShell tool, future custom
    /// runners) don't replicate the snapshot logic. Returns `Ok(false)`
    /// if the command should run unsandboxed (excluded, bypass, sandbox
    /// inactive, etc.); returns `Ok(true)` after the wrap is applied.
    ///
    /// On platform-wrap failure (e.g., bwrap binary missing at exec
    /// time), returns the [`crate::SandboxError`] — callers should
    /// fail-closed and refuse to spawn the command unsandboxed.
    pub fn try_wrap_command(
        &self,
        command: &str,
        bypass: SandboxBypass,
        cmd: &mut tokio::process::Command,
    ) -> crate::error::Result<bool> {
        self.try_wrap_command_with_binds(command, bypass, &[], cmd)
    }

    /// Same as [`Self::try_wrap_command`] but additionally bind-mounts
    /// per-command writable paths (the sandbox tmpdir) into the
    /// sandbox. Called by `coco_shell::ShellExecutor` so the
    /// freshly-allocated `TempDir` is visible inside bwrap / Seatbelt.
    pub fn try_wrap_command_with_binds(
        &self,
        command: &str,
        bypass: SandboxBypass,
        extra_writable_binds: &[std::path::PathBuf],
        cmd: &mut tokio::process::Command,
    ) -> crate::error::Result<bool> {
        let snap = self.command_snapshot(command, bypass);
        let Some(config) = snap.config else {
            return Ok(false);
        };
        if !snap.should_wrap {
            return Ok(false);
        }
        self.platform.wrap_command(
            &config,
            command,
            &self.session_tag,
            extra_writable_binds,
            cmd,
        )?;
        for (k, v) in &snap.proxy_env {
            cmd.env(k, v);
        }
        Ok(true)
    }

    /// Hot-reload sandbox configuration.
    ///
    /// Updates enforcement level, settings, and config without re-creating
    /// the entire state. Proxy servers and violation store are preserved.
    /// Called when the user changes sandbox settings via `/sandbox` command.
    pub fn update_config(
        &self,
        enforcement: EnforcementLevel,
        settings: SandboxSettings,
        config: SandboxConfig,
    ) {
        let mut m = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.enforcement = enforcement;
        m.settings = settings;
        m.config = config;
        tracing::info!(
            enforcement = ?m.enforcement,
            "Sandbox configuration hot-reloaded"
        );
    }
}

#[cfg(test)]
#[path = "state.test.rs"]
mod tests;
