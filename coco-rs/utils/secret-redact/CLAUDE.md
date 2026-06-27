# coco-secret-redact

Best-effort secret redaction for tool output and logs. Zero-copy when no match.

## Supported Patterns
| Pattern | Matches |
|---------|---------|
| Anthropic | `sk-ant-...` (applied before OpenAI to avoid overlap) |
| OpenAI | `sk-...` |
| GitHub | `ghp_/ghs_/gho_/ghu_/ghr_/github_pat_...` |
| Slack | `xox[bpras]-...` |
| AWS | `AKIA[A-Z0-9]{16}` access key IDs |
| Bearer | `Bearer <token>` |
| Assignment | `api_key=`, `token:`, `secret=`, `password:` `...` |

## Key Types
- `redact_secrets(&str) -> Cow<'_, str>` — single entry point; returns input verbatim on no match.
