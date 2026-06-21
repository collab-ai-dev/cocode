# @coco-rs/coco-cli

npm distribution for the `coco` Rust binary built from
[`coco-rs/`](../coco-rs).

After install:

```bash
npm install -g @coco-rs/coco-cli
coco-cli --help
```

## Supported platforms

| OS / arch          | npm package                       | target triple                  |
| ------------------ | --------------------------------- | ------------------------------ |
| Linux x86_64       | `@coco-rs/coco-cli-linux-x64`     | `x86_64-unknown-linux-musl`    |
| Linux aarch64      | `@coco-rs/coco-cli-linux-arm64`   | `aarch64-unknown-linux-musl`   |
| macOS Apple Silicon| `@coco-rs/coco-cli-darwin-arm64`  | `aarch64-apple-darwin`         |

Windows and macOS Intel are not supported. The `coco` binary is shipped
standalone — it does not depend on `ripgrep` or any other separately
installed CLI.

## Architecture

```
@coco-rs/coco-cli                  meta package, bin/coco-cli.js launcher
  ├── @coco-rs/coco-cli-linux-x64    vendor/x86_64-unknown-linux-musl/coco/coco
  ├── @coco-rs/coco-cli-linux-arm64  vendor/aarch64-unknown-linux-musl/coco/coco
  └── @coco-rs/coco-cli-darwin-arm64 vendor/aarch64-apple-darwin/coco/coco
```

All four tarballs publish under the **same** npm name `@coco-rs/coco-cli`,
disambiguated by version (`0.1.0` for the meta package, `0.1.0-linux-x64`
etc. for platform packages). The meta package's `optionalDependencies` use
the `npm:` alias syntax to map local names back to the right version, so
only one organization scope (`@coco-rs`) needs to be registered.

## Release flow

There are two paths: **CI-driven** (recommended) and **manual** (works
without a workflow).

### CI-driven

1. Bump `version` in `coco-cli/package.json` and the `coco-rs` workspace
   `Cargo.toml` so they agree.
2. Tag and push:
   ```bash
   git tag -a coco-v0.1.0 -m "Release 0.1.0"
   git push origin coco-v0.1.0
   ```
   This triggers `.github/workflows/coco-release.yml`, which builds all
   four targets in parallel and uploads each as an Actions artifact.
3. Once the workflow finishes, copy the run URL from the Actions tab and:
   ```bash
   python3 coco-cli/scripts/stage_npm_packages.py \
     --release-version 0.1.0 \
     --package coco-cli \
     --workflow-url https://github.com/mylinyuzhi/codex/actions/runs/<run-id>
   ```
   This downloads the artifacts, hydrates `coco-cli/vendor/`, and writes
   four tarballs to `dist/npm/`.
4. Publish — platform packages first, meta last (npm requires this order
   because the meta package's `optionalDependencies` reference platform
   versions that must already exist in the registry):
   ```bash
   for tarball in dist/npm/coco-cli-npm-linux-*-0.1.0.tgz \
                  dist/npm/coco-cli-npm-darwin-*-0.1.0.tgz; do
     npm publish "$tarball" --access public
   done
   npm publish dist/npm/coco-cli-npm-0.1.0.tgz --access public
   ```
5. Verify:
   ```bash
   npm install -g @coco-rs/coco-cli@0.1.0
   coco-cli --version
   ```

### Manual (no CI)

You can publish without the workflow by populating `vendor/` yourself.
Useful for staged rollouts where you only have binaries for a subset of
the three targets — npm will simply skip the missing optional deps; users
on those platforms see a "Missing optional dependency" error from the
launcher.

```bash
cd coco-cli

# 1. Build and stage every target you can produce locally.
make build TARGET=x86_64-unknown-linux-musl    # add `rustup target add` if needed
make build TARGET=aarch64-apple-darwin
# ...

# 2. Smoke-test the launcher against your host's target.
make smoke

# 3. Pack the meta tarball + every platform whose vendor/ is populated.
make pack VERSION=0.1.0

# 4. Publish in the right order.
make publish VERSION=0.1.0
```

## Local development

### Smoke test the launcher

```bash
cd coco-cli
make build         # compiles for the current host
make smoke         # runs node bin/coco-cli.js --version
```

The launcher's local fallback (`coco-cli/vendor/<triple>/coco/coco`) is
hit only when no `@coco-rs/coco-cli-*` package is resolvable via
`require.resolve` — i.e. exactly the dev case.

### Inspect a packed tarball

```bash
tar tzf dist/npm/coco-cli-npm-0.1.0.tgz                    # meta
tar tzf dist/npm/coco-cli-npm-linux-x64-0.1.0.tgz          # platform
tar xzOf dist/npm/coco-cli-npm-0.1.0.tgz package/package.json
```

## Files

```
coco-cli/
├── bin/coco-cli.js          launcher (resolves platform package, spawns vendor binary)
├── package.json             meta package manifest (name, bin, optionalDependencies)
├── vendor/                  populated at build/install time (gitignored)
├── scripts/
│   ├── build_npm_package.py stage one tarball (meta or platform)
│   ├── install_native_deps.py download CI artifacts into vendor/
│   └── stage_npm_packages.py one-shot: download + stage every package
├── Makefile                 quick-release helpers (build, pack, publish, tag)
└── README.md                this file
```

## Prerequisites

- Node.js 16+ and npm 11+ (for publishing)
- Python 3.10+
- Rust (`rustc` / `cargo`) — only for local builds
- [`zstd`](https://github.com/facebook/zstd) — only for `install_native_deps.py`
- [GitHub CLI](https://cli.github.com/) (`gh`) — only for `install_native_deps.py`

## Troubleshooting

**`Missing optional dependency @coco-rs/coco-cli-<platform>`**
The platform package for the user's OS/arch was not installed. Either it
hasn't been published yet for this version, or the install ran with
`--no-optional`/`--omit=optional`. Reinstall without those flags.

**`Unsupported platform: <os> (<arch>)`**
The user is on a platform outside the three supported targets (Linux
x86_64, Linux aarch64, macOS Apple Silicon). There is no prebuilt binary
to ship.

**Same-version republish fails on npm**
npm forbids reusing `name@version`. Bump the version (and the platform
suffixes derived from it) and retry. Within 72 hours you can `npm
unpublish` to free up the version, but a fresh patch release is usually
simpler.
