# Internal design notes

> **This is not user documentation.** User docs live in [`docs/`](../README.md).
>
> These are working notes: design proposals, migration plans, parity audits, and
> point-in-time comparisons against other projects. Many were written before the
> code they describe existed, and **some are now wrong**. They are kept for the
> reasoning and history, not as a reference.

## How to read anything in here

**The code wins.** When a note disagrees with the source, the source is right.
Known-stale examples, as of this writing:

- Several notes give config paths as `~/.coco/` and `.claude/`. The real ones
  are `~/.cocode/` and `.cocode/`.
- `hermes/02-feature-comparison.md` describes Mixture of Agents as missing and
  the workflow runtime as a stub. Both are implemented and shipping.
- `agentteam-architecture.md` lists `TeamCreate` / `TeamDelete` tools that have
  since been retired.

For anything ships-or-not, the single source of truth is the `FEATURES` table in
`coco-rs/common/types/src/features.rs`, not a document in this directory.

## The `jcode/` subdirectory is about a different project

`jcode/*` is an adversarial review of **jcode**, a separate external agent, done
to find gaps worth closing in cocode. Every memory, startup, and benchmark
number in those files is **jcode's, measured on jcode's machine** — none of them
describe cocode. Do not cite them for cocode's performance.

cocode's own measured numbers, and the harness that produces them, are in the
[project README](../../README.md#performance) and `scripts/bench-startup.py`.

## What's here

| Area | Notes |
| --- | --- |
| `crate-coco-*.md` | Per-crate design write-ups |
| `event-hub/` | Event Hub design and spec |
| `multi-session-app-server/` | Multi-session AppServer design |
| `ui/` | TUI architecture and rendering plans |
| `hermes/`, `jcode/` | Comparisons against other agents |
| everything else | Plans, audits, and trackers, mostly dated |
