/// Surface official-distribution provenance to the crate as `COCO_BUILD_OFFICIAL`,
/// joining the `COCO_BUILD_*` family that `app/cli/build.rs` emits for `--version`.
/// Only the release workflow sets it; every local build resolves to `0`, which is
/// what keeps a locally-built `--release` binary off the OS keychain. Always
/// emitted so `env!` in lib.rs resolves without an `option_env!` fallback.
fn main() {
    println!("cargo:rerun-if-env-changed=COCO_BUILD_OFFICIAL");
    let official = std::env::var("COCO_BUILD_OFFICIAL").unwrap_or_default();
    println!("cargo:rustc-env=COCO_BUILD_OFFICIAL={official}");
}
