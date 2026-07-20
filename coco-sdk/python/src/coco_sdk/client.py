"""Multi-turn coco client for interactive sessions."""

from __future__ import annotations

import asyncio
import json
import logging
from typing import TYPE_CHECKING, Any, AsyncIterator, Awaitable, Callable

from pydantic import BaseModel, TypeAdapter

from coco_sdk._composer import build_plain_text_turn_start
from coco_sdk._internal.transport import Transport
from coco_sdk._internal.transport.subprocess_cli import SubprocessCLITransport
from coco_sdk._message_router import MessageRouter
from coco_sdk.decorators import HookDefinition
from coco_sdk.errors import ProcessError
from coco_sdk.generated.protocol import (
    ApprovalDecision,
    ApprovalResolveRequest,
    CancelRequest,
    ConfigApplyFlagsRequest,
    ConfigReadRequest,
    ConfigReadResult,
    ConfigWriteRequest,
    ContextUsageRequest,
    ContextUsageResult,
    HookCallbackMatcher,
    InitializeRequest,
    KeepAliveRequest,
    McpConnectionStatus,
    McpReconnectRequest,
    McpServerConfig,
    McpSetServersRequest,
    McpSetServersResult,
    McpStatusRequest,
    McpStatusResult,
    McpToggleRequest,
    PermissionMode,
    PluginReloadRequest,
    PluginReloadResult,
    RewindFilesRequest,
    HookCallbackOutput,
    ServerNotification,
    ServerNotificationTurnEnded,
    ServerRequestMethod,
    SessionCloseRequest,
    SessionDeleteRequest,
    SessionListRequest,
    SessionListResult,
    SessionReadRequest,
    SessionReadResult,
    SessionResumeRequest,
    SessionResumeResult,
    SessionStartRequest,
    SessionStartResult,
    SessionTarget,
    SetModelRequest,
    SetPermissionModeRequest,
    SetThinkingRequest,
    StopTaskRequest,
    ThinkingLevel,
    TurnEndedParams,
    TurnInterruptRequest,
    UpdateEnvRequest,
)
from coco_sdk.types import ModelSpec

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from coco_sdk.tools import ToolDefinition

# Validator for inbound `ServerNotification` payloads. Pydantic's
# discriminated-union dispatch picks the right variant class based on
# the `method` field. `TypeAdapter` is the v2 idiom for validating
# against a `Union`/`Annotated[Union[...]]` type alias.
_SERVER_NOTIFICATION_ADAPTER: TypeAdapter[ServerNotification] = TypeAdapter(
    ServerNotification
)

# A follow-up `turn/start` can arrive in the brief window after the server emits
# `TurnEnded` but before the detached turn task's post-run cleanup clears the
# one-at-a-time slot, so the server transiently rejects it with "a turn is
# already running". Retry that specific rejection with a short backoff rather
# than surfacing a race to the caller. Bounded so a genuinely concurrent turn
# still raises.
_TURN_START_BUSY_MARKER = "already running"
_TURN_START_RETRY_INITIAL = 0.01
_TURN_START_RETRY_MAX_DELAY = 0.2
_TURN_START_RETRY_TIMEOUT = 5.0


def _safe_parse_notification(line_data: dict[str, Any]) -> ServerNotification | None:
    """Validate an inbound notification dict against the typed
    :class:`ServerNotification` discriminated union.

    Pydantic dispatches on the wire `method` field and returns one of
    the 71 typed `ServerNotification*` variants (e.g.
    `ServerNotificationTurnCompleted`). Returns `None` for unknown
    methods or malformed payloads — the consumer skips them.
    """
    try:
        return _SERVER_NOTIFICATION_ADAPTER.validate_python(line_data)
    except Exception as exc:
        logger.warning(
            "Failed to parse notification %s: %s",
            line_data.get("method"),
            exc,
        )
        return None


# Callback type for permission decisions.
# Input is a per-tool argument record — TS uses `z.record(z.string(), z.unknown())`;
# Python expresses that as `dict[str, Any]`. This is genuinely heterogeneous
# (per-tool input schema), so no further narrowing is possible.
CanUseTool = Callable[[str, dict[str, Any]], Awaitable[ApprovalDecision]]

# Hook handler: (callback_id, event_type, input) -> output.
# Output may be the typed `HookCallbackOutput` (TS-canonical wire shape) or
# a bare `dict` for callers that prefer raw form. The client normalizes
# both via `_normalize_hook_output` (Pydantic models dump via
# `by_alias=True` so the camelCase wire shape is preserved).
HookHandler = Callable[
    [str, str, dict[str, Any]],
    Awaitable[HookCallbackOutput | dict[str, Any]],
]


class CocoClient:
    """Multi-turn client for coco sessions with bidirectional control.

    On ``start()`` the client sends an ``initialize`` request to the
    Rust ``coco sdk`` process (registering hooks / agents / SDK-hosted
    MCP servers) and then a ``session/start`` request that carries the
    per-session knobs (models_main, max turns, budget, permission mode,
    system prompts, structured output ``json_schema``). The prompt itself
    goes out in a third ``turn/start`` request.

    Example::

        from coco_sdk import CocoClient
        from coco_sdk.types import DEEPSEEK

        async with CocoClient(prompt="Fix the bug in main.rs",
                              models_main=DEEPSEEK.flash_openai) as client:
            async for event in client.events():
                print(event.method, event.params)
    """

    def __init__(
        self,
        prompt: str,
        *,
        # Model selection
        models_main: str | ModelSpec | None = None,
        # Per-session knobs (mapped to SessionStartParams)
        max_turns: int | None = None,
        max_budget_usd: float | None = None,
        cwd: str | None = None,
        permission_mode: PermissionMode | str | None = None,
        system_prompt: str | None = None,
        append_system_prompt: str | None = None,
        # Initialize-time registrations.
        # `agents` is opaque on the wire (`InitializeParams.agents:
        # dict[str, Any]`), so the SDK passes user-built dicts through
        # untouched. `hooks` takes :class:`HookDefinition` instances
        # produced by ``@hook(...)`` — the wire shape
        # (:class:`HookCallbackMatcher`, keyed by event) is built in
        # :meth:`_send_initialize`.
        agents: dict[str, dict[str, Any]] | None = None,
        hooks: list[HookDefinition] | None = None,
        mcp_servers: dict[str, McpServerConfig] | None = None,
        tools: list["ToolDefinition"] | None = None,
        json_schema: dict[str, Any] | None = None,
        agent_progress_summaries: bool | None = None,
        prompt_suggestions: bool | None = None,
        # Bidirectional callbacks
        can_use_tool: CanUseTool | None = None,
        # Transport
        env: dict[str, str] | None = None,
        binary_path: str | None = None,
        transport: Transport | None = None,
    ):
        self._initial_prompt = prompt
        self._models_main = str(models_main) if models_main is not None else None
        self._max_turns = max_turns
        self._max_budget_usd = max_budget_usd
        self._cwd = cwd
        self._permission_mode = (
            PermissionMode(permission_mode)
            if isinstance(permission_mode, str)
            else permission_mode
        )
        self._system_prompt = system_prompt
        self._append_system_prompt = append_system_prompt
        self._agents = agents
        self._hooks = hooks
        self._mcp_servers = mcp_servers
        self._tools = tools
        self._json_schema = json_schema
        self._agent_progress_summaries = agent_progress_summaries
        self._prompt_suggestions = prompt_suggestions
        # `coco sdk` rejects the legacy default model at startup, so
        # `--models.main provider/model_id` must be set BEFORE the subcommand
        # rather than only sent on the wire via `session/start.model`.
        cli_args: list[str] = []
        if self._models_main:
            cli_args += ["--models.main", self._models_main]
        self._transport = transport or SubprocessCLITransport(
            binary_path=binary_path,
            cwd=cwd,
            env=env,
            cli_args=cli_args,
        )
        self._can_use_tool = can_use_tool
        self._hook_handlers: dict[str, HookHandler] = {}
        self._tool_registry: dict[str, "ToolDefinition"] = {}
        self._router: MessageRouter | None = None
        self._started = False
        self._session_id: str | None = None

        if tools:
            for tool_def in tools:
                self._tool_registry[tool_def.server_name] = tool_def
        if hooks:
            for h in hooks:
                handler = getattr(h, "fn", None)
                cb_id = getattr(h, "callback_id", None)
                if handler and cb_id:
                    self._hook_handlers[cb_id] = handler

    async def __aenter__(self) -> "CocoClient":
        await self.start()
        return self

    async def __aexit__(self, *args: object) -> None:
        await self.close()

    async def start(self) -> None:
        """Bring up the session: ``initialize`` → ``session/start`` → ``turn/start``.

        Three wire requests in sequence:

        1. ``initialize`` — register hooks/agents/client MCP servers
           (connection capabilities) with coco-rs.
        2. ``session/start`` — create the session shell (returns a
           ``session_id``) and carry the per-session execution policy
           (model, budget, permission mode, system prompts, structured
           output ``json_schema``). Does NOT run a turn — ``initial_prompt``
           on this request is metadata, not an instruction.
        3. ``turn/start`` — actually run the user's prompt and start
           the notification stream the caller iterates over.
        """
        try:
            await self._transport.start()
            self._router = MessageRouter(
                self._transport,
                server_request_handler=self._handle_server_request,
            )
            self._router.start()

            client_mcp_servers = await self._send_initialize()
            await self._send_session_start()
            await self._wait_for_client_mcp_servers(client_mcp_servers)
            await self._send_turn_start(self._initial_prompt)
            self._started = True
        except BaseException:
            await self.close()
            raise

    async def _send_initialize(self) -> list[str]:
        """Send the initialize handshake.

        Registers connection capabilities only: hooks, agents,
        client-hosted MCP servers, and the progress/suggestion flags.
        Per-session execution policy (system prompts, structured output
        ``json_schema``) rides ``session/start`` instead — Rust
        ``InitializeParams`` has no such fields.
        """
        client_mcp_servers: list[str] = []
        if self._tools:
            for tool_def in self._tools:
                client_mcp_servers.append(tool_def.server_name)

        hooks_map: dict[str, list[HookCallbackMatcher]] | None = None
        if self._hooks:
            hooks_map = {}
            for h in self._hooks:
                event = getattr(h, "event", None)
                cb_id = getattr(h, "callback_id", None)
                if event is None or cb_id is None:
                    continue
                matcher = HookCallbackMatcher(
                    hook_callback_ids=[cb_id],
                    matcher=getattr(h, "matcher", None),
                    timeout=_ms_to_seconds(getattr(h, "timeout_ms", None)),
                )
                hooks_map.setdefault(event, []).append(matcher)

        # `agents` is opaque pass-through; user supplies dicts already
        # in the shape coco-rs expects. No conversion needed.
        agents_map = self._agents or None

        params = InitializeRequest.InitializeRequestParams(
            agents=agents_map,
            hooks=hooks_map,
            client_mcp_servers=client_mcp_servers or None,
            agent_progress_summaries=self._agent_progress_summaries,
            prompt_suggestions=self._prompt_suggestions,
        )

        request = InitializeRequest(params=params)
        await self._request(request)
        return client_mcp_servers

    async def _wait_for_client_mcp_servers(self, server_names: list[str]) -> None:
        if not server_names:
            return
        deadline = asyncio.get_running_loop().time() + 10.0
        pending = set(server_names)
        while pending:
            status = await self.mcp_status()
            by_name = {server.name: server for server in status.mcp_servers}
            failed: dict[str, str | None] = {}
            for name in list(pending):
                server = by_name.get(name)
                if server is None:
                    continue
                if server.status == McpConnectionStatus.connected:
                    pending.remove(name)
                elif server.status == McpConnectionStatus.failed:
                    failed[name] = server.error
            if failed:
                details = ", ".join(
                    f"{name}: {error or 'failed'}"
                    for name, error in sorted(failed.items())
                )
                raise RuntimeError(f"client MCP server connection failed: {details}")
            if asyncio.get_running_loop().time() >= deadline:
                names = ", ".join(sorted(pending))
                raise TimeoutError(f"timed out waiting for client MCP servers: {names}")
            await asyncio.sleep(0.05)

    async def _send_session_start(self) -> None:
        # `initial_prompt` is intentionally omitted — it does not
        # auto-run a turn (verified empirically against `coco sdk`).
        # The actual prompt goes through `_send_turn_start`.
        params = SessionStartRequest.SessionStartRequestParams(
            model=self._models_main,
            max_turns=self._max_turns,
            max_budget_usd=self._max_budget_usd,
            cwd=self._cwd,
            permission_mode=self._permission_mode,
            system_prompt=self._system_prompt,
            append_system_prompt=self._append_system_prompt,
            json_schema=self._json_schema,
        )
        request = SessionStartRequest(params=params)
        result = SessionStartResult.model_validate(await self._request(request))
        self._session_id = result.session_id

    async def _send_turn_start(self, prompt: str) -> None:
        request = build_plain_text_turn_start(
            self._session_target(),
            prompt,
        )
        await self._request(request)

    async def events(self) -> AsyncIterator[ServerNotification]:
        """Yield notifications from the current turn.

        Server-initiated requests are never yielded — the router
        dispatches them to handlers and replies on the wire:

        * ``approval/askForApproval`` — answered by the ``can_use_tool``
          callback when configured; otherwise the client replies with a
          JSON-RPC error, which withdraws it from the approval broadcast
          so another connected client (e.g. a human TUI) can answer.
        * ``hook/callback`` — dispatched to the handler registered via
          :meth:`on_hook` / the ``hooks`` constructor argument.
        * ``mcp/routeMessage`` — dispatched to SDK-hosted tool servers.
        * Every other ``ServerRequest`` (user input, elicitation, …) is
          auto-error-replied (``-32601``), which likewise only withdraws
          this client from the broadcast under the server contract.

        Wire-frame routing:

        * notifications — yielded as :class:`ServerNotification`; the
          iterator terminates on ``turn/ended``.
        * responses — consumed by the request/reply machinery, never
          yielded.
        * error frames without a matching pending request — logged at
          WARNING and dropped.
        """
        router = self._require_router()
        while True:
            line_data = await router.next_event()
            event = _safe_parse_notification(line_data)
            if event is None:
                # Unknown method or malformed payload — already logged.
                continue
            yield event
            # Break on the wire-protocol turn terminator: `TurnEnded`
            # discriminates the outcome (`completed` / `failed` /
            # `interrupted` / `max_turns_reached` / `budget_exhausted`)
            # via `params.outcome.kind`. Without this, `events()` would
            # block forever on the non-success paths since the transport
            # stays open.
            if isinstance(event, ServerNotificationTurnEnded):
                break

    async def send(self, text: str) -> AsyncIterator[ServerNotification]:
        """Send a follow-up message and yield events from the new turn."""
        request = build_plain_text_turn_start(
            self._session_target(),
            text,
        )
        await self._start_turn_with_retry(request)
        async for event in self.events():
            yield event

    async def _start_turn_with_retry(self, request: Any) -> None:
        """Send ``turn/start``, tolerating the server's post-``TurnEnded``
        finalization window.

        After a turn ends, the server clears its one-at-a-time turn slot in a
        detached cleanup that runs *after* ``TurnEnded`` is already on the wire.
        A follow-up sent the instant a caller sees ``TurnEnded`` can therefore
        race that cleanup and be rejected with "a turn is already running". That
        is transient, so retry with a short exponential backoff; a genuinely
        concurrent turn still surfaces the error once the bounded window lapses.
        """
        loop = asyncio.get_event_loop()
        deadline = loop.time() + _TURN_START_RETRY_TIMEOUT
        delay = _TURN_START_RETRY_INITIAL
        while True:
            try:
                await self._request(request)
                return
            except ProcessError as exc:
                now = loop.time()
                if _TURN_START_BUSY_MARKER not in str(exc) or now >= deadline:
                    raise
                await asyncio.sleep(min(delay, deadline - now))
                delay = min(delay * 2, _TURN_START_RETRY_MAX_DELAY)

    # ── Bidirectional control methods ────────────────────────────────

    async def approve(
        self,
        request_id: str,
        decision: ApprovalDecision,
        *,
        feedback: str | None = None,
        permission_updates: list[Any] | None = None,
        updated_input: Any = None,
    ) -> None:
        """Resolve a pending approval request.

        ``feedback`` surfaces a short reason to the agent.
        ``updated_input`` lets the SDK rewrite the tool call before it
        runs (e.g. tighten a glob pattern). ``permission_updates`` add
        permission rules to one of the four scopes
        (``user``/``project``/``local``/``session``).
        """
        params = ApprovalResolveRequest.ApprovalResolveRequestParams(
            target=self._session_target(),
            request_id=request_id,
            decision=decision,
            feedback=feedback,
            permission_updates=permission_updates or [],
            updated_input=updated_input,
        )
        request = ApprovalResolveRequest(params=params)
        await self._notify(request)

    async def interrupt(self) -> None:
        """Interrupt the current turn."""
        request = TurnInterruptRequest(
            params=TurnInterruptRequest.TurnInterruptRequestParams(
                **self._session_target().model_dump()
            )
        )
        await self._notify(request)

    async def set_models_main(self, models_main: str | ModelSpec) -> None:
        """Change the main model for subsequent turns."""
        request = SetModelRequest(
            params=SetModelRequest.SetModelRequestParams(
                target=self._session_target(), model=str(models_main)
            )
        )
        await self._notify(request)

    async def set_permission_mode(self, mode: PermissionMode | str) -> None:
        """Change the permission mode."""
        if isinstance(mode, str):
            mode = PermissionMode(mode)
        request = SetPermissionModeRequest(
            params=SetPermissionModeRequest.SetPermissionModeRequestParams(
                target=self._session_target(), mode=mode
            )
        )
        await self._notify(request)

    async def set_thinking(self, level: ThinkingLevel | None) -> None:
        """Change the reasoning level for subsequent turns.

        Pass ``None`` to clear (server-side default applies). Use
        :func:`coco_sdk.types.thinking` to build the level.
        """
        request = SetThinkingRequest(
            params=SetThinkingRequest.SetThinkingRequestParams(
                target=self._session_target(), thinking_level=level
            )
        )
        await self._notify(request)

    async def stop_task(self, task_id: str) -> None:
        """Stop a running background task."""
        request = StopTaskRequest(
            params=StopTaskRequest.StopTaskRequestParams(
                target=self._session_target(), task_id=task_id
            )
        )
        await self._notify(request)

    async def update_env(self, env: dict[str, str]) -> None:
        """Update environment variables exposed to tool execution."""
        request = UpdateEnvRequest(
            params=UpdateEnvRequest.UpdateEnvRequestParams(
                target=self._session_target(), env=env
            )
        )
        await self._notify(request)

    async def rewind_files(
        self, user_message_id: str, *, dry_run: bool = False
    ) -> None:
        """Revert files to the state at a prior user message.

        Set ``dry_run=True`` to receive a preview notification without
        touching the filesystem.
        """
        request = RewindFilesRequest(
            params=RewindFilesRequest.RewindFilesRequestParams(
                target=self._session_target(),
                user_message_id=user_message_id,
                dry_run=dry_run,
            )
        )
        await self._notify(request)

    async def cancel_request(
        self, request_id: str, *, reason: str | None = None
    ) -> None:
        """Cancel a pending server-initiated request."""
        request = CancelRequest(
            params=CancelRequest.CancelRequestParams(
                request_id=request_id, reason=reason
            )
        )
        await self._notify(request)

    async def keep_alive(self) -> None:
        """Send a keepalive signal to prevent idle timeouts.

        The Rust ``ClientRequest::KeepAlive`` is a unit variant — it
        carries no parameters.
        """
        request = KeepAliveRequest(params=KeepAliveRequest.KeepAliveRequestParams())
        await self._notify(request)

    # NOTE: `respond_to_hook` / `_respond_to_mcp_route` were the
    # async-client-request variants of hook + MCP-route replies. They
    # are now dead — `hook/callback` and `mcp/routeMessage` responses
    # ride the synchronous JSON-RPC reply path through
    # `_handle_server_request` below.

    # ── Session management ───────────────────────────────────────────

    async def list_sessions(self) -> SessionListResult:
        """List saved sessions (typed response).

        The Rust ``ClientRequest::SessionList`` is a unit variant — it
        accepts no filtering parameters.
        """
        request = SessionListRequest(
            params=SessionListRequest.SessionListRequestParams()
        )
        raw = await self._send_and_await_response(request)
        return SessionListResult.model_validate(raw)

    async def read_session(self, session_id: str) -> SessionReadResult:
        """Read a session's items by ID without resuming (typed response)."""
        request = SessionReadRequest(
            params=SessionReadRequest.SessionReadRequestParams(
                target=SessionTarget(session_id=session_id)
            )
        )
        raw = await self._send_and_await_response(request)
        return SessionReadResult.model_validate(raw)

    async def close_session(self, session_id: str | None = None) -> None:
        """Close a live session while preserving its persisted transcript."""
        target_session_id = session_id or self._session_id
        if target_session_id is None:
            raise RuntimeError("no session id to close")
        request = SessionCloseRequest(
            params=SessionCloseRequest.SessionCloseRequestParams(
                target=self._close_target(target_session_id)
            )
        )
        await self._send_and_await_response(request)
        if target_session_id == self._session_id:
            self._session_id = None

    async def delete_session(self, session_id: str) -> None:
        """Delete durable session storage. The session must not be live."""
        request = SessionDeleteRequest(
            params=SessionDeleteRequest.SessionDeleteRequestParams(
                target=SessionTarget(session_id=session_id)
            )
        )
        await self._send_and_await_response(request)

    async def resume(self, session_id: str) -> AsyncIterator[ServerNotification]:
        """Resume an existing session by ID and yield events."""
        request = SessionResumeRequest(
            params=SessionResumeRequest.SessionResumeRequestParams(
                target=SessionTarget(session_id=session_id),
            )
        )
        result = SessionResumeResult.model_validate(await self._request(request))
        self._session_id = result.session.session_id
        async for event in self.events():
            yield event

    # ── Config ───────────────────────────────────────────────────────

    async def read_config(self) -> ConfigReadResult:
        """Read the merged effective configuration (typed response)."""
        request = ConfigReadRequest(
            params=ConfigReadRequest.ConfigReadRequestParams(
                target={"session": self._session_target()}
            )
        )
        raw = await self._send_and_await_response(request)
        return ConfigReadResult.model_validate(raw)

    async def write_config(
        self, key: str, value: Any, *, scope: str | None = None
    ) -> None:
        """Write a single configuration value."""
        request = ConfigWriteRequest(
            params=ConfigWriteRequest.ConfigWriteRequestParams(
                key=key, value=value, target=self._config_write_target(scope)
            )
        )
        await self._notify(request)

    async def apply_config_flags(self, settings: dict[str, Any]) -> None:
        """Apply runtime feature-flag settings."""
        request = ConfigApplyFlagsRequest(
            params=ConfigApplyFlagsRequest.ConfigApplyFlagsRequestParams(
                target=self._session_target(), settings=settings
            )
        )
        await self._notify(request)

    # ── MCP / plugins / context introspection ───────────────────────

    async def mcp_status(self) -> McpStatusResult:
        """Query the connection status of every MCP server (typed response)."""
        request = McpStatusRequest(
            params=McpStatusRequest.McpStatusRequestParams(
                **self._session_target().model_dump()
            )
        )
        raw = await self._send_and_await_response(request)
        return McpStatusResult.model_validate(raw)

    async def mcp_set_servers(
        self, servers: dict[str, McpServerConfig]
    ) -> McpSetServersResult:
        """Hot-reload the MCP server roster (typed request + response).

        `servers` is keyed by server name; each value is one of
        `StdioMcpServerConfig` / `SseMcpServerConfig` /
        `HttpMcpServerConfig` (the `McpServerConfig` union)."""
        wire_servers = {
            name: cfg.model_dump(mode="json", by_alias=True)
            for name, cfg in servers.items()
        }
        request = McpSetServersRequest(
            params=McpSetServersRequest.McpSetServersRequestParams(
                target=self._session_target(), servers=wire_servers
            )
        )
        raw = await self._send_and_await_response(request)
        return McpSetServersResult.model_validate(raw)

    async def mcp_reconnect(self, server_name: str) -> None:
        """Force-reconnect a single MCP server.

        Rust replies with `null` on success — no typed body. Errors
        surface as `ProcessError` via the JSON-RPC error frame.
        """
        request = McpReconnectRequest(
            params=McpReconnectRequest.McpReconnectRequestParams(
                target=self._session_target(), server_name=server_name
            )
        )
        await self._send_and_await_response(request)

    async def mcp_toggle(self, server_name: str, enabled: bool) -> None:
        """Enable or disable a single MCP server without reconnecting the others.

        Rust replies with `null` on success — no typed body.
        """
        request = McpToggleRequest(
            params=McpToggleRequest.McpToggleRequestParams(
                target=self._session_target(),
                server_name=server_name,
                enabled=enabled,
            )
        )
        await self._send_and_await_response(request)

    async def plugin_reload(self) -> PluginReloadResult:
        """Reload plugin definitions from disk (typed response)."""
        request = PluginReloadRequest(
            params=PluginReloadRequest.PluginReloadRequestParams(
                **self._session_target().model_dump()
            )
        )
        raw = await self._send_and_await_response(request)
        return PluginReloadResult.model_validate(raw)

    async def context_usage(self) -> ContextUsageResult:
        """Return the current context-window breakdown (typed response)."""
        request = ContextUsageRequest(
            params=ContextUsageRequest.ContextUsageRequestParams(
                **self._session_target().model_dump()
            )
        )
        raw = await self._send_and_await_response(request)
        return ContextUsageResult.model_validate(raw)

    # ── Hook handler registration ──────────────────────────────────

    def on_hook(self, callback_id: str, handler: HookHandler) -> None:
        """Register a hook callback handler.

        When ``hook/callback`` arrives with this ``callback_id``, the
        handler is invoked and the result is sent back automatically.
        """
        self._hook_handlers[callback_id] = handler

    # ── Convenience helpers ──────────────────────────────────────

    async def stream_text(self) -> AsyncIterator[str]:
        """Yield only text deltas from the current turn.

        Pattern-matches on the typed `ServerNotificationAgentMessageDelta`
        variant — Pydantic dispatches via the `method` discriminator so
        the matched `event.params` is the typed `ContentDeltaParams`.
        """
        from coco_sdk.generated.protocol import ServerNotificationAgentMessageDelta

        async for event in self.events():
            if isinstance(event, ServerNotificationAgentMessageDelta):
                yield event.params.delta

    async def wait_for_turn_ended(self) -> TurnEndedParams | None:
        """Consume all events and return the terminal `TurnEnded` params.

        Inspect ``result.outcome`` (a tagged union discriminated by
        ``kind``) to determine why the cycle ended:
        ``completed`` / ``failed`` / ``interrupted`` / ``max_turns_reached``
        / ``budget_exhausted``. ``completed.stop_reason`` is the only
        field that carries the model's terminal stop_reason — the other
        variants self-describe through their variant name.
        """
        async for event in self.events():
            if isinstance(event, ServerNotificationTurnEnded):
                return event.params
        return None

    async def get_final_text(self) -> str:
        """Consume all events and return the accumulated assistant text."""
        from coco_sdk.generated.protocol import ServerNotificationAgentMessageDelta

        parts: list[str] = []
        async for event in self.events():
            if isinstance(event, ServerNotificationAgentMessageDelta):
                parts.append(event.params.delta)
        return "".join(parts)

    async def close(self) -> None:
        """Close the session and the underlying transport."""
        if self._router is not None:
            await self._router.close()
            self._router = None
        await self._transport.close()
        self._started = False

    # ── Internal helpers ─────────────────────────────────────────────

    async def _send_and_await_response(self, request: Any) -> dict[str, Any]:
        return await self._request(request)

    def _require_router(self) -> MessageRouter:
        if self._router is None:
            self._router = MessageRouter(
                self._transport,
                server_request_handler=self._handle_server_request,
            )
            self._router.start()
        return self._router

    def _session_target(self) -> SessionTarget:
        if self._session_id is None:
            raise RuntimeError("no active session target")
        return SessionTarget(session_id=self._session_id)

    def _close_target(self, session_id: str) -> SessionTarget:
        return SessionTarget(session_id=session_id)

    def _config_write_target(self, scope: str | None) -> Any:
        if scope is None or scope == "user":
            return "user"
        if scope in {"project", "local"}:
            return {scope: self._session_target()}
        raise ValueError("config scope must be one of: user, project, local")

    async def _request(self, request: Any) -> dict[str, Any]:
        return await self._require_router().request(request)

    async def _notify(self, request: Any) -> None:
        await self._require_router().notify(request)

    async def _handle_server_request(self, line_data: dict[str, Any]) -> bool:
        method = line_data.get("method", "")
        request_id = line_data.get("id")
        params = line_data.get("params", {})
        router = self._require_router()

        if method == ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL:
            if self._can_use_tool is None:
                # No permission callback configured: reply with a JSON-RPC
                # error, NOT a `deny`. Approvals are broadcast to all Full
                # connections and the first VALID reply wins — a deny here
                # would consume the broadcast and cancel a human peer's
                # prompt in multi-client sessions. An error reply only
                # withdraws this client. Single-client headless keeps the
                # same observable outcome: sole recipient errors → the
                # server cancels the request → the tool is denied.
                await router.respond_error(
                    request_id,
                    code=-32601,
                    message="no approval handler configured",
                )
                return True
            decision = await self._can_use_tool(
                params.get("tool_name", ""),
                params.get("input", {}),
            )
            await router.respond(
                request_id,
                {
                    "request_id": params.get("request_id", ""),
                    "decision": decision.value
                    if hasattr(decision, "value")
                    else decision,
                },
            )
            return True

        if method == ServerRequestMethod.HOOK_CALLBACK:
            cb_id = params.get("callback_id", "")
            handler = self._hook_handlers.get(cb_id)
            if handler is None:
                return False
            try:
                output = await handler(
                    cb_id,
                    params.get("event_type", method),
                    params.get("input", {}),
                )
            except Exception as exc:
                # Handler crashed: emit an empty `HookCallbackOutput` so the
                # agent doesn't deadlock. An empty output is TS-canonical
                # for "no opinion, continue normally" — the previous
                # default of `{"behavior": "allow"}` was a fail-open
                # decision baked into the deprecated weak-typed shape.
                logger.warning("Hook handler %s raised: %s", cb_id, exc)
                output = {}
            output = self._normalize_hook_output(output, callback_id=cb_id)
            # Reply body is the bare `HookCallbackResult` shape: `{output}`.
            # Correlation is the outer JSON-RPC `id`; there is
            # no inner echo field — `callback_id` would be diagnostic-only
            # and Rust ignores it.
            await router.respond(request_id, {"output": output})
            return True

        if method == ServerRequestMethod.MCP_ROUTE_MESSAGE:
            response = await self._handle_mcp_message(
                params.get("server_name", ""),
                params.get("message", {}),
            )
            if response is None:
                return False
            # TS-canonical reply body: `{message}` — outer JSON-RPC
            # id correlates; no echo in the body.
            await router.respond(request_id, {"message": response})
            return True

        return False

    def _normalize_hook_output(
        self,
        output: Any,
        *,
        callback_id: str | None = None,
    ) -> dict[str, Any]:
        """Coerce a hook handler's return value into the canonical
        ``HookCallbackOutput`` wire shape (camelCase).

        ``None`` and unrecognized return types become ``{}`` —
        TS-canonical "no opinion, continue normally". The previous
        contract required ``{"behavior": ...}`` and silently fail-open'd
        when missing; we now ship the empty dict so a handler that
        forgets to return a decision doesn't accidentally grant
        permissions.

        Pydantic models (notably ``HookCallbackOutput`` itself) are dumped
        with ``by_alias=True`` so camelCase field names land on the
        wire — TS expects ``hookSpecificOutput``, ``stopReason``,
        ``additionalContext`` etc.
        """
        if output is None:
            return {}
        if isinstance(output, BaseModel):
            return output.model_dump(mode="json", exclude_none=True, by_alias=True)
        if not isinstance(output, dict):
            if callback_id is not None:
                logger.warning(
                    "Hook handler %s returned non-dict %s; sending empty output",
                    callback_id,
                    type(output).__name__,
                )
            return {}
        return output

    async def _handle_mcp_message(
        self,
        server_name: str,
        message: dict[str, Any],
    ) -> dict[str, Any] | None:
        tool_def = self._tool_registry.get(server_name)
        if tool_def is None:
            return None

        msg_id = message.get("id")
        method = message.get("method")
        if method == "initialize":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": message.get("params", {}).get(
                        "protocolVersion", "2024-11-05"
                    ),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": server_name, "version": "0.1.0"},
                },
            }
        if method == "notifications/initialized":
            return {"jsonrpc": "2.0", "id": msg_id, "result": {}}
        if method == "tools/list":
            return {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"tools": [tool_def.to_mcp_tool_def()]},
            }
        if method == "tools/call":
            mcp_params = message.get("params", {})
            try:
                result = await tool_def.invoke(mcp_params.get("arguments", {}))
                result_text = result if isinstance(result, str) else json.dumps(result)
                return {
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {
                        "content": [{"type": "text", "text": result_text}],
                    },
                }
            except Exception as exc:
                return {
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "error": {"code": -32603, "message": str(exc)},
                }
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"method not found: {method}"},
        }


def _ms_to_seconds(value: int | None) -> int | None:
    """Convert a millisecond timeout to the integer-seconds wire format."""
    if value is None:
        return None
    return max(1, value // 1000)
