"""End-to-end: model invokes the built-in ``Read`` tool through the SdkServer.

Drops a fixture file into the per-test cwd, asks the model to read it
with ``permission_mode="auto"`` (server auto-approves the tool call so
no SDK-side approval bridge is needed), and asserts at least one
``item/*`` event for a ``tool_call`` lands on the wire.

This exercises the full tool-runtime path that the protocol-only e2e
tests skip: tool-schema advertisement during ``initialize``, model
emission of a tool_use, server permission check, tool execution, and
the resulting item events.
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient
from coco_sdk.generated.protocol import NotificationMethod


async def test_model_invokes_read_tool_on_fixture(live_deepseek, isolated_cwd) -> None:
    fixture = isolated_cwd / "marker.txt"
    fixture.write_text("the lucky number is 47\n")

    tool_call_item_ids: set[str] = set()
    saw_terminal = False

    async with CocoClient(
        prompt=(
            "Use the Read tool on the file 'marker.txt' in the current"
            " working directory, then reply with just the lucky number"
            " digits — no other text."
        ),
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        permission_mode="auto",
        # Tool-using turns typically need 2+ turns: one for the tool
        # call, one for the final answer.
        max_turns=4,
    ) as client:
        try:
            async with asyncio.timeout(180):
                async for event in client.events():
                    method = event.method
                    if method in (
                        NotificationMethod.ITEM_STARTED,
                        NotificationMethod.ITEM_UPDATED,
                        NotificationMethod.ITEM_COMPLETED,
                    ):
                        item = event.params.get("item", {})
                        details = item.get("details", {})
                        if details.get("type") == "tool_call":
                            tool_call_item_ids.add(item.get("item_id", ""))
                    elif method == NotificationMethod.TURN_ENDED:
                        saw_terminal = True
                        # If the model already invoked the tool, we have
                        # everything we need — bail out early to keep
                        # the test cost low. Otherwise loop for another
                        # turn (the runner auto-continues when there
                        # are pending tool calls).
                        if tool_call_item_ids:
                            break
                    elif method == NotificationMethod.TURN_ENDED:
                        pytest.fail(f"turn failed: {event.params}")
        except asyncio.TimeoutError:
            pytest.fail(
                f"timed out waiting for tool_call item event "
                f"(tool_calls={len(tool_call_item_ids)}, terminal={saw_terminal})"
            )

    assert tool_call_item_ids, (
        "expected the model to emit at least one tool_call item via the"
        " Read tool — none observed. Either the prompt didn't trigger a"
        " tool use or the tool catalog wasn't advertised on initialize."
    )
