"""End-to-end: ``client.interrupt()`` cancels an in-flight turn.

Starts a turn that asks the model for a long response, interrupts it
shortly after the first delta arrives, then asserts the server emits
a terminal notification (``turn/interrupted``, ``turn/failed``, or
``turn/completed`` — providers vary on which signal they pick when a
turn is cancelled mid-stream) within a bounded window.
"""

from __future__ import annotations

import asyncio

import pytest

from coco_sdk import CocoClient
from coco_sdk.generated.protocol import NotificationMethod


TERMINAL_METHODS = {
    NotificationMethod.TURN_ENDED,
    NotificationMethod.TURN_ENDED,
    NotificationMethod.TURN_ENDED,
}


async def test_interrupt_terminates_in_flight_turn(live_deepseek, isolated_cwd) -> None:
    interrupted = False
    saw_terminal = False
    terminal_method: str | None = None

    async with CocoClient(
        prompt=(
            "Write a long detailed essay (at least 500 words) about the "
            "history of the printing press. Take your time."
        ),
        models_main=live_deepseek.models_main,
        cwd=str(isolated_cwd),
        max_turns=1,
    ) as client:
        try:
            async with asyncio.timeout(120):
                async for event in client.events():
                    method = event.method
                    # Trigger the interrupt as soon as the model starts
                    # streaming text or reasoning — that's the earliest
                    # point we can prove there's something to cancel.
                    if not interrupted and method in (
                        NotificationMethod.AGENT_MESSAGE_DELTA,
                        NotificationMethod.REASONING_DELTA,
                    ):
                        await client.interrupt()
                        interrupted = True
                    if method in TERMINAL_METHODS:
                        saw_terminal = True
                        terminal_method = (
                            method.value if hasattr(method, "value") else str(method)
                        )
                        break
        except asyncio.TimeoutError:
            pytest.fail(
                f"interrupt did not produce a terminal event within 120s "
                f"(interrupted={interrupted})"
            )

    assert interrupted, "test never observed any deltas to interrupt against"
    assert saw_terminal, (
        f"expected one of {[m.value for m in TERMINAL_METHODS]} after interrupt"
    )
    # We don't strictly require turn/interrupted because some providers
    # downgrade cancellation to turn/completed if the cancel races the
    # final token. The hard requirement is "the turn ended in bounded
    # time" — which the timeout above already enforces.
    assert terminal_method in {m.value for m in TERMINAL_METHODS}
