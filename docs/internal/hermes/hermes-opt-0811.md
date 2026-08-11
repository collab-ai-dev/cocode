# coco-rs 0811 Optimization Delta — Hermes v0.20 Absorption

Status: implementation and cross-validation complete (2026-08-12). This document
supersedes the priority ordering in
[hermes-opt-0724.md](hermes-opt-0724.md); the older report remains the detailed
evidence record for v0.19 and the 07-24 source tree.

## 1. Scope and evidence

- Hermes source: `/lyz/codespace/3rd/hermes-agent`, from the prior audit anchor
  `ef6ce56ca` to `c0106e50e` (2026-08-11).
- Release boundary: `v2026.8.3`, v0.20 “Herald”, followed by **1,146 commits**
  through the source HEAD. The primary release record is
  [NousResearch/hermes-agent v2026.8.3](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.8.3).
- coco-rs baseline: `d181c6cd` on `feat/hermes` before this implementation.
- Method: read the release notes, cluster the post-tag log by agent/tool/MCP/
  compression/security scope, inspect the relevant Hermes patches, then verify
  the corresponding coco-rs implementation rather than treating an absent name
  as an absent capability.

This is an incremental release audit, not a new whole-product comparison. The
architecture, self-learning and IM analyses in this directory remain useful,
but code is authoritative where their status has drifted.

## 2. What changed since the 07-24 audit

The main v0.20 release themes were voice/webhooks/A2A, mid-turn redirects,
tool self-recovery, compression hardening, smarter approval handling, startup
performance and lazy MCP schema loading. The transferable core-runtime work is
smaller than the release headline suggests because coco-rs landed most of the
07-24 plan immediately afterward:

| Earlier item | Current coco-rs status |
|---|---|
| Compact prompt language/date/input bound; Anthropic empty parts; ANSI cleanup; Edit near-match; loop guardrails; empty terminal nudge | Landed in `e9ae8f01` |
| MCP `list_changed`/keepalive; proactive prune; preflight compact; Gemini sentinel; MoA interrupt/timeout/prompt; hook cap | Landed in `5755caeb` |
| Durable background-job ledger and reasoning stall floors | Landed in `8ac96911` |
| ToolSearch size threshold | Landed in `7a515563` |
| Zero-LLM cron payload | Landed in `59287dee` |

Therefore the highest-value new slice is not another scheduler or compression
framework. It is a coherent **file-operation safety and self-recovery policy**
extracted from Hermes's post-tag hardening wave:

- `0e63ed1fe` / `e0b500598`: use `stat`-based special-file refusal, not a
  handful of dangerous path names;
- `fe66596df`: instruction files require approval even under broad auto-allow;
- `893792c99`: distinguish an empty file from a request beyond EOF;
- `fd452e26e`: diagnose Unicode-equivalent and near-miss paths;
- `2c8a932f8`: verify writes against the bytes actually present on disk;
- `1362ffc7d`: report binary types from content signatures, not only suffixes.

## 3. Implemented in this change

### 3.1 Descriptor-validated regular-file reads

`coco-utils-common::open_regular` is the cross-crate I/O boundary. It opens
first, then validates the opened descriptor rather than trusting a preceding
path lookup. Unix opens include `O_NONBLOCK | O_CLOEXEC`, so replacing a checked
path with a FIFO cannot hang a worker. Sync and async byte readers share this
contract. Large range reads inspect the prefix and stream lines from the same
descriptor instead of reopening the path.

`core/tools/src/tools/file_safety.rs` retains tool policy: diagnostic target
classification, mutation inspection with `symlink_metadata`, missing-path
recovery and atomic-commit proof conversion. Read, Write, Edit, NotebookEdit,
changed-file scans and file-history backups now use the validated I/O primitive
for actual bytes.

This is intentionally a breaking semantic change: `/dev/null` is no longer a
special exception, final symlinks are not mutation targets, and file tools mean
regular files.

### 3.2 Instruction integrity before every allow path

The shared write permission boundary recognizes the same instruction names that
`coco-context` loads, case-insensitively: `CLAUDE.md`, `AGENTS.md`,
`CLAUDE.local.md` and `AGENTS.local.md`. An explicit deny rule still wins.
Otherwise an original, intermediate or canonical match returns a one-operation
`Ask` with no persistent allow suggestion before internal-path, accept-edits,
tool-wide allow or bypass logic can run.

Bash applies the same check before its allow paths, including literal paths
inside interpreter commands. Redirect and write targets with multiple hard
links also require approval: shell tools mutate an inode in place, so an
ordinary-looking name could alias an instruction file. Local file tools instead
break hard links through atomic replacement, leaving the protected inode
unchanged.

The protected set comes from `coco-context`; the permission layer does not
maintain a second list that could drift from prompt loading.

### 3.3 Atomic, typed mutation results

Write, Edit and NotebookEdit commit a same-directory temporary file, fsync it,
atomically rename it over the target, fsync the parent directory on Unix, then
stream-compare the committed descriptor with the expected bytes. A mismatch is
a hard tool error. The success value is an unconstructible `VerifiedWrite`
proof serialized as literal `true`, so a typed output cannot represent
`verified: false`.

Existing final symlinks and non-regular entries are rejected. Atomic rename
intentionally breaks hard links and preserves existing permission mode bits.
The new inode is owned by the executing process; ACLs and extended attributes
are not copied, and this limitation is part of the common helper's documented
contract.

New Unix files use the normal `0o666` creation mode filtered through the process
umask instead of inheriting tempfile's private `0o600` default.

`coco-file-encoding::encode_with_format` is the single encoding path used to
compute Write's expected bytes, including BOM and line-ending normalization.
Read-state, skill-trigger and LSP bookkeeping happen only after proof. Blocking
disk work runs outside ordinary async polling; the commit itself is kept
cancellation-atomic.

ApplyPatch keeps its executor-backed filesystem abstraction. Equivalent
atomicity belongs on that abstraction so local and remote execution retain the
same semantics.

### 3.4 Actionable and bounded read diagnostics

Missing Read/Edit targets scan at most 512 sibling entries and return up to
three deterministically sorted suggestions. Directories larger than the bound
return no suggestion rather than a nondeterministic sample. A bare relative
name resolves against the session cwd. Unicode NFC, non-breaking/narrow spaces
and curly quotes are normalized for comparison; ordinary close typos use
normalized Levenshtein similarity. The parent directory and every candidate
must independently pass Read permission, ignore-pattern and sandbox policy, so
suggestions do not become a directory-listing side channel.

Empty text files report `totalLines: 0`; a past-EOF request retains its
separate offset warning. A bounded descriptor prefix recognizes PNG, JPEG, GIF,
PDF, ZIP, gzip, ELF, PE, WebAssembly, SQLite, RAR and 7z magic. This prevents
binary content with a text-looking name from reaching the text decoder.
Specialized image/PDF/notebook rendering remains extension-routed: signature
detection is a refusal diagnostic, not an implicit media execution path.

### 3.5 Operator-owned MCP execution trust

`McpExecutionPolicy::{AlwaysAsk, TrustReadOnlyHints, Full}` is resolved globally
and per server from `mcp.execution_policy` / `mcp.server_execution_policy`.
`AlwaysAsk` is the default. `TrustReadOnlyHints` is the only mode in which a
server's `readOnlyHint` can auto-approve a call; `Full` explicitly approves all
calls. The default `McpHandle` implementation also fails closed, so embedders
that omit policy wiring cannot silently inherit trust.

Dynamic MCP tools always report non-read-only and non-concurrency-safe to the
generic evaluator. Their dedicated permission check applies the typed policy;
server annotations therefore cannot upgrade themselves into a central read-
only fast path or parallel side-effect batch. Existing deny rules still run
first.

## 4. Architecture decisions

The implementation follows four constraints:

1. **Validate the object actually used.** Policy lookup may inspect a path for
   diagnostics, but byte reads validate the opened descriptor and mutation
   commits replace one directory entry atomically.
2. **One source of truth.** Protected names come from context discovery,
   encoding comes from the file-encoding crate, and MCP trust is typed operator
   policy rather than an interpretation scattered across tools.
3. **Bookkeeping only after proof.** History is captured before mutation, while
   read-state/LSP/skill bookkeeping happens only after `VerifiedWrite` exists.
4. **No compatibility branches.** The old device-name blocklist, `/dev/null`
   exception, symlink mutation behavior and implicit MCP hint trust are removed.

The change deliberately does not introduce a generic “file service” trait. The
local tools have one narrow descriptor/commit primitive; ApplyPatch remains on
the executor filesystem boundary.

## 5. Remaining scoped follow-ups

### P1: executor-backed ApplyPatch atomicity

Giving ApplyPatch the same atomic verified commit contract requires extending
the executor filesystem abstraction rather than reaching around it with local
`std::fs`. That work must cover local and remote implementations and preserve
the same explicit metadata contract.

### P2: stable/volatile prompt-cache boundaries (evidence required)

Hermes `214f2b82d` and `4c5be0c29` make the skill prompt's stable prefix an
explicit builder boundary. coco-rs already protects cache prefixes in several
paths. The audit did not confirm real prefix churn here, so no speculative
prompt rearchitecture was made; measure first and change only if traces show a
material miss source.

## 6. Explicit non-goals from this release

- Browser/computer use remains a project non-goal.
- Voice, consumer IM adapters, desktop HUD and Hermes-specific billing are not
  smuggled into the core runtime through this optimization pass.
- `AI_AGENT` attribution headers are provider/product metadata, not a generic
  agent-core feature.
- Python warm-start work is not transferable evidence for Rust startup work;
  measure coco-rs before adding lazy initialization.

## 7. Verification contract

Tests cover descriptor rejection of FIFOs, symlink mutation refusal, atomic
mode preservation and hard-link breakage, deterministic bounded suggestions,
binary signatures, empty-file rendering, protected names through file tools,
Bash redirects/interpreters/hard links, and all three MCP execution policies.
The required repository checks remain `just fmt`, targeted crate tests and
`just quick-check`.
