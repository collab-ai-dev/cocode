#!/usr/bin/env python3
"""Generate TypeScript protocol types from coco SDK JSON Schemas.

The Rust schema bundle remains the single source of truth. This generator emits
wire-shape TypeScript declarations only; runtime validation intentionally stays
out of the SDK's hot path for now.
"""

from __future__ import annotations

import json
import keyword
import re
import sys
from pathlib import Path
from typing import Any


TYPE_RENAMES = {
    "TurnInterruptedParams": "TurnInterruptedNotifParams",
    "KeepAliveParams": "KeepAliveNotifParams",
}

SKIP_TYPES = {
    "ClientRequest",
    "ClientRequestMethod",
    "NotificationMethod",
    "ServerNotification",
    "ServerRequest",
    "ServerRequestMethod",
}


def resolve_ref(ref: str) -> str:
    return TYPE_RENAMES.get(ref.rsplit("/", 1)[-1], ref.rsplit("/", 1)[-1])


def literal(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False)


def enum_values(schema: dict[str, Any]) -> list[Any]:
    if "const" in schema:
        return [schema["const"]]
    return schema.get("enum", []) or []


def is_identifier(name: str) -> bool:
    return bool(re.match(r"^[A-Za-z_$][A-Za-z0-9_$]*$", name)) and not keyword.iskeyword(name)


def prop_name(name: str) -> str:
    return name if is_identifier(name) else literal(name)


def safe_const_key(value: str) -> str:
    normalized = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", value)
    words = re.split(r"[^A-Za-z0-9]+", normalized)
    key = "_".join(word for word in words if word).upper()
    if not key or key[0].isdigit():
        key = f"VALUE_{key}"
    return key


def pascal_from_method(method: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in re.split(r"[^A-Za-z0-9]+", method) if part)


def schema_to_ts(schema: Any, defs: dict[str, Any]) -> str:
    if isinstance(schema, bool):
        return "unknown" if schema else "never"

    if "const" in schema:
        return literal(schema["const"])

    if "$ref" in schema:
        return resolve_ref(schema["$ref"])

    all_of = schema.get("allOf")
    if all_of:
        return " & ".join(schema_to_ts(part, defs) for part in all_of) or "unknown"

    any_of = schema.get("anyOf")
    if any_of:
        return union(schema_to_ts(part, defs) for part in any_of)

    one_of = schema.get("oneOf")
    if one_of:
        return union(schema_to_ts(part, defs) for part in one_of)

    enum = schema.get("enum")
    if enum:
        return union(literal(value) for value in enum)

    t = schema.get("type")
    if isinstance(t, list):
        return union(schema_to_ts({**schema, "type": item}, defs) for item in t)

    if t == "null":
        return "null"
    if t == "string":
        return "string"
    if t == "integer" or t == "number":
        return "number"
    if t == "boolean":
        return "boolean"
    if t == "array":
        items = schema.get("items", True)
        return f"Array<{schema_to_ts(items, defs)}>"
    if t == "object" or "properties" in schema or "additionalProperties" in schema:
        return object_type(schema, defs)

    return "unknown"


def union(parts: Any) -> str:
    flat: list[str] = []
    for part in parts:
        if not part:
            continue
        if part not in flat:
            flat.append(part)
    if not flat:
        return "never"
    return " | ".join(flat)


def object_type(schema: dict[str, Any], defs: dict[str, Any]) -> str:
    props = schema.get("properties") or {}
    required = set(schema.get("required") or [])
    additional = schema.get("additionalProperties", None)
    entries: list[str] = []

    for name, prop in props.items():
        optional = "?" if name not in required else ""
        entries.append(f"{prop_name(name)}{optional}: {schema_to_ts(prop, defs)};")

    if isinstance(additional, dict):
        entries.append(f"[key: string]: {schema_to_ts(additional, defs)};")
    elif additional is True:
        entries.append("[key: string]: unknown;")
    elif additional is False and not entries:
        return "Record<string, never>"

    if not entries:
        return "Record<string, unknown>"
    if len(entries) == 1 and entries[0].startswith("[key: string]"):
        return f"{{ {entries[0]} }}"
    return "{\n" + "\n".join(f"  {entry}" for entry in entries) + "\n}"


def generate_named_type(name: str, schema: dict[str, Any], defs: dict[str, Any]) -> str:
    ts_name = TYPE_RENAMES.get(name, name)
    description = schema.get("description")
    doc = doc_comment(description)

    if "oneOf" in schema and all(is_string_enum_variant(v) for v in schema["oneOf"]):
        values: list[Any] = []
        for variant in schema["oneOf"]:
            values.extend(enum_values(variant))
        body = f"export type {ts_name} = {union(literal(value) for value in values)};"
        return f"{doc}{body}" if doc else body

    if schema.get("type") == "string" and schema.get("enum"):
        body = f"export type {ts_name} = {union(literal(value) for value in schema['enum'])};"
        return f"{doc}{body}" if doc else body

    if schema.get("type") == "object" or "properties" in schema or "additionalProperties" in schema:
        rendered = object_type(schema, defs)
        if rendered.startswith("{\n"):
            body = f"export interface {ts_name} {rendered}"
        else:
            body = f"export type {ts_name} = {rendered};"
        return f"{doc}{body}" if doc else body

    body = f"export type {ts_name} = {schema_to_ts(schema, defs)};"
    return f"{doc}{body}" if doc else body


def is_string_enum_variant(schema: dict[str, Any]) -> bool:
    return schema.get("type") == "string" and ("enum" in schema or "const" in schema)


def doc_comment(text: str | None) -> str:
    if not text:
        return ""
    lines = ["/**"]
    for raw in text.splitlines():
        line = raw.rstrip()
        lines.append(f" * {line}" if line else " *")
    lines.append(" */")
    return "\n".join(lines) + "\n"


def collect_definitions(schema_dir: Path) -> dict[str, Any]:
    defs: dict[str, Any] = {}
    for path in sorted(schema_dir.glob("*.json")):
        doc = json.loads(path.read_text())
        defs.update(doc.get("$defs") or {})
        title = doc.get("title")
        if title and title not in defs and ("oneOf" in doc or "anyOf" in doc):
            defs[title] = {k: v for k, v in doc.items() if k != "$schema"}
    return defs


def load_top(schema_dir: Path, file_name: str) -> dict[str, Any]:
    return json.loads((schema_dir / file_name).read_text())


def generate_method_object(name: str, methods: list[str]) -> str:
    lines = [f"export const {name} = {{"]
    seen: set[str] = set()
    for method in methods:
        key = safe_const_key(method)
        if key in seen:
            key = f"{key}_{len(seen)}"
        seen.add(key)
        lines.append(f"  {key}: {literal(method)},")
    lines.append("} as const;")
    lines.append(f"export type {name} = (typeof {name})[keyof typeof {name}];")
    return "\n".join(lines)


def generate_tagged_union(top_name: str, schema: dict[str, Any], defs: dict[str, Any]) -> tuple[str, list[str]]:
    chunks: list[str] = []
    variants: list[str] = []
    methods: list[str] = []
    for variant in schema.get("oneOf", []):
        props = variant.get("properties") or {}
        method = (props.get("method") or {}).get("const")
        if not method:
            continue
        methods.append(method)
        type_name = f"{pascal_from_method(method)}{top_name.replace('ClientRequest', 'Request').replace('ServerRequest', 'ServerRequest').replace('ServerNotification', 'Notification')}"
        if top_name == "ClientRequest":
            type_name = f"{pascal_from_method(method)}Request"
        elif top_name == "ServerRequest":
            type_name = f"{pascal_from_method(method)}ServerRequest"
        elif top_name == "ServerNotification":
            type_name = f"{pascal_from_method(method)}Notification"
        required = set(variant.get("required") or [])
        params = props.get("params")
        lines = [f"export type {type_name} = {{"]
        lines.append(f"  method: {literal(method)};")
        if params is not None:
            optional = "?" if "params" not in required else ""
            lines.append(f"  params{optional}: {schema_to_ts(params, defs)};")
        lines.append("};")
        chunks.append(doc_comment(variant.get("description")) + "\n".join(lines))
        variants.append(type_name)
    chunks.append(f"export type {top_name} = {union(variants)};")
    return "\n\n".join(chunks), methods


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: postprocess_typescript.py <schema_dir> <output_file>")
    schema_dir = Path(sys.argv[1])
    out_file = Path(sys.argv[2])
    defs = collect_definitions(schema_dir)

    client = load_top(schema_dir, "client_request.json")
    server_notification = load_top(schema_dir, "server_notification.json")
    server_request = load_top(schema_dir, "server_request.json")

    client_union, client_methods = generate_tagged_union("ClientRequest", client, defs)
    notification_union, notification_methods = generate_tagged_union(
        "ServerNotification", server_notification, defs
    )
    server_request_union, server_request_methods = generate_tagged_union("ServerRequest", server_request, defs)

    chunks = [
        "/* eslint-disable */",
        "// Generated protocol types for the coco TypeScript SDK.",
        "// Regenerate with: ./coco-sdk/scripts/generate_typescript.sh",
        "// DO NOT EDIT MANUALLY.",
        "",
        generate_method_object("ClientRequestMethod", client_methods),
        generate_method_object("NotificationMethod", notification_methods),
        generate_method_object("ServerRequestMethod", server_request_methods),
    ]

    for name in sorted(defs):
        if name in SKIP_TYPES or TYPE_RENAMES.get(name) in SKIP_TYPES:
            continue
        chunks.append(generate_named_type(name, defs[name], defs))

    chunks.extend([client_union, notification_union, server_request_union])

    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text("\n\n".join(chunks) + "\n")


if __name__ == "__main__":
    main()
