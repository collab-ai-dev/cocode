//! In-process seccomp BPF filter compilation and application.
//!
//! Generates and applies seccomp filters at runtime using the `seccompiler`
//! crate. Eliminates the need for external `seccomp-apply` binaries and
//! pre-compiled BPF files.
//!
//! Two filter modes are supported (from codex-rs):
//! - **Restricted**: allows AF_UNIX sockets, blocks all IP sockets and network
//!   syscalls. Used when network namespace is fully isolated.
//! - **ProxyRouted**: allows AF_INET/AF_INET6 (proxy bridge) and AF_UNIX (the
//!   inner netns socat UNIX-CONNECTs to the bind-mounted bridge socket),
//!   blocking only exotic socket domains. Used when traffic routes through a
//!   managed proxy.
//!
//! Both modes always block `ptrace` and `io_uring` syscalls.

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::BTreeMap;

    use seccompiler::BpfProgram;
    use seccompiler::SeccompAction;
    use seccompiler::SeccompCmpArgLen;
    use seccompiler::SeccompCmpOp;
    use seccompiler::SeccompCondition;
    use seccompiler::SeccompFilter;
    use seccompiler::SeccompRule;
    use seccompiler::TargetArch;

    /// Seccomp network filtering mode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum NetworkSeccompMode {
        /// AF_UNIX sockets allowed, all IP sockets denied.
        /// Used inside fully-isolated network namespace (`bwrap --unshare-net`).
        Restricted,
        /// AF_INET/AF_INET6 allowed (proxy bridge loopback) plus AF_UNIX (the
        /// inner netns socat needs UDS to reach the bind-mounted bridge);
        /// exotic socket domains denied.
        ProxyRouted,
    }

    impl NetworkSeccompMode {
        /// Parse from command-line argument string.
        pub fn from_str_arg(s: &str) -> Option<Self> {
            match s {
                "restricted" => Some(Self::Restricted),
                "proxy-routed" => Some(Self::ProxyRouted),
                _ => None,
            }
        }

        /// Convert to command-line argument string.
        pub fn as_str_arg(self) -> &'static str {
            match self {
                Self::Restricted => "restricted",
                Self::ProxyRouted => "proxy-routed",
            }
        }
    }

    /// Determine the seccomp mode from sandbox configuration.
    ///
    /// Returns `None` if seccomp should be skipped (network allowed, no proxy).
    pub fn determine_seccomp_mode(
        allow_network: bool,
        proxy_active: bool,
    ) -> Option<NetworkSeccompMode> {
        if allow_network && !proxy_active {
            // Full network, no proxy → skip seccomp
            None
        } else if proxy_active {
            // Proxy active → allow IP sockets for bridge, block AF_UNIX
            Some(NetworkSeccompMode::ProxyRouted)
        } else {
            // Network blocked → allow AF_UNIX only, block IP sockets
            Some(NetworkSeccompMode::Restricted)
        }
    }

    /// Compile a seccomp BPF filter for the given mode.
    ///
    /// The filter is compiled in-process using the `seccompiler` crate.
    /// It blocks dangerous syscalls (ptrace, io_uring) and applies
    /// mode-specific socket restrictions.
    pub fn compile_seccomp_filter(
        mode: NetworkSeccompMode,
    ) -> std::result::Result<BpfProgram, seccompiler::BackendError> {
        let arch = target_arch();
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

        // Always deny: ptrace (prevents debugger attachment / sandbox escape)
        deny_syscall(&mut rules, libc::SYS_ptrace);

        // Always deny: io_uring (can bypass seccomp entirely)
        deny_syscall(&mut rules, libc::SYS_io_uring_setup);
        deny_syscall(&mut rules, libc::SYS_io_uring_enter);
        deny_syscall(&mut rules, libc::SYS_io_uring_register);

        match mode {
            NetworkSeccompMode::Restricted => {
                for syscall in NETWORK_SYSCALLS {
                    deny_syscall(&mut rules, *syscall);
                }

                // AF_UNIX must stay open: cargo, rust-analyzer, and other tools
                // use Unix sockets for IPC and would break without them.
                let deny_non_unix = SeccompRule::new(vec![SeccompCondition::new(
                    0, // first arg: socket domain
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    libc::AF_UNIX as u64,
                )?])?;
                rules.insert(libc::SYS_socket, vec![deny_non_unix.clone()]);
                rules.insert(libc::SYS_socketpair, vec![deny_non_unix]);
            }
            NetworkSeccompMode::ProxyRouted => {
                // AF_INET/AF_INET6 are allowed for the proxy bridge; AF_UNIX is
                // ALSO allowed because the inner netns socat does a UNIX-CONNECT
                // to the bind-mounted bridge socket (blocking it would EPERM the
                // bridge and silently break egress). Only exotic socket domains
                // are denied.
                //
                // seccompiler AND-combines conditions within a single rule:
                // deny if NOT AF_INET AND NOT AF_INET6 AND NOT AF_UNIX.
                let deny_exotic = SeccompRule::new(vec![
                    SeccompCondition::new(
                        0,
                        SeccompCmpArgLen::Dword,
                        SeccompCmpOp::Ne,
                        libc::AF_INET as u64,
                    )?,
                    SeccompCondition::new(
                        0,
                        SeccompCmpArgLen::Dword,
                        SeccompCmpOp::Ne,
                        libc::AF_INET6 as u64,
                    )?,
                    SeccompCondition::new(
                        0,
                        SeccompCmpArgLen::Dword,
                        SeccompCmpOp::Ne,
                        libc::AF_UNIX as u64,
                    )?,
                ])?;
                rules.insert(libc::SYS_socket, vec![deny_exotic]);
                // socketpair (AF_UNIX) is left allowed — the bridge needs it.
            }
        }

        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            arch,
        )?;

        filter.try_into()
    }

    /// Apply seccomp filter and exec into the target command.
    ///
    /// This is the entry point for the `--apply-seccomp` arg0 dispatch.
    /// Called inside a bubblewrap namespace where `CAP_SYS_ADMIN` is available,
    /// so `PR_SET_NO_NEW_PRIVS` is handled by `seccompiler::apply_filter`.
    ///
    /// # Exits
    /// Replaces the current process via `exec`. Only returns on error.
    pub fn apply_seccomp_and_exec(mode: NetworkSeccompMode, program: &str, args: &[String]) -> ! {
        use std::os::unix::process::CommandExt;

        let filter = compile_seccomp_filter(mode).unwrap_or_else(|e| {
            eprintln!("seccomp: failed to compile BPF filter: {e}");
            std::process::exit(1);
        });

        if let Err(e) = seccompiler::apply_filter(&filter) {
            eprintln!("seccomp: failed to apply filter: {e}");
            std::process::exit(1);
        }

        let err = std::process::Command::new(program).args(args).exec();
        eprintln!("seccomp: exec failed: {err}");
        std::process::exit(1);
    }

    // Network syscalls blocked in Restricted mode.
    const NETWORK_SYSCALLS: &[i64] = &[
        libc::SYS_connect,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_getpeername,
        libc::SYS_getsockname,
        libc::SYS_shutdown,
        libc::SYS_sendto,
        libc::SYS_sendmmsg,
        libc::SYS_recvmmsg,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
    ];

    fn deny_syscall(rules: &mut BTreeMap<i64, Vec<SeccompRule>>, syscall: i64) {
        // Empty rule vec = unconditional match → apply violation action (EPERM).
        rules.insert(syscall, vec![]);
    }

    fn target_arch() -> TargetArch {
        #[cfg(target_arch = "x86_64")]
        {
            TargetArch::x86_64
        }
        #[cfg(target_arch = "aarch64")]
        {
            TargetArch::aarch64
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            compile_error!("seccomp is only supported on x86_64 and aarch64");
        }
    }
}

#[cfg(test)]
#[path = "seccomp.test.rs"]
mod tests;
