"""Reconnecting transport wrapper with session resume.

Monitors the underlying transport's health and automatically restarts
the subprocess + resumes the session on unexpected termination::

    from coco_sdk._internal.transport.reconnecting import ReconnectingTransport
    from coco_sdk._internal.transport.subprocess_cli import SubprocessCLITransport

    base = SubprocessCLITransport()
    transport = ReconnectingTransport(base, max_reconnect_duration=600)
"""

from __future__ import annotations

import asyncio
import json
import logging
import random
import time
from collections import deque
from typing import Any, AsyncIterator, Callable, Awaitable

logger = logging.getLogger(__name__)

from coco_sdk._internal.transport import Transport
from coco_sdk._internal.transport.subprocess_cli import SubprocessCLITransport
from coco_sdk.generated.protocol import (
    ClientRequestMethod,
    NotificationMethod,
    ServerNotification,
)


class ReconnectingTransport(Transport):
    """Transport wrapper that reconnects on failure using session resume.

    On unexpected disconnection:
    1. Waits with exponential backoff
    2. Restarts the subprocess
    3. Sends ``session/resume`` with the stored session_id
    4. Replays buffered events for continuity
    """

    def __init__(
        self,
        base: SubprocessCLITransport,
        *,
        max_reconnect_duration: float = 600.0,
        base_backoff: float = 1.0,
        max_backoff: float = 30.0,
        buffer_size: int = 1000,
        on_reconnect: Callable[[], Awaitable[None]] | None = None,
    ):
        self._base = base
        self._max_reconnect_duration = max_reconnect_duration
        self._base_backoff = base_backoff
        self._max_backoff = max_backoff
        self._buffer: deque[dict[str, Any]] = deque(maxlen=buffer_size)
        self._on_reconnect = on_reconnect
        self._session_id: str | None = None
        self._started = False
        self._reconnect_lock = asyncio.Lock()

    async def start(self) -> None:
        await self._base.start()
        self._started = True

    async def send_line(self, line: str) -> None:
        await self._base.send_line(line)

    async def read_events(self) -> AsyncIterator[ServerNotification]:
        async for raw in self.read_lines():
            yield ServerNotification.model_validate(raw)

    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        """Yield JSON dicts, reconnecting on transport failure."""
        while True:
            try:
                async for line_data in self._base.read_lines():
                    # Track session_id for resume
                    if line_data.get("method") == NotificationMethod.SESSION_STARTED:
                        params = line_data.get("params", {})
                        self._session_id = params.get("session_id")

                    self._buffer.append(line_data)
                    yield line_data
                # Normal end of stream
                break
            except Exception as exc:
                logger.warning("Transport error: %s. Attempting reconnection...", exc)
                if not await self._try_reconnect():
                    logger.error("Reconnection failed after %.0fs", self._max_reconnect_duration)
                    raise

    async def _try_reconnect(self) -> bool:
        """Attempt reconnection with exponential backoff and jitter.

        Uses an asyncio.Lock to prevent concurrent reconnection attempts.
        """
        if not self._session_id:
            return False

        async with self._reconnect_lock:
            start_time = time.monotonic()
            attempt = 0

            while time.monotonic() - start_time < self._max_reconnect_duration:
                attempt += 1
                base_delay = min(
                    self._base_backoff * (2 ** (attempt - 1)),
                    self._max_backoff,
                )
                # Add jitter to prevent thundering herd
                delay = random.uniform(base_delay * 0.5, base_delay * 1.5)
                await asyncio.sleep(delay)

                try:
                    await self._base.close()
                    await self._base.start()

                    resume = {
                        "method": ClientRequestMethod.SESSION_RESUME.value,
                        "params": {
                            "session_id": self._session_id,
                        },
                    }
                    await self._base.send_line(json.dumps(resume))

                    if self._on_reconnect:
                        await self._on_reconnect()

                    return True
                except Exception:
                    continue

            return False

    async def close(self) -> None:
        await self._base.close()
        self._started = False
