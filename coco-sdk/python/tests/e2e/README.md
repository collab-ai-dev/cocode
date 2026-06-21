# coco-sdk Python e2e tests

Spawn a real `coco sdk` subprocess and drive it through the Python SDK
against [DeepSeek](https://deepseek.com).

## Layout

```
tests/e2e/
├── conftest.py               # .env loader + skip-if-no-key fixture
├── .env.example              # template — copy to .env to enable the tests
├── test_query_basic.py       # one-shot query() against deepseek-v4-flash
├── test_client_multi_turn.py # CocoClient initialize → session/start → follow-up
├── test_tool_callback.py     # @tool() in-process MCP tool round-trip
└── test_hook_callback.py     # @hook() PreToolUse callback
```

## Skip semantics (mirrors `coco-rs/tests/live/`)

Tests skip cleanly — never fail — when:

1. `DEEPSEEK_API_KEY` is unset, or
2. The `coco` binary can't be found.

Each skip prints `[skip] <reason>` to stderr (matches the Rust
`require_live!` macro) so CI logs surface the gating reason.

## Running

```bash
# From coco-sdk/python:
DEEPSEEK_API_KEY=sk-... python -m pytest tests/e2e -q

# Or use just from coco-rs/:
just sdk-py-test
```

`just pre-commit` includes the full Python suite (unit + e2e). E2E
tests skip when no key is present, so the recipe is a no-op cost on
machines without credentials.

## Credentials file

Either:

* `tests/e2e/.env` (or `.env.test`)
* `coco-rs/tests/live/.env` (or `.env.test`) — shared with the Rust suite

The first file found wins. Already-set environment variables are
never overwritten.

## Notes

* DeepSeek's `deepseek-v4-flash` is the canonical low-cost model. It
  costs roughly fractions of a cent per turn.
* Each test gets a fresh tempdir as the subprocess `cwd`, so any
  file-touching tools stay sandboxed.
* The model id `deepseek4-flash` does not exist; use
  `deepseek-v4-flash` (with hyphen).
