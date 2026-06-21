"""Transport abstraction for communicating with the coco binary."""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from typing import Any, AsyncIterator

from coco_sdk.generated.protocol import ServerNotification


class Transport(ABC):
    """Abstract transport for coco SDK communication.

    Two send paths:

    * :meth:`send_request` (preferred) — wraps a typed ``*Request``
      model in the JSON-RPC envelope ``{type, request_id, method,
      params}`` coco-rs requires, allocates a fresh request id, and
      returns it. Use this for every wire interaction.
    * :meth:`send_line` — raw NDJSON write. Reserved for cases where
      the caller is hand-building the envelope (for example, the
      ``mcp/routeMessageResponse`` flow that needs a custom JSON-RPC
      reply nested inside ``params``).
    """

    @abstractmethod
    async def start(self) -> None:
        """Start the transport (e.g., spawn subprocess)."""

    @abstractmethod
    async def send_line(self, line: str) -> None:
        """Send a raw JSON line. Caller wraps; rarely used directly."""

    async def send_request(self, typed_request: Any) -> int:
        """Wrap a typed request and send. Default impl works for any
        transport that implements :meth:`send_line` — subclasses can
        override for custom id allocation.
        """
        request_id = self.next_request_id()
        envelope: dict[str, Any] = {
            "type": "request",
            "request_id": request_id,
            "method": typed_request.method,
        }
        params = getattr(typed_request, "params", None)
        if params is not None:
            envelope["params"] = (
                params.model_dump(exclude_none=True)
                if hasattr(params, "model_dump")
                else params
            )
        await self.send_line(json.dumps(envelope))
        return request_id

    def next_request_id(self) -> int:
        """Allocate a fresh integer request id. Auto-increment, never zero."""
        if not hasattr(self, "_default_request_counter"):
            self._default_request_counter = 0
        self._default_request_counter += 1
        return self._default_request_counter

    @abstractmethod
    async def read_events(self) -> AsyncIterator[ServerNotification]:
        """Yield server events as typed notifications."""
        ...

    @abstractmethod
    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        """Yield raw JSON dicts from the server.

        Used by the client to distinguish ServerRequest from
        ServerNotification before parsing.
        """
        ...

    @abstractmethod
    async def close(self) -> None:
        """Shut down the transport."""
