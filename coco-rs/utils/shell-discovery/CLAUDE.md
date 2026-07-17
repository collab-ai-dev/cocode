# coco-shell-discovery

Shell binary discovery for hooks and exec: PowerShell, Git Bash, Windows→POSIX path conversion. L0 crate — no internal deps.

| Function | Purpose |
|----------|---------|
| `cached_powershell_path` | `pwsh` (v6+) then `powershell.exe`; process-lifetime OnceCell cache; `None` if neither on PATH |
| `build_powershell_args` | `-NoProfile -NonInteractive -Command <cmd>` argv tail |
| `find_git_bash_path` | Windows-only: PATH, then Program Files / `%LOCALAPPDATA%\Programs\Git`; always `None` elsewhere |
| `windows_path_to_posix_path` | `C:\Users\foo` → `/c/Users/foo` for Git Bash; identity on non-Windows |

The platform asymmetry is deliberate: PowerShell discovery is **cross-platform** (a `shell: "powershell"` hook works anywhere `pwsh` is installed), while Git Bash discovery and path conversion only do work on **Windows** — every other platform's default shell is already bash-compatible, so callers fall back to `/bin/sh`.
