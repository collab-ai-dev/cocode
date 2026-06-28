"""Runtime discovery helpers for locating the ``coco`` CLI binary."""

from __future__ import annotations

import os
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from coco_sdk.errors import CLINotFoundError


@dataclass(frozen=True)
class CocoRuntime:
    """Resolved coco runtime executable.

    ``source`` is diagnostic-only and helps tests or callers explain
    whether the binary came from an explicit argument, ``COCO_PATH``,
    ``PATH``, or a well-known local install location.
    """

    binary_path: str
    source: str


def resolve_coco_runtime(
    binary_path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> CocoRuntime:
    """Resolve the coco CLI runtime executable.

    This is intentionally lightweight: today it discovers an already
    installed binary. Keeping the logic behind one public seam lets a
    future platform-specific runtime package plug in without changing
    the subprocess transport or e2e harness.
    """

    environment = env if env is not None else os.environ
    if binary_path is not None:
        return _runtime_from_candidate(Path(binary_path), "explicit")

    env_path = environment.get("COCO_PATH")
    if env_path:
        return _runtime_from_candidate(Path(env_path), "COCO_PATH")

    on_path = shutil.which("coco")
    if on_path:
        return CocoRuntime(binary_path=on_path, source="PATH")

    for candidate in _common_install_paths():
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return CocoRuntime(binary_path=str(candidate), source="common_path")

    raise CLINotFoundError(
        "coco binary not found. Install it or set COCO_PATH environment variable."
    )


def find_coco_binary(
    binary_path: str | os.PathLike[str] | None = None,
    *,
    env: Mapping[str, str] | None = None,
) -> str:
    """Return only the resolved binary path for simple callers."""

    return resolve_coco_runtime(binary_path, env=env).binary_path


def _runtime_from_candidate(candidate: Path, source: str) -> CocoRuntime:
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return CocoRuntime(binary_path=str(candidate), source=source)
    raise CLINotFoundError(f"coco binary not found at {candidate}")


def _common_install_paths() -> tuple[Path, ...]:
    return (
        Path.home() / ".cargo" / "bin" / "coco",
        Path("/usr/local/bin/coco"),
    )
