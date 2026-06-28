# coco TypeScript SDK

TypeScript SDK for the `coco sdk` JSON-RPC subprocess protocol.

```ts
import { query, NotificationMethod } from "@coco-rs/coco-sdk";

for await (const event of query("List the Rust crates")) {
  if (event.method === NotificationMethod.AGENT_MESSAGE_DELTA) {
    process.stdout.write(event.params.delta);
  }
}
```

Protocol types are generated from `coco-sdk/schemas/json`:

```bash
../scripts/generate_typescript.sh
```
