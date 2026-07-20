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
JSONRPC_VERSION = "2.0"

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
        self._handler_tasks: dict[int | str, asyncio.Task[None]] = {}
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
        for task in list(self._handler_tasks.values()):
            task.cancel()
        if self._handler_tasks:
            await asyncio.gather(*self._handler_tasks.values(), return_exceptions=True)
        self._fail_all(TransportClosedError("transport closed"))

    async def request(self, typed_request: Any) -> dict[str, Any]:
        request_id = self._transport.next_request_id()
        early = self._early_responses.pop(request_id, None)
        if early is not None:
            await self._send_typed_request(request_id, typed_request)
            if isinstance(early, BaseException):
                raise early
            return early
        if self._closed:
            raise TransportClosedError("transport closed")
        loop = asyncio.get_running_loop()
        waiter: asyncio.Future[dict[str, Any]] = loop.create_future()
        self._pending[request_id] = waiter
        try:
            await self._send_typed_request(request_id, typed_request)
        except BaseException:
            self._pending.pop(request_id, None)
            waiter.cancel()
            raise
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
        await self._transport.send_line(
            json.dumps(
                {
                    "jsonrpc": JSONRPC_VERSION,
                    "id": request_id,
                    "result": result if result is not None else {},
                }
            )
        )

    async def respond_error(
        self,
        request_id: int | str,
        *,
        code: int = -32603,
        message: str,
    ) -> None:
        await self._transport.send_line(
            json.dumps(
                {
                    "jsonrpc": JSONRPC_VERSION,
                    "id": request_id,
                    "error": {
                        "code": code,
                        "message": message,
                    },
                }
            )
        )

    async def next_event(self) -> dict[str, Any]:
        item = await self._events.get()
        if isinstance(item, BaseException):
            raise item
        return item

    async def _send_typed_request(
        self, request_id: int | str, typed_request: Any
    ) -> None:
        envelope: dict[str, Any] = {
            "jsonrpc": JSONRPC_VERSION,
            "id": request_id,
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
                if data.get("jsonrpc") != JSONRPC_VERSION:
                    raise ProcessError(
                        f"invalid JSON-RPC version from coco: {data.get('jsonrpc')!r}"
                    )
                if "id" in data and "result" in data:
                    self._route_response(data)
                elif "error" in data:
                    self._route_error(data)
                elif "id" in data and "method" in data:
                    self._route_server_request(data)
                    # Preserve causal order for already-buffered frames while
                    # keeping slow approval callbacks concurrent/cancellable.
                    await asyncio.sleep(0)
                elif "method" in data:
                    if data.get("method") == "control/cancelRequest":
                        params = data.get("params") or {}
                        request_id = params.get("request_id")
                        task = self._handler_tasks.pop(request_id, None)
                        if task is not None:
                            task.cancel()
                    else:
                        await self._events.put(data)
                else:
                    raise ProcessError(f"invalid JSON-RPC message from coco: {data!r}")
        except asyncio.CancelledError:
            raise
        except BaseException as exc:
            self._fail_all(exc)
        else:
            self._fail_all(TransportClosedError("transport closed"))

    def _route_response(self, data: dict[str, Any]) -> None:
        request_id = data.get("id")
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
        request_id = data.get("id")
        error_obj = data.get("error") or {}
        if request_id in self._ignored_responses:
            self._ignored_responses.discard(request_id)
            return
        waiter = self._pending.pop(request_id, None)
        error = ProcessError(
            f"coco rejected request {request_id}: {error_obj.get('message', '')}",
            exit_code=error_obj.get("code"),
        )
        if waiter and not waiter.done():
            waiter.set_exception(error)
            return
        if request_id is not None:
            self._early_responses[request_id] = error
            return
        logger.warning(
            "wire error from coco: code=%s message=%s",
            error_obj.get("code"),
            error_obj.get("message"),
        )

    def _route_server_request(self, data: dict[str, Any]) -> None:
        request_id = data.get("id")
        if request_id is None:
            return

        async def run_handler() -> None:
            try:
                handled = False
                if self._server_request_handler is not None:
                    handled = await self._server_request_handler(data)
                if not handled:
                    await self.respond_error(
                        request_id,
                        code=-32601,
                        message=f"unsupported server request: {data.get('method', '')}",
                    )
            except asyncio.CancelledError:
                raise
            except BaseException as exc:
                try:
                    await self.respond_error(request_id, message=str(exc))
                except BaseException:
                    logger.exception(
                        "failed to report server-request handler error for %r",
                        request_id,
                    )
            finally:
                self._handler_tasks.pop(request_id, None)

        task = asyncio.create_task(run_handler())
        self._handler_tasks[request_id] = task

    def _fail_all(self, exc: BaseException) -> None:
        self._closed = True
        for waiter in self._pending.values():
            if not waiter.done():
                waiter.set_exception(exc)
        self._pending.clear()
        for task in self._handler_tasks.values():
            task.cancel()
        self._handler_tasks.clear()
        self._events.put_nowait(exc)
