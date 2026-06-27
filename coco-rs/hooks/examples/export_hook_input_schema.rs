//! Emit the JSON Schema for [`coco_hooks::HookInput`] so the SDK
//! schema bundle (and the multi-language codegens that read it) can
//! resolve `HookCallbackParams.input` to its typed variants.
//!
//! `coco-types` cannot import `coco-hooks` (architectural layering —
//! types is foundational, hooks is above it), so the hook input
//! schemas live here. The schema bundle's `HookCallbackParams.input`
//! field carries a `$ref` to `#/$defs/HookInput`; this example
//! materialises HookInput plus all referenced subtypes into a
//! standalone file that `generate_schemas.sh` merges into the bundle.
//!
//! Run via `cargo run --example export_hook_input_schema --features schema -- <output_dir>`.

use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use schemars::schema_for;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(output_dir) = args.next() else {
        eprintln!("usage: export_hook_input_schema <output_dir>");
        return ExitCode::from(2);
    };
    let output_path = PathBuf::from(&output_dir).join("hook_input.json");
    let Some(parent) = output_path.parent() else {
        eprintln!("error: output path has no parent");
        return ExitCode::from(2);
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        eprintln!("error: create_dir_all({}): {e}", parent.display());
        return ExitCode::from(2);
    }

    let schema = schema_for!(coco_hooks::inputs::HookInput);
    let json = match serde_json::to_string_pretty(&schema) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: serialise schema: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = std::fs::write(&output_path, format!("{json}\n")) {
        eprintln!("error: write {}: {e}", output_path.display());
        return ExitCode::from(2);
    }
    eprintln!("==> wrote {}", relative_to_cwd(&output_path));
    ExitCode::SUCCESS
}

fn relative_to_cwd(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}
