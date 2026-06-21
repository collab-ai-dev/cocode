#!/usr/bin/env python3
"""Regenerate `coco_sdk/__init__.py` from the current protocol.py.

Scans all top-level class definitions in `generated/protocol.py` and
rewrites `__init__.py` to import every one of them. This avoids stale
imports after regeneration.

Usage: regen_init.py <protocol_path>
"""
from __future__ import annotations

import ast
import sys
from pathlib import Path


STATIC_EXPORTS = [
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
    # Multi-provider helper namespace (mostly re-exports of schema-
    # derived types from coco_sdk.generated.protocol; ModelSpec adds an
    # ergonomic cli_arg/__str__ subclass; ModelAlias and DEEPSEEK are
    # hand-written because they live in coco-rs/common/config, not
    # coco-types). See coco_sdk/types.py.
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

# Names re-exported by coco_sdk.types that must NOT also appear in
# the protocol-import block. Two reasons to filter:
#   1. ModelSpec — types.py defines a cli_arg-aware subclass; the
#      generated import would re-bind the name to the raw Pydantic
#      class.
#   2. ModelRole / ProviderApi — types.py re-exports the generated
#      enums verbatim, so listing them in both import blocks would
#      duplicate them in `__all__` (purely cosmetic; harmless but
#      ugly). The types.py import wins; protocol.py keeps them at
#      `coco_sdk.generated.protocol.X` for direct consumers.
TYPES_PY_SHADOWS = {
    "ModelSpec",
    "ModelRole",
    "ProviderApi",
}

HEADER = '''"""coco SDK — programmatic access to the coco multi-provider LLM CLI.

Two usage patterns:

1. One-shot query (simplest)::

    from coco_sdk import query

    async for event in query("Fix the bug"):
        print(event.method, event.params)

2. Multi-turn client::

    from coco_sdk import CocoClient

    async with CocoClient(prompt="Fix the bug") as client:
        async for event in client.events():
            print(event.method)
"""

from coco_sdk.client import CanUseTool, CocoClient, HookHandler
from coco_sdk.decorators import HookDefinition, hook
from coco_sdk.errors import (
    CLIConnectionError,
    CLINotFoundError,
    CocoSDKError,
    JSONDecodeError,
    ProcessError,
    SessionNotFoundError,
    TransportClosedError,
)
from coco_sdk.query import query
from coco_sdk.structured import TypedClient
from coco_sdk.tools import ToolDefinition, tool
from coco_sdk.types import (
    DEEPSEEK,
    ModelAlias,
    ModelRole,
    ModelSpec,
    ProviderApi,
    thinking,
)

# Protocol types — auto-generated from coco-rs schemas.
# Regenerate with: ./coco-sdk/scripts/generate_all.sh
from coco_sdk.generated.protocol import (
'''

FOOTER_TEMPLATE = '''
__version__ = "0.1.0"

__all__ = [
{all_items}
]
'''


def main(proto_path: Path) -> int:
    try:
        tree = ast.parse(proto_path.read_text())
    except SyntaxError as e:
        print(f"error: protocol.py parse failed: {e}", file=sys.stderr)
        return 1

    # Discover both classes (`class Foo: ...`) AND module-level type
    # aliases (`Foo = Bar | Baz`). The codegen emits union aliases for
    # schemars `pub enum X { Int(i64), String(String) }` — without
    # picking up `Assign` nodes, those names would silently miss the
    # __init__.py re-export and force consumers into deep imports.
    discovered: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            discovered.add(node.name)
        elif isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id[0].isupper():
                    discovered.add(target.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id[0].isupper():
                discovered.add(node.target.id)
    # Drop names re-exported by coco_sdk.types so its versions win in
    # the package namespace.
    class_names = sorted(n for n in discovered if n not in TYPES_PY_SHADOWS)

    init_path = proto_path.parent.parent / "__init__.py"

    parts = [HEADER]
    for name in class_names:
        parts.append(f"    {name},\n")
    parts.append(")\n")

    all_items = "\n".join(
        f'    "{name}",' for name in STATIC_EXPORTS + class_names
    )
    parts.append(FOOTER_TEMPLATE.format(all_items=all_items))

    init_path.write_text("".join(parts))
    print(
        f"  wrote {init_path.relative_to(init_path.parent.parent.parent.parent)} "
        f"({len(class_names)} protocol types + {len(STATIC_EXPORTS)} static)"
    )
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: regen_init.py <protocol_path>", file=sys.stderr)
        sys.exit(2)
    sys.exit(main(Path(sys.argv[1])))
