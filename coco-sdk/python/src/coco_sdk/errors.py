"""Error types for the coco SDK."""

from __future__ import annotations


class CocoSDKError(Exception):
    """Base exception for all coco SDK errors."""


class CLINotFoundError(CocoSDKError):
    """coco binary not found on PATH or common install locations."""


class CLIConnectionError(CocoSDKError):
    """Unable to connect to the coco binary."""


class ProcessError(CocoSDKError):
    """CLI process exited with a non-zero exit code.

    Attributes:
        exit_code: The process exit code.
        stderr: Captured stderr output (if available).
    """

    def __init__(
        self,
        message: str,
        *,
        exit_code: int | None = None,
        stderr: str | None = None,
    ):
        super().__init__(message)
        self.exit_code = exit_code
        self.stderr = stderr


class JSONDecodeError(CocoSDKError):
    """Failed to decode JSON from the CLI output stream."""

    def __init__(self, message: str, *, raw_line: str | None = None):
        super().__init__(message)
        self.raw_line = raw_line


class TransportClosedError(CocoSDKError):
    """Transport was closed unexpectedly (e.g., stdin EOF)."""


class SessionNotFoundError(CocoSDKError):
    """Referenced session does not exist."""

    def __init__(self, session_id: str):
        super().__init__(f"Session not found: {session_id}")
        self.session_id = session_id
