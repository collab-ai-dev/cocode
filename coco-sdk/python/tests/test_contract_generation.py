"""Generated SDK artifact contract tests."""

from __future__ import annotations

import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]


def test_python_generated_artifacts_are_up_to_date() -> None:
    """Regenerating Python exports/types should not change checked-in files."""

    subprocess.run(
        [str(REPO_ROOT / "coco-sdk" / "scripts" / "generate_python.sh"), "--check"],
        cwd=REPO_ROOT,
        check=True,
    )


def test_protocol_schema_bundle_contains_jsonrpc2_envelope() -> None:
    """The checked-in protocol schema should require strict JSON-RPC 2.0 fields."""

    import json

    schema_path = REPO_ROOT / "coco-sdk" / "schemas" / "json" / "jsonrpc_message.json"
    schema = json.loads(schema_path.read_text())
    defs = schema["$defs"]

    assert defs["JsonRpcRequest"]["required"] == ["jsonrpc", "id", "method"]
    assert defs["JsonRpcResponse"]["required"] == ["jsonrpc", "id"]
    assert defs["JsonRpcError"]["required"] == ["jsonrpc", "id", "error"]
    assert "request_id" not in defs["JsonRpcRequest"]["properties"]


def test_check_python_script_is_executable() -> None:
    script = REPO_ROOT / "coco-sdk" / "scripts" / "check_python.sh"
    assert script.is_file()
    assert script.stat().st_mode & 0o111, f"{script} must be executable"
