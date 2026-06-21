"""End-to-end: ``@hook()`` PreToolUse callback against deepseek-v4-flash.

Registers a PreToolUse hook that allows every tool call but records it,
asks the model to use a built-in tool, and asserts the hook fired.
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient, hook
from coco_sdk.generated.protocol import NotificationMethod


async def test_pre_tool_use_hook_fires(live_deepseek, isolated_cwd) -> None:
    seen: list[dict] = []

    @hook(event="PreToolUse", matcher="Bash|Read|Glob", timeout_ms=5000)
    async def record_pre_tool_use(callback_id, event_type, input):
        seen.append({"callback_id": callback_id, "input": input})
        return {"behavior": "allow"}

    # Drop a tiny file so the model has something concrete to read.
    fixture = isolated_cwd / "marker.txt"
    fixture.write_text("the lucky number is 47\n")

    async with CocoClient(
        prompt=(
            "Use the Read tool on the file 'marker.txt' in the current"
            " directory, then reply with the lucky number it contains."
        ),
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        hooks=[record_pre_tool_use],
        permission_mode="auto",  # auto-approve so the test doesn't hang
        max_turns=4,
    ) as client:
        try:
            async with asyncio.timeout(180):
                async for event in client.events():
                    if event.method == NotificationMethod.TURN_ENDED:
                        if seen:
                            break
                    elif event.method == NotificationMethod.TURN_ENDED:
                        pytest.fail(f"turn failed: {event.params}")
        except asyncio.TimeoutError:
            pytest.fail(
                f"timed out waiting for hook to fire (seen={len(seen)})"
            )

    assert seen, "expected the PreToolUse hook to fire at least once"
