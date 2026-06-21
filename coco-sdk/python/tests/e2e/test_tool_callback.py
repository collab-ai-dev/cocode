"""End-to-end: in-process ``@tool()`` round-trip against deepseek-v4-flash.

Registers an SDK-hosted MCP tool, asks the model to call it, and asserts
that the tool actually executed in-process and its result reached the
final response.
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient, tool
from coco_sdk.generated.protocol import NotificationMethod


async def test_sdk_hosted_tool_invocation(live_deepseek, isolated_cwd) -> None:
    invocations: list[dict] = []

    @tool(name="lucky_number", description="Return the user's lucky number.")
    async def lucky_number() -> str:
        invocations.append({})
        return "The lucky number is 47."

    async with CocoClient(
        prompt=(
            "Call the `lucky_number` tool exactly once, then reply with"
            " the number it returned (just the digits)."
        ),
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        tools=[lucky_number],
        # SDK-hosted tools are user-authored; the test bypasses Default-mode
        # permission gating so the focus stays on tool invocation rather
        # than the approval handshake.
        permission_mode="bypassPermissions",
        max_turns=4,
    ) as client:
        text_parts: list[str] = []
        try:
            async with asyncio.timeout(180):
                async for event in client.events():
                    if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                        text_parts.append(event.params.delta)
                    elif event.method == NotificationMethod.TURN_ENDED:
                        # Some providers do tool-then-final-answer in one
                        # turn; others split into two. Loop until the
                        # model's final reply mentions the number or
                        # we exhaust turns.
                        if invocations and "47" in "".join(text_parts):
                            break
                        # Otherwise wait for another turn — coco-rs will
                        # auto-continue if there are pending tool calls.
                    elif event.method == NotificationMethod.TURN_ENDED:
                        pytest.fail(f"turn failed: {event.params}")
        except asyncio.TimeoutError:
            pytest.fail(
                f"timed out waiting for tool to be invoked"
                f" (invocations={len(invocations)}, text={''.join(text_parts)!r})"
            )

    assert invocations, "expected the model to call the lucky_number tool"
    assert "47" in "".join(text_parts), (
        f"expected the tool's return value to surface in the final reply,"
        f" got: {''.join(text_parts)!r}"
    )
