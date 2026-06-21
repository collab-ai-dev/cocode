# coco-process-hardening

OS-level process hardening. Rust-only, no TS equivalent — uses `prctl`, `ptrace(PT_DENY_ATTACH)`, env sanitization via libc FFI.

## Key Types

Platform-specific hardening functions (libc FFI wrappers). No dependencies beyond `libc`.
