"""End-to-end: assert deltas arrive *incrementally* (not all-at-once).

A real ``stream_text``-shaped consumption pattern. We accumulate text
as it arrives and assert (a) we see more than one chunk, (b) the first
chunk arrives well before turn/completed (so the producer is actually
streaming, not buffering the full response then flushing).
"""

from __future__ import annotations

import asyncio
import time

import pytest

from coco_sdk import query
from coco_sdk.generated.protocol import NotificationMethod


async def test_text_arrives_incrementally(live_deepseek, isolated_cwd) -> None:
    delta_arrival_times: list[float] = []
    accumulated: list[str] = []
    turn_completed_at: float | None = None

    started = time.monotonic()
    try:
        async with asyncio.timeout(120):
            async for event in query(
                # Ask for ~25-50 short tokens so we reliably see more than
                # one delta even on a fast provider.
                "List the numbers 1 through 20 separated by commas; nothing else.",
                    models_main=live_deepseek.models_main,
                cwd=str(isolated_cwd),
                max_turns=1,
            ):
                method = event.method
                if method == NotificationMethod.AGENT_MESSAGE_DELTA:
                    if event.params.delta:
                        delta_arrival_times.append(time.monotonic() - started)
                        accumulated.append(event.params.delta)
                elif method == NotificationMethod.TURN_ENDED:
                    turn_completed_at = time.monotonic() - started
                    break
                elif method == NotificationMethod.TURN_ENDED:
                    pytest.fail(f"turn failed: {event.params}")
    except asyncio.TimeoutError:
        pytest.fail(
            f"streaming test timed out (got {len(delta_arrival_times)} deltas)"
        )

    assert len(delta_arrival_times) >= 2, (
        f"expected at least 2 deltas (streaming), got {len(delta_arrival_times)}"
    )
    assert turn_completed_at is not None, "expected turn/completed terminator"
    # Streaming sanity: the first delta should land BEFORE turn/completed,
    # not concurrently. Allow a small floor (network jitter) but require
    # a real gap.
    assert delta_arrival_times[0] < turn_completed_at - 0.05, (
        f"first delta at {delta_arrival_times[0]:.3f}s vs "
        f"turn/completed at {turn_completed_at:.3f}s — "
        f"looks like the producer buffered the whole response"
    )
    assert any(c.strip() for c in accumulated), (
        f"expected non-empty text content, got {accumulated!r}"
    )
