"""Unit tests for reusable e2e harness helpers."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pytest

from coco_sdk._internal.transport import Transport
from coco_sdk.generated.protocol import ServerNotification
from tests.e2e.harness import TranscriptTransport


class FakeTransport(Transport):
    def __init__(self) -> None:
        self.sent: list[str] = []
        self._next = 0

    async def start(self) -> None:
        return None

    async def send_line(self, line: str) -> None:
        self.sent.append(line)

    def next_request_id(self) -> int:
        self._next += 1
        return self._next

    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        yield {"jsonrpc": "2.0", "id": 1, "result": {"ok": True}}

    async def read_events(self) -> AsyncIterator[ServerNotification]:
        if False:
            yield None  # pragma: no cover

    async def close(self) -> None:
        return None


@pytest.mark.asyncio
async def test_transcript_transport_records_json_frames(tmp_path: Path) -> None:
    transport = TranscriptTransport(FakeTransport())

    await transport.send_line(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "x"}))
    received = [frame async for frame in transport.read_lines()]

    assert received == [{"jsonrpc": "2.0", "id": 1, "result": {"ok": True}}]
    assert [frame.direction for frame in transport.frames] == [
        "client_to_server",
        "server_to_client",
    ]
    assert "client_to_server" in transport.text_dump()

    dump_path = tmp_path / "wire.jsonl"
    transport.dump_jsonl(dump_path)
    assert len(dump_path.read_text().splitlines()) == 2
