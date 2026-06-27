#!/usr/bin/env python3
"""Generate Python Pydantic protocol types from JSON Schema.

Reads the JSON Schema files produced by the Rust `export-app-server-schema`
binary and generates a single `protocol.py` with ergonomic Pydantic models.

This replaces `datamodel-code-generator` which cannot handle our tagged-union
patterns (`serde(tag = "method", content = "params")`).

Usage:
    python postprocess_python.py <schema_dir> <output_file>

    schema_dir: Path to coco-rs/app-server-protocol/schema/json/
    output_file: Path to generated protocol.py
"""

from __future__ import annotations

import json
import re
import sys
import textwrap
from pathlib import Path

# NOTIFICATION_ACCESSORS, SERVER_REQUEST_ACCESSORS, and CLIENT_REQUEST_WRAPPERS
# are derived from the JSON schema at generation time. The schema — emitted
# from the Rust `wire_tagged_enum!` macro — is the single source of truth
# for every wire method, so no hand-maintained dict can drift.

THREAD_ITEM_ACCESSORS = {
    "agent_message": ("as_agent_message", "AgentMessageItem"),
    "reasoning": ("as_reasoning", "ReasoningItem"),
    "command_execution": ("as_command_execution", "CommandExecutionItem"),
    "file_change": ("as_file_change", "FileChangeItem"),
    "mcp_tool_call": ("as_mcp_tool_call", "McpToolCallItem"),
    "web_search": ("as_web_search", "WebSearchItem"),
    "subagent": ("as_subagent", "SubagentItem"),
    "tool_call": ("as_tool_call", "GenericToolCallItem"),
    "error": ("as_error_item", "ErrorItem"),
}

# Rename map: Rust type name -> Python type name. Applied when the Rust-side
# name would collide with an existing Python class (e.g. TurnInterruptedParams
# vs ClientRequest's TurnInterrupt) or where a pre-existing public API uses
# a different name.
TYPE_RENAMES = {
    "TurnInterruptedParams": "TurnInterruptedNotifParams",
    "KeepAliveParams": "KeepAliveNotifParams",
}

# Types that should be generated as str Enums (not object models)
ENUM_TYPES = set()  # auto-detected from schema

# Types that are referenced but should NOT be generated separately
# (they're embedded in tagged unions)
SKIP_TYPES = {
    "ThreadItemDetails",
}


def resolve_ref(ref: str) -> str:
    """Extract type name from $ref."""
    return ref.rsplit("/", 1)[-1]


def enum_values(prop: dict) -> list:
    """Return the closed set of values for a string-typed schema property.

    schemars 1.x (JSON Schema 2020-12) emits single-value enums as
    ``"const": <v>`` and multi-value enums as ``"enum": [<v>, ...]``.
    schemars 0.8 used ``"enum": [<v>]`` even for the single-value case.
    This helper hides the version skew so callers can treat both forms
    uniformly.
    """
    if "const" in prop:
        return [prop["const"]]
    return prop.get("enum", []) or []


def has_enum_or_const(prop: dict) -> bool:
    """True if the property is a closed-set string (enum or const form)."""
    return "const" in prop or "enum" in prop


def schema_to_python_type(prop: dict, required: bool, defs: dict) -> str:
    """Convert a JSON schema property to a Python type annotation."""
    if isinstance(prop, bool):
        return "Any"

    # Single-value `const` → `Literal[value]`. This preserves the
    # discriminator constraint when the codegen renders an inner
    # struct that participates in a tagged union (e.g. `HookInput`
    # variants lifted out of schemars's tag+flatten emission by the
    # merge step). Without this, `{type: string, const: "PreToolUse"}`
    # would degrade to plain `str` and Pydantic would lose the
    # variant-validation hint.
    if "const" in prop:
        const_val = prop["const"]
        if isinstance(const_val, (str, int, bool)):
            return f"Literal[{const_val!r}]"

    if "$ref" in prop:
        name = resolve_ref(prop["$ref"])
        name = TYPE_RENAMES.get(name, name)
        return name

    # Handle allOf with a single $ref (common schemars pattern for required refs)
    all_of = prop.get("allOf")
    if all_of and len(all_of) == 1 and "$ref" in all_of[0]:
        name = resolve_ref(all_of[0]["$ref"])
        name = TYPE_RENAMES.get(name, name)
        return name

    # Handle anyOf [$ref, null] — nullable reference (schemars Option<T>)
    any_of = prop.get("anyOf")
    if any_of:
        non_null = [v for v in any_of if v != {"type": "null"}]
        has_null = any(v == {"type": "null"} for v in any_of)
        if len(non_null) == 1:
            base = schema_to_python_type(non_null[0], True, defs)
            return f"{base} | None" if has_null else base
        # Multi-type anyOf
        types = [schema_to_python_type(v, True, defs) for v in any_of if v != {"type": "null"}]
        result = " | ".join(types)
        if has_null:
            result += " | None"
        return result

    t = prop.get("type")

    # Handle nullable types: {"type": ["integer", "null"]}
    if isinstance(t, list):
        non_null = [x for x in t if x != "null"]
        is_nullable = "null" in t
        if len(non_null) == 1:
            base = schema_to_python_type({**prop, "type": non_null[0]}, required, defs)
            return f"{base} | None" if is_nullable else base
        return "Any"

    if "oneOf" in prop and all(
        v.get("type") == "string" and has_enum_or_const(v) for v in prop["oneOf"]
    ):
        # Inline enum — this shouldn't happen at property level, but handle it
        return "str"

    if "oneOf" in prop:
        # Union type
        types = []
        for variant in prop["oneOf"]:
            types.append(schema_to_python_type(variant, True, defs))
        return " | ".join(types)

    if t == "string":
        return "str"
    if t == "integer":
        return "int"
    if t == "number":
        return "float"
    if t == "boolean":
        return "bool"
    if t == "array":
        items = prop.get("items", {})
        if isinstance(items, bool) or not items:
            return "list[Any]"
        inner = schema_to_python_type(items, True, defs)
        return f"list[{inner}]"
    if t == "object":
        addl = prop.get("additionalProperties")
        if isinstance(addl, dict) and addl:
            val_type = schema_to_python_type(addl, True, defs)
            return f"dict[str, {val_type}]"
        # schemars 1.x emits `HashMap<EnumKey, V>` as a closed object —
        # one property per enum variant, all sharing the same value
        # schema, with `additionalProperties: false`. Recover the
        # `dict[str, V]` shape from that form.
        props = prop.get("properties") or {}
        if props and addl is False:
            shapes = {json.dumps(v, sort_keys=True) for v in props.values()}
            if len(shapes) == 1:
                val_type = schema_to_python_type(
                    next(iter(props.values())), True, defs
                )
                return f"dict[str, {val_type}]"
        if isinstance(addl, bool) and addl:
            return "dict[str, Any]"
        return "dict[str, Any]"

    return "Any"


def make_optional(py_type: str) -> str:
    """Wrap type in Optional."""
    if py_type == "Any":
        return "Any"
    return f"{py_type} | None"


def generate_enum(name: str, schema: dict) -> str:
    """Generate a str Enum class from either:

    * ``{oneOf: [{type: string, enum: [v]}, ...]}`` — schemars's form for
      enums where some variants carry doc comments. Each variant
      sub-schema usually holds **one** value, but schemars *groups
      consecutive no-doc variants into a single sub-schema with a
      multi-value enum*. So a Rust enum with 8 plain variants and
      1 documented variant produces
      ``oneOf: [{enum: [<8 values>]}, {enum: [<doc'd value>]}]``.
      We must iterate every value in every variant's enum list.
    * ``{type: string, enum: [v, ...]}`` — flat string enum form
      (no per-variant docs anywhere).
    """
    py_name = TYPE_RENAMES.get(name, name)
    lines = [f"class {py_name}(str, Enum):"]
    values: list[str] = []
    if "oneOf" in schema:
        for variant in schema["oneOf"]:
            values.extend(enum_values(variant))
    else:
        values.extend(enum_values(schema))
    for value in values:
        ident = str(value).replace("/", "_").replace("-", "_")
        lines.append(f"    {ident} = {value!r}")
    return "\n".join(lines)


def generate_model(name: str, schema: dict, all_defs: dict) -> str:
    """Generate a BaseModel class from an object schema."""
    py_name = TYPE_RENAMES.get(name, name)
    required_fields = set(schema.get("required", []))
    properties = schema.get("properties", {})

    lines = [f"class {py_name}(BaseModel):"]

    if not properties:
        lines.append("    pass")
        return "\n".join(lines)

    # Fields that shadow BaseModel attributes / Python keywords and
    # need explicit aliases. Wire name is preserved through
    # `Field(alias=...)` so serde round-tripping stays bidirectional.
    FIELD_ALIASES = {
        "schema": "schema_",
        "from": "from_",
        # Python reserved words that appear on the wire (TS hook output
        # schema uses `async` and `continue`).
        "async": "async_",
        "continue": "continue_",
    }

    def py_field_for(name: str) -> str:
        """Convert a wire field name to the idiomatic Python name.
        Python is snake_case; the wire is camelCase (TS canonical) or
        snake_case (legacy). Reserved-word table wins; otherwise
        camelCase wire names are converted via the standard two-pass
        regex transform — `asyncTimeout` → `async_timeout`,
        `hookSpecificOutput` → `hook_specific_output`,
        `updatedMCPToolOutput` → `updated_mcp_tool_output`.
        Leading underscores (e.g. `_cocoRsProtocolVersion`) are
        stripped because Pydantic forbids private-looking attribute
        names; the wire-name alias preserves the underscore.
        Already-snake_case names pass through unchanged.
        """
        if name in FIELD_ALIASES:
            return FIELD_ALIASES[name]
        stripped = name.lstrip("_")
        # Two-pass camelCase → snake_case with acronym handling.
        s1 = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", stripped)
        return re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s1).lower()

    # Sort: required fields first, then optional
    req_props = [(k, v) for k, v in properties.items() if k in required_fields]
    opt_props = [(k, v) for k, v in properties.items() if k not in required_fields]

    # Wire fields that schemars emits as `true` (any value) but that are
    # always object-shaped on the wire — TS schemas use
    # `z.record(z.string(), z.unknown())` for these. Tighten the Python
    # type to `dict[str, Any] | None` so callers get autocomplete and
    # can't accidentally pass a non-object. Names match Rust's
    # `serde_json::Value` fields where the contract is object-only.
    _OBJECT_SHAPED_FIELD_NAMES = {
        "updated_input",
        "updatedInput",
        "updated_mcp_tool_output",
        "updatedMCPToolOutput",
        "original_input",
        "originalInput",
        "content",  # Elicitation.content / ElicitationResult.content
    }

    for field_name, prop in req_props + opt_props:
        is_required = field_name in required_fields
        py_type = schema_to_python_type(prop, is_required, all_defs)
        # Tighten `Any` to `dict[str, Any]` for object-shaped slots.
        if py_type == "Any" and field_name in _OBJECT_SHAPED_FIELD_NAMES:
            py_type = "dict[str, Any]"
        py_field_name = py_field_for(field_name)
        alias_annotation = ""
        if py_field_name != field_name:
            # Optional fields need an explicit `default=None`; required
            # fields use `Field(alias=...)` without a default so pydantic
            # still enforces the missing-field check.
            if is_required:
                alias_annotation = f' = Field(alias={field_name!r})'
            else:
                alias_annotation = f' = Field(default=None, alias={field_name!r})'

        # If the type already includes "| None" (from nullable schema),
        # it needs a default even if required
        is_nullable = "| None" in py_type

        # Boolean schema (true = any value) has no dict methods
        prop_dict = prop if isinstance(prop, dict) else {}

        if alias_annotation:
            # Field with alias
            lines.append(f"    {py_field_name}: {py_type}{alias_annotation}")
        elif not is_required:
            default = prop_dict.get("default")
            if py_type == "bool":
                default_str = str(default) if default is not None else "False"
                lines.append(f"    {py_field_name}: {py_type} = {default_str}")
            elif default is not None:
                if isinstance(default, str):
                    lines.append(f"    {py_field_name}: {py_type} = {default!r}")
                elif isinstance(default, list):
                    lines.append(f"    {py_field_name}: {py_type} = []")
                elif isinstance(default, dict):
                    lines.append(f"    {py_field_name}: {py_type} = {{}}")
                else:
                    lines.append(f"    {py_field_name}: {py_type} = {default}")
            else:
                if not is_nullable:
                    py_type = make_optional(py_type)
                lines.append(f"    {py_field_name}: {py_type} = None")
        elif is_nullable:
            lines.append(f"    {py_field_name}: {py_type} = None")
        else:
            lines.append(f"    {py_field_name}: {py_type}")

    return "\n".join(lines)


def generate_tagged_union(
    name: str,
    doc: str,
    tag_field: str,
    params_field: str | None,
    accessors: dict[str, tuple[str, str]],
) -> str:
    """Generate a tagged union class with accessor methods."""
    lines = [f'class {name}(BaseModel):']
    lines.append(f'    """{doc}"""')
    lines.append("")
    lines.append(f"    {tag_field}: str")
    if params_field:
        lines.append(f"    {params_field}: dict[str, Any] = {{}}")

    for key, (method_name, return_type) in accessors.items():
        lines.append("")
        if params_field:
            lines.append(f"    def {method_name}(self) -> {return_type} | None:")
            lines.append(f"        if self.{tag_field} == {key!r}:")
            lines.append(f"            return {return_type}.model_validate(self.{params_field})")
            lines.append("        return None")
        else:
            # ThreadItem uses model_extra
            lines.append(f"    def {method_name}(self) -> {return_type} | None:")
            lines.append(f"        if self.{tag_field} == {key!r}:")
            lines.append(f"            return {return_type}.model_validate(self.model_extra or {{}})")
            lines.append("        return None")

    return "\n".join(lines)


def generate_thread_item() -> str:
    """Generate the ThreadItem class with extra-fields pattern."""
    lines = ['class ThreadItem(BaseModel):']
    lines.append('    """A discrete operation within a turn."""')
    lines.append("")
    lines.append("    id: str")
    lines.append("    type: str")
    lines.append('    model_config = {"extra": "allow"}')

    for key, (method_name, return_type) in THREAD_ITEM_ACCESSORS.items():
        lines.append("")
        lines.append(f"    def {method_name}(self) -> {return_type} | None:")
        lines.append(f"        if self.type == {key!r}:")
        lines.append(f"            return {return_type}.model_validate(self.model_extra or {{}})")
        lines.append("        return None")

    return "\n".join(lines)


def generate_client_request_wrapper(
    method: str, class_name: str, params_type: str
) -> str:
    """Generate a ClientRequest wrapper class."""
    lines = [f"class {class_name}(BaseModel):"]
    lines.append(f"    method: str = {method!r}")
    lines.append(f"    params: {class_name}Params")
    lines.append("")
    # Nested params class alias (re-exported at module level)
    lines.append(f"    class {class_name}Params({params_type}):")
    lines.append("        pass")
    return "\n".join(lines)


def is_enum_schema(schema: dict) -> bool:
    """Check if a schema defines a string enum.

    Handles both the schemars-tagged form (`oneOf: [{type: string, enum: [v]}]`)
    used for variant-typed enums and the plain form (`type: string, enum: [...]`)
    used for simple closed vocabularies like `ReasoningEffort`. schemars 1.x
    may emit single-value variants as ``const`` instead of ``enum: [v]``;
    accept either.
    """
    if "oneOf" in schema:
        return all(
            v.get("type") == "string" and has_enum_or_const(v) for v in schema["oneOf"]
        )
    return schema.get("type") == "string" and has_enum_or_const(schema)


def is_union_alias_schema(schema: dict) -> bool:
    """Check if a schema defines a top-level union type alias.

    Schemars emits `pub enum RequestId { Int(i64), String(String) }` as
    `{"anyOf": [{"type": "integer"}, {"type": "string"}]}` — no
    discriminator, no per-variant data. Python expresses this as a
    plain type alias: ``RequestId = int | str``.

    Returns False for **tagged unions** (`oneOf` variants that share a
    `const` discriminator field) — those flow through
    `is_tagged_oneof_schema` / `generate_tagged_oneof` so Python gets
    one Pydantic class per variant plus a discriminated `Annotated[Union[...]]`.
    """
    if schema.get("type") or is_enum_schema(schema):
        return False
    if is_tagged_oneof_schema(schema):
        return False
    variants = schema.get("anyOf") or schema.get("oneOf")
    if not variants:
        return False
    return all(
        isinstance(v, dict) and not has_enum_or_const(v) for v in variants
    )


def _sanitize_const_for_class_name(const: str) -> str:
    """Convert a wire-protocol const value into a Python PascalCase
    class-name suffix.

    Examples:
        `session/started` → `SessionStarted`
        `mcp/setServers` → `McpSetServers`
        `agentMessage/delta` → `AgentMessageDelta`
        `add_rules` → `AddRules`
        `tool-call` → `ToolCall`
        `reasoning-file` → `ReasoningFile`
    """
    # Split on `/`, `-`, `_`, then PascalCase each piece. CamelCase
    # pieces become PascalCase too (first letter upper, internals
    # preserved).
    pieces = re.split(r"[/_\-]+", const)
    result = []
    for piece in pieces:
        if not piece:
            continue
        result.append(piece[0].upper() + piece[1:])
    return "".join(result) or "Empty"


def _variant_is_pure_ref(v: dict) -> bool:
    """True when ``v`` carries nothing but a ``$ref`` (and an optional
    sibling ``description``). These variants point at an existing
    named def that already encodes everything about the variant
    shape; the codegen reuses the referenced class instead of
    synthesizing a new one."""
    return (
        isinstance(v, dict)
        and "$ref" in v
        and not (set(v.keys()) - {"$ref", "description"})
    )


def _tagged_oneof_discriminator(
    schema: dict, all_defs: dict | None = None
) -> str | None:
    """Return the discriminator field name if ``schema`` is a tagged
    `oneOf` worth promoting to a Pydantic discriminated union.

    Two accepted shapes:

    1. **Inline-variant** (e.g. ``ServerNotification`` /
       ``PermissionUpdate`` / ``AssistantContentPart``): every
       ``oneOf`` variant is an inline ``type: object`` schema with at
       least one string-``const`` property; the single shared
       property name is the discriminator. The codegen synthesizes
       one Pydantic class per variant via ``generate_tagged_oneof``.

    2. **Pure-``$ref`` variant** (e.g. ``HookInput`` after the
       ``generate_schemas.sh`` merge step lifts schemars's
       tag+flatten discriminator into the inner defs): every variant
       is just ``{$ref: <T>}``, and every referenced def has a
       shared string-``const`` property. The codegen emits a thin
       ``Annotated[Union[<existing classes>], Field(discriminator=...)]``
       and reuses the referenced classes — no synthetic wrappers.
       Requires ``all_defs`` for ref resolution; without it this
       branch is skipped and the function falls through to the
       inline check.

    The discriminator field name AND each const value can contain
    `/`, `-`, or `_` — those are sanitized for Python identifier use
    via [`_sanitize_const_for_class_name`]. The wire form is
    preserved through `Field(alias=...)`.
    """
    variants = schema.get("oneOf")
    if not variants or len(variants) < 2 or not all(isinstance(v, dict) for v in variants):
        return None

    # Shape 2: pure-$ref oneOf with shared const in the referenced defs.
    if all_defs is not None and all(_variant_is_pure_ref(v) for v in variants):
        const_keys_per_variant: list[set[str]] = []
        const_vals: list[str] = []
        for v in variants:
            ref_name = v["$ref"].rsplit("/", 1)[-1]
            target = all_defs.get(ref_name)
            if not target or target.get("type") != "object":
                return None
            props = target.get("properties") or {}
            keys = {
                k for k, p in props.items()
                if isinstance(p, dict) and isinstance(p.get("const"), str)
            }
            if not keys:
                return None
            const_keys_per_variant.append(keys)
        shared = set.intersection(*const_keys_per_variant)
        if len(shared) != 1:
            return None
        disc = next(iter(shared))
        if not disc or any(c.isspace() or c == "." for c in disc):
            return None
        for v in variants:
            ref_name = v["$ref"].rsplit("/", 1)[-1]
            cv = all_defs[ref_name]["properties"][disc]["const"]
            if not isinstance(cv, str) or cv in const_vals:
                return None
            const_vals.append(cv)
        return disc

    # Shape 1: inline variants carrying the const directly.
    const_keys_per_variant: list[set[str]] = []
    for v in variants:
        if v.get("type") != "object":
            return None
        # Pure $ref without inline const — leave as a plain Union alias
        # (the `AssistantContentPart` / `Message` / `AttachmentBody`
        # pattern, when shape 2 above didn't fire because all_defs is
        # absent or the refs lack a shared const).
        if "$ref" in v:
            return None
        props = v.get("properties") or {}
        keys = set()
        for key, prop in props.items():
            if isinstance(prop, dict) and isinstance(prop.get("const"), str):
                keys.add(key)
        if not keys:
            return None
        const_keys_per_variant.append(keys)
    shared = set.intersection(*const_keys_per_variant)
    if len(shared) != 1:
        return None
    disc = next(iter(shared))
    if not disc or any(c.isspace() or c == "." for c in disc):
        return None
    sanitized = set()
    for v in variants:
        const_val = v["properties"][disc]["const"]
        if not isinstance(const_val, str):
            return None
        sani = _sanitize_const_for_class_name(const_val)
        if not sani or sani in sanitized:
            return None
        sanitized.add(sani)
    return disc


def is_tagged_oneof_schema(schema: dict, all_defs: dict | None = None) -> bool:
    return _tagged_oneof_discriminator(schema, all_defs) is not None


def _py_field_name_for_discriminator(disc: str) -> str:
    """Translate a wire discriminator field name to its Python form.

    - `type` shadows Python's builtin, so emit as `type_` with
      `Field(alias='type')`. Pydantic discriminator accepts the
      Python field name.
    - `hookEventName` → `hook_event_name` (camelCase → snake_case).
    - `method` stays `method`.
    """
    if disc == "type":
        return "type_"
    # camelCase → snake_case
    s1 = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", disc)
    return re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s1).lower()


def generate_tagged_oneof(name: str, schema: dict, all_defs: dict) -> str:
    """Emit a discriminated union for ``schema``.

    Two paths, mirroring [`_tagged_oneof_discriminator`]'s two shapes:

    1. **Inline variants** — synthesize one Pydantic BaseModel per
       variant (with a `Literal[const]` discriminator field) plus
       the `Annotated[Union[...], Field(discriminator=...)]` alias.

    2. **Pure-`$ref` variants** — reuse the referenced classes
       directly (they already carry a `Literal[const]` discriminator
       field, injected by the `generate_schemas.sh` merge step).
       No new class blocks; just the `Annotated` alias. This avoids
       duplicating types like `PreToolUseInput` as
       `HookInputPreToolUse`.

    Pydantic v2 enforces the discriminator at validation time, so
    deserializing a wire object dispatches to the right variant
    automatically.
    """
    disc = _tagged_oneof_discriminator(schema, all_defs)
    assert disc is not None
    py_disc = _py_field_name_for_discriminator(disc)
    py_name = TYPE_RENAMES.get(name, name)
    variants_raw = schema["oneOf"]

    # Pure-$ref path: reuse referenced classes; the discriminator
    # field is already a `Literal[...]` on each (see the schema
    # normalization in `generate_schemas.sh`).
    if all(_variant_is_pure_ref(v) for v in variants_raw):
        members: list[str] = []
        for v in variants_raw:
            ref_name = v["$ref"].rsplit("/", 1)[-1]
            members.append(TYPE_RENAMES.get(ref_name, ref_name))
        union_body = ", ".join(members)
        return (
            f"{py_name} = Annotated[\n"
            f"    Union[{union_body}],\n"
            f"    Field(discriminator={py_disc!r}),\n"
            f"]"
        )

    variant_classes: list[tuple[str, str]] = []
    blocks: list[str] = []
    for variant in variants_raw:
        props = variant.get("properties") or {}
        const_value = props[disc]["const"]
        suffix = _sanitize_const_for_class_name(const_value)
        variant_class = f"{py_name}{suffix}"
        # Build a transient schema without the discriminator-const so
        # `generate_model` doesn't try to render the const as a regular
        # string field. Re-inject the discriminator below as a typed
        # `Literal[...]` with proper alias.
        non_disc_props = {k: v for k, v in props.items() if k != disc}
        variant_schema = {
            "type": "object",
            "properties": non_disc_props,
            "required": [k for k in variant.get("required", []) if k != disc],
            "description": variant.get("description"),
        }
        body = generate_model(variant_class, variant_schema, all_defs)
        # Discriminator field — always aliased so the Python name is
        # idiomatic snake_case while the wire stays whatever the
        # schema declared (camelCase / shadow-builtin / etc.).
        disc_field_line = (
            f"    {py_disc}: Literal[{const_value!r}] = "
            f"Field(default={const_value!r}, alias={disc!r})"
        )
        if body.endswith("\n    pass"):
            body = body[: -len("\n    pass")]
            body += "\n" + disc_field_line
        else:
            header, _, rest = body.partition("\n")
            body = f"{header}\n{disc_field_line}\n{rest}"
        body = body.replace(
            f"class {variant_class}(BaseModel):",
            f"class {variant_class}(BaseModel):\n"
            f'    model_config = {{"populate_by_name": True}}',
            1,
        )
        blocks.append(body)
        variant_classes.append((variant_class, const_value))
    union_body = ", ".join(v for v, _ in variant_classes)
    alias = (
        f"{py_name} = Annotated[\n"
        f"    Union[{union_body}],\n"
        f"    Field(discriminator={py_disc!r}),\n"
        f"]"
    )
    return "\n\n".join([*blocks, alias])


_BUILTIN_PY_TYPES = {
    "int", "str", "float", "bool", "bytes", "None", "Any",
    "list", "dict", "tuple", "set",
}


def _looks_like_class_ref(part: str) -> bool:
    """True if ``part`` contains an identifier that isn't a Python builtin.

    Distinguishes ``RequestId = int | str`` (all builtins, safe to emit as
    ``int | str``) from ``Message = UserMessage | AssistantMessage | ...``
    (class refs that may not be defined yet at the alias's emit point).
    """
    return any(
        token.isidentifier() and token not in _BUILTIN_PY_TYPES
        for token in re.findall(r"[A-Za-z_][A-Za-z0-9_]*", part)
    )


def generate_union_alias(name: str, schema: dict, defs: dict) -> str:
    """Emit a Python type alias from an anyOf schema.

    Aliases that reference user-defined classes are emitted as
    ``Union["A", "B", ...]`` so forward refs work — the alias section
    runs before the class section, and ``from __future__ import
    annotations`` defers *annotation* evaluation but not module-level
    assignments like ``X = A | B``. Pure-builtin aliases
    (``RequestId = int | str``) keep the ``|`` syntax.
    """
    parts: list[str] = []
    for variant in schema.get("anyOf") or schema.get("oneOf") or []:
        if variant == {"type": "null"}:
            parts.append("None")
        else:
            parts.append(schema_to_python_type(variant, True, defs))
    deduped_parts = list(dict.fromkeys(parts))
    if not deduped_parts:
        body = "Any"
    elif any(_looks_like_class_ref(p) for p in deduped_parts):
        body = "Union[" + ", ".join(
            "None" if p == "None" else f'"{p}"' for p in deduped_parts
        ) + "]"
    else:
        body = " | ".join(deduped_parts)
    py_name = TYPE_RENAMES.get(name, name)
    desc = schema.get("description", "").strip().splitlines()
    if desc:
        return f"# {desc[0][:120]}\n{py_name} = {body}"
    return f"{py_name} = {body}"


def _defs_of(schema: dict) -> dict:
    """Return the schema's referenceable subschemas.

    schemars 1.x (JSON Schema 2020-12) uses ``$defs`` by default;
    pre-1.x (draft-07) used ``definitions``. Fall back to the legacy
    key so this script keeps working if either side regresses.
    """
    return schema.get("$defs") or schema.get("definitions") or {}


def collect_definitions(schema_dir: Path) -> dict[str, dict]:
    """Collect all type definitions from all schema files."""
    defs: dict[str, dict] = {}
    # Read individual schemas first (more accurate)
    for path in sorted(schema_dir.glob("*.json")):
        if path.name == "coco_app_server_protocol.schemas.json":
            continue
        with open(path) as f:
            schema = json.load(f)
        for name, defn in _defs_of(schema).items():
            if name not in defs:
                defs[name] = defn

    # Also read the bundle for types only defined there
    # (e.g., hook input/output types, standalone config types).
    # Each bundle definition is a complete root schema for that type.
    bundle_path = schema_dir / "coco_app_server_protocol.schemas.json"
    if bundle_path.exists():
        with open(bundle_path) as f:
            bundle = json.load(f)
        for name, entry in _defs_of(bundle).items():
            if name in defs:
                continue
            # Bundle entries are full root schemas. We accept four
            # shapes downstream codegen knows how to handle:
            #   * `type: object` → Pydantic BaseModel
            #   * `oneOf: [...]` → tagged union (variant struct or
            #     descriptioned enum)
            #   * `type: string` + `enum: [...]` → flat str Enum
            #     (e.g. ProviderApi, WireApi — variants without
            #     per-variant doc comments)
            #   * `anyOf: [...]` (no `type`) → union type alias
            #     (e.g. `RequestId = int | str` from
            #     `pub enum RequestId { Int(i64), String(String) }`)
            if (
                entry.get("type") == "object"
                or "oneOf" in entry
                or is_enum_schema(entry)
                or is_union_alias_schema(entry)
            ):
                defs[name] = entry

    # Extract inline item types from ThreadItem oneOf variants.
    # These types (AgentMessageItem, ReasoningItem, etc.) are defined
    # inline in the schema, not in the definitions section.
    thread_item_path = schema_dir / "thread_item.json"
    if thread_item_path.exists():
        with open(thread_item_path) as f:
            ti_schema = json.load(f)
        for variant in ti_schema.get("oneOf", []):
            type_prop = variant.get("properties", {}).get("type", {})
            type_values = enum_values(type_prop)
            if not type_values:
                continue
            type_val = type_values[0]
            # Convert snake_case type to PascalCase + "Item"
            class_name = _type_to_item_class(type_val)
            if class_name and class_name not in defs:
                # Build a synthetic schema for this item type
                props = dict(variant.get("properties", {}))
                props.pop("type", None)  # Remove the discriminator
                props.pop("id", None)    # Remove the shared id field
                required = [
                    r for r in variant.get("required", [])
                    if r not in ("type", "id")
                ]
                defs[class_name] = {
                    "type": "object",
                    "properties": props,
                    "required": required,
                }
    return defs


# Map ThreadItem type discriminator values to Python class names
_ITEM_CLASS_MAP = {
    "agent_message": "AgentMessageItem",
    "reasoning": "ReasoningItem",
    "command_execution": "CommandExecutionItem",
    "file_change": "FileChangeItem",
    "mcp_tool_call": "McpToolCallItem",
    "web_search": "WebSearchItem",
    "subagent": "SubagentItem",
    "tool_call": "GenericToolCallItem",
    "error": "ErrorItem",
}


def _type_to_item_class(type_val: str) -> str | None:
    """Map a ThreadItem type discriminator to a Python class name."""
    return _ITEM_CLASS_MAP.get(type_val)


def extract_variants(schema: dict) -> list[dict]:
    """Extract full variant metadata from a wire-tagged-union schema.

    Each result dict has:
      - wire: wire-method string (e.g. "session/started")
      - description: human-readable doc from the variant
      - params_ref: `$ref`-resolved type name, or None
      - params_inline: inline params schema object, or None
      - has_params: whether the variant carries any params at all
    """
    variants: list[dict] = []
    for variant in schema.get("oneOf", []):
        props = variant.get("properties", {})
        method_vals = enum_values(props.get("method", {}))
        wire = method_vals[0] if method_vals else None
        if not wire:
            continue
        params = props.get("params")
        entry: dict = {
            "wire": wire,
            "description": variant.get("description", ""),
            "params_ref": None,
            "params_inline": None,
            "has_params": False,
        }
        if params is None:
            pass  # unit variant
        elif "$ref" in params:
            entry["params_ref"] = resolve_ref(params["$ref"])
            entry["has_params"] = True
        elif params.get("type") == "object":
            entry["params_inline"] = params
            entry["has_params"] = True
        variants.append(entry)
    return variants


def load_schema(schema_dir: Path, name: str) -> dict:
    """Read a single top-level schema file by name (without .json)."""
    with open(schema_dir / f"{name}.json") as f:
        return json.load(f)


def extract_notification_methods(schema_dir: Path) -> list[tuple[str, str]]:
    """Legacy extractor kept for back-compat: (method, params_ref) pairs for
    variants with external `$ref` params. Prefer `extract_variants()`."""
    variants = extract_variants(load_schema(schema_dir, "server_notification"))
    return [(v["wire"], v["params_ref"]) for v in variants if v["params_ref"]]


def extract_client_request_methods(schema_dir: Path) -> list[tuple[str, str]]:
    """Legacy extractor kept for back-compat; see `extract_variants`."""
    variants = extract_variants(load_schema(schema_dir, "client_request"))
    return [(v["wire"], v["params_ref"]) for v in variants if v["params_ref"]]


def _wire_words(wire: str) -> list[str]:
    """Split a wire string into lowercase component words.

        session/stateChanged    -> ["session", "state", "changed"]
        plan_approval/requested -> ["plan", "approval", "requested"]
        rateLimit               -> ["rate", "limit"]
        error                   -> ["error"]
    """
    s = wire.replace("/", "_")
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    return [part.lower() for part in s.split("_") if part]


def wire_to_enum_member(wire: str) -> str:
    """Wire string -> Python Enum member name (SCREAMING_SNAKE_CASE)."""
    return "_".join(_wire_words(wire)).upper()


def wire_to_accessor(wire: str) -> str:
    """Wire string -> `as_x_y_z` accessor method name."""
    return "as_" + "_".join(_wire_words(wire))


def _pascal_case(words: list[str]) -> str:
    return "".join(w[:1].upper() + w[1:] for w in words)


def _accessor_to_params_class(accessor: str) -> str:
    """`as_session_state_changed` -> `SessionStateChangedParams`."""
    base = accessor.removeprefix("as_")
    return _pascal_case(base.split("_")) + "Params"


def _derive_request_wrapper_name(variant: dict) -> str:
    """Derive the ClientRequest wrapper class name for a variant.

    Rule:
      1. If the variant has a `$ref` params type, strip `Params` from the
         type name and use that as the base.
      2. Otherwise (unit or inline) PascalCase the wire string with the
         leading `control/` prefix stripped.
      3. Append `Request` unless the base already ends in `Request` (avoids
         `CancelRequestRequest`).
    """
    params_ref = variant.get("params_ref")
    if params_ref:
        base = params_ref.removesuffix("Params")
    else:
        wire = variant["wire"]
        if wire.startswith("control/"):
            wire = wire[len("control/") :]
        base = _pascal_case(_wire_words(wire))
    if not base.endswith("Request"):
        base = base + "Request"
    return base


def derive_accessors(variants: list[dict]) -> dict[str, tuple[str, str]]:
    """Build `{wire: (accessor_name, params_class)}` from variant metadata.

    Variants without params still get accessor entries pointing at a
    synthesised empty Pydantic model (so the `as_x() -> Params | None`
    idiom works uniformly). Renames in `TYPE_RENAMES` are applied to the
    `$ref`-based class names.
    """
    result: dict[str, tuple[str, str]] = {}
    for v in variants:
        wire = v["wire"]
        accessor = wire_to_accessor(wire)
        if v["params_ref"]:
            params_class = TYPE_RENAMES.get(v["params_ref"], v["params_ref"])
        else:
            params_class = _accessor_to_params_class(accessor)
        result[wire] = (accessor, params_class)
    return result


def generate_inline_params_models(
    variants: list[dict],
    all_defs: dict,
    emitted: set[str],
) -> str:
    """Emit synthesized Pydantic models for inline-params / unit variants.

    `emitted` tracks class names already produced this run to avoid
    duplicates when a synthesized name collides with an existing type.
    """
    blocks: list[str] = []
    for v in variants:
        if v["params_ref"]:
            continue
        class_name = _accessor_to_params_class(wire_to_accessor(v["wire"]))
        if class_name in emitted or class_name in all_defs:
            continue
        emitted.add(class_name)
        if v["params_inline"]:
            blocks.append(generate_model(class_name, v["params_inline"], all_defs))
        else:
            # Unit variant — empty params model with `extra='allow'` so it
            # round-trips any stray fields without validation errors.
            blocks.append(
                textwrap.dedent(
                    f'''\
                    class {class_name}(BaseModel):
                        """Empty params for the wire-method `{v["wire"]}`."""

                        model_config = {{"extra": "allow"}}
                    '''
                ).rstrip()
            )
        blocks.append("")
    return "\n".join(blocks)


def _method_enum_block(
    schema_dir: Path,
    schema_name: str,
    class_name: str,
    doc: str,
) -> str:
    """Emit `class X(str, Enum)` from a method-enum schema file."""
    schema = load_schema(schema_dir, schema_name)
    lines = [f"class {class_name}(str, Enum):", f'    """{doc}"""', ""]
    for wire in schema.get("enum", []):
        lines.append(f"    {wire_to_enum_member(wire)} = {wire!r}")
    return "\n".join(lines)


def generate_notification_method_enum(schema_dir: Path) -> str:
    return _method_enum_block(
        schema_dir,
        "notification_method",
        "NotificationMethod",
        "Wire-method identifier for every `ServerNotification` variant. "
        "Mirrors the Rust `NotificationMethod` enum. Members inherit from "
        "`str`, so equality with raw wire strings Just Works.",
    )


def generate_client_request_method_enum(schema_dir: Path) -> str:
    return _method_enum_block(
        schema_dir,
        "client_request_method",
        "ClientRequestMethod",
        "Wire-method identifier for every `ClientRequest` variant. "
        "Mirrors the Rust `ClientRequestMethod` enum.",
    )


def generate_server_request_method_enum(schema_dir: Path) -> str:
    return _method_enum_block(
        schema_dir,
        "server_request_method",
        "ServerRequestMethod",
        "Wire-method identifier for every `ServerRequest` variant. "
        "Mirrors the Rust `ServerRequestMethod` enum.",
    )


def main() -> None:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <schema_dir> <output_file>", file=sys.stderr)
        sys.exit(1)

    schema_dir = Path(sys.argv[1])
    output_path = Path(sys.argv[2])

    # Collect all definitions
    all_defs = collect_definitions(schema_dir)

    # Types handled explicitly (skip from generic generation).
    #
    # `ServerNotification` / `ServerRequest` / `ClientRequest` USED to
    # live here so we could emit a wrapper class with `as_X()` accessors
    # and `params: dict[str, Any]`. They're now removed: each one is a
    # tagged `oneOf` (discriminator `method`) and the auto-detection
    # path emits them as proper Pydantic discriminated unions with one
    # typed variant class per method. The wrapper-with-accessor codegen
    # is deleted further down.
    explicitly_handled = {
        "ThreadItem", "ThreadItemDetails",
        # MCP config types handled manually
        "McpServerConfig",
        # Method enums emitted by the dedicated `generate_*_method_enum`
        # helpers near the bottom of the file. Without this skip, the
        # generic enum loop would also emit them — producing two `class X`
        # definitions per method enum and silently shadowing the first.
        "ClientRequestMethod", "ServerRequestMethod", "NotificationMethod",
    }

    # Classify types
    enum_names: set[str] = set()
    model_names: set[str] = set()
    union_alias_names: set[str] = set()
    tagged_oneof_names: set[str] = set()
    # Scalar newtype aliases — Rust `#[serde(transparent)] struct X(T)`
    # where T is a primitive. Schema is `{"type": "<scalar>"}` with no
    # `enum` / `properties` / `oneOf`. Without explicit handling these
    # would silently disappear from the generated module and any class
    # referencing them by name fails Pydantic forward-ref resolution
    # at module import. Each becomes `Name = <pytype>` so the wire
    # shape (transparent passthrough) is preserved.
    scalar_alias_names: dict[str, str] = {}
    _SCALAR_PY_TYPE = {
        "string": "str",
        "integer": "int",
        "number": "float",
        "boolean": "bool",
    }
    for name, defn in all_defs.items():
        if name in SKIP_TYPES or name in explicitly_handled:
            continue
        if is_enum_schema(defn):
            enum_names.add(name)
        elif is_tagged_oneof_schema(defn, all_defs):
            tagged_oneof_names.add(name)
        elif is_union_alias_schema(defn):
            union_alias_names.add(name)
        elif defn.get("type") == "object":
            model_names.add(name)
        elif (
            isinstance(defn.get("type"), str)
            and defn["type"] in _SCALAR_PY_TYPE
            and "enum" not in defn
            and "properties" not in defn
            and "oneOf" not in defn
        ):
            scalar_alias_names[name] = _SCALAR_PY_TYPE[defn["type"]]

    ENUM_TYPES.update(enum_names)

    # Determine generation order based on dependencies
    # Generate: enums first, then models, then tagged unions, then wrappers
    sections: list[str] = []

    # Header
    sections.append(textwrap.dedent('''\
        """Generated protocol types for the coco SDK.

        These types mirror the Rust `coco-app-server-protocol` crate.
        Regenerate with: `scripts/generate_python.sh`

        Source schemas: coco-rs/app-server-protocol/schema/json/

        DO NOT EDIT MANUALLY — changes will be overwritten by the generator.
        """

        from __future__ import annotations

        from enum import Enum
        from typing import Annotated, Any, Literal, Union

        from pydantic import BaseModel, Field
    '''))

    # ── Section: Usage ──
    sections.append("# " + "-" * 75)
    sections.append("# Usage")
    sections.append("# " + "-" * 75)
    sections.append("")
    if "Usage" in all_defs:
        sections.append(generate_model("Usage", all_defs["Usage"], all_defs))
        model_names.discard("Usage")
    sections.append("")

    # ── Section: Scalar newtype aliases ──
    # Emit BEFORE any model class references them. Schema `type:
    # string|integer|number|boolean` without further constraints is a
    # transparent Rust newtype (`#[serde(transparent)]`); Python
    # mirrors with a plain type alias.
    if scalar_alias_names:
        sections.append("# " + "-" * 75)
        sections.append("# Scalar newtype aliases (transparent Rust newtypes)")
        sections.append("# " + "-" * 75)
        sections.append("")
        for name in sorted(scalar_alias_names):
            py_type = scalar_alias_names[name]
            sections.append(f"{name} = {py_type}")
        sections.append("")
        sections.append("")

    # ── Section: Enums ──
    sections.append("# " + "-" * 75)
    sections.append("# Enums")
    sections.append("# " + "-" * 75)
    sections.append("")
    for name in sorted(enum_names):
        sections.append(generate_enum(name, all_defs[name]))
        sections.append("")
    sections.append("")

    # ── Section: Union type aliases ──
    # `pub enum X { Int(i64), String(String) }` — schemars emits
    # `{anyOf: [{type:integer},{type:string}]}`; Python expresses
    # this as a flat type alias (`X = int | str`). Must be emitted
    # before any model that references it.
    if union_alias_names:
        sections.append("# " + "-" * 75)
        sections.append("# Union type aliases")
        sections.append("# " + "-" * 75)
        sections.append("")
        for name in sorted(union_alias_names):
            sections.append(generate_union_alias(name, all_defs[name], all_defs))
            sections.append("")
        sections.append("")

    # ── Section: Tagged discriminated unions ──
    # `oneOf` schemas whose variants share a `const`-discriminator
    # field (e.g. `HookSpecificOutput` discriminating on
    # `hookEventName`). Emitted as one Pydantic class per variant
    # plus an `Annotated[Union[...], Field(discriminator=...)]` alias
    # so deserialization dispatches typed automatically.
    #
    # Pure-`$ref` tagged unions (e.g. `HookInput`, whose variants
    # point at existing top-level classes like `PreToolUseInput`)
    # are emitted in a later section instead — the referenced
    # classes must be defined first.
    deferred_ref_tagged = {
        name for name in tagged_oneof_names
        if all(
            _variant_is_pure_ref(v)
            for v in (all_defs[name].get("oneOf") or [])
        )
    }
    inline_tagged_oneof_names = tagged_oneof_names - deferred_ref_tagged
    if inline_tagged_oneof_names:
        sections.append("# " + "-" * 75)
        sections.append("# Tagged discriminated unions")
        sections.append("# " + "-" * 75)
        sections.append("")
        for name in sorted(inline_tagged_oneof_names):
            sections.append(generate_tagged_oneof(name, all_defs[name], all_defs))
            sections.append("")
        sections.append("")

    # ── Section: Item types ──
    item_types = [
        "AgentMessageItem", "ReasoningItem", "CommandExecutionItem",
        "FileChange", "FileChangeKind", "FileChangeItem",
        "McpToolCallResult", "McpToolCallError", "McpToolCallItem",
        "WebSearchItem", "SubagentItem", "GenericToolCallItem", "ErrorItem",
    ]
    sections.append("# " + "-" * 75)
    sections.append("# Item types")
    sections.append("# " + "-" * 75)
    sections.append("")
    for name in item_types:
        if name in all_defs:
            if name in enum_names:
                continue  # Already generated
            sections.append(generate_model(name, all_defs[name], all_defs))
            sections.append("")
            model_names.discard(name)
    sections.append("")

    # ── Section: ThreadItem ──
    sections.append("# " + "-" * 75)
    sections.append("# ThreadItem (tagged union with extra fields)")
    sections.append("# " + "-" * 75)
    sections.append("")
    sections.append(generate_thread_item())
    sections.append("")
    sections.append("")

    # Parse all three wire-tagged unions once so we can derive params models,
    # accessor maps, and wrapper classes from the same variant lists.
    notif_variants = extract_variants(load_schema(schema_dir, "server_notification"))
    sr_variants = extract_variants(load_schema(schema_dir, "server_request"))
    cr_variants = extract_variants(load_schema(schema_dir, "client_request"))

    notif_param_types = {v["params_ref"] for v in notif_variants if v["params_ref"]}
    sr_param_types = {v["params_ref"] for v in sr_variants if v["params_ref"]}
    emitted_synth: set[str] = set()
    generated_params: set[str] = set()

    def emit_ref_params(types: set[str]) -> None:
        for params_type in sorted(types):
            if params_type in generated_params:
                continue
            generated_params.add(params_type)
            if params_type in all_defs and params_type not in enum_names:
                sections.append(generate_model(params_type, all_defs[params_type], all_defs))
                sections.append("")
                model_names.discard(params_type)

    # ── Section: Notification params ──
    sections.append("# " + "-" * 75)
    sections.append("# Server notification params")
    sections.append("# " + "-" * 75)
    sections.append("")
    emit_ref_params(notif_param_types)
    synth = generate_inline_params_models(notif_variants, all_defs, emitted_synth)
    if synth:
        sections.append(synth)
    sections.append("")

    # ── Section: NotificationMethod enum (protocol constants) ──
    sections.append("# " + "-" * 75)
    sections.append("# Notification wire-method constants")
    sections.append("# " + "-" * 75)
    sections.append("")
    sections.append(generate_notification_method_enum(schema_dir))
    sections.append("")
    sections.append("")

    # `ServerNotification` is now emitted by the **tagged discriminated
    # union** section above (auto-detected from the `oneOf` schema with
    # `method` as the const-discriminator). Each variant is a typed
    # `ServerNotificationSessionStarted(method=Literal[...], params: SessionStartedParams)`
    # — no more `params: dict[str, Any]`, no more `as_X()` accessors.

    # ── Section: Server request params ──
    sections.append("# " + "-" * 75)
    sections.append("# Server request param types")
    sections.append("# " + "-" * 75)
    sections.append("")
    emit_ref_params(sr_param_types)
    synth = generate_inline_params_models(sr_variants, all_defs, emitted_synth)
    if synth:
        sections.append(synth)
    sections.append("")
    sections.append(generate_server_request_method_enum(schema_dir))
    sections.append("")
    sections.append("")
    # `ServerRequest` is also emitted as a typed discriminated union by
    # the auto-detection path above.

    # ── Section: MCP server config (special: tagged union with 'type' field) ──
    # These must come BEFORE client request params since SessionStartRequestParams
    # references AgentDefinitionConfig, McpServerConfig, etc.
    sections.append("# " + "-" * 75)
    sections.append("# MCP server config types")
    sections.append("# " + "-" * 75)
    sections.append("")
    sections.append(textwrap.dedent('''\
        class StdioMcpServerConfig(BaseModel):
            """Subprocess-based MCP server (stdio transport)."""

            type: str = "stdio"
            command: str
            args: list[str] = []
            env: dict[str, str] | None = None


        class SseMcpServerConfig(BaseModel):
            """SSE-based MCP server."""

            type: str = "sse"
            url: str


        class HttpMcpServerConfig(BaseModel):
            """HTTP-based MCP server."""

            type: str = "http"
            url: str


        McpServerConfig = StdioMcpServerConfig | SseMcpServerConfig | HttpMcpServerConfig
    '''))

    # ── Section: Remaining config types ──
    remaining_configs = [
        "AgentHookConfig", "AgentDefinitionConfig", "HookCallbackConfig",
        "SandboxConfig", "ThinkingConfig", "OutputFormatConfig",
        "SystemPromptConfig", "ToolsConfig", "HookMatcherConfig",
        "CommandInfo",
        # Initialize / session management result types
        "ClientInfo", "InitializeCapabilities",
        "SessionSummary",
    ]
    sections.append("# " + "-" * 75)
    sections.append("# Config types")
    sections.append("# " + "-" * 75)
    sections.append("")
    for name in remaining_configs:
        if name in all_defs and name not in enum_names and name in model_names:
            sections.append(generate_model(name, all_defs[name], all_defs))
            sections.append("")
            model_names.discard(name)

    # Union-type aliases (oneOf types that aren't enums or objects)
    # These need to be defined as `Any` since they're complex unions
    # (e.g., SystemPromptConfig = str | {preset, append}).
    union_types = [
        "SystemPromptConfig",
        "ToolsConfig",
        "ErrorInfo",
        # PermissionUpdate is a oneOf tagged union of action objects; no
        # Pydantic-class generation yet, fall back to `Any` so ApprovalResolve
        # request params resolve without a forward-ref error.
        "PermissionUpdate",
    ]
    for name in union_types:
        if (
            name in all_defs
            and name not in model_names
            and name not in enum_names
            and name not in union_alias_names
            and name not in tagged_oneof_names
        ):
            sections.append(f"# Union type: see Rust source for variants")
            sections.append(f"{name} = Any")
            sections.append("")
    sections.append("")

    # ── Section: Hook input/output types ──
    hook_types = [
        "PreToolUseHookInput", "PostToolUseHookInput", "PostToolUseFailureHookInput",
        "HookCallbackOutput",
        "StopHookInput", "SubagentStartHookInput", "SubagentStopHookInput",
        "UserPromptSubmitHookInput", "NotificationHookInput",
        "PreCompactHookInput", "PermissionRequestHookInput",
        "SessionStartHookInput", "SessionEndHookInput",
    ]
    sections.append("# " + "-" * 75)
    sections.append("# Hook input/output types")
    sections.append("# " + "-" * 75)
    sections.append("")
    for name in hook_types:
        if name in all_defs and name not in enum_names and name in model_names:
            sections.append(generate_model(name, all_defs[name], all_defs))
            sections.append("")
            model_names.discard(name)
    sections.append("")

    # ── Section: Client request params ──
    cr_param_types = {v["params_ref"] for v in cr_variants if v["params_ref"]}
    sections.append("# " + "-" * 75)
    sections.append("# Client request params")
    sections.append("# " + "-" * 75)
    sections.append("")
    for params_type in sorted(cr_param_types):
        if params_type in generated_params:
            continue
        generated_params.add(params_type)
        if params_type in all_defs and params_type not in enum_names:
            if params_type not in notif_param_types and params_type not in sr_param_types:
                sections.append(generate_model(params_type, all_defs[params_type], all_defs))
                sections.append("")
                model_names.discard(params_type)
    sections.append("")

    # ── Section: ClientRequestMethod enum (protocol constants) ──
    sections.append("# " + "-" * 75)
    sections.append("# Client request wire-method constants")
    sections.append("# " + "-" * 75)
    sections.append("")
    sections.append(generate_client_request_method_enum(schema_dir))
    sections.append("")
    sections.append("")

    # ── Section: Client request wrappers (auto-derived from schema) ──
    #
    # Each variant of `ClientRequest` gets its own typed BaseModel:
    # `InitializeRequest`, `SessionStartRequest`, etc. The `method`
    # field is `Literal['initialize']` (not bare `str`) so the union
    # at the bottom can dispatch via Pydantic's discriminator. Params
    # are typed via the dedicated params struct.
    sections.append("# " + "-" * 75)
    sections.append("# Client request wrappers (one Pydantic class per variant)")
    sections.append("# " + "-" * 75)
    sections.append("")
    cr_variant_classes: list[str] = []
    for variant in cr_variants:
        method = variant["wire"]
        class_name = _derive_request_wrapper_name(variant)
        cr_variant_classes.append(class_name)
        params_ref = variant.get("params_ref")
        py_params = TYPE_RENAMES.get(params_ref, params_ref) if params_ref else None
        lines = [
            f"class {class_name}(BaseModel):",
            '    model_config = {"populate_by_name": True}',
            f"    method: Literal[{method!r}] = Field(default={method!r})",
            f"    params: {class_name}Params",
            "",
        ]
        if py_params:
            lines.append(f"    class {class_name}Params({py_params}):")
            lines.append("        pass")
        else:
            # Unit variant — keep the nested Params class for caller API
            # symmetry; `extra='allow'` tolerates any stray fields.
            lines.append(f"    class {class_name}Params(BaseModel):")
            lines.append('        model_config = {"extra": "allow"}')
        sections.append("\n".join(lines))
        sections.append("")
        sections.append(f"{class_name}Params = {class_name}.{class_name}Params")
        sections.append("")

    # `ClientRequest` discriminated union — inbound parsing dispatches
    # to the typed wrapper above via the `method` field.
    union_members = ", ".join(cr_variant_classes)
    sections.append(
        f"ClientRequest = Annotated[\n"
        f"    Union[{union_members}],\n"
        f"    Field(discriminator='method'),\n"
        f"]"
    )
    sections.append("")

    # ── Section: Remaining types (PermissionSuggestion, etc.) ──
    if model_names:
        sections.append("# " + "-" * 75)
        sections.append("# Additional types")
        sections.append("# " + "-" * 75)
        sections.append("")
        for name in sorted(model_names):
            if name in all_defs:
                sections.append(generate_model(name, all_defs[name], all_defs))
                sections.append("")

    # ── Section: Deferred ref-based tagged unions ──
    # Pure-`$ref` tagged unions like `HookInput` must be emitted
    # AFTER the model section so all referenced classes
    # (`PreToolUseInput`, `PostToolUseInput`, …) are already in
    # scope at evaluation time. `Annotated[Union[...], Field(...)]`
    # evaluates its arguments eagerly, so forward-ref strings won't
    # work here (Pydantic needs the real classes for discriminator
    # dispatch setup).
    if deferred_ref_tagged:
        sections.append("# " + "-" * 75)
        sections.append("# Tagged discriminated unions (ref-based)")
        sections.append("# " + "-" * 75)
        sections.append("")
        for name in sorted(deferred_ref_tagged):
            sections.append(generate_tagged_oneof(name, all_defs[name], all_defs))
            sections.append("")

    # Schema-driven derivation is exhaustive by construction, so the prior
    # hand-maintained dicts no longer exist. Keep a lightweight summary
    # print for visibility.
    validation_errors: list[str] = []

    if validation_errors:
        # Warn, don't fail. The accessor maps are hand-written and may
        # lag the Rust schema; missing methods just mean the Python side
        # won't get convenient `as_X()` accessors for those variants,
        # but the underlying Pydantic models are still generated correctly.
        print("VALIDATION WARNINGS:", file=sys.stderr)
        for err in validation_errors:
            print(f"  - {err}", file=sys.stderr)
        print(
            "  (continuing anyway — accessor maps lag the Rust schema; "
            "update them in postprocess_python.py if you need typed "
            "accessors for the missing methods)",
            file=sys.stderr,
        )

    print(
        f"Generated: {len(notif_variants)} notifications, "
        f"{len(sr_variants)} server requests, "
        f"{len(cr_variants)} client requests"
    )

    # ── Section: forward-ref resolution ──
    # Pydantic v2 `TypeAdapter(<discriminated union>)` constructs its
    # validator eagerly at module-import time. If any variant class
    # references a Pydantic model that's defined LATER in the file
    # (`from __future__ import annotations` defers annotation
    # evaluation but not the TypeAdapter construction), Pydantic
    # raises `class-not-fully-defined` on the first `.validate_python`
    # call. The fix is to walk every BaseModel-subclass we emitted and
    # call `model_rebuild()` so each class has its forward refs
    # resolved against the now-complete module namespace.
    #
    # Done unconditionally as a tail section so any future class
    # introduced by the generator is auto-rebuilt. Errors are
    # swallowed: classes without forward refs (the common case)
    # raise `PydanticUserError("already-defined")`, which is harmless.
    sections.append(
        "\n# ── Resolve forward refs for every emitted BaseModel ──\n"
        "# Pydantic v2's TypeAdapter (used in discriminated unions)\n"
        "# constructs validators eagerly; classes that reference\n"
        "# later-defined models would error on first validation\n"
        "# without an explicit rebuild pass.\n"
        "import sys as _sys\n"
        "for _name in list(globals()):\n"
        "    _obj = globals()[_name]\n"
        "    if isinstance(_obj, type) and issubclass(_obj, BaseModel):\n"
        "        try:\n"
        "            _obj.model_rebuild()\n"
        "        except Exception:\n"
        "            pass\n"
        "del _name, _obj\n"
    )

    # Write output
    content = "\n".join(sections)
    # Clean up triple+ blank lines
    while "\n\n\n\n" in content:
        content = content.replace("\n\n\n\n", "\n\n\n")
    output_path.write_text(content)
    print(f"Generated: {output_path} ({len(content)} bytes)")


if __name__ == "__main__":
    main()
