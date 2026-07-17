# [DEPRECATED] Tool Input Schema — Validated Newtype + Single Parse Seam

> **此文档已废弃。**
>
> 继任方案：[tool-schema-source-plan.md](tool-schema-source-plan.md)
>
> 本文档保留作为历史记录，**不要按它实施**。

## 为何废弃

外部 review 在文档外指出 8 条 finding，**全部成立**。其中 4 条是架构层根本错误，
不是补丁能修的：

1. **Input↔Schema 类型绑定是错的不变量。**
   旧 plan 主张 `type InputSchema: InputSchemaSource<Input = Self::Input>` —— 编译期强制
   schema 由 Input 派生。但 `coco-rs` 已有 6 个静态工具的 model schema **故意与 Input 解耦或更窄**：
   - `AgentTool` —— 隐藏 `mcp_servers`（内部字段，permission/hook 改写时用），手动 `required: [description, prompt]`
   - `BashTool` —— 隐藏 `_simulatedSedEdit`（TUI dialog payload）
   - `AskUserQuestionTool` —— `questions` / `answers` / `annotations` / `metadata` 都是 `Value`，约束在手写 schema 里
   - `TodoWriteTool` —— 先 `derive_input_schema::<TodoWriteInput>()` 再修改 `status` enum
   - `WebFetchTool` / `WebSearchTool` —— 完全手写

   按旧 plan，这 6 个工具的 schema 会被 schemars 派生替换，丢失内部字段隐藏 + 手动 required 约束 + Value 字段约束。

2. **"抄 codex-rs strict subset" 引用错误。**
   旧 plan 反复引用 `codex-rs::parse_tool_input_schema` 并列出"拒绝 anyOf / $ref / type 数组 / type:null"的清单。
   **codex-rs 实际是 `sanitize_json_schema` —— 保留 `anyOf` 和 `$ref`，从形状推断缺失的 `type`，归一化 type 数组，只拒绝最致命的根 singleton `type:null`。**
   按旧 plan 的拒绝清单实施：
   - schemars 1.2 给 `Option<T>` 派生的 `anyOf` 形态 → 每个有 Option 字段的静态工具启动失败
   - 真实 MCP 服务器普遍用 `$ref` → 真实 MCP 集成全部打死
   - 旧 plan 想保护的 schemars 派生路径，反而被它自己的 strict-subset 算法拒绝

3. **当前生产隐性 bug B2 不在旧 plan 视野内。**
   `core/tool-runtime/src/schema.rs::effective_tool_schema`（validator 用）优先用完整 `input_json_schema()` 包络；
   `services/inference/src/tool_schemas.rs::generate_tool_schemas`（model 看到的）只用浅 `ToolInputSchema.properties`，连 `required` 都丢。
   两条路径生产**不同**的 schema —— 当前生产隐性 bug。旧 plan 没意识到，cache key hash 修不对，trait surface 收紧也无意义。

4. **Phase 阶段独立性不成立。**
   旧 plan Phase 3 给 `impl Tool for ReadTool` 加 `type InputSchema = TypedSchema<ReadInput>;`，
   但 `Tool` trait 直到 Phase 4 才有该 associated type —— Phase 3 单独无法编译。"每阶段 CI 绿"是假的。

完整 review 见 [tool-schema-source-plan.md#review-summary](tool-schema-source-plan.md#review-summary)。

## 新方案变更要点

| 维度 | 旧 plan (validated-newtype) | 新 plan (three-source) |
|------|------|------|
| Schema 与 Input 关系 | type-level 绑定 `<Input = Self::Input>` | **解耦**：`type InputSchema: InputSchemaSource`（无 Input bound） |
| Schema 源类型 | 2 种：`TypedSchema` / `DynamicSchema` | **3 种**：`TypedSchema<I>` / `ManualSchema` / `DynamicSchema` |
| Strict-subset 语义 | reject 11 种形状 | **sanitize-preserve**，照搬真 codex-rs |
| `SchemaError` 变体 | 11 个 | **3 个** |
| 隐性 bug B2（两条生产路径分歧）| 不知道存在 | **Phase 0 先合并** |
| Trait surface 新增时机 | Phase 4 | **Phase 1（带 delegate）**，让 Phase 2/3 真独立 |
| StructuredOutputTool | 走 `from_wire`（破坏 array-root 合约）| **`ManualSchema::lax`**，保留宽松 |

请按 [tool-schema-source-plan.md](tool-schema-source-plan.md) 实施。
