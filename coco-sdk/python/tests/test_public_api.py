"""Public API contract tests for the coco Python SDK."""

from __future__ import annotations

import importlib.resources as resources
from pathlib import Path

import tomllib

import coco_sdk


EXPECTED_STATIC_EXPORTS = [
    "__version__",
    "query",
    "CanUseTool",
    "CocoClient",
    "HookHandler",
    "HookDefinition",
    "hook",
    "ToolDefinition",
    "tool",
    "TypedClient",
    "CocoRuntime",
    "find_coco_binary",
    "resolve_coco_runtime",
    "DEEPSEEK",
    "ModelAlias",
    "ModelRole",
    "ModelSpec",
    "ProviderApi",
    "thinking",
    "CLIConnectionError",
    "CLINotFoundError",
    "CocoSDKError",
    "JSONDecodeError",
    "ProcessError",
    "SessionNotFoundError",
    "TransportClosedError",
]


def test_static_public_exports_stay_curated() -> None:
    """Hand-written exports should stay intentional and ordered."""

    assert coco_sdk.__all__[: len(EXPECTED_STATIC_EXPORTS)] == EXPECTED_STATIC_EXPORTS


def test_package_includes_py_typed_marker() -> None:
    marker = resources.files("coco_sdk").joinpath("py.typed")
    assert marker.is_file()


def test_package_version_matches_pyproject() -> None:
    pyproject_path = Path(__file__).resolve().parents[1] / "pyproject.toml"
    pyproject = tomllib.loads(pyproject_path.read_text())
    assert coco_sdk.__version__ == pyproject["project"]["version"]


def test_root_does_not_export_internal_modules() -> None:
    assert not any(
        name.startswith("_") and name != "__version__" for name in coco_sdk.__all__
    )
    assert "Transport" not in coco_sdk.__all__
    assert "MessageRouter" not in coco_sdk.__all__
