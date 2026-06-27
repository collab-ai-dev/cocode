"""E2E pytest fixtures.

Mirrors the conventions of ``coco-rs/tests/live/``:

* Live tests skip cleanly when ``DEEPSEEK_API_KEY`` is missing — same
  shape as the Rust ``require_live!`` macro. Stderr gets a ``[skip]``
  line so CI logs make the gating visible.
* ``.env`` / ``.env.test`` is auto-loaded from
  ``python/tests/e2e/`` and from ``coco-rs/tests/live/`` so a single
  credentials file works for both suites.
* Each test gets its own tempdir as ``cwd``, isolating any file-touching
  tools from the rest of the workspace.
* The ``coco`` binary is located via ``COCO_PATH`` or PATH; if it can't
  be found, every test in this directory skips.
"""

from __future__ import annotations

import os
import shutil
import sys
import tempfile
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

from coco_sdk.types import DEEPSEEK, ModelSpec

DEEPSEEK_ENV_KEY = "DEEPSEEK_API_KEY"


def _candidate_env_files() -> list[Path]:
    """Locations searched for a credentials file, in priority order."""
    here = Path(__file__).resolve().parent
    repo_root = here.parents[3]  # python/tests/e2e/ → repo
    return [
        here / ".env",
        here / ".env.test",
        repo_root / "coco-rs" / "tests" / "live" / ".env",
        repo_root / "coco-rs" / "tests" / "live" / ".env.test",
    ]


def _load_env_file_once() -> None:
    """Lightweight ``.env`` parser — no external dependency."""
    for candidate in _candidate_env_files():
        if not candidate.is_file():
            continue
        with candidate.open() as f:
            for raw in f:
                line = raw.strip()
                if not line or line.startswith("#"):
                    continue
                if "=" not in line:
                    continue
                key, _, value = line.partition("=")
                key = key.strip()
                value = value.strip().strip('"').strip("'")
                # First file wins; never clobber an env var the caller already set.
                os.environ.setdefault(key, value)
        return


_load_env_file_once()


def _resolve_coco_binary() -> str | None:
    """Find the ``coco`` binary, preferring an explicit override."""
    explicit = os.environ.get("COCO_PATH")
    if explicit and Path(explicit).is_file():
        return explicit
    on_path = shutil.which("coco")
    if on_path:
        return on_path
    for candidate in (
        Path.home() / ".cargo" / "bin" / "coco",
        Path("/usr/local/bin/coco"),
    ):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    return None


def _skip(reason: str) -> None:
    """Print to stderr in the same shape as the Rust ``require_live!`` macro, then skip."""
    print(f"[skip] {reason}", file=sys.stderr)
    pytest.skip(reason, allow_module_level=False)


@dataclass(frozen=True)
class LiveTarget:
    """A resolved provider + model + binary, ready to drive a coco subprocess."""

    binary_path: str
    models_main: ModelSpec

    @property
    def cli_args(self) -> list[str]:
        return ["--models.main", self.models_main.cli_arg]


def deepseek_target(model: ModelSpec) -> LiveTarget:
    """Resolve the live DeepSeek target, or skip the test.

    Two gates (matching ``require_live!``):

    1. ``DEEPSEEK_API_KEY`` is set — otherwise the test is skipped.
    2. The ``coco`` binary is locatable — otherwise skipped.

    ``model`` is **required** — there is no silent fallback. Pick one
    from :data:`coco_sdk.types.DEEPSEEK` (``flash_openai`` /
    ``flash_anthropic`` / ``pro_openai``) or build your own
    :class:`~coco_sdk.types.ModelSpec`. If the running coco binary
    doesn't have the named provider/model configured, the subprocess
    exits non-zero on the first ``session/start`` and a
    :class:`coco_sdk.errors.ProcessError` propagates to the test.
    """
    if model is None:
        raise ValueError(
            "deepseek_target() requires an explicit `model` "
            "(e.g. DEEPSEEK.flash_openai); no default fallback."
        )
    if not os.environ.get(DEEPSEEK_ENV_KEY):
        _skip(f"{DEEPSEEK_ENV_KEY} not set; deepseek e2e tests require it")
    binary = _resolve_coco_binary()
    if not binary:
        _skip("`coco` binary not found on PATH or COCO_PATH; build coco-rs first")
    return LiveTarget(binary_path=binary, models_main=model)


@pytest.fixture
def live_deepseek() -> LiveTarget:
    """Default e2e target: ``DEEPSEEK.flash_openai`` (cheapest model).

    Tests that need a different model should call
    :func:`deepseek_target` directly with an explicit
    :class:`~coco_sdk.types.ModelSpec`.
    """
    return deepseek_target(DEEPSEEK.flash_openai)


@pytest.fixture
def isolated_cwd() -> Iterator[Path]:
    """Per-test tempdir; tools that touch the filesystem stay sandboxed there."""
    with tempfile.TemporaryDirectory(prefix="coco-sdk-e2e-") as tmp:
        yield Path(tmp)


@pytest.fixture(autouse=True)
def isolated_coco_config_dir(monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """Per-test user config dir so e2e does not read the developer's ~/.coco."""
    with tempfile.TemporaryDirectory(
        prefix="coco-sdk-config-",
        ignore_cleanup_errors=True,
    ) as tmp:
        config_dir = Path(tmp)
        monkeypatch.setenv("COCO_CONFIG_DIR", str(config_dir))
        yield config_dir
