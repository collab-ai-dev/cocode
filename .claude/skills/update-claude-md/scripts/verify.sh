#!/usr/bin/env bash
# Mechanical verification for a regenerated root CLAUDE.md.
# Usage: bash .claude/skills/update-claude-md/scripts/verify.sh [path-to-CLAUDE.md]
# Exit 0 = PASS (warnings allowed), exit 1 = FAIL.
set -u
f="${1:-CLAUDE.md}"
fail=0
ok()   { echo "OK:   $*"; }
warn() { echo "WARN: $*"; }
err()  { echo "FAIL: $*"; fail=1; }

if [[ ! -f "$f" ]]; then echo "FAIL: $f not found"; exit 1; fi
dir=$(dirname "$f")

# 1. Size budget: target < 30k, hard ceiling 40k (CC memory warning threshold)
chars=$(wc -c < "$f" | tr -d ' ')
if (( chars >= 40000 )); then err "char count $chars >= 40000 (CC warning threshold)"
elif (( chars >= 30000 )); then warn "char count $chars over 30000 target (hard ceiling 40000)"
else ok "char count $chars < 30000"
fi

# 2. Fragile counts: "(N)" in headings, "N crates/impls/roles/..."
hits=$(grep -nE '^#{1,4} .*\([0-9]+\)' "$f")
if [[ -n "$hits" ]]; then err "count in heading:"; echo "$hits"; else ok "no counts in headings"; fi
hits=$(grep -nE '(^|[^0-9A-Za-z-])[0-9]+\+? (crates|impls|roles|modules|contexts|actions|tools|variants)\b' "$f")
if [[ -n "$hits" ]]; then err "fragile count phrase:"; echo "$hits"; else ok "no fragile count phrases"; fi

# 3. Banned section: Key Design Patterns
hits=$(grep -niE '^#{1,4} .*key design patterns' "$f")
if [[ -n "$hits" ]]; then err "'Key Design Patterns' section present:"; echo "$hits"; else ok "no 'Key Design Patterns' section"; fi

# 4. Banned column: Key Types in root crate tables
hits=$(grep -n '| Key Types' "$f")
if [[ -n "$hits" ]]; then err "'Key Types' column in a root table:"; echo "$hits"; else ok "no 'Key Types' column"; fi

# 5. Domain-scoped convention blocks (belong in the owning crate's CLAUDE.md)
hits=$(grep -niE '^#{1,4} .*(ratatui|tui).*(style|convention)' "$f")
if [[ -n "$hits" ]]; then err "domain-scoped TUI convention heading at root:"; echo "$hits"; else ok "no domain-scoped TUI convention block"; fi

# 6. Crate-table row length: rows are routing entries, not summaries
long=$(awk 'length($0) > 160 && /^\| `/' "$f")
if [[ -n "$long" ]]; then err "crate-table rows over 160 chars:"; echo "$long"
else
  longish=$(awk 'length($0) > 140 && /^\| `/' "$f")
  if [[ -n "$longish" ]]; then warn "crate-table rows over 140 chars (soft cap):"; echo "$longish"; else ok "all crate rows within row cap"; fi
fi

# 7. Exactly 3 data-flow diagrams under "## Key Data Flows"
flows=$(awk '/^## Key Data Flows/{inflow=1;next} /^## /{inflow=0} inflow && /^```/{n++} END{print int(n/2)}' "$f")
if [[ "$flows" != "3" ]]; then err "expected exactly 3 data-flow diagrams under 'Key Data Flows', found ${flows:-0}"
else ok "exactly 3 data-flow diagrams"
fi

# 8. Dead local links
dead=0
while IFS= read -r p; do
  p="${p%%#*}"
  [[ -z "$p" ]] && continue
  if [[ ! -e "$dir/$p" ]]; then err "dead link: $p"; dead=1; fi
done < <(grep -oE '\]\([^)]+\)' "$f" | sed -E 's/^\]\(//; s/\)$//' | grep -vE '^(https?:|#|mailto:)')
[[ $dead -eq 0 ]] && ok "all local links resolve"

echo
if (( fail )); then echo "RESULT: FAIL"; exit 1; else echo "RESULT: PASS"; exit 0; fi
