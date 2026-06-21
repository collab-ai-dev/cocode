//! Inner-stage arg0 dispatch for the sandbox self-re-exec contract.
//!
//! The sandbox wraps a command as
//! `<coco_exe> --apply-seccomp <mode> -- <prog> <args>` (Linux) or
//! `<coco_exe> --apply-windows-sandbox <b64> -- <prog> <args>` (Windows).
//! The binary must recognize these argv shapes BEFORE clap parses, dispatch to
//! the matching inner handler (which never returns — it applies the sandbox
//! filter and execs the real program), and otherwise return so normal CLI
//! parsing proceeds.
//!
//! Mirrors the argv0/self-dispatch convention used by
//! `@anthropic-ai/sandbox-runtime` (referenced in `sandbox-adapter.ts:351`),
//! expressed here as an explicit `--apply-*` flag rather than argv0 magic.

use std::ffi::OsString;

/// Arg1 flag for the Linux seccomp-apply inner stage. Canonical owner; the
/// emit side (`platform::linux`) re-exports this.
pub const APPLY_SECCOMP_ARG1: &str = "--apply-seccomp";

/// Arg1 flag for the Windows restricted-token inner stage. Canonical owner;
/// the emit side (`platform::windows`) re-exports this.
pub const APPLY_WINDOWS_SANDBOX_ARG1: &str = "--apply-windows-sandbox";

/// Inspect the process argv for an inner-stage magic flag and, if present,
/// dispatch to the matching handler. Handlers never return (they exec or
/// exit), so this function only returns when argv is a normal CLI invocation.
///
/// MUST be called as the first statement of `main()`, before `Cli::parse()` —
/// clap would otherwise reject the unknown `--apply-*` flag and the inner stage
/// would die before it could apply the sandbox filter.
///
/// Argv layout when matched:
/// `[0]=coco_exe [1]=<flag> [2]=<payload> [3]="--" [4]=<program> [5..]=<args>`
pub fn dispatch_or_continue<I>(args: I)
where
    I: IntoIterator<Item = OsString>,
{
    let argv: Vec<OsString> = args.into_iter().collect();
    let Some(flag) = argv.get(1).and_then(|a| a.to_str()) else {
        return; // no arg1, or arg1 not UTF-8 → normal CLI path
    };
    match flag {
        APPLY_SECCOMP_ARG1 => {
            let (mode, program, args) = parse_payload(&argv, APPLY_SECCOMP_ARG1);
            seccomp_handoff(mode, program, args);
        }
        APPLY_WINDOWS_SANDBOX_ARG1 => {
            let (b64, program, args) = parse_payload(&argv, APPLY_WINDOWS_SANDBOX_ARG1);
            crate::platform::windows::apply_windows_sandbox_and_exec(&b64, &program, &args);
        }
        _ => {} // normal CLI invocation → return, let clap parse
    }
}

/// Extract `(payload, program, args)` from the fixed
/// `[exe, flag, payload, "--", program, args...]` layout. Fail-closed: any
/// structural mismatch exits the process rather than falling through to a
/// normal (unsandboxed) CLI run.
fn parse_payload(argv: &[OsString], flag: &str) -> (String, String, Vec<String>) {
    let payload = match argv.get(2).and_then(|a| a.to_str()) {
        Some(p) => p.to_string(),
        None => bail(flag, "missing or non-UTF8 payload arg"),
    };
    match argv.get(3).and_then(|a| a.to_str()) {
        Some("--") => {}
        _ => bail(flag, "expected `--` separator after payload"),
    }
    let program = match argv.get(4).and_then(|a| a.to_str()) {
        Some(p) => p.to_string(),
        None => bail(flag, "missing or non-UTF8 program"),
    };
    // argv.len() >= 5 here (get(4) succeeded), so `[5..]` never panics.
    let args = argv[5..]
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    (payload, program, args)
}

fn bail(flag: &str, msg: &str) -> ! {
    eprintln!("sandbox inner stage ({flag}): {msg}");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn seccomp_handoff(mode_str: String, program: String, args: Vec<String>) -> ! {
    match crate::seccomp::NetworkSeccompMode::from_str_arg(&mode_str) {
        Some(mode) => crate::seccomp::apply_seccomp_and_exec(mode, &program, &args),
        None => bail(
            APPLY_SECCOMP_ARG1,
            &format!("unknown seccomp mode `{mode_str}`"),
        ),
    }
}

#[cfg(not(target_os = "linux"))]
fn seccomp_handoff(_mode_str: String, _program: String, _args: Vec<String>) -> ! {
    bail(
        APPLY_SECCOMP_ARG1,
        "invoked on a non-Linux platform; refusing",
    )
}

#[cfg(test)]
#[path = "inner_stage.test.rs"]
mod tests;
