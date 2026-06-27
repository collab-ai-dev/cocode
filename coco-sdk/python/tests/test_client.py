"""Tests for CocoClient with a mock transport.

All wire-method comparisons go through the typed `NotificationMethod` /
`ClientRequestMethod` / `ServerRequestMethod` enums generated from the
coco-rs schema — no raw wire strings.
"""

import asyncio
import json
from typing import Any, AsyncIterator

import pytest

from coco_sdk._internal.transport import Transport
from coco_sdk.client import CocoClient
from coco_sdk.errors import ProcessError
from coco_sdk.generated.protocol import (
    ClientRequestMethod,
    NotificationMethod,
    SdkHookOutput,
    ServerNotification,
    ServerRequestMethod,
)


class MockTransport(Transport):
    """In-memory transport for testing."""

    def __init__(self, responses: list[dict[str, Any]] | None = None):
        self.sent_lines: list[str] = []
        self._responses = responses or []
        self._started = False
        self._closed = False

    async def start(self) -> None:
        self._started = True

    async def send_line(self, line: str) -> None:
        self.sent_lines.append(line)

    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        for resp in self._responses:
            yield resp

    async def read_events(self) -> AsyncIterator[ServerNotification]:
        for resp in self._responses:
            yield ServerNotification.model_validate(resp)

    async def close(self) -> None:
        self._closed = True


def _notif(method: NotificationMethod, **params: Any) -> dict[str, Any]:
    return {"method": method.value, "params": params}


_DEFAULT_USAGE: dict[str, Any] = {
    "input_tokens": {"total": 1},
    "output_tokens": {"total": 1},
}


def _turn_ended_completed(
    turn_id: str = "t1",
    usage: dict[str, Any] | None = None,
    stop_reason: str = "end_turn",
) -> dict[str, Any]:
    """Build a `turn/ended` notification with `outcome.kind=completed`."""
    return _notif(
        NotificationMethod.TURN_ENDED,
        turn_id=turn_id,
        usage=usage or _DEFAULT_USAGE,
        outcome={"kind": "completed", "data": {"stop_reason": stop_reason}},
    )


def _session_started(session_id: str = "s1", **overrides: Any) -> dict[str, Any]:
    """Build a `session/started` notification whose params satisfy the
    typed `SessionStartedParams` schema (cwd/model/permission_mode/
    version/protocol_version are all required wire fields)."""
    params = {
        "session_id": session_id,
        "protocol_version": "1",
        "cwd": "/tmp",
        "model": "test-model",
        "permission_mode": "default",
        "version": "0.0.0-test",
    }
    params.update(overrides)
    return {"method": NotificationMethod.SESSION_STARTED.value, "params": params}


def _server_request(method: ServerRequestMethod, **params: Any) -> dict[str, Any]:
    request_id = params.pop("_wire_request_id", f"srv_{method.value}")
    return {
        "type": "request",
        "request_id": request_id,
        "method": method.value,
        "params": params,
    }


def _sent_methods(transport: MockTransport) -> list[str]:
    return [json.loads(line)["method"] for line in transport.sent_lines]


@pytest.mark.asyncio
async def test_client_sends_initialize_session_start_turn_start() -> None:
    transport = MockTransport(responses=[
        _response(1, {}),
        _response(2, {"session_id": "s1"}),
        _response(3, {"turn_id": "t1"}),
        _session_started(session_id="s1"),
        _notif(
            NotificationMethod.TURN_ENDED,
            turn_id="t1",
            usage={"input_tokens": {"total": 10}, "output_tokens": {"total": 5}},
        outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}},
        ),
    ])

    client = CocoClient(prompt="hello", transport=transport)
    await client.start()
    events = [event async for event in client.events()]
    await client.close()

    # Three wire requests: initialize → session/start → turn/start.
    # session/start does NOT auto-run; the prompt goes into turn/start.
    methods = _sent_methods(transport)
    assert methods == [
        ClientRequestMethod.INITIALIZE.value,
        ClientRequestMethod.SESSION_START.value,
        ClientRequestMethod.TURN_START.value,
    ]
    turn_start = json.loads(transport.sent_lines[2])
    assert turn_start["params"]["prompt"] == "hello"

    assert len(events) == 2
    assert events[0].method == NotificationMethod.SESSION_STARTED
    assert events[1].method == NotificationMethod.TURN_ENDED


@pytest.mark.asyncio
async def test_client_send_follow_up() -> None:
    transport = MockTransport(responses=[
        _response(1, {"turn_id": "t2"}),
        _notif(
            NotificationMethod.TURN_ENDED,
            turn_id="t2",
            usage={"input_tokens": {"total": 5}, "output_tokens": {"total": 3}},
        outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}},
        ),
    ])

    client = CocoClient(prompt="init", transport=transport)
    client._started = True
    events = [event async for event in client.send("follow up")]

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.TURN_START
    assert sent["params"]["prompt"] == "follow up"
    assert len(events) == 1


@pytest.mark.asyncio
async def test_start_turn_retries_transient_turn_busy(monkeypatch) -> None:
    """A follow-up turn briefly rejected with "a turn is already running"
    (the post-TurnEnded finalization window) is retried, not surfaced."""
    client = CocoClient(prompt="init", transport=MockTransport())
    attempts = {"n": 0}

    async def flaky_request(_request: Any) -> dict[str, Any]:
        attempts["n"] += 1
        if attempts["n"] < 3:
            raise ProcessError(
                "coco rejected request 4: a turn is already running; "
                "call turn/interrupt first"
            )
        return {}

    monkeypatch.setattr(client, "_request", flaky_request)
    await client._start_turn_with_retry(object())
    assert attempts["n"] == 3, "should retry the transient busy rejection"


@pytest.mark.asyncio
async def test_start_turn_does_not_retry_other_process_errors(monkeypatch) -> None:
    """A non-busy ProcessError surfaces immediately (no retry loop)."""
    client = CocoClient(prompt="init", transport=MockTransport())
    attempts = {"n": 0}

    async def failing_request(_request: Any) -> dict[str, Any]:
        attempts["n"] += 1
        raise ProcessError("no active session; call session/start first")

    monkeypatch.setattr(client, "_request", failing_request)
    with pytest.raises(ProcessError):
        await client._start_turn_with_retry(object())
    assert attempts["n"] == 1, "must not retry a non-transient error"


@pytest.mark.asyncio
async def test_client_auto_approval() -> None:
    transport = MockTransport(responses=[
        _server_request(
            ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL,
            request_id="r1",
            tool_name="Read",
            tool_use_id="tu1",
            input={},
        ),
        _notif(
            NotificationMethod.TURN_ENDED,
            turn_id="t1",
            usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
        outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}},
        ),
    ])

    from coco_sdk.generated.protocol import ApprovalDecision

    async def auto_approve(tool_name: str, input: dict) -> ApprovalDecision:
        return ApprovalDecision.allow

    client = CocoClient(prompt="test", transport=transport, can_use_tool=auto_approve)
    client._started = True
    events = [event async for event in client.events()]

    assert len(events) == 1
    assert events[0].method == NotificationMethod.TURN_ENDED

    approval_sent = json.loads(transport.sent_lines[0])
    assert approval_sent["type"] == "response"
    assert approval_sent["result"]["decision"] == "allow"
    assert approval_sent["result"]["request_id"] == "r1"


@pytest.mark.asyncio
async def test_client_denies_approval_without_callback() -> None:
    # With no `can_use_tool` callback the client must still RESPOND to an
    # approval request (denying it), or the server hangs waiting for a reply.
    # Regression: AskUserQuestion now always asks, so a headless session that
    # didn't set a callback used to hang on it.
    transport = MockTransport(responses=[
        _server_request(
            ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL,
            request_id="r1",
            tool_name="AskUserQuestion",
            tool_use_id="tu1",
            input={},
        ),
        _notif(
            NotificationMethod.TURN_ENDED,
            turn_id="t1",
            usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
            outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}},
        ),
    ])

    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    events = [event async for event in client.events()]

    assert len(events) == 1
    assert events[0].method == NotificationMethod.TURN_ENDED

    approval_sent = json.loads(transport.sent_lines[0])
    assert approval_sent["type"] == "response"
    assert approval_sent["result"]["decision"] == "deny"
    assert approval_sent["result"]["request_id"] == "r1"


@pytest.mark.asyncio
async def test_client_interrupt() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.interrupt()

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.TURN_INTERRUPT


@pytest.mark.asyncio
async def test_client_set_models_main_string() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.set_models_main("anthropic/claude-opus-4-7")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_SET_MODEL
    assert sent["params"]["model"] == "anthropic/claude-opus-4-7"


@pytest.mark.asyncio
async def test_client_set_models_main_spec() -> None:
    from coco_sdk.types import DEEPSEEK

    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.set_models_main(DEEPSEEK.flash_openai)

    sent = json.loads(transport.sent_lines[0])
    assert sent["params"]["model"] == "deepseek-openai/deepseek-v4-flash"


@pytest.mark.asyncio
async def test_client_set_thinking() -> None:
    from coco_sdk.generated.protocol import ReasoningEffort
    from coco_sdk.types import thinking

    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.set_thinking(thinking(ReasoningEffort.high, budget_tokens=8000))

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_SET_THINKING
    assert sent["params"]["thinking_level"]["effort"] == "high"
    assert sent["params"]["thinking_level"]["budget_tokens"] == 8000


@pytest.mark.asyncio
async def test_client_rewind_files() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.rewind_files("msg_42", dry_run=True)

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_REWIND_FILES
    assert sent["params"]["user_message_id"] == "msg_42"
    assert sent["params"]["dry_run"] is True


@pytest.mark.asyncio
async def test_client_sdk_hook_output_serializes_camelcase_wire_shape() -> None:
    """`SdkHookOutput` is the TS-canonical wire shape; the typed Pydantic
    model dumps camelCase aliases so the wire matches TS exactly."""
    output = SdkHookOutput.model_validate(
        {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "no rm -rf",
                "updatedInput": {"command": "ls"},
            }
        }
    )
    wire = output.model_dump(mode="json", exclude_none=True, by_alias=True)
    assert wire["hookSpecificOutput"]["hookEventName"] == "PreToolUse"
    assert wire["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert wire["hookSpecificOutput"]["updatedInput"] == {"command": "ls"}


@pytest.mark.asyncio
async def test_client_respond_to_question_uses_answer_field() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.respond_to_question("r1", "yes")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.INPUT_RESOLVE_USER_INPUT
    assert sent["params"]["answer"] == "yes"
    assert sent["params"]["request_id"] == "r1"


@pytest.mark.asyncio
async def test_client_cancel_request() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.cancel_request("req_42", reason="user_aborted")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_CANCEL_REQUEST
    assert sent["params"]["request_id"] == "req_42"
    assert sent["params"]["reason"] == "user_aborted"


def _response(request_id: int, result: Any) -> dict[str, Any]:
    """Matches the real wire shape coco-rs sends back."""
    return {"type": "response", "request_id": request_id, "result": result}


@pytest.mark.asyncio
async def test_client_mcp_status_returns_typed_response() -> None:
    """`mcp_status()` returns a typed `McpStatusResult`, not a raw dict."""
    from coco_sdk import McpStatusResult

    transport = MockTransport(responses=[_response(1, {"mcpServers": []})])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.mcp_status()

    assert isinstance(result, McpStatusResult)
    assert result.mcp_servers == []
    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.MCP_STATUS


@pytest.mark.asyncio
async def test_client_context_usage_returns_typed_response() -> None:
    """`context_usage()` returns a typed `ContextUsageResult`."""
    from coco_sdk import ContextUsageResult

    transport = MockTransport(responses=[
        _response(1, {
            "total_tokens": 1000, "max_tokens": 100000,
            "raw_max_tokens": 100000, "percentage": 1.0,
            "model": "claude-opus", "categories": [],
            "is_auto_compact_enabled": False,
        }),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.context_usage()

    assert isinstance(result, ContextUsageResult)
    assert result.total_tokens == 1000
    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTEXT_USAGE


@pytest.mark.asyncio
async def test_client_mcp_toggle() -> None:
    transport = MockTransport(responses=[_response(1, {})])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.mcp_toggle("filesystem", enabled=False)

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.MCP_TOGGLE
    assert sent["params"]["server_name"] == "filesystem"
    assert sent["params"]["enabled"] is False


@pytest.mark.asyncio
async def test_client_resolve_elicitation() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.resolve_elicitation(
        request_id="elic_1",
        mcp_server_name="github",
        approved=True,
        values={"token": "secret"},
    )

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.ELICITATION_RESOLVE
    assert sent["params"]["mcp_server_name"] == "github"
    assert sent["params"]["values"] == {"token": "secret"}


@pytest.mark.asyncio
async def test_client_initialize_includes_hooks_map() -> None:
    from coco_sdk import hook

    @hook(event="PreToolUse", matcher="Bash", timeout_ms=5000)
    async def block_rm(callback_id, event_type, input):  # pragma: no cover - decorator only
        return {"behavior": "allow"}

    transport = MockTransport(responses=[
        _response(1, {}),
        _response(2, {"session_id": "s1"}),
        _response(3, {"turn_id": "t1"}),
    ])
    client = CocoClient(prompt="test", transport=transport, hooks=[block_rm])
    await client.start()

    init = json.loads(transport.sent_lines[0])
    assert init["method"] == ClientRequestMethod.INITIALIZE
    hooks = init["params"]["hooks"]
    assert "PreToolUse" in hooks
    assert hooks["PreToolUse"][0]["hook_callback_ids"] == [block_rm.callback_id]
    assert hooks["PreToolUse"][0]["matcher"] == "Bash"
    assert hooks["PreToolUse"][0]["timeout"] == 5  # ms → seconds

    # Handler is auto-registered for the callback_id
    assert block_rm.callback_id in client._hook_handlers


@pytest.mark.asyncio
async def test_client_waits_for_sdk_mcp_servers_before_session_start() -> None:
    from coco_sdk import tool

    @tool(name="ping", description="Ping")
    async def ping() -> str:  # pragma: no cover - decorator only
        return "pong"

    transport = MockTransport(responses=[
        _response(1, {}),
        _response(2, {"mcpServers": [{"name": ping.server_name, "status": "pending"}]}),
        _response(3, {"mcpServers": [{"name": ping.server_name, "status": "connected"}]}),
        _response(4, {"session_id": "s1"}),
        _response(5, {"turn_id": "t1"}),
    ])
    client = CocoClient(prompt="test", transport=transport, tools=[ping])
    await client.start()

    assert _sent_methods(transport) == [
        ClientRequestMethod.INITIALIZE.value,
        ClientRequestMethod.MCP_STATUS.value,
        ClientRequestMethod.MCP_STATUS.value,
        ClientRequestMethod.SESSION_START.value,
        ClientRequestMethod.TURN_START.value,
    ]


@pytest.mark.asyncio
async def test_client_context_manager() -> None:
    transport = MockTransport(responses=[
        _response(1, {}),
        _response(2, {"session_id": "s1"}),
        _response(3, {"turn_id": "t1"}),
        _session_started(session_id="s1"),
        _notif(
            NotificationMethod.TURN_ENDED,
            turn_id="t1",
            usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
        outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}},
        ),
    ])

    async with CocoClient(prompt="hi", transport=transport) as client:
        _ = [event async for event in client.events()]

    assert transport._closed


# ── Wire-format coverage for the rest of the control surface ─────────


@pytest.mark.asyncio
async def test_client_set_permission_mode_string_coerced() -> None:
    from coco_sdk.generated.protocol import PermissionMode
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.set_permission_mode("auto")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_SET_PERMISSION_MODE
    assert sent["params"]["mode"] == PermissionMode.auto.value


@pytest.mark.asyncio
async def test_client_set_permission_mode_enum() -> None:
    from coco_sdk.generated.protocol import PermissionMode
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.set_permission_mode(PermissionMode.plan)

    sent = json.loads(transport.sent_lines[0])
    assert sent["params"]["mode"] == "plan"


@pytest.mark.asyncio
async def test_client_stop_task() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.stop_task("task-42")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_STOP_TASK
    assert sent["params"]["task_id"] == "task-42"


@pytest.mark.asyncio
async def test_client_update_env() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.update_env({"FOO": "bar", "BAZ": "qux"})

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_UPDATE_ENV
    assert sent["params"]["env"] == {"FOO": "bar", "BAZ": "qux"}


@pytest.mark.asyncio
async def test_client_keep_alive_with_timestamp() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.keep_alive(timestamp=1700000000)

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_KEEP_ALIVE
    assert sent["params"]["timestamp"] == 1700000000


@pytest.mark.asyncio
async def test_client_keep_alive_without_timestamp() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.keep_alive()

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONTROL_KEEP_ALIVE
    # Optional field is omitted from the wire when not provided.
    assert "timestamp" not in sent.get("params", {})


@pytest.mark.asyncio
async def test_client_list_sessions_returns_typed_response() -> None:
    """`list_sessions()` returns a typed `SessionListResult`."""
    from coco_sdk import SessionListResult

    transport = MockTransport(responses=[
        _response(1, {"sessions": [
            {"session_id": "s1", "model": "m", "cwd": "/", "created_at": "2025-01-01T00:00:00Z"},
            {"session_id": "s2", "model": "m", "cwd": "/", "created_at": "2025-01-01T00:00:00Z"},
        ]})
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.list_sessions(limit=10)

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.SESSION_LIST
    assert sent["params"]["limit"] == 10
    assert isinstance(result, SessionListResult)
    assert result.sessions[0].session_id == "s1"


@pytest.mark.asyncio
async def test_client_read_session_returns_typed_response() -> None:
    """`read_session()` returns a typed `SessionReadResult`."""
    from coco_sdk import SessionReadResult

    transport = MockTransport(responses=[_response(1, {
        "session": {"session_id": "s1", "model": "m", "cwd": "/", "created_at": "2025-01-01T00:00:00Z"},
        "messages": [],
    })])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.read_session("s1")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.SESSION_READ
    assert sent["params"]["session_id"] == "s1"
    assert isinstance(result, SessionReadResult)
    assert result.session.session_id == "s1"


@pytest.mark.asyncio
async def test_client_archive_session() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.archive_session("s1")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.SESSION_ARCHIVE
    assert sent["params"]["session_id"] == "s1"


@pytest.mark.asyncio
async def test_client_read_config_returns_typed_response() -> None:
    """`read_config()` returns a typed `ConfigReadResult`."""
    from coco_sdk import ConfigReadResult

    transport = MockTransport(responses=[
        _response(1, {"config": {"theme": "dark"}, "sources": {}})
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.read_config()

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONFIG_READ
    assert isinstance(result, ConfigReadResult)
    assert result.config == {"theme": "dark"}


@pytest.mark.asyncio
async def test_client_write_config_with_scope() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.write_config("theme", "light", scope="user")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONFIG_VALUE_WRITE
    assert sent["params"]["key"] == "theme"
    assert sent["params"]["value"] == "light"
    assert sent["params"]["scope"] == "user"


@pytest.mark.asyncio
async def test_client_apply_config_flags() -> None:
    transport = MockTransport()
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.apply_config_flags({"feature.x": True, "feature.y": False})

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.CONFIG_APPLY_FLAGS
    assert sent["params"]["settings"]["feature.x"] is True


@pytest.mark.asyncio
async def test_client_mcp_set_servers() -> None:
    """`mcp_set_servers()` takes typed `McpServerConfig` values and
    returns a typed `McpSetServersResult`."""
    from coco_sdk import McpSetServersResult
    from coco_sdk.generated.protocol import StdioMcpServerConfig

    transport = MockTransport(responses=[
        _response(1, {"added": ["fs"], "removed": [], "errors": {}})
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.mcp_set_servers(
        {"fs": StdioMcpServerConfig(command="fs-server")}
    )

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.MCP_SET_SERVERS
    assert sent["params"]["servers"]["fs"]["command"] == "fs-server"
    assert sent["params"]["servers"]["fs"]["type"] == "stdio"
    assert isinstance(result, McpSetServersResult)
    assert result.added == ["fs"]


@pytest.mark.asyncio
async def test_client_mcp_reconnect() -> None:
    transport = MockTransport(responses=[_response(1, None)])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    await client.mcp_reconnect("fs")

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.MCP_RECONNECT
    assert sent["params"]["server_name"] == "fs"


@pytest.mark.asyncio
async def test_client_plugin_reload() -> None:
    """`plugin_reload()` returns a typed `PluginReloadResult`."""
    from coco_sdk import PluginReloadResult

    transport = MockTransport(responses=[_response(1, {
        "plugins": ["a", "b", "c"], "commands": [], "agents": [], "error_count": 0,
    })])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.plugin_reload()

    sent = json.loads(transport.sent_lines[0])
    assert sent["method"] == ClientRequestMethod.PLUGIN_RELOAD
    assert isinstance(result, PluginReloadResult)
    assert result.plugins == ["a", "b", "c"]


# ── Server-driven flows: error frames, request matching, hook dispatch ─


@pytest.mark.asyncio
async def test_send_and_await_raises_on_error_frame() -> None:
    """Error response with matching request_id surfaces as ProcessError."""
    from coco_sdk.errors import ProcessError

    transport = MockTransport(responses=[
        {"type": "error", "request_id": 1, "code": -32601, "message": "method not found"}
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    with pytest.raises(ProcessError) as exc_info:
        await client.mcp_status()
    assert "method not found" in str(exc_info.value)


@pytest.mark.asyncio
async def test_send_and_await_skips_other_request_ids() -> None:
    """Responses with non-matching ids are skipped until ours arrives."""
    transport = MockTransport(responses=[
        # Stale response for an earlier request — should be ignored.
        _response(99, {"stale": True}),
        # Notifications interleaved on the wire — also ignored by req/resp matcher.
        _notif(NotificationMethod.SESSION_STATE_CHANGED, state="running"),
        # Our actual response — must be a valid `mcp/status` payload now
        # that the client decodes into a typed `McpStatusResult`.
        _response(1, {"mcpServers": []}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    result = await client.mcp_status()

    assert result.mcp_servers == []


@pytest.mark.asyncio
async def test_concurrent_control_query_and_events_share_reader() -> None:
    """A control request and event stream can both complete with one stdout reader."""
    transport = MockTransport(responses=[
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
        _response(1, {"mcpServers": []}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    status_task = asyncio.create_task(client.mcp_status())
    events_task = asyncio.create_task(client.wait_for_turn_ended())

    status = await status_task
    assert status.mcp_servers == []
    completed = await events_task
    assert completed is not None
    assert completed.turn_id == "t1"


@pytest.mark.asyncio
async def test_stdout_close_fails_pending_request() -> None:
    transport = MockTransport(responses=[])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    from coco_sdk.errors import TransportClosedError

    with pytest.raises(TransportClosedError):
        await client.mcp_status()


@pytest.mark.asyncio
async def test_fire_and_forget_response_is_discarded() -> None:
    transport = MockTransport(responses=[
        _response(1, {}),
        _response(2, {"mcpServers": []}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    await client.interrupt()
    status = await client.mcp_status()
    assert status.mcp_servers == []


@pytest.mark.asyncio
async def test_events_loop_drops_error_frames() -> None:
    """Error frames during events() get logged-and-dropped, never raised."""
    transport = MockTransport(responses=[
        {"type": "error", "request_id": 99, "code": -32600, "message": "bad request"},
        {"type": "notification", **_notif(NotificationMethod.TURN_ENDED,
                                          turn_id="t1",
                                          usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}})},
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    events = [e async for e in client.events()]
    assert len(events) == 1
    assert events[0].method == NotificationMethod.TURN_ENDED


@pytest.mark.asyncio
async def test_events_loop_drops_response_frames() -> None:
    """Response frames during events() are silently dropped (handled by req/resp matcher)."""
    transport = MockTransport(responses=[
        {"type": "response", "request_id": 1, "result": {"orphan": True}},
        {"type": "notification", **_notif(NotificationMethod.TURN_ENDED,
                                          turn_id="t1",
                                          usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}})},
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    events = [e async for e in client.events()]
    assert len(events) == 1
    assert events[0].method == NotificationMethod.TURN_ENDED


@pytest.mark.asyncio
async def test_hook_callback_dispatches_to_registered_handler() -> None:
    """When the server sends hook/callback, the registered handler runs and a response is sent."""
    seen: list[dict[str, Any]] = []

    async def my_handler(callback_id: str, event_type: str, input: dict[str, Any]) -> dict[str, Any]:
        seen.append({"cb": callback_id, "event": event_type, "input": input})
        return {"behavior": "deny", "reason": "test rule"}

    transport = MockTransport(responses=[
        _server_request(
            ServerRequestMethod.HOOK_CALLBACK,
            callback_id="cb_xyz",
            event_type="PreToolUse",
            input={"tool_name": "Bash", "tool_input": {"command": "rm -rf /"}},
        ),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    client.on_hook("cb_xyz", my_handler)

    events = [e async for e in client.events()]

    # Handler ran with the right payload.
    assert len(seen) == 1
    assert seen[0]["cb"] == "cb_xyz"
    assert seen[0]["event"] == "PreToolUse"
    assert seen[0]["input"]["tool_name"] == "Bash"

    # Client responded to the hook invocation with our decision.
    # Reply body is the bare `HookCallbackResult` shape — `{output}`,
    # no `callback_id` echo. Correlation is the outer JSON-RPC
    # request_id. The handler returned a flat-format `{behavior, reason}`
    # which the SDK ships verbatim (legacy shape; the typed
    # SdkHookOutput is preferred but Pydantic models also accept it).
    sent = [json.loads(line) for line in transport.sent_lines]
    hook_responses = [m for m in sent if m.get("type") == "response"]
    assert len(hook_responses) == 1
    assert "callback_id" not in hook_responses[0]["result"]
    # Whatever the handler returned ends up in `output` verbatim.
    assert hook_responses[0]["result"]["output"]["behavior"] == "deny"

    # Subsequent terminator notification still flows through.
    assert any(e.method == NotificationMethod.TURN_ENDED for e in events)


@pytest.mark.asyncio
async def test_hook_callback_handler_exception_falls_back_to_empty_output() -> None:
    """Handler raising → client sends `{}` (TS-canonical "no opinion,
    continue normally") so the agent doesn't deadlock AND doesn't
    silently fail-open. The previous contract returned
    `{"behavior": "allow"}` — a weak-typed shape that always granted
    permission on crash. The TS-canonical empty dict surfaces the
    crash through logs while letting the loop continue.
    """
    async def boom(callback_id: str, event_type: str, input: dict[str, Any]) -> dict[str, Any]:
        raise RuntimeError("handler crashed")

    transport = MockTransport(responses=[
        _server_request(ServerRequestMethod.HOOK_CALLBACK,
                        callback_id="cb_x", event_type="PreToolUse", input={}),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    client.on_hook("cb_x", boom)

    _ = [e async for e in client.events()]

    sent = [json.loads(line) for line in transport.sent_lines]
    responses = [m for m in sent if m.get("type") == "response"]
    assert len(responses) == 1
    assert responses[0]["result"]["output"] == {}


@pytest.mark.asyncio
async def test_hook_callback_unregistered_id_yields_event() -> None:
    """When the callback_id has no handler registered, the notification is yielded so the caller can decide."""
    transport = MockTransport(responses=[
        _server_request(ServerRequestMethod.HOOK_CALLBACK,
                        callback_id="cb_unknown", event_type="PreToolUse", input={}),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True
    # No handler registered — expect the request to be yielded as a parsed notification.

    events = [e async for e in client.events()]
    assert any(event.method == NotificationMethod.TURN_ENDED for event in events)


# ── Convenience helpers ─────────────────────────────────────────────


@pytest.mark.asyncio
async def test_stream_text_yields_only_deltas() -> None:
    """`stream_text` filters everything except agent_message/delta payloads."""
    transport = MockTransport(responses=[
        _notif(NotificationMethod.TURN_STARTED, turn_id="t1", turn_number=1),
        _notif(NotificationMethod.AGENT_MESSAGE_DELTA, item_id="i1", turn_id="t1", delta="Hello "),
        _notif(NotificationMethod.AGENT_MESSAGE_DELTA, item_id="i1", turn_id="t1", delta="world"),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 2}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    chunks = [chunk async for chunk in client.stream_text()]
    assert chunks == ["Hello ", "world"]


@pytest.mark.asyncio
async def test_get_final_text_concatenates_deltas() -> None:
    transport = MockTransport(responses=[
        _notif(NotificationMethod.AGENT_MESSAGE_DELTA, item_id="i1", turn_id="t1", delta="Hello "),
        _notif(NotificationMethod.AGENT_MESSAGE_DELTA, item_id="i1", turn_id="t1", delta="world"),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 2}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    text = await client.get_final_text()
    assert text == "Hello world"


@pytest.mark.asyncio
async def test_wait_for_turn_ended_returns_params() -> None:
    transport = MockTransport(responses=[
        _notif(NotificationMethod.AGENT_MESSAGE_DELTA, item_id="i1", turn_id="t1", delta="ok"),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 4}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    completed = await client.wait_for_turn_ended()
    assert completed is not None
    assert completed.turn_id == "t1"
    assert completed.usage.output_tokens.total == 1


@pytest.mark.asyncio
async def test_resume_emits_session_resume_then_streams() -> None:
    """`resume` sends a session/resume request and yields the resulting events."""
    transport = MockTransport(responses=[
        _response(1, {"session_id": "s_old"}),
        _session_started(session_id="s_old"),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t-resumed", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])
    client = CocoClient(prompt="test", transport=transport)
    client._started = True

    events = [e async for e in client.resume("s_old")]
    sent_methods = _sent_methods(transport)
    assert sent_methods[0] == ClientRequestMethod.SESSION_RESUME
    sent = json.loads(transport.sent_lines[0])
    assert sent["params"]["session_id"] == "s_old"
    assert any(e.method == NotificationMethod.TURN_ENDED for e in events)


# ── Full bidirectional flow: server-initiated approval round-trip ────


@pytest.mark.asyncio
async def test_can_use_tool_deny_flows_through_approve() -> None:
    """When `can_use_tool` denies, the approve() call carries the deny + feedback."""
    from coco_sdk.generated.protocol import ApprovalDecision

    transport = MockTransport(responses=[
        _server_request(
            ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL,
            request_id="approval_42",
            tool_name="Bash",
            tool_use_id="tu_1",
            input={"command": "rm -rf /"},
        ),
        _notif(NotificationMethod.TURN_ENDED,
               turn_id="t1", usage={"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
               outcome={"kind": "completed", "data": {"stop_reason": "end_turn"}}),
    ])

    async def deny_dangerous(tool_name: str, input: dict[str, Any]) -> ApprovalDecision:
        return ApprovalDecision.deny

    client = CocoClient(prompt="test", transport=transport, can_use_tool=deny_dangerous)
    client._started = True
    _ = [e async for e in client.events()]

    sent = [json.loads(line) for line in transport.sent_lines]
    approval_resolves = [m for m in sent if m.get("type") == "response"]
    assert len(approval_resolves) == 1
    assert approval_resolves[0]["result"]["decision"] == "deny"
    assert approval_resolves[0]["result"]["request_id"] == "approval_42"
