# coco-cli scripts

Internal Python helpers for npm packaging. End users should follow the
release flow in [../README.md](../README.md) and the `make` targets in
[../Makefile](../Makefile).

| Script                    | Purpose                                                      |
| ------------------------- | ------------------------------------------------------------ |
| `build_npm_package.py`    | Stage one tarball — meta or a single platform package.       |
| `install_native_deps.py`  | Download `coco-<triple>.zst` artifacts from a CI workflow run into `vendor/`. |
| `stage_npm_packages.py`   | One-shot: download artifacts + stage every package into `dist/npm/`. |

The release tag pattern is `coco-v<version>` (e.g. `coco-v0.1.0`). The
GitHub Actions workflow that produces the artifacts is `coco-release`
(see `.github/workflows/coco-release.yml`); both constants are configured
at the top of the scripts.
