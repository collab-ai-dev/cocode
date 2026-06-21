#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

url="https://openrouter.ai/api/v1/models"
out="common/model-card/data/openrouter-models.json"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

curl -fsSL "$url" -o "$tmp"

jq -e '
  .data
  | type == "array" and length > 0 and all(.[]; has("id") and has("pricing"))
' "$tmp" >/dev/null

chmod 0644 "$tmp"
mv "$tmp" "$out"
trap - EXIT

total="$(jq '.data | length' "$out")"
with_cutoff="$(jq '[.data[] | select(.knowledge_cutoff != null)] | length' "$out")"
with_pricing="$(jq '[.data[] | select(.pricing.prompt != null and .pricing.completion != null)] | length' "$out")"

printf 'updated %s\n' "$out"
printf 'models: %s\n' "$total"
printf 'knowledge_cutoff: %s\n' "$with_cutoff"
printf 'pricing: %s\n' "$with_pricing"
