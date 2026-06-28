"""Reusable e2e harness helpers for SDK subprocess tests."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from coco_sdk._internal.transport import Transport
from coco_sdk.generated.protocol import ServerNotification


@dataclass(frozen=True)
class TranscriptFrame:
    """One JSON-RPC frame observed by the SDK test harness."""

    direction: str
    payload: dict[str, Any]


class TranscriptTransport(Transport):
    """Transport wrapper that records every sent and received JSON frame.

    Use this around ``SubprocessCLITransport`` in e2e tests when a
    failure needs an inspectable wire transcript without changing the
    production transport implementation.
    """

    def __init__(self, inner: Transport):
        self.inner = inner
        self.frames: list[TranscriptFrame] = []

    async def start(self) -> None:
        await self.inner.start()

    async def send_line(self, line: str) -> None:
        payload = json.loads(line)
        if isinstance(payload, dict):
            self.frames.append(TranscriptFrame("client_to_server", payload))
        await self.inner.send_line(line)

    def next_request_id(self) -> int:
        return self.inner.next_request_id()

    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        async for payload in self.inner.read_lines():
            self.frames.append(TranscriptFrame("server_to_client", dict(payload)))
            yield payload

    async def read_events(self) -> AsyncIterator[ServerNotification]:
        async for event in self.inner.read_events():
            yield event

    async def close(self) -> None:
        await self.inner.close()

    def dump_jsonl(self, path: str | Path) -> None:
        """Write the captured transcript to ``path`` as JSONL."""

        target = Path(path)
        target.write_text(
            "".join(
                json.dumps(
                    {"direction": frame.direction, "payload": frame.payload},
                    sort_keys=True,
                )
                + "\n"
                for frame in self.frames
            )
        )

    def text_dump(self) -> str:
        """Return a compact human-readable transcript."""

        return "\n".join(
            f"{frame.direction}: {json.dumps(frame.payload, sort_keys=True)}"
            for frame in self.frames
        )
