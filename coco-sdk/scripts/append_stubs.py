#!/usr/bin/env python3
"""Append compatibility stubs to generated/protocol.py.

Scans src/coco_sdk/**/*.py and tests/**/*.py for imports from
`coco_sdk.generated.protocol` that reference class names the
generator did NOT emit. For each missing name, appends a loose
`BaseModel` subclass with `model_config = {"extra": "allow"}` so
downstream imports succeed.

These stubs are a bridge while the Rust schema generator catches up
with what legacy client.py / tests expect. They should shrink to zero
as Phase 2 schema coverage grows.

Usage: append_stubs.py <python_dir> <protocol_path>
"""
from __future__ import annotations

import ast
import sys
from pathlib import Path


def main(python_dir: Path, proto_path: Path) -> int:
    src_text = proto_path.read_text()
    try:
        tree = ast.parse(src_text)
    except SyntaxError as e:
        print(f"error: protocol.py parse failed: {e}", file=sys.stderr)
        return 1

    # "Existing" names include classes AND module-level assignments
    # (type aliases like `McpServerConfig = StdioMcpServerConfig | ...`).
    # Without `Assign`/`AnnAssign` detection, append_stubs would happily
    # define a stub class with the same name *below* the alias,
    # silently shadowing it for downstream importers.
    existing: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            existing.add(node.name)
        elif isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    existing.add(target.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            existing.add(node.target.id)

    missing: set[str] = set()
    scan_dirs = [python_dir / "src" / "coco_sdk", python_dir / "tests"]
    # `__init__.py` is auto-regenerated *after* this script by
    # regen_init.py — its imports are downstream of protocol.py, not
    # an authoritative source of user-side references. Including it
    # here would create a feedback loop: the previous run's
    # `__init__.py` lists every protocol class (including stubs from
    # earlier runs), which would re-trigger the same stubs forever.
    SKIP_FILES = {"__init__.py"}
    for root in scan_dirs:
        if not root.exists():
            continue
        for py in root.rglob("*.py"):
            if "generated" in py.parts or "__pycache__" in py.parts:
                continue
            if py.name in SKIP_FILES:
                continue
            try:
                t = ast.parse(py.read_text())
            except SyntaxError:
                continue
            for node in ast.walk(t):
                if (
                    isinstance(node, ast.ImportFrom)
                    and node.module == "coco_sdk.generated.protocol"
                ):
                    for alias in node.names:
                        if alias.name not in existing:
                            missing.add(alias.name)

    if not missing:
        print("  (no stubs needed)")
        return 0

    # Don't double-append: strip any prior stub block before rewriting.
    marker = "\n\n# ── Compatibility stubs ──"
    idx = src_text.find(marker)
    if idx != -1:
        src_text = src_text[:idx].rstrip() + "\n"

    stub_lines = [
        "",
        "",
        "# ── Compatibility stubs ──",
        "#",
        "# Loose BaseModel subclasses for names referenced by client.py / tests",
        "# but not yet emitted by the coco-rs schema generator. These accept any",
        "# fields (`extra='allow'`) and are regenerated on every run of",
        "# ./coco-sdk/scripts/generate_python.sh.",
        "",
    ]
    for name in sorted(missing):
        stub_lines.extend(
            [
                f"class {name}(BaseModel):",
                f'    """Stub for {name} pending coco-rs schema emission."""',
                '    model_config = {"extra": "allow"}',
                "",
                "",
            ]
        )

    with proto_path.open("w") as f:
        f.write(src_text + "\n".join(stub_lines))

    print(f"  appended {len(missing)} stubs: {', '.join(sorted(missing))}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: append_stubs.py <python_dir> <protocol_path>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(Path(sys.argv[1]), Path(sys.argv[2])))
