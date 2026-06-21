"""Protocol-surface smoke test.

Exercises the API the user code depends on, end-to-end (import →
construct → access). Catches regressions where the codegen pipeline
silently drops a type — the kind of bug where ``protocol.py`` looks
fine on disk but ``coco_sdk.X`` no longer exists or won't construct.

Why this exists: when ``export_schema.rs`` was refactored to use
schemars's transitive `$ref` closure, schemars's habit of *inlining*
some union variants meant `JsonRpcRequest` / `JsonRpcResponse` / etc.
silently disappeared from the bundle and from ``protocol.py``. Pure
schema-equality checks (``generate_schemas.sh --check``) said
"up-to-date" because the bundle was internally consistent. This file
is the safety net for that class of bug.
"""

from __future__ import annotations

import inspect

import pytest
from pydantic import BaseModel


# ── 1. Top-level package surface ───────────────────────────────────


def test_top_level_imports_resolve() -> None:
    """Every name in ``coco_sdk.__all__`` must be importable."""
    import coco_sdk

    missing = [n for n in coco_sdk.__all__ if not hasattr(coco_sdk, n)]
    assert not missing, f"names in __all__ but not on module: {missing}"


def test_top_level_exports_have_no_duplicates() -> None:
    """``__all__`` should be a true set — duplicates leak from regen bugs."""
    import coco_sdk

    seen: set[str] = set()
    dups = [n for n in coco_sdk.__all__ if n in seen or seen.add(n)]
    assert not dups, f"duplicates in __all__: {dups}"


# ── 2. Schema-derived types: every BaseModel must be constructable ──


def test_every_basemodel_resolves_forward_refs() -> None:
    """Every Pydantic model must be fully resolvable.

    A model whose annotations reference an undefined name passes
    ``import`` (because ``from __future__ import annotations`` makes
    them strings) but blows up at first ``model_validate`` /
    ``model_construct`` with ``PydanticUserError: not fully defined``.
    Force ``model_rebuild()`` to surface those at unit-test time.
    """
    import coco_sdk.generated.protocol as proto

    broken: list[tuple[str, str]] = []
    for name in dir(proto):
        cls = getattr(proto, name)
        if (
            inspect.isclass(cls)
            and issubclass(cls, BaseModel)
            and cls is not BaseModel
        ):
            try:
                cls.model_rebuild()
            except Exception as exc:
                broken.append((name, str(exc).splitlines()[0][:120]))
    assert not broken, "models with unresolved annotations:\n" + "\n".join(
        f"  - {n}: {e}" for n, e in broken
    )


# ── 3. Wire envelope — the JSON-RPC types must round-trip ──────────


def test_jsonrpc_request_roundtrips_int_id() -> None:
    from coco_sdk import JsonRpcRequest

    req = JsonRpcRequest(method="session/start", request_id=1, params={"foo": "bar"})
    assert req.method == "session/start"
    assert req.request_id == 1
    assert JsonRpcRequest.model_validate(req.model_dump()).request_id == 1


def test_jsonrpc_request_roundtrips_str_id() -> None:
    from coco_sdk import JsonRpcRequest

    req = JsonRpcRequest(method="x", request_id="abc-42")
    assert req.request_id == "abc-42"


def test_jsonrpc_response_roundtrips() -> None:
    from coco_sdk import JsonRpcResponse

    resp = JsonRpcResponse(request_id=7, result={"ok": True})
    assert resp.result == {"ok": True}


def test_jsonrpc_error_roundtrips() -> None:
    from coco_sdk import JsonRpcError

    err = JsonRpcError(request_id=1, code=-32602, message="invalid params")
    assert err.code == -32602


def test_jsonrpc_notification_roundtrips() -> None:
    from coco_sdk import JsonRpcNotification

    notif = JsonRpcNotification(method="x/y", params={})
    assert notif.method == "x/y"


def test_request_id_is_int_or_str_alias() -> None:
    """``RequestId = int | str`` is emitted as a real type alias."""
    from coco_sdk import RequestId

    assert RequestId == (int | str)


# ── 4. Multi-provider types from coco_sdk.types ────────────────────


def test_provider_api_enum_has_all_six_variants() -> None:
    from coco_sdk import ProviderApi

    expected = {"anthropic", "openai", "gemini", "volcengine", "zai", "openai_compat"}
    actual = {m.value for m in ProviderApi}
    assert actual == expected


def test_model_role_enum_has_all_eight_variants() -> None:
    from coco_sdk import ModelRole

    expected = {
        "main", "fast", "plan", "explore",
        "review", "hook_agent", "memory", "subagent",
    }
    assert {m.value for m in ModelRole} == expected


def test_model_spec_subclass_has_cli_arg() -> None:
    """``coco_sdk.ModelSpec`` is the cli_arg-aware subclass, not the raw generated."""
    from coco_sdk import ModelSpec, ProviderApi

    spec = ModelSpec(
        provider="anthropic",
        model_id="claude-opus-4-7",
        api=ProviderApi.anthropic,
        display_name="Opus 4.7",
    )
    assert spec.cli_arg == "anthropic/claude-opus-4-7"
    assert str(spec) == "anthropic/claude-opus-4-7"


def test_deepseek_presets_are_valid_model_specs() -> None:
    from coco_sdk import DEEPSEEK, ModelSpec, ProviderApi

    assert isinstance(DEEPSEEK.flash_openai, ModelSpec)
    assert DEEPSEEK.flash_openai.api == ProviderApi.openai_compat
    assert DEEPSEEK.flash_openai.cli_arg == "deepseek-openai/deepseek-v4-flash"
    assert DEEPSEEK.pro_openai.model_id == "deepseek-v4-pro"
    assert DEEPSEEK.flash_anthropic.api == ProviderApi.anthropic


def test_thinking_builder_produces_thinking_level() -> None:
    from coco_sdk import ReasoningEffort, ThinkingLevel, thinking

    level = thinking(ReasoningEffort.high, budget_tokens=8000)
    assert isinstance(level, ThinkingLevel)
    assert level.effort == ReasoningEffort.high
    assert level.budget_tokens == 8000


# ── 5. MCP wire types — union alias must NOT be shadowed by stub ────


def test_mcp_server_config_is_union_alias() -> None:
    """``McpServerConfig`` is a type alias over the three transport variants —
    not a stub BaseModel that would lose all field info.
    """
    import types
    from coco_sdk import (
        HttpMcpServerConfig,
        McpServerConfig,
        SseMcpServerConfig,
        StdioMcpServerConfig,
    )

    assert isinstance(McpServerConfig, types.UnionType)
    # Discrimination by ``type`` field works because each variant fixes its
    # own literal.
    stdio = StdioMcpServerConfig(command="echo", args=["hi"])
    assert stdio.type == "stdio"
    sse = SseMcpServerConfig(url="https://example.com/sse")
    assert sse.type == "sse"
    http = HttpMcpServerConfig(url="https://example.com")
    assert http.type == "http"


# ── 6. Wire-method enums — every variant must have a defined value ──


def test_client_request_methods_match_rust_wire_strings() -> None:
    """Spot-check a handful of ClientRequestMethod values against the Rust
    wire-method strings. If the enum drifts (e.g. snake_case → camelCase
    in Rust without a serde rename), this test fails.
    """
    from coco_sdk import ClientRequestMethod

    spot = {
        "initialize": ClientRequestMethod.INITIALIZE,
        "session/start": ClientRequestMethod.SESSION_START,
        "turn/start": ClientRequestMethod.TURN_START,
        "approval/resolve": ClientRequestMethod.APPROVAL_RESOLVE,
        "control/setThinking": ClientRequestMethod.CONTROL_SET_THINKING,
        "mcp/status": ClientRequestMethod.MCP_STATUS,
        "context/usage": ClientRequestMethod.CONTEXT_USAGE,
        "elicitation/resolve": ClientRequestMethod.ELICITATION_RESOLVE,
    }
    for wire, member in spot.items():
        assert member.value == wire, f"{member.name} = {member.value!r}, expected {wire!r}"


def test_server_request_methods_cover_variants() -> None:
    from coco_sdk import ServerRequestMethod

    expected = {
        "approval/askForApproval",
        "input/requestUserInput",
        "mcp/routeMessage",
        "mcp/requestElicitation",
        "hook/callback",
        "control/cancelRequest",
    }
    assert {m.value for m in ServerRequestMethod} == expected


# ── 7. Hook + permission types must construct ─────────────────────


def test_hook_callback_matcher_constructs() -> None:
    from coco_sdk import HookCallbackMatcher

    m = HookCallbackMatcher(
        hook_callback_ids=["abc"], matcher="Bash|Read", timeout=5
    )
    assert m.hook_callback_ids == ["abc"]
    assert m.timeout == 5


def test_approval_decision_enum() -> None:
    from coco_sdk import ApprovalDecision

    assert ApprovalDecision.allow.value == "allow"
    assert ApprovalDecision.deny.value == "deny"


def test_permission_mode_enum() -> None:
    from coco_sdk import PermissionMode

    # camelCase per coco-types serde annotation
    assert {m.value for m in PermissionMode} >= {"default", "auto", "bubble"}


# ── 8. Notifications — accessor pattern works on tagged unions ─────


def test_server_notification_typed_dispatch() -> None:
    """``ServerNotification`` is a Pydantic discriminated union; validating
    a wire payload dispatches to the typed variant class based on the
    `method` discriminator. ``TurnEnded`` further discriminates by
    ``outcome.kind`` — the unified terminal event covers what used to be
    ``turn/completed`` / ``turn/failed`` / ``turn/interrupted`` /
    ``turn/maxReached``.
    """
    from pydantic import TypeAdapter
    from coco_sdk import (
        NotificationMethod,
        ServerNotification,
        ServerNotificationSessionStarted,
        ServerNotificationTurnEnded,
    )

    adapter = TypeAdapter(ServerNotification)
    notif = adapter.validate_python(
        {
            "method": NotificationMethod.TURN_ENDED.value,
            "params": {
                "turn_id": "t1",
                "usage": {"input_tokens": {"total": 1}, "output_tokens": {"total": 1}},
                "outcome": {"kind": "completed", "data": {"stop_reason": "end_turn"}},
            },
        }
    )
    assert isinstance(notif, ServerNotificationTurnEnded)
    assert notif.params.turn_id == "t1"
    assert notif.params.usage.input_tokens.total == 1
    assert notif.params.outcome.kind == "completed"
    # Typed access: `outcome.data` is a `CompletedOutcome` Pydantic
    # model (H2 fix — was `dict[str, Any]` before the named-data
    # refactor of `TurnOutcome` variants in `coco-types`).
    assert notif.params.outcome.data.stop_reason == "end_turn"

    # Different method → different concrete variant class.
    other = adapter.validate_python(
        {
            "method": NotificationMethod.SESSION_STARTED.value,
            "params": {
                "session_id": "s1",
                "cwd": "/",
                "model": "claude",
                "permission_mode": "default",
                "protocol_version": "1",
                "version": "0.0",
            },
        }
    )
    assert isinstance(other, ServerNotificationSessionStarted)
    assert not isinstance(other, ServerNotificationTurnEnded)

    # All five outcome variants parse via the discriminated union on
    # `outcome.kind`. Sample each so the wire contract is locked. The
    # H2 fix means `outcome.data` is now a typed Pydantic model
    # (CompletedOutcome / FailedOutcome / …); the test reads typed
    # attributes rather than dict keys.
    for outcome, attr_check in [
        (
            {"kind": "failed", "data": {"error": {"message": "boom", "code": "provider"}}},
            lambda d: d.error.message == "boom" and d.error.code == "provider",
        ),
        (
            {"kind": "interrupted", "data": {"abort_reason": "user_cancel"}},
            lambda d: d.abort_reason == "user_cancel",
        ),
        (
            {"kind": "max_turns_reached", "data": {"max_turns": 5}},
            lambda d: d.max_turns == 5,
        ),
        (
            {
                "kind": "budget_exhausted",
                "data": {"used_tokens": 12, "budget_tokens": 100},
            },
            lambda d: d.used_tokens == 12 and d.budget_tokens == 100,
        ),
        # C5 invariant: `budget_tokens: None` round-trips when the
        # engine had no `config.max_tokens` set.
        (
            {"kind": "budget_exhausted", "data": {"used_tokens": 12}},
            lambda d: d.used_tokens == 12 and d.budget_tokens is None,
        ),
    ]:
        ev = adapter.validate_python(
            {
                "method": NotificationMethod.TURN_ENDED.value,
                "params": {
                    "turn_id": "t-x",
                    "usage": {"input_tokens": {"total": 0}, "output_tokens": {"total": 0}},
                    "outcome": outcome,
                },
            }
        )
        assert isinstance(ev, ServerNotificationTurnEnded)
        assert ev.params.outcome.kind == outcome["kind"]
        assert attr_check(ev.params.outcome.data), (
            f"typed-data assertion failed for outcome {outcome!r}"
        )


# ── 9. Decorator + Python-only types ───────────────────────────────


def test_hook_decorator_produces_definition() -> None:
    from coco_sdk import HookDefinition, hook

    @hook(event="PreToolUse", matcher="Bash", timeout_ms=5000)
    async def h(callback_id, event_type, input):
        return {"behavior": "allow"}

    assert isinstance(h, HookDefinition)
    assert h.event == "PreToolUse"
    assert h.matcher == "Bash"
    assert h.timeout_ms == 5000
    assert h.callback_id  # uuid generated


def test_tool_decorator_produces_definition() -> None:
    from coco_sdk import ToolDefinition, tool

    @tool(name="echo", description="Echo input")
    def echo(text: str) -> str:
        return text

    assert isinstance(echo, ToolDefinition)
    assert echo.name == "echo"
    assert echo.to_mcp_tool_def()["inputSchema"]["properties"]["text"]["type"] == "string"


# ── 10. Errors module ─────────────────────────────────────────────


def test_error_hierarchy_intact() -> None:
    from coco_sdk import (
        CLIConnectionError,
        CLINotFoundError,
        CocoSDKError,
        JSONDecodeError,
        ProcessError,
        SessionNotFoundError,
        TransportClosedError,
    )

    for cls in (
        CLIConnectionError,
        CLINotFoundError,
        JSONDecodeError,
        ProcessError,
        SessionNotFoundError,
        TransportClosedError,
    ):
        assert issubclass(cls, CocoSDKError), f"{cls.__name__} not a CocoSDKError"


# ── 11. Sanity: no stub leftovers ─────────────────────────────────


def test_no_stub_classes_in_protocol() -> None:
    """``BaseModel(extra='allow')`` placeholder stubs were a long-running
    debt; every name must now resolve to a real schema-derived type.
    """
    import coco_sdk.generated.protocol as proto

    src_path = inspect.getfile(proto)
    with open(src_path) as f:
        text = f.read()
    assert '"""Stub for' not in text, (
        "compat stubs reappeared in generated/protocol.py — investigate "
        "scripts/append_stubs.py and the schema bundle composition"
    )
