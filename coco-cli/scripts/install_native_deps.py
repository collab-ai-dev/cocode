#!/usr/bin/env python3
"""Install Coco native binaries (Rust CLI)."""

import argparse
from contextlib import contextmanager
import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
import sys
from typing import Sequence

SCRIPT_DIR = Path(__file__).resolve().parent
COCO_CLI_ROOT = SCRIPT_DIR.parent
# Optional: set to a known-good Actions run URL so callers can omit --workflow-url.
DEFAULT_WORKFLOW_URL = ""
GITHUB_REPO = "mylinyuzhi/codex"
VENDOR_DIR_NAME = "vendor"
BINARY_TARGETS = (
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "aarch64-apple-darwin",
)


@dataclass(frozen=True)
class BinaryComponent:
    artifact_prefix: str  # matches the artifact filename prefix (e.g. coco-<target>.zst)
    dest_dir: str  # directory under vendor/<target>/ where the binary is installed
    binary_basename: str  # executable name inside dest_dir (before optional .exe)
    targets: tuple[str, ...] | None = None  # limit installation to specific targets


BINARY_COMPONENTS = {
    "coco": BinaryComponent(
        artifact_prefix="coco",
        dest_dir="coco",
        binary_basename="coco",
    ),
}


def _gha_enabled() -> bool:
    # GitHub Actions supports "workflow commands" (e.g. ::group:: / ::error::) that make logs
    # much easier to scan: groups collapse noisy sections and error annotations surface the
    # failure in the UI without changing the actual exception/traceback output.
    return os.environ.get("GITHUB_ACTIONS") == "true"


def _gha_escape(value: str) -> str:
    # Workflow commands require percent/newline escaping.
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def _gha_error(*, title: str, message: str) -> None:
    # Emit a GitHub Actions error annotation. This does not replace stdout/stderr logs; it just
    # adds a prominent summary line to the job UI so the root cause is easier to spot.
    if not _gha_enabled():
        return
    print(
        f"::error title={_gha_escape(title)}::{_gha_escape(message)}",
        flush=True,
    )


@contextmanager
def _gha_group(title: str):
    # Wrap a block in a collapsible log group on GitHub Actions. Outside of GHA this is a no-op
    # so local output remains unchanged.
    if _gha_enabled():
        print(f"::group::{_gha_escape(title)}", flush=True)
    try:
        yield
    finally:
        if _gha_enabled():
            print("::endgroup::", flush=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Install native Coco binaries.")
    parser.add_argument(
        "--workflow-url",
        help=(
            "GitHub Actions workflow URL that produced the artifacts. Defaults to a "
            "known good run when omitted."
        ),
    )
    parser.add_argument(
        "--component",
        dest="components",
        action="append",
        choices=tuple(BINARY_COMPONENTS),
        help=(
            "Limit installation to the specified components."
            " May be repeated. Defaults to coco."
        ),
    )
    parser.add_argument(
        "root",
        nargs="?",
        type=Path,
        help=(
            "Directory containing package.json for the staged package. If omitted, the "
            "repository checkout is used."
        ),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    coco_cli_root = (args.root or COCO_CLI_ROOT).resolve()
    vendor_dir = coco_cli_root / VENDOR_DIR_NAME
    vendor_dir.mkdir(parents=True, exist_ok=True)

    components = args.components or ["coco"]

    workflow_url = (args.workflow_url or DEFAULT_WORKFLOW_URL).strip()
    if not workflow_url:
        print(
            "ERROR: No workflow URL provided and no default configured.\n"
            "Pass --workflow-url <url> pointing to a GitHub Actions workflow run.",
            file=sys.stderr,
        )
        return 1

    workflow_id = workflow_url.rstrip("/").split("/")[-1]
    print(f"Downloading native artifacts from workflow {workflow_id}...")

    with _gha_group(f"Download native artifacts from workflow {workflow_id}"):
        with tempfile.TemporaryDirectory(prefix="coco-native-artifacts-") as artifacts_dir_str:
            artifacts_dir = Path(artifacts_dir_str)
            _download_artifacts(workflow_id, artifacts_dir)
            install_binary_components(
                artifacts_dir,
                vendor_dir,
                [BINARY_COMPONENTS[name] for name in components if name in BINARY_COMPONENTS],
            )

    print(f"Installed native dependencies into {vendor_dir}")
    return 0


def _download_artifacts(workflow_id: str, dest_dir: Path) -> None:
    if not GITHUB_REPO:
        raise RuntimeError(
            "GITHUB_REPO is not configured. Set it at the top of install_native_deps.py."
        )
    cmd = [
        "gh",
        "run",
        "download",
        "--dir",
        str(dest_dir),
        "--repo",
        GITHUB_REPO,
        workflow_id,
    ]
    subprocess.check_call(cmd)


def install_binary_components(
    artifacts_dir: Path,
    vendor_dir: Path,
    selected_components: Sequence[BinaryComponent],
) -> None:
    if not selected_components:
        return

    for component in selected_components:
        component_targets = list(component.targets or BINARY_TARGETS)

        print(
            f"Installing {component.binary_basename} binaries for targets: "
            + ", ".join(component_targets)
        )
        max_workers = min(len(component_targets), max(1, (os.cpu_count() or 1)))
        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {
                executor.submit(
                    _install_single_binary,
                    artifacts_dir,
                    vendor_dir,
                    target,
                    component,
                ): target
                for target in component_targets
            }
            for future in as_completed(futures):
                installed_path = future.result()
                print(f"  installed {installed_path}")


def _install_single_binary(
    artifacts_dir: Path,
    vendor_dir: Path,
    target: str,
    component: BinaryComponent,
) -> Path:
    artifact_subdir = artifacts_dir / target
    archive_name = _archive_name_for_target(component.artifact_prefix, target)
    archive_path = artifact_subdir / archive_name
    if not archive_path.exists():
        raise FileNotFoundError(f"Expected artifact not found: {archive_path}")

    dest_dir = vendor_dir / target / component.dest_dir
    dest_dir.mkdir(parents=True, exist_ok=True)

    dest = dest_dir / component.binary_basename
    dest.unlink(missing_ok=True)
    extract_archive(archive_path, dest)
    dest.chmod(0o755)
    return dest


def _archive_name_for_target(artifact_prefix: str, target: str) -> str:
    return f"{artifact_prefix}-{target}.zst"


def extract_archive(archive_path: Path, dest: Path) -> None:
    """Extract a zstd-compressed archive to dest."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    output_path = archive_path.parent / dest.name
    subprocess.check_call(
        ["zstd", "-f", "-d", str(archive_path), "-o", str(output_path)]
    )
    shutil.move(str(output_path), dest)


if __name__ == "__main__":
    sys.exit(main())
