"""Async message router for the coco SDK subprocess transport."""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Awaitable, Callable
from typing import Any

from coco_sdk._internal.transport import Transport
from coco_sdk.errors import ProcessError, TransportClosedError

logger = logging.getLogger(__name__)

ServerRequestHandler = Callable[[dict[str, Any]], Awaitable[bool]]


class MessageRouter:
    """Own stdout and route wire frames by request id.

    Only this class consumes ``Transport.read_lines()``. JSON-RPC
    responses wake the matching pending request, notifications flow to
    one event queue, and server requests are dispatched concurrently.
    """

    def __init__(
        self,
        transport: Transport,
        *,
        server_request_handler: ServerRequestHandler | None = None,
    ) -> None:
        self._transport = transport
        self._server_request_handler = server_request_handler
        self._pending: dict[int | str, asyncio.Future[dict[str, Any]]] = {}
        self._ignored_responses: set[int | str] = set()
        self._early_responses: dict[int | str, dict[str, Any] | BaseException] = {}
        self._events: asyncio.Queue[dict[str, Any] | BaseException] = asyncio.Queue()
        self._handler_tasks: set[asyncio.Task[None]] = set()
        self._reader_task: asyncio.Task[None] | None = None
        self._closed = False

    def start(self) -> None:
        if self._reader_task is None:
            self._reader_task = asyncio.create_task(self._read_messages())

    async def close(self) -> None:
        self._closed = True
        if self._reader_task:
            self._reader_task.cancel()
            try:
                await self._reader_task
            except asyncio.CancelledError:
                pass
            self._reader_task = None
        for task in list(self._handler_tasks):
            task.cancel()
        if self._handler_tasks:
            await asyncio.gather(*self._handler_tasks, return_exceptions=True)
        self._fail_all(TransportClosedError("transport closed"))

    async def request(self, typed_request: Any) -> dict[str, Any]:
        request_id = self._transport.next_request_id()
        if self._closed and request_id not in self._early_responses:
            raise TransportClosedError("transport closed")
        loop = asyncio.get_running_loop()
        waiter: asyncio.Future[dict[str, Any]] = loop.create_future()
        self._pending[request_id] = waiter
        try:
            await self._send_typed_request(request_id, typed_request)
        except BaseException:
            self._pending.pop(request_id, None)
            raise
        early = self._early_responses.pop(request_id, None)
        if early is not None and not waiter.done():
            self._pending.pop(request_id, None)
            if isinstance(early, BaseException):
                waiter.set_exception(early)
            else:
                waiter.set_result(early)
        return await waiter

    async def notify(self, typed_request: Any) -> None:
        """Send a request-shaped control message without awaiting a reply."""
        request_id = self._transport.next_request_id()
        self._ignored_responses.add(request_id)
        try:
            await self._send_typed_request(request_id, typed_request)
        except BaseException:
            self._ignored_responses.discard(request_id)
            raise

    async def respond(self, request_id: int | str, result: Any) -> None:
        await self._transport.send_line(json.dumps({
            "type": "response",
            "request_id": request_id,
            "result": result if result is not None else {},
        }))

    async def respond_error(
        self,
        request_id: int | str,
        *,
        code: int = -32603,
        message: str,
    ) -> None:
        await self._transport.send_line(json.dumps({
            "type": "error",
            "request_id": request_id,
            "code": code,
            "message": message,
        }))

    async def next_event(self) -> dict[str, Any]:
        item = await self._events.get()
        if isinstance(item, BaseException):
            raise item
        return item

    async def _send_typed_request(self, request_id: int | str, typed_request: Any) -> None:
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
        await self._transport.send_line(json.dumps(envelope))

    async def _read_messages(self) -> None:
        try:
            async for data in self._transport.read_lines():
                msg_type = data.get("type")
                if msg_type == "response":
                    self._route_response(data)
                elif msg_type == "error":
                    self._route_error(data)
                elif msg_type == "request":
                    self._route_server_request(data)
                else:
                    await self._events.put(data)
        except asyncio.CancelledError:
            raise
        except BaseException as exc:
            self._fail_all(exc)
        else:
            self._fail_all(TransportClosedError("transport closed"))

    def _route_response(self, data: dict[str, Any]) -> None:
        request_id = data.get("request_id")
        if request_id in self._ignored_responses:
            self._ignored_responses.discard(request_id)
            return
        waiter = self._pending.pop(request_id, None)
        if waiter and not waiter.done():
            result = data.get("result", {}) or {}
            waiter.set_result(result)
        elif request_id is not None:
            self._early_responses[request_id] = data.get("result", {}) or {}

    def _route_error(self, data: dict[str, Any]) -> None:
        request_id = data.get("request_id")
        if request_id in self._ignored_responses:
            self._ignored_responses.discard(request_id)
            return
        waiter = self._pending.pop(request_id, None)
        error = ProcessError(
            f"coco rejected request {request_id}: {data.get('message', '')}",
            exit_code=data.get("code"),
        )
        if waiter and not waiter.done():
            waiter.set_exception(error)
            return
        if request_id is not None:
            self._early_responses[request_id] = error
            return
        logger.warning(
            "wire error from coco: code=%s message=%s",
            data.get("code"),
            data.get("message"),
        )

    def _route_server_request(self, data: dict[str, Any]) -> None:
        async def run_handler() -> None:
            handled = False
            if self._server_request_handler is not None:
                handled = await self._server_request_handler(data)
            if not handled:
                await self._events.put(data)

        task = asyncio.create_task(run_handler())
        self._handler_tasks.add(task)
        task.add_done_callback(self._handler_tasks.discard)

    def _fail_all(self, exc: BaseException) -> None:
        self._closed = True
        for waiter in self._pending.values():
            if not waiter.done():
                waiter.set_exception(exc)
        self._pending.clear()
        self._events.put_nowait(exc)
