"""Subprocess transport — spawns ``coco sdk`` and speaks NDJSON over stdio."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
from typing import Any, AsyncIterator

from coco_sdk.errors import (
    CLIConnectionError,
    CLINotFoundError,
    ProcessError,
    TransportClosedError,
)
from coco_sdk.generated.protocol import ServerNotification

from . import Transport

logger = logging.getLogger("coco_sdk.transport")

# asyncio's StreamReader defaults to a 64 KiB line buffer; a single NDJSON
# message can far exceed that (a large session list, a tool result carrying file
# contents, a long assistant reply), and `readline()` then raises
# `ValueError: Separator is found, but chunk is longer than limit`, tearing down
# the transport mid-stream. Raise the ceiling so realistic messages stream
# cleanly. It is a cap, not a preallocation — memory grows only with the line
# actually read; 64 MiB bounds a runaway line while covering any real payload.
_STDOUT_LINE_LIMIT = 64 * 1024 * 1024


def _find_coco_binary() -> str:
    """Locate the coco binary on PATH or common install locations."""
    binary = shutil.which("coco")
    if binary:
        return binary

    candidates = [
        os.path.expanduser("~/.cargo/bin/coco"),
        "/usr/local/bin/coco",
    ]
    for path in candidates:
        if os.path.isfile(path) and os.access(path, os.X_OK):
            return path

    raise CLINotFoundError(
        "coco binary not found. Install it or set COCO_PATH environment variable."
    )


class SubprocessCLITransport(Transport):
    """Transport that spawns ``coco sdk`` as a subprocess.

    The Rust binary's ``sdk`` subcommand speaks the **coco-rs SDK
    control protocol** over NDJSON on stdin/stdout — a JSON-RPC-like
    envelope (``{type, request_id, method, params}`` /
    ``{type, request_id, result|error}``) but NOT strict JSON-RPC 2.0
    (no ``jsonrpc: "2.0"`` field) and NOT identical to the TS SDK's
    ``control_request``/``control_response`` envelope (coco-rs flattens
    ``subtype``→``method``). See ``coco_types::jsonrpc`` for the
    canonical envelope definition. Stderr is captured and logged.

    ``cli_args`` are appended verbatim after ``sdk`` so callers can
    pass model selection, system prompt, permission mode, etc. without
    going through the wire protocol.
    """

    MAX_START_RETRIES = 3
    INITIAL_BACKOFF = 1.0

    def __init__(
        self,
        binary_path: str | None = None,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        cli_args: list[str] | None = None,
    ):
        self._binary_path = binary_path or os.environ.get("COCO_PATH") or _find_coco_binary()
        self._cwd = cwd
        self._env = env
        self._cli_args = list(cli_args) if cli_args else []
        self._process: asyncio.subprocess.Process | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._next_request_id = 0

    async def start(self) -> None:
        last_error: Exception | None = None
        for attempt in range(self.MAX_START_RETRIES):
            try:
                await self._start_process()
                return
            except OSError as e:
                last_error = e
                backoff = self.INITIAL_BACKOFF * (2 ** attempt)
                logger.warning(
                    "Failed to start coco (attempt %d/%d): %s. Retrying in %.1fs",
                    attempt + 1, self.MAX_START_RETRIES, e, backoff,
                )
                await asyncio.sleep(backoff)
        raise CLIConnectionError(
            f"Failed to start coco after {self.MAX_START_RETRIES} attempts"
        ) from last_error

    async def _start_process(self) -> None:
        # `--models.main`, `--log-stderr`, etc. are top-level flags — clap
        # parses them BEFORE the subcommand. Putting `cli_args` after
        # `sdk` gives "unexpected argument" errors.
        cmd = [self._binary_path, *self._cli_args, "sdk"]

        process_env = os.environ.copy()
        process_env["COCO_ENTRYPOINT"] = "sdk-py"
        if self._env:
            process_env.update(self._env)

        self._process = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=self._cwd,
            env=process_env,
            # Raise the StreamReader line-buffer ceiling above asyncio's 64 KiB
            # default so a large NDJSON message does not crash the read loop.
            limit=_STDOUT_LINE_LIMIT,
        )

        self._stderr_task = asyncio.create_task(self._read_stderr())

    async def _read_stderr(self) -> None:
        if not self._process or not self._process.stderr:
            return
        while True:
            line = await self._process.stderr.readline()
            if not line:
                break
            text = line.decode().rstrip()
            if text:
                logger.debug("coco stderr: %s", text)

    async def send_line(self, line: str) -> None:
        """Send a raw NDJSON line. Caller is responsible for wrapping.

        Most callers should use :meth:`send_request` instead — it wraps
        the typed request in the ``{type, request_id, method, params}``
        envelope coco-rs's dispatcher expects.
        """
        if not self._process or not self._process.stdin:
            raise TransportClosedError("Transport not started")
        data = (line.rstrip("\n") + "\n").encode()
        self._process.stdin.write(data)
        await self._process.stdin.drain()

    def next_request_id(self) -> int:
        """Allocate a fresh integer request id. Auto-increment, never zero."""
        self._next_request_id += 1
        return self._next_request_id

    async def send_request(self, typed_request: Any) -> int:
        """Wrap a generated ``*Request`` model into a JSON-RPC envelope and send.

        Returns the assigned ``request_id`` so callers can match the
        eventual response. Coco-rs dispatcher requires every client→
        server message to carry ``{type: "request", request_id, method,
        params}`` — sending raw ``{method, params}`` triggers
        ``parse error: missing field `type``` and the subprocess
        terminates.
        """
        request_id = self.next_request_id()
        envelope = {
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

    async def read_lines(self) -> AsyncIterator[dict[str, Any]]:
        if not self._process or not self._process.stdout:
            raise TransportClosedError("Transport not started")

        while True:
            line = await self._process.stdout.readline()
            if not line:
                returncode = self._process.returncode
                if returncode is not None and returncode != 0:
                    raise ProcessError(
                        f"coco process exited with code {returncode}",
                        exit_code=returncode,
                    )
                break
            line_str = line.decode().strip()
            if not line_str:
                continue
            try:
                yield json.loads(line_str)
            except json.JSONDecodeError as e:
                logger.warning("Malformed JSON from coco: %s (line: %s)", e, line_str[:200])

    async def read_events(self) -> AsyncIterator[ServerNotification]:
        """Yield only ``type: "notification"`` messages from the wire.

        Responses (``type: "response"``) and server-initiated requests
        (``type: "request"``) are filtered out — they're consumed by
        the request/reply machinery in ``CocoClient`` instead. For raw
        access to every wire frame, use :meth:`read_lines`.
        """
        async for data in self.read_lines():
            msg_type = data.get("type")
            if msg_type and msg_type != "notification":
                # Response, server request, or error — not an event.
                continue
            try:
                yield ServerNotification.model_validate(data)
            except Exception as e:
                logger.warning("Failed to parse server event: %s", e)

    async def close(self) -> None:
        if self._process:
            if self._process.stdin:
                self._process.stdin.close()
            try:
                self._process.terminate()
                await asyncio.wait_for(self._process.wait(), timeout=5.0)
            except (ProcessLookupError, asyncio.TimeoutError):
                self._process.kill()
            self._process = None
        if self._stderr_task:
            self._stderr_task.cancel()
            self._stderr_task = None
