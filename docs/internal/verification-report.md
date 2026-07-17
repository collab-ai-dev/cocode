# coco-rs 文档完整性验证报告

验证日期: 2026-04-02 | 最近更新: 2026-04-03 (Round 5: String→Enum + cross-verification)
TS 文件总数: 1884
Plan doc 总数: 22 (含本报告)

> 2026-05-14 freshness note: this is a historical verification snapshot.
> Current fix ordering lives in `current-gap-fix-plan.md`; the Round 5 rows
> F16/F17/F18/F20/F21 below have been rechecked and are no longer open plan-doc
> blockers.

---

## 1. 目录级覆盖: 53/53 (100%)

所有 35 个第一层目录 + 18 个顶层文件全部在 ts-to-rust-mapping.md 中有映射。

**已知问题**:
- 7 个 v3 条目 crate 为 "TBD"（可接受，v3 暂不实现）
- 69 个条目缺少 Strategy 列（在补充表格中，主表格完整）

## 2. 子目录级覆盖: 190/190 (100%)

所有 31 个 `utils/` 子目录、36 个 `services/` 条目、40 个 `tools/` 子目录、67 个 `commands/` 子目录全部有映射覆盖。

## 3. Crate Doc 质量: 11/15 通过全部检查

| 检查结果 | 文件 | 问题 |
|----------|------|------|
| FAIL | crate-coco-types.md | 缺 `## Dependencies` 标题 + 无函数签名 |
| FAIL | crate-coco-modules.md | 缺顶层 TS source 行 + 缺 "does NOT depend" |
| FAIL | crate-coco-app.md | 缺 "does NOT depend" |
| FAIL | crate-coco-query.md | "depends on" 格式偏差（有括号插入） |

## 4. 类型唯一性: ~167 个类型，0 个 crate 间重复

Round 5 新增 13 个类型: ToolId, AgentTypeId, ToolName (41 variants), SubagentType (7 variants),
MessageKind, HookOutcome, CommandAvailability, CommandSource, UserType, Entrypoint,
NormalizedMessage, SystemMessageLevel, PartialCompactDirection。全部在 crate-coco-types.md 中定义。

已修复 (Round 4):
- `ThinkingLevel` → 已移至 crate-coco-config.md
- `ModelHub` → 已在 crate-coco-inference.md 添加定义

## 5. 依赖合规: 78 条依赖，0 个层级违反

## 6. 函数级覆盖抽样

| 文件 | TS 项目 | 已覆盖 | 延迟(v2/UI) | 缺失 | 覆盖率 |
|------|---------|--------|-------------|------|--------|
| Tool.ts | 47 | 24 | 15 | 4 | 75% |
| QueryEngine.ts | 7 | 4 | 0 | 3 | 57% |
| StreamingToolExecutor.ts | 19 | 14 | 0 | 4 | 74% |
| utils/messages.ts | ~93 | 7 | 0 | ~70 | 9% (P2) |
| services/mcp/client.ts | 26 | 12 | 0 | 14 | 46% |

## 7. 待修复项清单

### Must Fix (格式/完整性)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| F1 | crate-coco-types.md | 缺 `## Dependencies` 标题 | 将 `### Dependency Rule` 改为 `## Dependencies` |
| F2 | crate-coco-types.md | 缺 "does NOT depend" | 添加 |
| F3 | crate-coco-modules.md | 缺顶层 TS source 行 | 在文件头加 `TS source:` |
| F4 | crate-coco-modules.md | 缺 "does NOT depend" | 在依赖块中添加 |
| F5 | crate-coco-app.md | 缺 "does NOT depend" | 在各子 crate 依赖中添加 |
| F6 | crate-coco-query.md | "depends on" 格式偏差 | 移除括号 |
| F7 | multi-provider-plan.md | `ThinkingLevel` 无 canonical owner | 移到 crate-coco-config.md |
| F8 | crate-coco-inference.md | `ModelHub` struct 未定义 | 添加 pub struct ModelHub |
| F9 | crate-coco-tool.md | 缺 `prompt()`, `isLsp`, `strict`, `mapToolResultToToolResultBlockParam` | 添加到 trait |
| F10 | crate-coco-query.md | 缺 `interrupt()`, `getSessionId()`, `setModel()` | 添加到 impl |
| F11 | ts-to-rust-mapping.md | 69 条目缺 Strategy 列 | 补充表格加 Strategy 列 |

### 修复状态

| # | 问题 | 状态 |
|---|------|------|
| F1 | crate-coco-types.md 缺 Dependencies 标题 | **FIXED** |
| F2 | crate-coco-types.md 缺 "does NOT depend" | **FIXED** |
| F3 | crate-coco-modules.md 缺顶层 TS source | **FIXED** |
| F4 | crate-coco-modules.md 缺 "does NOT depend" | **FIXED** |
| F5 | crate-coco-app.md 缺 "does NOT depend" | **FIXED** |
| F6 | crate-coco-query.md 格式偏差 | **FIXED** |
| F7 | ThinkingLevel 无 canonical owner | **FIXED** → crate-coco-config.md |
| F8 | ModelHub struct 未在 inference doc 定义 | **FIXED** → added pub struct + impl |
| F9 | Tool trait 缺 4 个方法 | **FIXED** → added prompt, is_lsp, strict, map_tool_result_to_block |
| F10 | QueryEngine 缺 3 个方法 | **FIXED** → added interrupt, session_id, set_model |
| F11 | 69 条目缺 Strategy 列 | **FIXED** → all supplementary tables now have Strategy column |

### Must Fix (Round 5 — String→Enum cross-verification)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| F12 | crate-coco-types.md | `ToolName` enum (41 variants) referenced by ToolId::Builtin but not defined | 添加 `pub enum ToolName { Read, Write, Edit, Bash, Glob, Grep, ... }` |
| F13 | crate-coco-types.md | `SubagentType` enum (7 variants) referenced by AgentTypeId::Builtin but never defined | 添加 `pub enum SubagentType { Explore, Plan, Review, StatusLine, ClaudeCodeGuide, ... }` |
| F14 | crate-coco-hooks.md, crate-coco-config.md | `ShellType` used but undefined; coco-context defines `ShellKind` | 统一为 `ShellKind`，引用 coco-context 定义 |
| F15 | crate-coco-types.md, crate-coco-inference.md | `EffortValue` used but undefined;其余 10+ 处用 `EffortLevel` | 统一为 `EffortLevel`，解决 L1 依赖问题 |
| F16 | crate-coco-types.md | `BuiltinPluginDefinition` 引用 `PluginManifest` (L4 coco-plugins) — L1→L4 层级违反 | 移至 coco-plugins 或改 manifest 为 Value |
| F17 | crate-coco-skills.md | `SkillDefinition.hooks: Option<HooksSettings>` 但未声明 coco-hooks 依赖 | 改为 `Option<Value>` (config isolation pattern) |
| F18 | crate-coco-messages.md | `filter_by_role(role: MessageRole)` 但 MessageRole 未定义 | 改用 `MessageKind` (已在 coco-types 定义) |

### 修复状态 (Round 5)

| # | 问题 | 状态 |
|---|------|------|
| F12 | ToolName enum 未定义 | **FIXED** → added 41-variant enum to crate-coco-types.md |
| F13 | SubagentType enum 未定义 | **FIXED** → added 7-variant enum to crate-coco-types.md |
| F14 | ShellType vs ShellKind | **FIXED** → unified to ShellKind in coco-hooks + coco-config |
| F15 | EffortValue vs EffortLevel | **FIXED** → unified to EffortLevel in coco-types + coco-inference |
| F16 | BuiltinPluginDefinition 层级违反 | **FIXED** → `manifest` uses `serde_json::Value`; plugin crate deserializes |
| F17 | SkillDefinition.hooks 类型 | **FIXED** → `Option<serde_json::Value>` |
| F18 | MessageRole 未定义 | **SUPERSEDED** → message APIs use `MessageKind`; system-reminder has local `MessageRole` |
| F19 | ThinkingLevel 名称冲突 (config enum vs inference struct) | **FIXED** → 移除 config enum，struct 移至 coco-types，ModelInfo 字段恢复 |
| F20 | TaskStateBase 双重定义 (coco-types vs coco-tasks) | **FIXED** → `coco-tasks` uses `coco_types::TaskStateBase` |
| F21 | OAuthTokens 冲突 (coco-inference vs coco-mcp) | **ACCEPTED** → distinct crate-local structs; docs use `ApiOAuthTokens` / `McpOAuthTokens` naming for ownership clarity |

### String→Enum Audit Summary (Round 5 — 2026-04-03)

完成度: 全量 String→Enum 审计完成。67 个 String 字段确认为动态值 (正确保留 String)；
所有标识字段已转换为类型安全 enum (ToolId, AgentTypeId, MessageKind 等)。
变更范围: crate-coco-types.md (主要) + 10 个 crate doc 联动更新 + CLAUDE.md 同步。
详见 audit-gaps.md Cross-Review Round 5。

### P2 (已知延迟 — 实现时补充)

| # | 文件 | 问题 | 状态 |
|---|------|------|------|
| P1 | crate-coco-messages.md | ~70 个 messages.ts 函数未列出 | 实现时按类别文档化 |
| P2 | crate-coco-mcp.md | 14 个 client.ts 工具函数未列出 | 实现时补充 |
