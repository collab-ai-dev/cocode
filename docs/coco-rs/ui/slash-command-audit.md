# Slash Command Audit

Source scan: `/Users/linyuzhi/codespace/myagent/agents/claude-code-kim/src/commands`
on 2026-05-16.

This audit is the I11 checkpoint for TUI slash-command parity. It accounts for
every TS command source path as implemented, compatibility-only, backend
unsupported, or deliberately omitted. Cross-check `coco-rs/commands/CLAUDE.md`
before treating a TS-only command as a TUI bug.

## Implemented Or Wired In coco-rs

These commands are registered in `coco-commands`, intercepted locally by the
TUI, or routed through the CLI slash dispatcher as text, prompt, overlay, or
transcript-visible status output.

| TS command | coco-rs route |
|---|---|
| `/add-dir` | Local sentinel command handled by the runner. |
| `/agents` | Local overlay/text command. |
| `/branch` | Local overlay/text command; alias `/fork`. |
| `/clear` | TUI-local clear plus engine reset; aliases `/reset`, `/new`. |
| `/color` | Local session-color command. |
| `/compact` | TUI intercept dispatches `UserCommand::Compact`. |
| `/config` | Local config command. |
| `/context` | Local overlay/text command; alias `/ctx`. |
| `/copy` | TUI-local clipboard command. |
| `/cost` | Local text command. |
| `/diff` | Local overlay/text command. |
| `/doctor` | Local diagnostics command. |
| `/effort` | Local effort command. |
| `/exit` | TUI shutdown command; alias `/quit`. |
| `/export` | TUI export picker and local export command. |
| `/files` | Local git-tracked file listing. |
| `/help` | TUI-local i18n help; aliases `/h`, `/?`. |
| `/hooks` | Local hook configuration inspection. |
| `/ide` | Local compatibility text stub until bridge UX is finalized. |
| `/init` | Prompt/local init handler. |
| `/keybindings` | Opens or creates keybindings config. |
| `/mcp` | Local MCP management command. |
| `/memory` | TUI memory picker and editor/opener service. |
| `/model` | TUI model picker or model selection handler. |
| `/permissions` | Local permission rule command; aliases `/perms`, `/allowed-tools`. |
| `/plan` | TUI plan-mode flow and plan-file opener. |
| `/plugin` | Local plugin management; aliases `/plugins`, `/marketplace`. |
| `/pr-comments` | Prompt command. |
| `/reload-plugins` | Local reload sentinel. |
| `/rename` | Local rename sentinel. |
| `/resume` | Local session resume command; alias `/continue`. |
| `/review` | Prompt command. |
| `/rewind` | TUI rewind picker; alias `/checkpoint`. |
| `/sandbox` | Local sandbox configuration command. |
| `/security-review` | Prompt command. |
| `/session` | Local session management command; alias `/remote`. |
| `/skills` | Local skill listing command. |
| `/stats` | Local usage/activity stats command. |
| `/status` | TUI-local status output; alias `/st`. |
| `/statusline` | Prompt command. |
| `/summary` | Auto-memory gated local summary command. |
| `/tag` | Local session tag sentinel. |
| `/tasks` | Local task panel/status command; aliases `/todo`, TS `/bashes`. |
| `/theme` | TUI-local theme overlay/application command. |
| `/usage` | Local usage command. |
| `/version` | Local version command. |
| `/vim` | Local editor-mode command. |
| `commit.ts` `/commit` | Prompt command. |
| `commit-push-pr.ts` `/commit-push-pr` | Prompt command. |
| `insights.ts` `/insights` | Prompt command using the checked-in insights prompt. |

## Compatibility Or Thinned Implementations

These commands intentionally do less than TS but still return a durable command
result or status instead of failing silently.

| TS command | coco-rs disposition |
|---|---|
| `/output-style` | Hidden deprecation stub; users should use `/config` / settings-backed output styles. |
| `/install` | Local compatibility text; Anthropic installer UX is not copied. |
| `/upgrade` | Local update-check compatibility text. |
| `/remote-control` (`bridge/`, alias `/rc`) | Bridge crate exists, but slash UX remains compatibility-only. |
| `/web-setup` (`remote-setup/`) | Remote setup is product/backend-specific; no local CCR setup flow. |
| `/remote-env` | Remote-only TS surface; no local provider-neutral UI. |

## Deliberately Omitted Provider/Product Commands

These are Anthropic account, product, platform, or CCR-specific and should not
be reported as missing coco-rs TUI work.

| TS command | Reason |
|---|---|
| `/login`, `/logout` | Anthropic account OAuth; provider auth belongs in provider crates. |
| `/feedback` / `/bug` | Anthropic Statsig feedback endpoint. |
| `/fast` | Claude.ai/console fast-mode picker; coco-rs exposes fast state through runtime controls. |
| `/privacy-settings` | Anthropic consumer-account settings. |
| `/rate-limit-options`, `/reset-limits`, `/extra-usage` | Anthropic billing/limit flows. |
| `/install-github-app`, `/install-slack-app` | Anthropic marketplace/OAuth flows. |
| `/chrome`, `/mobile`, `/desktop`, `/passes` | First-party platform/app promotion flows. |
| `/terminal-setup` | Anthropic `claude` CLI terminal binding installer. |
| `/advisor`, `/brief`, `/ultraplan`, `/ultrareview` | Anthropic internal/CCR/KAIROS backends. |
| `/voice`, `/think-back`, `/thinkback-play` | Experimental or first-party media/backplay backends. |

## TS Hidden Stubs And Dev-Only Sources

These TS files are literal disabled stubs, internal diagnostics, or cosmetic
easter eggs. They stay hidden or deliberately absent in coco-rs.

| TS source | Disposition |
|---|---|
| `ant-trace/` | Disabled internal tracing stub. |
| `autofix-pr/` | Disabled stub. |
| `backfill-sessions/` | Internal migration helper. |
| `break-cache/` | Dev-only cache invalidation stub. |
| `btw/`, `good-claude/`, `stickers/` | Cosmetic/easter-egg surfaces. |
| `bughunter/`, `issue/`, `mock-limits/`, `perf-issue/` | Disabled internal/reporting stubs. |
| `ctx_viz/`, `heapdump/` | Internal runtime diagnostics with no Rust TUI equivalent. |
| `debug-tool-call/`, `env/` | Hidden Rust debug commands exist for power users. |
| `oauth-refresh/`, `onboarding/`, `share/`, `teleport/` | Disabled TS stubs or product-specific session flows. |
| `release-notes/` | Anthropic-hosted changelog; not TUI slash-invoked. |
| `init-verifiers.ts` | TS setup helper; no standalone TUI slash surface. |
| `createMovedToPluginCommand.ts` | TS migration helper, not a user command. |

