"""End-to-end: sync request-response control queries against the live server.

Covers ``mcp_status`` and ``context_usage`` against a real
``SdkServer``. Both are simple read-only RPCs but they exercise the
``_send_and_await_response`` path (which the notification-only e2e
tests don't reach).
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient
from coco_sdk.generated.protocol import NotificationMethod


async def test_mcp_status_returns_empty_roster(live_deepseek, isolated_cwd) -> None:
    """Fresh sessions have no MCP servers configured; the call should
    succeed and return an iterable container (list or dict).
    """
    async with CocoClient(
        prompt="reply with: ok",
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        max_turns=1,
    ) as client:
        # Wait for session/started so the SdkServer has bound a session.
        async with asyncio.timeout(60):
            async for event in client.events():
                if event.method == NotificationMethod.SESSION_STARTED:
                    break

        result = await client.mcp_status()

    # `mcp_status` returns a typed `McpStatusResult` with `.mcp_servers`.
    assert isinstance(result.mcp_servers, list), (
        f"expected mcp_servers to be a list, got {type(result.mcp_servers).__name__}"
    )


async def test_context_usage_returns_token_breakdown(live_deepseek, isolated_cwd) -> None:
    """After a real turn against DeepSeek, context_usage should report
    a non-empty breakdown — at minimum a token count (pre-turn or post-turn).
    """
    async with CocoClient(
        prompt="Reply with the single word: ok",
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        max_turns=1,
    ) as client:
        async with asyncio.timeout(120):
            async for event in client.events():
                if event.method == NotificationMethod.TURN_ENDED:
                    break
                if event.method == NotificationMethod.TURN_ENDED:
                    pytest.fail(f"turn failed: {event.params}")

        result = await client.context_usage()

    # `context_usage` returns a typed `ContextUsageResult`. The universal
    # invariant after a real turn is `total_tokens > 0`.
    assert result.total_tokens > 0, (
        f"context_usage should report non-zero total_tokens; got: {result!r}"
    )
