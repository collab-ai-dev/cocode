"""Tests for error type hierarchy."""

from coco_sdk.errors import (
    CLIConnectionError,
    CLINotFoundError,
    CocoSDKError,
    JSONDecodeError,
    ProcessError,
    SessionNotFoundError,
    TransportClosedError,
)


def test_error_hierarchy():
    """All SDK errors inherit from CocoSDKError."""
    assert issubclass(CLINotFoundError, CocoSDKError)
    assert issubclass(CLIConnectionError, CocoSDKError)
    assert issubclass(ProcessError, CocoSDKError)
    assert issubclass(JSONDecodeError, CocoSDKError)
    assert issubclass(TransportClosedError, CocoSDKError)
    assert issubclass(SessionNotFoundError, CocoSDKError)


def test_process_error_attributes():
    err = ProcessError("failed", exit_code=1, stderr="boom")
    assert err.exit_code == 1
    assert err.stderr == "boom"
    assert "failed" in str(err)


def test_json_decode_error_attributes():
    err = JSONDecodeError("bad json", raw_line="{broken")
    assert err.raw_line == "{broken"


def test_session_not_found():
    err = SessionNotFoundError("sess_123")
    assert err.session_id == "sess_123"
    assert "sess_123" in str(err)


def test_catch_base_exception():
    """CocoSDKError catches all SDK errors."""
    try:
        raise CLINotFoundError("not found")
    except CocoSDKError as e:
        assert "not found" in str(e)
