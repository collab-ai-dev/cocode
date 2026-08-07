#!/usr/bin/env bash
# Catch struct fields that nothing consumes.
#
# Why this exists: `ToolUseContext` reached 104 fields, and eight of them had
# been superseded without the old slot being removed — in-flight ids by
# `StreamingHandle`'s `JoinSet`, read limits by `Tool::max_result_size_bound`,
# tool decisions by `permission_resolution_detail`, and so on. A wide struct is
# where replaced mechanisms hide: nobody spots the orphan among a hundred
# neighbours. Worse, the same blind spot let `rendered_system_prompt` sit
# hardcoded to `None` while `AgentTool` failed closed on it, silently disabling
# fork-subagent mode outright.
#
# Two rules, because "consumed" means different things:
#
#   plain struct      dead when nothing reads it   — `.field` appears nowhere
#   Serialize struct  dead when nothing writes it  — the reader is off-process
#                                                    (JSON), so reads prove
#                                                    nothing; a field never
#                                                    constructed never reaches
#                                                    the wire
#
# Deliberately conservative — it reports only what it can prove:
#
#   * A field name shared with a live field on another struct hides the dead
#     one. Accepted: a missed orphan is cheap, a false alarm gets the whole
#     check disabled.
#   * Only structs listed in TARGETS are scanned. Whole-workspace scanning is
#     slow and would drown the signal; these are the structs whose width has
#     actually caused this.
#
# Add a struct here when it grows past the point where review can police it.
# Wired into `just check-seam` (quick-check / pre-commit). Run alone:
#   ./scripts/check-live-fields.sh
#
# Non-zero exit + the offending fields on violation; silent + status 0 clean.

set -euo pipefail

cd "$(dirname "$0")/.."

# <file>:<StructName>
TARGETS=(
    "core/tool-runtime/src/context.rs:ToolUseContext"
    "app/query/src/config.rs:QueryEngineConfig"
    "core/system-reminder/src/generator.rs:GeneratorContext"
    # Persisted announce baseline: a section added here but never wired into
    # the diff would stay `None` forever and silently stop announcing.
    "common/types/src/world_state.rs:WorldStateSnapshot"
)

violations=0

for target in "${TARGETS[@]}"; do
    file="${target%%:*}"
    struct="${target##*:}"

    [[ -f $file ]] || {
        echo "✗ check-live-fields: ${file} is gone; update TARGETS in $0"
        violations=$((violations + 1))
        continue
    }

    # A `Serialize` derive means the consumer is a JSON reader we cannot see,
    # so switch the rule from "is it read" to "is it ever constructed".
    if awk -v s="$struct" '
        /^#\[derive/ { d = $0 }
        $0 ~ "^pub struct " s "([<[:space:]]|$)" { print d; exit }
    ' "$file" | grep -q 'Serialize'; then
        mode=construct
    else
        mode=read
    fi

    fields=$(awk -v s="$struct" '
        $0 ~ "^pub struct " s "([<[:space:]]|$)" { inside = 1; next }
        inside && /^\}/ { exit }
        inside && match($0, /^[[:space:]]+pub(\([^)]*\))?[[:space:]]+[a-z_0-9]+[[:space:]]*:/) {
            line = $0
            sub(/^[[:space:]]+pub(\([^)]*\))?[[:space:]]+/, "", line)
            sub(/[[:space:]]*:.*$/, "", line)
            print line
        }
    ' "$file")

    [[ -n $fields ]] || {
        echo "✗ check-live-fields: found no fields on ${struct}; the parser or the struct moved"
        violations=$((violations + 1))
        continue
    }

    dead=""
    for field in $fields; do
        if [[ $mode == read ]]; then
            # Any binding may hold the struct, so match the access shape, not
            # a binding name. Excluding the defining file keeps the
            # declaration and the struct's own `self.field` plumbing (clone
            # helpers, accessors) from counting as consumers.
            hits=$(grep -rIl --include='*.rs' -e "\.${field}\b" . \
                --exclude-dir=target --exclude-dir=.git 2>/dev/null \
                | grep -v "^\./${file}$" | head -1 || true)
        else
            # Struct-literal or builder assignment, anywhere but the
            # declaration itself.
            hits=$(grep -rIl --include='*.rs' -e "^[[:space:]]*${field}[[:space:]]*:" . \
                --exclude-dir=target --exclude-dir=.git 2>/dev/null \
                | grep -v "^\./${file}$" | head -1 || true)
        fi
        [[ -n $hits ]] || dead+="    ${struct}.${field}"$'\n'
    done

    if [[ -n $dead ]]; then
        echo "✗ ${struct} (${file}) carries fields nothing ${mode}s:"
        printf '%s' "$dead"
        violations=$((violations + 1))
    fi
done

if ((violations > 0)); then
    cat <<'EOF'

  Delete the field, or wire the consumer that was supposed to read it — a
  slot kept "for later" is indistinguishable from a feature that silently
  does nothing. See coco-rs/CLAUDE.md → Code Hygiene ("No deprecated code").
EOF
    exit 1
fi
