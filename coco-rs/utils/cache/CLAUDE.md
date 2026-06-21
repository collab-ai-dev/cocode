# coco-utils-cache

LRU cache protected by a Tokio mutex, plus a SHA-1 content-hash helper.

## Key Types
| Type | Purpose |
|------|---------|
| `BlockingLruCache<K, V>` | Generic LRU cache; no-ops outside a Tokio runtime |
| `sha1_digest` | 20-byte SHA-1 for content-based cache keys |

Operations outside a Tokio runtime return `None` / fall through to the factory — callers never need to guard with `Handle::try_current()`.
