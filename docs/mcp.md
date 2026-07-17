# Model Context Protocol (MCP)

cocode is an MCP client. It connects to MCP servers you configure, discovers the tools and resources they expose, and hands those tools to the model alongside its built-in ones. This page covers configuring servers, the transports that work, tool naming, OAuth, and the MCP tools the model can call.

MCP support is gated by the `mcp` feature, which is **Stable and enabled by default**. You only need to touch it if you want to turn MCP off entirely:

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    "mcp": false // hides all MCP tools from the model
  }
}
```

See [configuration](configuration.md) for how feature toggles layer.

## Configuring servers

Servers are defined in JSON files under an `mcpServers` object. The file has no other top-level keys that cocode reads, so a minimal config is just the one object.

### Where the files live

cocode loads every file below that exists and merges them by server name. Later scopes override earlier ones, so a project definition beats a user one, and policy scopes cannot be shadowed by anything.

| Order | Scope | Path |
|-------|-------|------|
| 1 | User | `~/.cocode/mcp.json` |
| 2 | Project | `<project root>/.mcp.json` |
| 3 | Project | `<project root>/.cocode/mcp.json` |
| 4 | Local | `<cwd>/.cocode.local/mcp.json` |
| 5 | Enterprise | `~/.cocode/enterprise-mcp.json` |
| 6 | Managed | `~/.cocode/managed-mcp.json` |

`~/.cocode/` is the config home; set `$COCO_CONFIG_DIR` to relocate it, and every path in the table moves with it.

`.mcp.json` at the project root is the file to reach for when you want a server checked into the repository and shared with everyone working on it. `~/.cocode/mcp.json` is the right place for personal servers you want in every project. Use `.cocode.local/mcp.json` for machine-specific overrides you do not want committed.

Enterprise and managed configs load last precisely so an administrator-pushed server definition cannot be overridden by a project or a user. They are otherwise ordinary files with the same shape.

### Shape

There is no explicit `transport` field for stdio servers — the transport is inferred from the shape of the entry. An entry with a `command` key is a stdio server; an entry with a `url` key is an HTTP or SSE server.

```jsonc
// .mcp.json — checked into the repo
{
  "mcpServers": {
    // stdio: cocode launches the process and talks to it over stdin/stdout
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/srv/data"],
      "env": {
        "LOG_LEVEL": "info"
      },
      "cwd": "/srv/data"
    },

    // SSE: this is the default when "url" is present and "transport" is absent
    "docs": {
      "url": "https://mcp.example.com/sse",
      "headers": {
        "X-Tenant": "acme"
      }
    },

    // Streamable HTTP: requires the explicit transport tag
    "issues": {
      "transport": "http",
      "url": "https://mcp.example.com/mcp"
    },

    // Kept in the file but not loaded
    "retired": {
      "command": "old-server",
      "disabled": true
    }
  }
}
```

### Supported transports

Only three transports can be expressed in a config file. Anything else is rejected as an unrecognized entry.

| Transport | Trigger | Keys |
|-----------|---------|------|
| stdio | `command` is present | `command`, `args`, `env`, `cwd` |
| SSE | `url` is present, `transport` absent or anything other than `"http"` | `url`, `headers`, `headersHelper`, `oauth` |
| Streamable HTTP | `url` is present and `"transport": "http"` | `url`, `headers`, `headersHelper`, `oauth` |

Note the default: a bare `url` with no `transport` gives you **SSE**, not HTTP. If your server speaks streamable HTTP you must say so explicitly.

`disabled: true` on any entry drops it at parse time — the server is never launched or connected.

`headersHelper` names an external command whose output supplies request headers, for servers that need a short-lived token minted per connection.

### Environment variable expansion

Values in a server entry may reference environment variables using `${VAR}` or `${VAR:-default}`. Expansion happens against the process environment before the config reaches the transport, so you can commit a `.mcp.json` that carries no secrets:

```jsonc
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_TOKEN": "${GITHUB_TOKEN}",
        "GITHUB_HOST": "${GITHUB_HOST:-github.com}"
      }
    }
  }
}
```

If a referenced variable is unset and has no default, the reference is left as the literal `${VAR}` text and a warning is logged. It is not an error, so watch the logs if a server behaves as though it got no credentials.

### Timeouts

Two runtime knobs live in `settings.json` under the `mcp` key rather than in the server config:

```jsonc
// ~/.cocode/settings.json
{
  "mcp": {
    "tool_timeout_ms": 120000, // hard cap on a single MCP tool call
    "tool_idle_timeout_ms": 30000 // cap on silence during a call; 0 disables
  }
}
```

Both can be overridden by environment variable: `COCO_MCP_TOOL_TIMEOUT_MS` and `COCO_MCP_TOOL_IDLE_TIMEOUT_MS`.

## Managing servers

> **Do not use `coco mcp list`, `coco mcp add`, or `coco mcp remove`.** These three subcommands are unimplemented placeholders. `coco mcp list` always prints `MCP servers: (none connected)` regardless of your configuration; `add` and `remove` print what they would do and exit without touching a file. Only `coco mcp login` and `coco mcp logout` do real work.

The `/mcp` slash command, run inside a session, is the supported management path. It reads through the same loader described above, so what it lists is what actually connects:

| Command | What it does |
| --- | --- |
| `/mcp` or `/mcp list` | Lists every server the loader finds, with its scope, transport, and whether it is disabled |
| `/mcp add <name> <command> [args...]` | Writes the server into `<project>/.cocode/mcp.json`, and warns if a higher-precedence file already defines that name |
| `/mcp enable <name>` / `/mcp disable <name>` | Sets or clears `"disabled"` in the file that actually defines the server |
| `/mcp remove <name>` | Deletes the server from its defining file |

Each command names the file it touched, so you can check the result yourself.

Enable, disable, and remove edit **the file where the server is defined**, which is the only thing that works: a `"disabled": true` written into a different file does not mask a definition from a lower-precedence one — the loader skips disabled entries rather than recording them, so the original definition simply survives. Servers defined by enterprise or managed policy are refused rather than edited.

Editing the config files by hand does the same job; to disable a server that way, set `"disabled": true` on its entry in whichever file defines it.

## Tool naming

Every tool an MCP server exposes reaches the model under a namespaced name:

```
mcp__<server>__<tool>
```

A server named `github` exposing a `create_issue` tool becomes `mcp__github__create_issue`. Server and tool names are normalized on the way in: any character outside `[a-zA-Z0-9_-]` is replaced with an underscore, so a server named `my server` yields the prefix `mcp__my_server__`.

This naming is what you match against in permission rules, so an allow rule for a whole server is a prefix match on `mcp__github__`. See [configuration](configuration.md) for permission rule syntax.

## OAuth

**No configuration is needed for ordinary OAuth.** Any HTTP or SSE server is eligible for OAuth login as long as its entry does not carry a static `Authorization` header. Client registration and endpoint discovery are handled against the server automatically, so a plain entry like this is enough:

```jsonc
{
  "mcpServers": {
    "issues": {
      "transport": "http",
      "url": "https://mcp.example.com/mcp"
    }
  }
}
```

The optional `oauth` block exists for advanced cases — chiefly enterprise IDP token exchange, configured under `oauth.xaa`, where a `clientId` is required. If you have configured a static `Authorization` header instead, the login commands will tell you no OAuth is needed and exit. Stdio servers do not support OAuth login at all; give them credentials through `env` instead.

Authenticate from the shell:

```bash
coco mcp login issues
```

This clears any existing credentials for the server, opens your browser to the authorization URL, and waits for the callback. On a headless box, ask for the URL instead of a browser:

```bash
coco mcp login issues --no-browser
```

That prints the authorization URL for you to open elsewhere and blocks until authorization completes. `--headless` is accepted as an alias.

To sign out:

```bash
coco mcp logout issues
```

Both commands resolve the server from the same config files described above, so the name you pass must match a configured server. If the named server does not use OAuth, both commands say so and exit cleanly rather than failing.

Tokens are stored in your system keyring when one is available, and fall back to `~/.cocode/.credentials.json` otherwise. The fallback file is readable by any process running as your user, which is worth knowing if you are on a machine with no keyring.

The model can also trigger authentication mid-session through the `McpAuth` tool, which is useful when a server's token expires during a turn.

## MCP tools available to the model

Beyond the per-server `mcp__*` tools, cocode registers four built-in tools for working with MCP. All four are hidden when the `mcp` feature is off.

| Tool | Purpose |
|------|---------|
| `McpAuth` | Authenticate with a server by name. Takes `server_name`. A fallback for servers that do not expose their own authenticate tool. |
| `ListMcpResourcesTool` | List resources across connected servers. Takes an optional `server` to filter by. |
| `ReadMcpResourceTool` | Read one resource. Takes `server` and `uri`. |
| `ReadMcpResourceDirTool` | List the direct children of a directory resource. Takes `server` and `uri`. Non-recursive. |

### Resources

Resources are server-exposed content addressed by URI — files, records, documents. cocode discovers them at connect time from any server that advertises the resources capability, and the model reads them through `ReadMcpResourceTool`.

`ReadMcpResourceDirTool` is narrower: it only works against servers that declare support for directory listing, and returns an error elsewhere. The listing is one level deep. Subdirectories come back with the mime type `inode/directory` and their own `uri`, which the model passes back to the same tool to descend.

## Elicitation

Elicitation lets a server ask the user a question mid-call — a form to fill in, or a URL to visit to complete a consent flow. Support depends on which surface you are running.

**SDK and AppServer clients** get elicitation requests bridged to them as a server request, and their response is returned to the MCP server.

**The terminal UI does not support elicitation.** An incoming request is dropped and you get an error toast telling you the server tried. If you depend on a server that elicits, drive it from an SDK client.

**Hooks fire either way.** The `Elicitation` hook runs before any dialog is attempted and may program-respond with accept, decline, or cancel — which short-circuits the request entirely and makes it work even on surfaces with no dialog. The `ElicitationResult` hook runs after and can override the action or block it. This is the supported way to handle elicitation non-interactively, including in the TUI. See [extending](extending.md) for hook configuration.
