#!/usr/bin/env bash
# Generate JSON Schema files from coco-rs protocol types.
#
# Builds the `export_schema` example under the `schema` feature in
# coco-types, then runs the resulting binary. Output lands in
# `coco-sdk/schemas/json/`.
#
# Usage:
#   ./coco-sdk/scripts/generate_schemas.sh              # regenerate (skip if up-to-date)
#   ./coco-sdk/scripts/generate_schemas.sh --check      # exit 1 if regen would change anything
#   ./coco-sdk/scripts/generate_schemas.sh --force      # regenerate unconditionally
#   ./coco-sdk/scripts/generate_schemas.sh --quiet      # suppress per-file progress
#
# `--check` is the CI mode: it runs the generator into a tempdir and
# diffs against the committed bundle. Exits 0 if identical, 1 + a diff
# summary if not. Does NOT modify the working tree.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CARGO_MANIFEST="$REPO_ROOT/coco-rs/common/types/Cargo.toml"
HOOK_CARGO_MANIFEST="$REPO_ROOT/coco-rs/hooks/Cargo.toml"
SCHEMA_DIR="$REPO_ROOT/coco-sdk/schemas/json"
BUNDLE="$SCHEMA_DIR/coco_app_server_protocol.schemas.json"
HOOK_INPUT_FILE="$SCHEMA_DIR/hook_input.json"

CHECK_MODE=false
FORCE_MODE=false
QUIET_MODE=false

for arg in "$@"; do
    case "$arg" in
        --check) CHECK_MODE=true ;;
        --force) FORCE_MODE=true ;;
        --quiet|-q) QUIET_MODE=true ;;
        -h|--help)
            sed -n '2,17p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown flag '$arg' (use --help)" >&2
            exit 2
            ;;
    esac
done

log() {
    if ! $QUIET_MODE; then
        echo "$@"
    fi
}

# ---------------------------------------------------------------------------
# Skip regen when nothing has changed
# ---------------------------------------------------------------------------
#
# The schema is fully derived from `coco-types` source + the example
# itself. If every input is older than the bundle, the cargo invocation
# would be wasted work — skip unless `--force` or `--check`.
needs_regen() {
    [[ ! -f "$BUNDLE" ]] && return 0
    local newer
    newer=$(find \
        "$REPO_ROOT/coco-rs/common/types/src" \
        "$REPO_ROOT/coco-rs/common/types/examples" \
        "$REPO_ROOT/coco-rs/common/types/Cargo.toml" \
        "$REPO_ROOT/coco-rs/hooks/src/inputs.rs" \
        "$REPO_ROOT/coco-rs/hooks/examples" \
        "$REPO_ROOT/coco-rs/hooks/Cargo.toml" \
        -newer "$BUNDLE" -print -quit 2>/dev/null || true)
    [[ -n "$newer" ]]
}

if ! $FORCE_MODE && ! $CHECK_MODE && ! needs_regen; then
    log "==> Schemas already up-to-date (use --force to regenerate)."
    exit 0
fi

# ---------------------------------------------------------------------------
# Build the example once, then exec. Splits build vs. run so warm-cache
# invocations don't reprint the cargo build banner. `--quiet` suppresses
# the "Compiling …" lines on cold builds; errors still surface.
# ---------------------------------------------------------------------------
log "==> Building export_schema + export_hook_input_schema (debug profile)..."
BUILD_FLAGS=(--manifest-path "$CARGO_MANIFEST" -p coco-types --features schema --example export_schema)
HOOK_BUILD_FLAGS=(--manifest-path "$HOOK_CARGO_MANIFEST" -p coco-hooks --features schema --example export_hook_input_schema)
if $QUIET_MODE; then
    cargo build "${BUILD_FLAGS[@]}" --quiet
    cargo build "${HOOK_BUILD_FLAGS[@]}" --quiet
else
    cargo build "${BUILD_FLAGS[@]}"
    cargo build "${HOOK_BUILD_FLAGS[@]}"
fi

TARGET_DIR="$(cargo metadata --manifest-path "$CARGO_MANIFEST" --format-version 1 \
    | python3 -c 'import json,sys;m=json.load(sys.stdin);print(m["target_directory"])')"
EXAMPLE_BIN="$TARGET_DIR/debug/examples/export_schema"
HOOK_EXAMPLE_BIN="$TARGET_DIR/debug/examples/export_hook_input_schema"

if [[ ! -x "$EXAMPLE_BIN" ]]; then
    echo "error: built binary not found at $EXAMPLE_BIN" >&2
    exit 1
fi
if [[ ! -x "$HOOK_EXAMPLE_BIN" ]]; then
    echo "error: built binary not found at $HOOK_EXAMPLE_BIN" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Merge `hook_input.json` (from coco-hooks) into the protocol bundle so
# the SDK schema is self-contained. Patches `HookCallbackParams.input`
# to `{$ref: "#/$defs/HookInput"}`. `coco-types` cannot import
# `coco-hooks` (architectural layering), so the merge happens at the
# script level rather than at schema-generation time inside Rust.
# ---------------------------------------------------------------------------
merge_hook_input_into_bundle() {
    local out_dir="$1"
    python3 - "$out_dir" <<'PY'
import json
import sys
from pathlib import Path

out_dir = Path(sys.argv[1])
bundle_path = out_dir / "coco_app_server_protocol.schemas.json"
server_request_path = out_dir / "server_request.json"
hook_path = out_dir / "hook_input.json"

hook = json.loads(hook_path.read_text())
hook_top = {k: v for k, v in hook.items() if k not in ("$schema", "$defs")}
hook_defs = hook.get("$defs") or {}


def normalize_hook_input_discriminator(hi_schema: dict, defs: dict) -> None:
    """Push the tag+flatten oneOf discriminator (hook_event_name) down
    into each referenced inner def.

    Schemars's tag+flatten emission for ``#[serde(tag = "X")]`` on an
    enum whose variants embed a struct via ``#[serde(flatten)]``
    produces variants shaped like::

        {type: object, properties: {<disc>: {const: <v>}}, $ref: <T>, required: [<disc>]}

    — the discriminator lives on the variant wrapper, not on the
    referenced struct. Downstream codegen (Pydantic ``Literal``
    discriminator field, TS native discriminated-union narrowing)
    wants the const directly on the variant class so each generated
    inner-struct class carries a typed discriminator. Lift it once
    here so the SDK schema is consumer-friendly across languages.
    """
    if not hi_schema or not isinstance(hi_schema.get("oneOf"), list):
        return
    new_variants = []
    for variant in hi_schema["oneOf"]:
        ref = variant.get("$ref", "")
        props = variant.get("properties") or {}
        const_props = {
            k: p for k, p in props.items()
            if isinstance(p, dict) and "const" in p
        }
        if not ref or len(const_props) != 1:
            new_variants.append(variant)
            continue
        ref_name = ref.rsplit("/", 1)[-1]
        target = defs.get(ref_name)
        if not target:
            new_variants.append(variant)
            continue
        disc_name = next(iter(const_props))
        target_props = target.setdefault("properties", {})
        target_props[disc_name] = const_props[disc_name]
        required = target.setdefault("required", [])
        if disc_name not in required:
            required.insert(0, disc_name)
        new_variants.append({"$ref": ref})
    hi_schema["oneOf"] = new_variants


def patch_schema_doc(path: Path) -> None:
    """Inject HookInput + its $defs into ``path``'s ``$defs`` and
    rewrite ``HookCallbackParams.input`` to ``{$ref: HookInput}``.
    `coco-types` cannot import `coco-hooks` (architectural layering),
    so the HookInput shape is bolted on at the script level rather
    than emitted by the Rust schemars run."""
    if not path.exists():
        return
    doc = json.loads(path.read_text())
    defs = doc.setdefault("$defs", {})
    # Variant definitions: bundle wins on conflict (HookEventType /
    # ToolName / PermissionUpdate already live there).
    for name, schema in hook_defs.items():
        defs.setdefault(name, schema)
    defs["HookInput"] = hook_top

    hcp = defs.get("HookCallbackParams")
    if hcp and "properties" in hcp and "input" in hcp["properties"]:
        description = hcp["properties"]["input"].get("description")
        new_prop = {"$ref": "#/$defs/HookInput"}
        if description:
            new_prop["description"] = description
        hcp["properties"]["input"] = new_prop

    normalize_hook_input_discriminator(defs.get("HookInput"), defs)
    path.write_text(json.dumps(doc, indent=2) + "\n")


def patch_hook_input_file(path: Path) -> None:
    """Apply the discriminator-lifting transform in place on
    ``hook_input.json``. Unlike the bundle, ``HookInput`` is the
    root schema here (not a ``$defs`` entry), but the inner Input
    structs live in this file's ``$defs`` — and Python codegen reads
    them from this file first via ``collect_definitions``."""
    if not path.exists():
        return
    doc = json.loads(path.read_text())
    defs = doc.get("$defs") or {}
    normalize_hook_input_discriminator(doc, defs)
    path.write_text(json.dumps(doc, indent=2) + "\n")


# Patch the bundle (single source of truth across schemas), server_request.json
# (codegen reads its individual schemas via `collect_definitions` first),
# AND hook_input.json (collect_definitions reads its $defs first too, so
# inner-struct shapes there need the discriminator injection as well).
patch_schema_doc(bundle_path)
patch_schema_doc(server_request_path)
patch_hook_input_file(hook_path)
PY
}

# ---------------------------------------------------------------------------
# --check: run into a tempdir, diff the bundle, never touch the tree.
# ---------------------------------------------------------------------------
if $CHECK_MODE; then
    TMP_OUT="$(mktemp -d)"
    trap 'rm -rf "$TMP_OUT"' EXIT
    log "==> Running export_schema + export_hook_input_schema into $TMP_OUT..."
    if $QUIET_MODE; then
        "$EXAMPLE_BIN" "$TMP_OUT" > /dev/null
        "$HOOK_EXAMPLE_BIN" "$TMP_OUT" > /dev/null
    else
        "$EXAMPLE_BIN" "$TMP_OUT"
        "$HOOK_EXAMPLE_BIN" "$TMP_OUT"
    fi
    merge_hook_input_into_bundle "$TMP_OUT"
    if diff -q "$BUNDLE" "$TMP_OUT/coco_app_server_protocol.schemas.json" > /dev/null 2>&1 \
        && diff -q "$HOOK_INPUT_FILE" "$TMP_OUT/hook_input.json" > /dev/null 2>&1; then
        log "==> OK: schemas are up-to-date."
        exit 0
    fi
    echo "ERROR: schemas are out of date. Run:" >&2
    echo "    ./coco-sdk/scripts/generate_schemas.sh" >&2
    echo "    ./coco-sdk/scripts/generate_python.sh" >&2
    diff -u "$BUNDLE" "$TMP_OUT/coco_app_server_protocol.schemas.json" | head -60
    exit 1
fi

# ---------------------------------------------------------------------------
# Regenerate in place
# ---------------------------------------------------------------------------
log "==> Running export_schema + export_hook_input_schema → $SCHEMA_DIR..."
if $QUIET_MODE; then
    "$EXAMPLE_BIN" "$SCHEMA_DIR" > /dev/null
    "$HOOK_EXAMPLE_BIN" "$SCHEMA_DIR" > /dev/null
else
    "$EXAMPLE_BIN" "$SCHEMA_DIR"
    "$HOOK_EXAMPLE_BIN" "$SCHEMA_DIR"
fi
merge_hook_input_into_bundle "$SCHEMA_DIR"

# ---------------------------------------------------------------------------
# End-of-run summary — replaces the noisy `ls -la`.
# ---------------------------------------------------------------------------
SCHEMA_COUNT=$(find "$SCHEMA_DIR" -maxdepth 1 -name '*.json' | wc -l | tr -d ' ')
BUNDLE_KB=$(du -k "$BUNDLE" | cut -f1)
DEF_COUNT=$(python3 -c "import json;s=json.load(open('$BUNDLE'));d=s.get('\$defs') or s.get('definitions') or {};print(len(d))")
log ""
log "==> Wrote $SCHEMA_COUNT schema file(s); bundle is ${BUNDLE_KB}K with $DEF_COUNT type definitions."
