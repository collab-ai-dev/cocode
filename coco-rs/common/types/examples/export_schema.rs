//! JSON Schema export tool.
//!
//! Generates JSON Schema files for all SDK protocol types into
//! `coco-sdk/schemas/json/` (or a custom output directory). These schemas
//! are the source of truth for multi-language SDK type generation (Python
//! via `scripts/generate_python.sh`, TypeScript, Go, etc.).
//!
//! Usage:
//!   cargo run -p coco-types --features schema --example export_schema
//!   cargo run -p coco-types --features schema --example export_schema -- /custom/path
//!
//! ## Bundle composition
//!
//! The bundled schema (`coco_app_server_protocol.schemas.json`) is built
//! by registering a small set of **entry-point types** and letting
//! `schemars` walk their `$ref` closure transitively. Concretely,
//! `schema_for!(T)` returns a `Schema` (newtype on `serde_json::Value`)
//! whose top-level `$defs` already contains every type that `T`
//! (recursively) references via `$ref`. We lift those flat into the
//! bundle's outer `$defs` map. The result: any type reachable from any
//! entry point appears in the bundle automatically — no per-type
//! `bundle_entry` line to maintain, and no silent gaps for cross-language
//! clients.
//!
//! Adding a new wire type? Make sure it is reachable from one of the
//! entry points below. If it is a brand-new top-level concept (not
//! referenced by any existing union/struct), add it to
//! `ENTRY_POINTS` instead of sprinkling individual lines elsewhere.
//!
//! TS reference: cocode-rs `app-server-protocol/src/export.rs` (454 lines).
//! coco-rs is shorter because the merger replaces the per-variant list.

#[cfg(not(feature = "schema"))]
fn main() {
    eprintln!(
        "error: export_schema requires the `schema` feature.\n\
         Run: cargo run -p coco-types --features schema --example export_schema"
    );
    std::process::exit(1);
}

#[cfg(feature = "schema")]
use std::collections::BTreeMap;
#[cfg(feature = "schema")]
use std::fs;
#[cfg(feature = "schema")]
use std::path::Path;
#[cfg(feature = "schema")]
use std::path::PathBuf;

#[cfg(feature = "schema")]
use schemars::JsonSchema;
#[cfg(feature = "schema")]
use schemars::schema_for;
#[cfg(feature = "schema")]
use serde_json::Value;
#[cfg(feature = "schema")]
use serde_json::json;

#[cfg(feature = "schema")]
/// Write a single type's schema to `<out_dir>/<name>.json`.
fn write_schema<T: JsonSchema>(out_dir: &Path, name: &str) {
    let schema = schema_for!(T);
    let path = out_dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&schema).expect("serialize schema");
    fs::write(&path, json).unwrap_or_else(|e| {
        panic!("write {} failed: {e}", path.display());
    });
    println!("  ✓ {}", path.display());
}

#[cfg(feature = "schema")]
/// Bundle accumulator with explicit-vs-transitive precedence.
///
/// Each entry point is registered via `add::<T>(name)` which:
///   1. inserts T's top-level schema (the metadata-rich
///      `schema_for!(T)` value, with its embedded `$defs` /
///      `$schema` stripped) under `name` — marked **explicit**.
///   2. lifts every entry of the top-level `$defs` flat into the
///      bundle — marked **transitive**.
///
/// Two precedence rules:
///   * **Explicit always wins**. When `ProviderApi` is registered
///     explicitly *and* reached transitively via `ModelSpec.api`,
///     the richer top-level schema (with `title` etc.) wins; the
///     leaner transitive copy is silently dropped.
///   * **First transitive wins**. When two unrelated entries both
///     reach the same inner type, the schemas are equal in every
///     case we have observed (schemars is deterministic), so we
///     keep the first and only flag a real divergence as a conflict.
struct BundleBuilder {
    entries: BTreeMap<String, Value>,
    /// Names registered via the explicit top-level path. These
    /// outrank later transitive picks for the same name.
    explicit: BTreeMap<String, ()>,
    /// Conflicting names where two non-equal schemas landed under
    /// the same name — almost always a real bug worth investigating.
    conflicts: Vec<String>,
}

#[cfg(feature = "schema")]
impl BundleBuilder {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            explicit: BTreeMap::new(),
            conflicts: Vec::new(),
        }
    }

    /// Register an entry-point type and pull in its full transitive
    /// `$ref` closure. See struct docs for precedence rules.
    fn add<T: JsonSchema>(&mut self, name: &str) {
        // `schema_for!(T)` returns `schemars::Schema` — a newtype on
        // `serde_json::Value`. The top-level object carries the type's
        // own schema plus a `$defs` map of every type reachable from
        // `T` via `$ref` (default per JSON Schema 2020-12 settings).
        let mut top = schema_for!(T).to_value();

        // 1) Split out `$defs` and shed envelope-level metadata
        //    (`$schema`) that only makes sense on a standalone schema,
        //    not on a nested entry inside the bundle.
        let mut defs = serde_json::Map::new();
        if let Some(obj) = top.as_object_mut() {
            if let Some(Value::Object(map)) = obj.remove("$defs") {
                defs = map;
            }
            obj.remove("$schema");
        }
        self.entries.insert(name.to_string(), top);
        self.explicit.insert(name.to_string(), ());

        // 2) Every transitively-reachable type → bundle[X], marked
        //    **transitive** (loses to explicit, ties resolved
        //    first-write-wins with conflict detection).
        for (inner_name, inner_schema) in defs {
            self.insert_transitive(&inner_name, inner_schema);
        }
    }

    fn insert_transitive(&mut self, name: &str, value: Value) {
        // Explicit registrations are never overwritten by transitive
        // picks — preserves the metadata-rich schema for entry points.
        if self.explicit.contains_key(name) {
            return;
        }
        if let Some(existing) = self.entries.get(name) {
            if existing != &value {
                self.conflicts.push(name.to_string());
                return;
            }
        }
        self.entries.insert(name.to_string(), value);
    }

    fn into_definitions(self) -> (serde_json::Map<String, Value>, Vec<String>) {
        let mut map = serde_json::Map::new();
        for (k, v) in self.entries {
            map.insert(k, v);
        }
        (map, self.conflicts)
    }
}

#[cfg(feature = "schema")]
/// Walk every `$ref` in the bundle's definitions and return any that
/// point at a name not present as a top-level definition. Each entry
/// in the returned map is `(missing_target, [sources_that_referenced_it])`.
///
/// Accepts both `#/$defs/...` (schemars 1.x / JSON Schema 2020-12) and
/// legacy `#/definitions/...` to stay resilient to future settings
/// changes.
fn find_dangling_refs(
    definitions: &serde_json::Map<String, Value>,
) -> BTreeMap<String, Vec<String>> {
    let known: std::collections::BTreeSet<String> = definitions.keys().cloned().collect();
    let mut dangling: BTreeMap<String, Vec<String>> = BTreeMap::new();

    fn walk(value: &Value, sink: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(s)) = map.get("$ref") {
                    let name = s
                        .strip_prefix("#/$defs/")
                        .or_else(|| s.strip_prefix("#/definitions/"));
                    if let Some(name) = name {
                        sink.push(name.to_string());
                    }
                }
                for v in map.values() {
                    walk(v, sink);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    walk(v, sink);
                }
            }
            _ => {}
        }
    }

    for (source, schema) in definitions {
        let mut refs = Vec::new();
        walk(schema, &mut refs);
        for target in refs {
            if !known.contains(&target) {
                dangling.entry(target).or_default().push(source.clone());
            }
        }
    }
    dangling
}

#[cfg(feature = "schema")]
fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Default to coco-sdk/schemas/json relative to the repo root.
    // CARGO_MANIFEST_DIR = coco-rs/common/types, so repo root is ../../.. from there.
    let out_dir: PathBuf = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest
            .join("../../..")
            .join("coco-sdk/schemas/json")
            .canonicalize()
            .unwrap_or_else(|_| manifest.join("../../../coco-sdk/schemas/json"))
    };

    fs::create_dir_all(&out_dir).expect("create out_dir");
    println!("Writing schemas to {}", out_dir.display());
    println!();

    // --- Individual top-level schema files ---
    //
    // Note: `CoreEvent` itself is NOT exported — it's an in-process dispatch
    // wrapper around Protocol/Stream/Tui and is not meant to cross the wire.
    // The three child enums are the actual wire types.
    println!("Individual schemas:");
    write_schema::<coco_types::ServerNotification>(&out_dir, "server_notification");
    write_schema::<coco_types::NotificationMethod>(&out_dir, "notification_method");
    write_schema::<coco_types::AgentStreamEvent>(&out_dir, "agent_stream_event");
    write_schema::<coco_types::TuiOnlyEvent>(&out_dir, "tui_only_event");
    write_schema::<coco_types::ClientRequest>(&out_dir, "client_request");
    write_schema::<coco_types::ClientRequestMethod>(&out_dir, "client_request_method");
    write_schema::<coco_types::ServerRequest>(&out_dir, "server_request");
    write_schema::<coco_types::ServerRequestMethod>(&out_dir, "server_request_method");
    write_schema::<coco_types::JsonRpcMessage>(&out_dir, "jsonrpc_message");
    write_schema::<coco_types::ThreadItem>(&out_dir, "thread_item");
    write_schema::<coco_types::SessionStartParams>(&out_dir, "session_start_request");
    println!();

    // --- Bundled schema: all types reachable from any entry point ---
    //
    // The bundle is the source of truth for cross-language clients
    // (Python is the in-tree consumer; TS/Go/etc. only have this file).
    // Entry points are the smallest set whose transitive `$ref` closure
    // covers the full wire surface. Add a new entry **only** if the
    // type is unreachable from every existing entry — otherwise it
    // flows in for free via `BundleBuilder::add`.
    println!("Bundled schema:");

    let mut bundle = BundleBuilder::new();

    // Wire envelope + 3-layer event taxonomy. `ServerNotification` /
    // `ClientRequest` / `ServerRequest` carry the bulk of the param
    // structs as transitives; `AgentStreamEvent` and `TuiOnlyEvent`
    // mirror the same item/delta types from a different angle.
    bundle.add::<coco_types::ServerNotification>("ServerNotification");
    bundle.add::<coco_types::NotificationMethod>("NotificationMethod");
    bundle.add::<coco_types::AgentStreamEvent>("AgentStreamEvent");
    bundle.add::<coco_types::TuiOnlyEvent>("TuiOnlyEvent");
    bundle.add::<coco_types::ClientRequest>("ClientRequest");
    bundle.add::<coco_types::ClientRequestMethod>("ClientRequestMethod");
    bundle.add::<coco_types::ServerRequest>("ServerRequest");
    bundle.add::<coco_types::ServerRequestMethod>("ServerRequestMethod");
    bundle.add::<coco_types::JsonRpcMessage>("JsonRpcMessage");

    // JsonRpcMessage's four variants — schemars **inlines** the
    // variant structs into the union's `oneOf` rather than using
    // `$ref`, so they don't reach the bundle transitively. Register
    // explicitly so consumers can refer to them by name (e.g. the
    // Python SDK's `JsonRpcRequest(method=..., request_id=...)`
    // helper, or TS clients pattern-matching on the standalone
    // shapes).
    bundle.add::<coco_types::JsonRpcRequest>("JsonRpcRequest");
    bundle.add::<coco_types::JsonRpcResponse>("JsonRpcResponse");
    bundle.add::<coco_types::JsonRpcError>("JsonRpcError");
    bundle.add::<coco_types::JsonRpcNotification>("JsonRpcNotification");
    bundle.add::<coco_types::RequestId>("RequestId");
    bundle.add::<coco_types::ApprovalDecision>("ApprovalDecision");

    // ThreadItem is a tagged union; its variant payload structs come
    // through transitively. Registered explicitly because it's a
    // top-level cross-language consumer (timeline rendering).
    bundle.add::<coco_types::ThreadItem>("ThreadItem");

    // Provider / model addressing. Not reachable through the wire
    // unions (these live in config-layer init paths, not on the
    // session/turn/event hot paths) but cross-language SDKs need them
    // to express multi-provider model selection.
    bundle.add::<coco_types::ProviderApi>("ProviderApi");
    bundle.add::<coco_types::ModelRole>("ModelRole");
    bundle.add::<coco_types::ModelSpec>("ModelSpec");
    bundle.add::<coco_types::Capability>("Capability");
    bundle.add::<coco_types::WireApi>("WireApi");
    bundle.add::<coco_types::ApplyPatchToolType>("ApplyPatchToolType");

    // SDK hook callback output — TS-canonical `hookJSONOutputSchema`.
    // Not reachable from `ClientRequest`/`ServerRequest` (responses
    // ride the synchronous JSON-RPC reply path, which isn't a typed
    // union schema), so each TS-mirrored type is registered explicitly
    // for cross-language SDK codegen. `HookSpecificOutput` is a tagged
    // union; its variant payloads come through transitively.
    bundle.add::<coco_types::SdkHookOutput>("SdkHookOutput");
    bundle.add::<coco_types::HookSpecificOutput>("HookSpecificOutput");
    bundle.add::<coco_types::HookDecision>("HookDecision");
    // `HookPermissionDecision` is shared with the silent attachment
    // payload (`messages::attachment_body::HookPermissionDecision`) —
    // single source of truth, registered once for both call sites.
    bundle.add::<coco_types::ElicitationAction>("ElicitationAction");
    bundle.add::<coco_types::PermissionRequestDecision>("PermissionRequestDecision");
    bundle.add::<coco_types::HookCallbackResult>("HookCallbackResult");
    bundle.add::<coco_types::McpRouteMessageResult>("McpRouteMessageResult");

    // SDK ClientRequest synchronous reply bodies — registered
    // explicitly so the Python SDK can deserialize them as typed
    // Pydantic models (`client.mcp_status() → McpStatusResult`, etc.).
    // They're not reachable from any wire union because JSON-RPC
    // results are method-keyed, not tagged-enum-discriminated.
    bundle.add::<coco_types::ConfigReadResult>("ConfigReadResult");
    bundle.add::<coco_types::ContextUsageResult>("ContextUsageResult");
    bundle.add::<coco_types::InitializeResult>("InitializeResult");
    bundle.add::<coco_types::McpStatusResult>("McpStatusResult");
    bundle.add::<coco_types::McpSetServersResult>("McpSetServersResult");
    bundle.add::<coco_types::PluginReloadResult>("PluginReloadResult");
    bundle.add::<coco_types::SessionListResult>("SessionListResult");
    bundle.add::<coco_types::SessionReadResult>("SessionReadResult");
    bundle.add::<coco_types::SessionResumeResult>("SessionResumeResult");
    bundle.add::<coco_types::SessionStartResult>("SessionStartResult");
    bundle.add::<coco_types::TurnStartResult>("TurnStartResult");
    bundle.add::<coco_types::RewindFilesResult>("RewindFilesResult");

    let (definitions, conflicts) = bundle.into_definitions();

    if !conflicts.is_empty() {
        eprintln!();
        eprintln!(
            "WARNING: {} schema name(s) had divergent shapes from different entry points:",
            conflicts.len()
        );
        for name in conflicts.iter().take(10) {
            eprintln!("  - {name}");
        }
        eprintln!(
            "These are usually harmless (schemars sometimes emits subtly \
             different metadata for the same type from different roots) but \
             can mask real bugs — investigate if a known-canonical type appears."
        );
    }

    // Hard check: every `$ref: "#/definitions/X"` in the bundle must
    // resolve. If a type is referenced by a field but never registered
    // as an entry point AND not picked up via transitives (e.g.
    // schemars inlined the parent union's variants instead of $ref'ing
    // them), cross-language clients break silently. This makes that
    // failure mode loud and immediate.
    let dangling = find_dangling_refs(&definitions);
    if !dangling.is_empty() {
        eprintln!();
        eprintln!(
            "ERROR: bundle has {} unresolved $ref(s) — cross-language clients will break:",
            dangling.len()
        );
        for (target, sources) in dangling.iter().take(20) {
            let preview: Vec<&String> = sources.iter().take(3).collect();
            let extra = if sources.len() > 3 {
                format!(" (+{} more)", sources.len() - 3)
            } else {
                String::new()
            };
            eprintln!("  - missing: {target}, referenced from: {preview:?}{extra}");
        }
        eprintln!();
        eprintln!(
            "Fix: register the missing type as an entry point in `main()`. \
             Common cause: schemars inlines tagged-union variant structs into \
             the parent's `oneOf` instead of emitting a `$ref`, so they aren't \
             collected by the transitive walk."
        );
        std::process::exit(1);
    }

    let bundle_value = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "coco-app-server-protocol",
        "description": "JSON Schema bundle for the coco-rs SDK protocol types. \
            Source: coco-rs/common/types. Generated by `cargo run -p coco-types \
            --features schema --example export_schema`. Composition: a small \
            set of entry-point types + every type they transitively reference \
            via `$ref` (collected by `schemars::schema_for!` and flattened into \
            `$defs`). To add a new type, ensure it is reachable from an \
            existing entry; only add a new entry if it is a true root.",
        "$defs": definitions,
    });

    let bundle_path = out_dir.join("coco_app_server_protocol.schemas.json");
    let bundle_json = serde_json::to_string_pretty(&bundle_value).expect("serialize bundle");
    fs::write(&bundle_path, bundle_json).expect("write bundle");
    println!("  ✓ {}", bundle_path.display());

    println!();
    println!("Done.");
}
