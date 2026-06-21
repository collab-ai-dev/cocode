"""End-to-end: multi-turn ``CocoClient`` against deepseek-v4-flash.

Verifies the full handshake (initialize → session/start → turn loop →
follow-up turn → graceful close) against the real Rust binary.
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient
from coco_sdk.generated.protocol import NotificationMethod


async def _drain_until_turn_completed(client: CocoClient) -> str:
    parts: list[str] = []
    async with asyncio.timeout(120):
        async for event in client.events():
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                parts.append(event.params.delta)
            elif event.method == NotificationMethod.TURN_ENDED:
                break
            elif event.method == NotificationMethod.TURN_ENDED:
                pytest.fail(f"turn failed: {event.params}")
    return "".join(parts)


async def test_client_two_turns(live_deepseek, isolated_cwd) -> None:
    async with CocoClient(
        prompt="Remember the number 47. Then reply with: noted.",
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        max_turns=2,
    ) as client:
        first = await _drain_until_turn_completed(client)
        assert first.strip(), "expected non-empty assistant text on turn 1"

        # Follow-up turn — verify the model has the conversation context.
        second_parts: list[str] = []
        async with asyncio.timeout(120):
            async for event in client.send("What number did I ask you to remember? Reply with just the digits."):
                if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                    second_parts.append(event.params.delta)
                elif event.method == NotificationMethod.TURN_ENDED:
                    break
                elif event.method == NotificationMethod.TURN_ENDED:
                    pytest.fail(f"turn failed: {event.params}")

        second = "".join(second_parts)
        assert "47" in second, (
            f"expected the model to recall '47' from turn 1, got: {second!r}"
        )
