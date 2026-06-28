"""Tests for coco runtime discovery."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from coco_sdk.errors import CLINotFoundError
from coco_sdk.runtime import find_coco_binary, resolve_coco_runtime


def test_resolve_coco_runtime_uses_explicit_binary(tmp_path: Path) -> None:
    binary = tmp_path / "coco"
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)

    runtime = resolve_coco_runtime(binary)

    assert runtime.binary_path == str(binary)
    assert runtime.source == "explicit"
    assert find_coco_binary(binary) == str(binary)


def test_resolve_coco_runtime_uses_coco_path(tmp_path: Path) -> None:
    binary = tmp_path / "coco"
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)

    runtime = resolve_coco_runtime(env={"COCO_PATH": str(binary)})

    assert runtime.binary_path == str(binary)
    assert runtime.source == "COCO_PATH"


def test_resolve_coco_runtime_rejects_missing_explicit_binary(tmp_path: Path) -> None:
    with pytest.raises(CLINotFoundError):
        resolve_coco_runtime(tmp_path / "missing-coco")


def test_resolve_coco_runtime_finds_path_binary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary = tmp_path / "coco"
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)
    monkeypatch.setenv("PATH", str(tmp_path))
    monkeypatch.delenv("COCO_PATH", raising=False)

    runtime = resolve_coco_runtime()

    assert os.path.samefile(runtime.binary_path, binary)
    assert runtime.source == "PATH"
