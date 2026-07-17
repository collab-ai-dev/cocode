# coco-utils-json-repair

LLM-output JSON repair: strict `serde_json` first, `llm_json::repair_json` fallback.

| Function | Purpose |
|----------|---------|
| `parse_with_repair` | `(Value, RepairOutcome)` — `Clean` vs `Repaired` tag for telemetry |
| `repair_to_string` | Repaired text without parsing, for logging/caching |

- **Never call during streaming accumulation** — the repairer "closes" a half-arrived `{"a":1,` into wrong content. Parse only at the terminal event (`ToolInputEnd` / `ToolCall`).
- Empty/whitespace input is `JsonRepairError::EmptyInput`; callers wanting `"" → {}` semantics special-case upstream (tool-input parsing does).
- Deliberate parallel twin in `vercel-ai-provider-utils::json_repair` — layering forbids that crate from depending on `coco-*`. Both delegate to `llm_json::repair_json`, bounding drift.
