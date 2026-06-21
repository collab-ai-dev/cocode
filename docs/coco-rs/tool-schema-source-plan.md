# Plan: Tool Input Schema — Three Sources + Sanitize-Preserving + Unified Producer

> Supersedes [tool-schema-validated-newtype-plan.md](tool-schema-validated-newtype-plan.md).
> 旧 plan 的"Input↔Schema 类型绑定 + reject-style strict subset"在 review
> 中被证伪：它会破坏 AgentTool/BashTool/AskUserQuestion 等 6 个手写 schema
> 的工具，并且与 schemars 派生 `Option<T>` 的 anyOf 形态、真实 MCP 服务器
> 的 `$ref` 用法不兼容。本文档替代之。

## Context

`coco-rs` 的 `Tool` trait 现状（`core/tool-runtime/src/traits.rs:382-391`）：

```rust
pub trait Tool: Send + Sync + 'static {
    type Input: for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static;
    type Output: Serialize + for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static;

    fn input_schema(&self) -> ToolInputSchema { /* default derives from Input */ }
    fn input_json_schema(&self) -> Option<Value> { /* default derives from Input */ }
    fn input_schema_for_session(&self, _ctx: &SchemaContext) -> ToolInputSchema { ... }
    fn input_json_schema_for_session(&self, _ctx: &SchemaContext) -> Option<Value> { ... }
    // ...
}
```

把三件**本质独立**的事捆在 `Self::Input` 上：
1. **反序列化**（`Deserialize`）—— wire→typed 边界
2. **schema 派生**（`JsonSchema`）—— 给 LLM / strict provider 看的 schema 来源
3. **执行 payload 类型**（`execute(input: Self::Input, ...)`）—— 业务 seam

41 个静态工具 + 2 个动态工具 (`McpTool` / `StructuredOutputTool`) 共用这一接口。
动态工具把 `type Input = Value`，schemars 给 `Value` 派生的 schema
是 garbage（`type: null` 或 `anyOf:[...]`），strict OpenAI-compatible 提供商
（DeepSeek、xAI Grok strict 等）会 wire-level 400 拒绝。

### 当前生产状态

| 路径 | 实现 | `type Input` | 当前补救 |
|------|------|--------------|----------|
| External MCP server | `McpTool` | `Value` | ✅ X3 fix `0303dc3ef2` override `input_json_schema()` |
| SDK custom tool（in-process MCP） | `McpTool` | `Value` | ✅ 同上 |
| `--json-schema` 结构化输出 | `StructuredOutputTool` | `Value` | ✅ P0 fix（本计划触发）override |
| 41 个静态工具 | typed struct（**unit struct**） | typed | N/A |

其中 6 个静态工具的模型可见 schema **不等于** Input 派生的 schema —— 不是
trivial unit struct + derive。这一事实在旧 plan 中被忽略，是它失败的根因之一。

### 当前已知 bug

| # | bug | 状态 |
|---|-----|------|
| B1 | release build 无 `debug_assert!` 保护，任何漏 override 的动态 Tool 静默坏 | 部分缓解（McpTool / StructuredOutputTool 显式 override），但根因仍在 |
| B2 | `core/tool-runtime/src/schema.rs::effective_tool_schema`（validator 用）和 `services/inference/src/tool_schemas.rs::generate_tool_schemas`（model 看到的）**生产不同的 schema** —— 后者只用浅 `ToolInputSchema.properties`，连 `required` 都丢；前者优先用完整 `input_json_schema()` 包络 | **未修复**（review 阶段新发现） |
| B3 | `ToolSchemaValidator` cache key 用 `ToolId` 而 schema 内容可能在 MCP 重连或 `register_mcp_tools` 序列后变化，旧 validator 残留 | **未修复**（review 阶段新发现） |
| B4 | `core/tools/CLAUDE.md` 写 "MCPTool is the only dynamic tool"，但 `StructuredOutputTool` 同样 `type Input = Value` —— 文档自己漂移 | 文字层面已修，结构性不可能再漂移留待本计划 |
| B5 | 文档"MUST override" 是文化约定，未来 Plugin / Custom Tool 框架会重蹈覆辙 | **未修复** |

### 旧 plan（validated-newtype）被否决的原因

外部 review 在文档外指出 4 个深层问题（详见 [review summary](#review-summary)）：

1. **Input↔Schema 类型绑定是错的不变量** —— AgentTool 隐藏 `mcp_servers`，BashTool 隐藏 `_simulatedSedEdit`，AskUserQuestionTool 4 个字段是 `Value`，TodoWriteTool 先 derive 再修改 enum，WebFetchTool/WebSearchTool 完全手写 —— 这 6 个工具的 schema **故意比 Input 窄**或与 Input 解耦。`InputSchema: InputSchemaSource<Input = Self::Input>` bound 会直接打死它们。
2. **"抄 codex-rs strict subset" 引用错误** —— codex-rs `parse_tool_input_schema` 实际是 `sanitize_json_schema`：**保留** `anyOf` / `$ref` / 嵌套 `$defs`，按形状推断缺失的 `type`，只拒最致命的 singleton `type:null`。旧 plan 拒绝 `anyOf` / `$ref` / 数组 `type` 的清单 —— schemars 1.2 给 `Option<T>` 派生的就是 `anyOf`，真实 MCP 服务器普遍用 `$ref` —— 按旧 plan 执行会同时打死自家静态工具和真实 MCP 集成。
3. **B2 隐性 bug 不在旧 plan 视野内** —— 不先合并两条生产路径，cache hash 修不对，trait surface 收紧也无意义。
4. **Phase 阶段独立性不成立** —— Phase 3 给 `impl Tool for ReadTool` 加 `type InputSchema = ...`，但该 associated type 直到 Phase 4 才进 trait —— Phase 3 单独无法编译。

---

## Problem Statement

**核心目标**：把"schema 是否合法"从文化约定下沉为类型系统 + 单一构造路径
的硬约束，同时保留 41 个静态工具的 typed `execute(input: Self::Input)` seam。

**反目标**：不强制"schema 由 Input 派生"。这是错的不变量。

**衍生目标**：
- 修 B2（两条生产路径分歧 —— 当前隐性 bug）
- 修 B3（cache key 不含 schema 内容）
- 给将来 Plugin / Custom / HTTP Tool 留 by-construction 的入口

---

## Architecture Choice — Option J（Three Sources）

### 核心抽象：`InputSchemaSource` trait + 三个实现

```rust
// core/tool-runtime/src/schema_source.rs
pub trait InputSchemaSource: Send + Sync + 'static {
    /// 返回完整、已校验、wire-合法的 schema。
    fn schema(&self) -> &ToolInputSchema;
}
```

**注意没有** `type Input` —— schema 类型与 Input 类型在 type system 层
**解耦**。Tool 的 schema 来源是它自己的选择，不需要与 Input 派生一致。

#### 1. `TypedSchema<I>` — schemars 派生

```rust
pub struct TypedSchema<I: schemars::JsonSchema> {
    schema: ToolInputSchema,
    _marker: PhantomData<fn() -> I>,
}

impl<I: schemars::JsonSchema> TypedSchema<I> {
    /// 启动期构造：派生 schema + sanitize；不通过 sanitize 启动期 panic。
    pub fn new() -> Self {
        let raw = schemars_derive_value::<I>();
        let schema = ToolInputSchema::sanitize(raw)
            .expect("TypedSchema: schemars-derived schema must sanitize cleanly");
        Self { schema, _marker: PhantomData }
    }
}

impl<I: schemars::JsonSchema + Send + Sync + 'static> InputSchemaSource for TypedSchema<I> {
    fn schema(&self) -> &ToolInputSchema { &self.schema }
}
```

适用：~35 个静态工具，schema 完全等于 Input 派生的 schema（典型如 `ReadTool` / `WriteTool` / `EditTool` / `GlobTool` / 等）。

#### 2. `ManualSchema` — 手写 Value + sanitize 校验

```rust
pub struct ManualSchema {
    schema: ToolInputSchema,
}

impl ManualSchema {
    /// 严格模式：sanitize + 拒最致命问题。
    /// 用于：AgentTool / BashTool / AskUserQuestionTool / TodoWriteTool /
    ///       WebFetchTool / WebSearchTool（手写 schema，要求 wire 合法）。
    pub fn from_value(raw: Value) -> Result<Self, SchemaError> {
        Ok(Self { schema: ToolInputSchema::sanitize(raw)? })
    }

    /// 宽松模式：跳过 sanitize 校验，只跑 jsonschema 自己的 meta-validation。
    /// 用于：StructuredOutputTool —— 它是终态 assistant 输出（不走 strict
    /// provider 的 tool API），strict-subset 约束不适用；用户给的 schema
    /// 可能是顶层 array、`oneOf`、缺 `type` —— 全部合法。
    pub fn lax(raw: Value) -> Result<Self, SchemaError> {
        Ok(Self { schema: ToolInputSchema::from_value_lax(raw)? })
    }
}

impl InputSchemaSource for ManualSchema {
    fn schema(&self) -> &ToolInputSchema { &self.schema }
}
```

适用：6 个手写 schema 静态工具 + `StructuredOutputTool`（lax 路径）。

#### 3. `DynamicSchema` — wire-provided

```rust
pub struct DynamicSchema {
    schema: ToolInputSchema,
}

impl DynamicSchema {
    /// MCP 服务器 / SDK 转发 / 未来 PluginTool 的 wire schema 入口。
    pub fn from_wire(raw: Value) -> Result<Self, SchemaError> {
        Ok(Self { schema: ToolInputSchema::sanitize(raw)? })
    }
}

impl InputSchemaSource for DynamicSchema {
    fn schema(&self) -> &ToolInputSchema { &self.schema }
}
```

适用：`McpTool` + 未来 PluginTool / CustomTool / HTTPTool。

#### 4. `ToolInputSchema` 升级为完整 schema newtype

```rust
// core/tool-runtime/src/schema.rs（替代 common/types::ToolInputSchema）
pub struct ToolInputSchema {
    /// 完整 JSON Schema envelope，**保证根是 object 且含 type:"object"**。
    /// 私有字段：所有公开构造器走 sanitize / lax 校验，
    /// 外部不可能拿到未校验的实例。
    inner: serde_json::Value,
}

impl ToolInputSchema {
    /// 严格构造：照搬 codex-rs `sanitize_json_schema` 真实算法。
    /// - 保留 `anyOf` / `oneOf` / `allOf`
    /// - 保留 `$ref` 和 reachable `$defs` / `definitions`
    /// - 类型推断：缺 `type` 时按形状推断（有 `properties` → object，
    ///   有 `items` → array，等）
    /// - 归一化：`type: ["X","null"]` 接受；`true` schema 转为 `{type:"string"}`
    /// - 拒绝（仅此两种）：根 singleton `type:"null"`、根不是 object
    pub(crate) fn sanitize(raw: Value) -> Result<Self, SchemaError>;

    /// 宽松构造：仅校验"`jsonschema::validator_for` 能编译"，
    /// 不强制 object 根、不做 sanitize。
    /// 仅供 ManualSchema::lax 使用。
    pub(crate) fn from_value_lax(raw: Value) -> Result<Self, SchemaError>;

    // 视图方法
    pub fn as_value(&self) -> &Value { &self.inner }
    pub fn as_object(&self) -> Option<&serde_json::Map<String, Value>> { self.inner.as_object() }

    /// Session-aware mutator：返回去掉指定字段的 owned 副本。
    /// AgentTool `lazySchema().omit({run_in_background})` 等价物。
    pub fn omit_property(self, field: &str) -> Self;

    /// Canonical JSON 字节，用于 cache key hash（BTreeMap-ordered re-serialize）。
    pub fn stable_hash(&self) -> u64;
}

impl Clone for ToolInputSchema { /* derive */ }

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("schema root must be a JSON object with type:\"object\"")]
    RootNotObject,
    #[error("schema root is the singleton null type, rejected to avoid wire 400")]
    RootTypeNull,
    /// jsonschema 自身的 meta-validation 失败（lax 路径用）。
    #[error("schema failed JSON Schema meta-validation: {0}")]
    InvalidSchema(String),
}
```

**关键设计点**：
- `inner: Value`（不是 `Map<String, Value>`），所以 `as_value(&self) -> &Value` 可以直接借用 —— 修了旧 plan API 的内部不一致。
- `SchemaError` 只 3 个变体。旧 plan 的 11 个变体（`DisallowedTopLevel` / `UnresolvedRef` / `TypeAsArray` 等）全部消失 —— sanitize 路径吸收。
- 任何持有 `ToolInputSchema` 的代码**不可能**绕过校验，因为构造器全是 `pub(crate)` 或通过 `TypedSchema` / `ManualSchema` / `DynamicSchema` 间接。

### Sanitize 算法（核心）

照搬 `codex-rs/tools/src/json_schema.rs:392 sanitize_json_schema` 的递归处理。
**不重新设计**。具体步骤：

1. 递归遍历整个 schema 树。
2. **保留** `anyOf` / `oneOf` / `allOf` / `$ref` / `$defs` / `definitions`。
3. 缺 `type` 时按形状推断：
   - 有 `properties` / `required` / `additionalProperties` → `object`
   - 有 `items` / `prefixItems` → `array`
   - 有 `enum` / `format` → `string`
   - 有 `minimum` / `maximum` / `multipleOf` → `number`
   - 仅有 `$ref` / `anyOf` → 不补 `type`
   - 都没有 → 退化为 `{}`（permissive）
4. `type: ["X","null"]` 数组形态接受。
5. `true` / `false` schema 形态转为 permissive object。
6. `const` 折成单值 `enum`。
7. 修剪 unreachable `$defs` / `definitions`。
8. 在最后验证根是 `{type: "object", ...}` —— 否则 `RootNotObject`。
9. 检测根 singleton `type: "null"` —— `RootTypeNull`。

**为什么不重写**：codex-rs 已有完整 negative-case 测试集（`json_schema_tests.rs`），
porting 时一并搬运。TS-parity 保证。

### Tool trait 收紧（最终形态）

```rust
// core/tool-runtime/src/traits.rs
pub trait Tool: Send + Sync + 'static {
    type Input: for<'de> Deserialize<'de> + Send + Sync + 'static;
    //         ↑ 删 JsonSchema bound —— Value 不再"伪合规"

    /// Schema 源。**不绑定 Input 类型**。
    type InputSchema: InputSchemaSource;

    type Output: Serialize + for<'de> Deserialize<'de> + JsonSchema + Send + Sync + 'static;

    /// Schema 源访问器 —— 无 default，漏写是 `error[E0046]`。
    fn input_schema_source(&self) -> &Self::InputSchema;

    /// Runtime validation schema。静态，借引用。
    fn validation_input_schema(&self) -> &ToolInputSchema {
        self.input_schema_source().schema()
    }

    /// Model-visible schema。session-aware tool（AgentTool）可 own 一个变形副本。
    /// TS-parity 来源：`AgentTool.tsx:110-125 lazySchema().omit(...)`。
    fn model_input_schema(&self, _ctx: &SchemaContext) -> Cow<'_, ToolInputSchema> {
        Cow::Borrowed(self.validation_input_schema())
    }

    // execute / render_for_model 等保持 typed
    async fn execute(&self, input: Self::Input, ctx: &ToolUseContext) -> Result<...>;
    fn render_for_model(&self, output: &Self::Output) -> Vec<...>;
}
```

**删除**（所有四个老方法）：
- `Tool::input_schema()` —— 返回老 `coco_types::ToolInputSchema { properties, required }`
- `Tool::input_json_schema()`
- `Tool::input_schema_for_session()`
- `Tool::input_json_schema_for_session()`

**删除 bound**：`Self::Input: JsonSchema`。

**保留两种 schema 语义**：
- `validation_input_schema()` —— 执行前校验，静态
- `model_input_schema(ctx)` —— 模型可见，可 session-mutate

### 三种 source 的设计不变量

| 不变量 | 强制机制 |
|--------|---------|
| 任何 `ToolInputSchema` 实例都通过 sanitize / lax 路径 | `inner: Value` 私有；公开构造器只有 `sanitize` / `from_value_lax`（都 `pub(crate)`） |
| 严格路径下 schema wire 合法 | sanitize 拒 `RootNotObject` / `RootTypeNull`；中间形态归一化 |
| Tool 永远不会"忘 override" schema | `input_schema_source()` 无 default，漏写 `error[E0046]` |
| 动态 schema 必经 `from_wire` | `DynamicSchema` 只有 `from_wire` 构造器；`ToolInputSchema::sanitize` 是 `pub(crate)` |
| 手写 schema 必经 sanitize（严格）或 lax（StructuredOutputTool） | `ManualSchema::from_value` / `lax` 是唯一入口 |
| Validator cache 不会用旧 schema | cache key 含 `schema.stable_hash()` |
| Schema 与 Input 解耦 | `type InputSchema: InputSchemaSource`（无 `Input = ...` bound）—— **故意不绑定** |

---

## Migration Plan

按依赖顺序分阶段，每阶段独立可 commit、独立 CI 绿。

### Phase 0 — 修隐性 bug + 安全 ground（不依赖新 trait surface）

**目标**：先修 B2（两条生产路径分歧）+ B3（cache key 不含 schema 内容）。
这两件事**与新 trait surface 解耦**，可独立 ship。

- **0.a 统一两条 model-facing schema 生产路径**
  - 当前 `services/inference/src/tool_schemas.rs:55-77 generate_tool_schemas` 只用浅 `ToolInputSchema.properties`，丢 `required`、`additionalProperties`、`$defs` 等。
  - 改为：消费完整 `Value`（`effective_tool_schema(tool)` 的产物）。
  - 字段改造：`ToolSchemaSource.input_schema: ToolInputSchema` → `input_schema: Value`。
  - 上游 `ToolRegistry::definitions()` 也要同步：返回 `Vec<(String, Value)>` 或新建 `definitions_v2()` 走完整 schema 路径。
  - 单点：让 `effective_tool_schema` 成为**唯一**的 model-facing schema 生产函数；`tool_schemas::generate_tool_schemas` 调用它。

- **0.b Cache key 加 schema hash**
  - `ToolSchemaValidator::cache` key `ToolId` → `(ToolId, u64)`。
  - hash 来源：`effective_tool_schema(tool)` 的 canonical bytes（BTreeMap re-serialize）。
  - 同时改 `validate` 和 `validate_collect` 两个入口。
  - regression test：MCP 重连同名 tool 改 schema → cache miss + 新 validator。

**改动文件**：~4 个（`services/inference/src/tool_schemas.rs` + `core/tool-runtime/src/{schema,registry}.rs` + 调用点）
**估算工时**：1 day
**阻塞**：无
**独立可 ship**：✓ 当前生产 bug 修复，与本计划其余部分解耦

### Phase 1 — 新基础设施引入 + trait surface 提前

**目标**：把新 trait surface 提前到 Phase 1（带默认 delegate），其余阶段才能真独立。

- 新增 `core/tool-runtime/src/schema_source.rs`：`InputSchemaSource` trait + `TypedSchema<I>` + `ManualSchema` + `DynamicSchema`。
- 新增 `core/tool-runtime/src/schema_v2.rs::ToolInputSchemaV2`（V2 后缀仅 Phase 1-4 期间用；Phase 4 rename 回 `ToolInputSchema`）：
  - `sanitize` / `from_value_lax`：照搬 codex-rs `sanitize_json_schema` 算法 + jsonschema meta-validate。
  - `omit_property` / `stable_hash` / `as_value`。
- `SchemaError` 3 变体，挂 `coco-error::StatusCode::Resource`。
- **`Tool` trait 同步加两件事**（带默认实现 delegate 到旧方法）：
  ```rust
  type InputSchema: InputSchemaSource = LegacyAdapter<Self>;
  //                                  ↑ 默认 delegate 到老 input_schema()
  fn input_schema_source(&self) -> &Self::InputSchema { ... }
  ```
  其中 `LegacyAdapter<T: Tool>` 内部调 `T::input_schema()` 生成 `ToolInputSchemaV2`。
  这样 Phase 1 之后所有 Tool 仍编译，Phase 2/3 可以增量改某些工具，其余继续走 legacy delegate。
- 单元测试：每个 sanitize 边界（`anyOf` 保留 / `$ref` 保留 / type 数组归一 / type 推断 / RootTypeNull 拒绝）

**改动文件**：~6 个新文件 + `core/tool-runtime/src/traits.rs`
**估算工时**：1.5 day
**阻塞**：无（与 Phase 0 可并行）
**独立可 ship**：✓ 新类型并存，旧路径未改

### Phase 2 — 动态 Tool 迁移

**目标**：`McpTool` 走 `DynamicSchema::from_wire`，`StructuredOutputTool` 走 `ManualSchema::lax`。

- `core/tools/src/tools/mcp_tools.rs::McpTool`
  - 加字段 `schema: DynamicSchema`
  - `McpTool::new` → `Result<Self, SchemaError>`，内部 `DynamicSchema::from_wire(wire_schema)`
  - `Tool::input_json_schema` 老 override 删除（schema 现在通过 `input_schema_source().schema().as_value()`）
- `core/tools/src/tools/structured_output.rs::StructuredOutputTool`
  - 加字段 `schema: ManualSchema`
  - `StructuredOutputTool::new` 内部走 `ManualSchema::lax(schema)` —— **保留宽松行为**（用户顶层 array / `oneOf` 仍合法）
  - 删除老 `input_json_schema` override
- `core/tools/src/lib.rs::register_mcp_tools`
  - 签名改为返回 `RegisterMcpResult { registered: Vec<ToolId>, skipped: Vec<(String, SchemaError)> }`
  - invalid wire schema 的 server tool 跳过 + 进 `skipped` 列表
- `app/cli/src/headless.rs::register_structured_output_tool` 处理 `Result<_, SchemaError>`
- 调用方决定 skipped tools 呈现策略（建议：warn log + init 事件附带）

**改动文件**：~6 个
**估算工时**：0.5 day
**阻塞**：Phase 1
**独立可 ship**：✓

### Phase 3 — 静态 Tool 批量迁移

**目标**：41 个静态工具按 schema 来源**两路**迁移。

#### 分类（已对照源码确认）

| Source 类型 | Tool count | 工具列表 |
|---|---|---|
| **TypedSchema** | ~35 | Read / Write / Edit / Glob / Grep / NotebookEdit / ApplyPatch / PowerShell / REPL / Sleep / ToolSearch / Skill / SendMessage / TeamCreate / TeamDelete / TaskCreate / TaskGet / TaskList / TaskUpdate / TaskStop / TaskOutput / EnterPlanMode / ExitPlanMode / VerifyPlanExecution / EnterWorktree / ExitWorktree / Config / Brief / Lsp / McpAuth / ListMcpResources / ReadMcpResource / CronCreate / CronDelete / CronList / RemoteTrigger |
| **ManualSchema** (严格) | 6 | AgentTool / BashTool / AskUserQuestionTool / TodoWriteTool / WebFetchTool / WebSearchTool |
| ManualSchema::lax | 1 | StructuredOutputTool（Phase 2 已完成） |
| DynamicSchema | 1 | McpTool（Phase 2 已完成） |

#### TypedSchema 工具迁移模式（机械化）

```diff
-pub struct ReadTool;
+pub struct ReadTool {
+    schema: TypedSchema<ReadInput>,
+}
+impl ReadTool {
+    pub fn new() -> Self { Self { schema: TypedSchema::new() } }
+}
+impl Default for ReadTool {
+    fn default() -> Self { Self::new() }
+}

 impl Tool for ReadTool {
     type Input = ReadInput;
+    type InputSchema = TypedSchema<ReadInput>;
+    fn input_schema_source(&self) -> &Self::InputSchema { &self.schema }
-    fn input_schema(&self) -> ToolInputSchema {
-        coco_tool_runtime::derive::derive_input_schema::<ReadInput>()
-    }
 }
```

注册点 `register_all_tools` / `register_core_tools`：

```diff
-registry.register(Arc::new(ReadTool));
+registry.register(Arc::new(ReadTool::new()));
```

#### ManualSchema 工具迁移模式（AgentTool 示例）

```diff
-pub struct AgentTool;
+pub struct AgentTool {
+    schema: ManualSchema,
+}
+impl AgentTool {
+    pub fn new() -> Self {
+        Self { schema: ManualSchema::from_value(Self::build_schema())
+            .expect("AgentTool schema must sanitize") }
+    }
+    fn build_schema() -> Value {
+        json!({
+            "type": "object",
+            "properties": { /* 原 input_schema() 内容（含 mcp_servers 不暴露） */ },
+            "required": ["description", "prompt"]
+        })
+    }
+}

 impl Tool for AgentTool {
     type Input = AgentInput;
+    type InputSchema = ManualSchema;
+    fn input_schema_source(&self) -> &Self::InputSchema { &self.schema }

-    fn input_schema(&self) -> ToolInputSchema { /* 老手写 properties */ }
-    fn input_json_schema(&self) -> Option<Value> { ... }
-    fn input_schema_for_session(&self, ctx) -> ToolInputSchema { ... }
-    fn input_json_schema_for_session(&self, ctx) -> Option<Value> { ... }

+    fn model_input_schema(&self, ctx: &SchemaContext) -> Cow<'_, ToolInputSchema> {
+        if ctx.background_tasks_disabled || ctx.fork_mode_active {
+            Cow::Owned(self.schema.schema().clone().omit_property("run_in_background"))
+        } else {
+            Cow::Borrowed(self.schema.schema())
+        }
+    }
 }
```

`TodoWriteTool` 的"先 derive 后修改 enum"模式同样收敛到 `ManualSchema`：
在 `build_schema()` 里先调 schemars，再修改 enum，最后 `ManualSchema::from_value`。

#### 分批 PR 策略

| PR # | 范围 | Tool count | Source |
|---|---|---|---|
| 3.1 | File I/O：Read / Write / Edit / Glob / Grep / NotebookEdit / ApplyPatch | 7 | TypedSchema |
| 3.2 | Shell / 调度：PowerShell / REPL / Sleep / CronCreate / CronDelete / CronList / RemoteTrigger | 7 | TypedSchema |
| 3.3 | Agent / Skill / Team：Skill / SendMessage / TeamCreate / TeamDelete | 4 | TypedSchema |
| 3.4 | Task V2：TaskCreate / Get / List / Update / Stop / Output | 6 | TypedSchema |
| 3.5 | Plan / Worktree / Misc：EnterPlanMode / ExitPlanMode / VerifyPlanExecution / EnterWorktree / ExitWorktree / Config / Brief / Lsp / McpAuth / ListMcpResources / ReadMcpResource / ToolSearch | 12 | TypedSchema |
| 3.6 | ManualSchema 工具：AgentTool / BashTool / AskUserQuestionTool / TodoWriteTool / WebFetchTool / WebSearchTool | 6 | ManualSchema |

每 PR 独立 `just pre-commit`、独立 review、独立 merge。

**改动行数**：~600 行净增（每 Tool ~12-15 行）
**估算工时**：1.5 day
**阻塞**：Phase 1（不依赖 Phase 2）

### Phase 4 — Trait surface 收紧（破坏性变更）

**目标**：删除老 trait 方法 + `JsonSchema` bound + 老 `ToolInputSchema` struct。

- `core/tool-runtime/src/traits.rs`
  - 删 `Tool::input_schema()` / `input_json_schema()` / `input_schema_for_session()` / `input_json_schema_for_session()`
  - 删 `Self::Input: JsonSchema` bound
  - 删 `LegacyAdapter`（Phase 1 引入的过渡桥）
  - 把 `type InputSchema: InputSchemaSource` 的 default 拆掉，强制每个 Tool 写
- `core/tool-runtime/src/derive.rs::derive_input_schema_value` 改 `pub(crate)`（仅供 `TypedSchema::new` 内部用）
- `common/types/src/tool.rs::ToolInputSchema` 老 struct **删除**
- `core/tool-runtime/src/schema_v2.rs::ToolInputSchemaV2` rename → `ToolInputSchema`
- 消费侧：`app/query/src/engine_prompt.rs::build_language_model_tools` 改 `tool.model_input_schema(&ctx).as_value()`；`app/query/src/tool_input_validate.rs` 改 `tool.validation_input_schema()`

**改动文件**：~15 个
**估算工时**：1 day
**阻塞**：Phase 2 + Phase 3 全完
**注意**：破坏性变更，集中一次 ship

### Phase 5 — 加固

- `core/tool-runtime/CLAUDE.md`：明确"任何 `type Input = Value` Tool 必须 by-construction 走 `DynamicSchema::from_wire` 或 `ManualSchema::lax`"
- `core/tools/CLAUDE.md`：删除 "MCPTool is the only dynamic tool" 旧描述（已在 Phase 0 文档层级修过，此处补结构层面）
- Integration test：`coco-rs/tests/all_tools_schema_sanitize.rs` 扫描全 registry，断言每个工具 `validation_input_schema().as_value()` 通过 sanitize
- 自定义 clippy lint（可选）：`type Input = Value` 必须配 `type InputSchema in {DynamicSchema, ManualSchema}`

**改动文件**：~3-5 个
**估算工时**：0.5 day
**阻塞**：Phase 4

### Phase 6 — AgentTool DTO 拆分（单独立项）

**目标**：把"模型可见输入"与"运行时执行 payload"完全分离。

```rust
pub struct AgentToolWireInput { /* 模型可见字段 */ }
pub struct AgentToolRuntimeInput { /* 完整运行时字段（含 mcp_servers）*/ }

impl Tool for AgentTool {
    type Input = AgentToolWireInput;
    // execute() 把 wire input 加上 runtime-resolved 字段拼成 RuntimeInput
}
```

**独立 PR**：仅影响 AgentTool；与主问题正交；阻塞 = Phase 4 完成。

---

## Risks & Mitigations

| 风险 | 严重度 | 缓解 |
|------|-------|------|
| Phase 3 改 41 个 Tool 量大 review 累 | M | 已拆 6 个 sub-PR（分 source 类型）；每 PR 独立 `just pre-commit` |
| 静态 Tool unit struct → struct 影响 ergonomics | M | 各 sub-PR 同步改注册点 + 测试构造；机械替换可脚本辅助 |
| Tool 加 schema 字段，内存膨胀 | L | 每个 Tool ~100-500 bytes；41 个共 < 20 KB；registry singleton |
| TypedSchema startup-time panic 难懂 | L | panic 消息指引"use ManualSchema for hand-written schemas"；startup integration test 提前 catch |
| 消费侧 `&ToolInputSchema` borrow 链调整 | M | Phase 3 后期实测；困难时单点改 `Arc<ToolInputSchema>` |
| `Cow<'_, ToolInputSchema>` session-aware 返回类型 | L | 现有 session-aware 调用点 ≤ 5 处，手改 |
| sanitize 算法与 codex-rs 漂移 | M | Phase 1 单元测试照搬 codex-rs negative case；后续 codex-rs 更新同步 |
| Validator cache key 改动影响其它使用方 | L | Phase 0 单独 ship，灰度时间足够 |
| Plugin 框架对接 | L | Phase 4 完成后 Plugin 框架立项；`DynamicSchema::from_wire` 已是 API 边界 |
| 旧 `coco_types::ToolInputSchema` 多 crate 引用（B2 暴露的） | M | Phase 0 已收敛到完整 `Value` 流；Phase 4 时只剩 type 内部位置移动 |

### 已知架构问题不在本计划范围

| 问题 | 处理 |
|------|------|
| `Self::Output: JsonSchema` bound 不对称 | 暂保留 —— Output 历史无 `Value` 问题；未来出现同模式加 `OutputSchemaSource` |
| `engine.rs::run_session_loop` 1900 LoC | 独立工作（已记入 X2 deferred） |
| `turn_id: String` 改 `Arc<str>` | 独立优化 |
| `AgentTool` wire/runtime input 拆分 | Phase 6 单独立项 |

---

## Benefits

### Phase 0 完成后（独立可 ship）

- **修 B2**：model-facing schema 与 validator schema 一致，当前生产隐性 bug 消除
- **修 B3**：MCP 重连后 stale validator bug 消除

### Phase 2 完成后（动态 Tool 受控）

- 动态 Tool 无法绕过 wire schema 校验 —— Rust privacy 强制
- Plugin / Custom Tool 立项时 schema 入口 = `DynamicSchema::from_wire`，by-construction
- `StructuredOutputTool` 保留宽松合约（用户 array 顶层 schema 仍合法），同时受 `ManualSchema::lax` 入口管控

### Phase 4 完成后（trait surface 整理）

- 删 blanket default —— bug class 编译期消失
- 删 `Self::Input: JsonSchema` bound —— `Value` 不再"伪合规"
- 单一 schema 数据源（每 Tool 一个 `InputSchemaSource` 字段）—— drift 不可能
- 错误前置到启动期 / 构造期 —— 永远不会有 wire-time 400 因为 schema garbage
- `validation_input_schema` 与 `model_input_schema` 两种语义在 trait surface 显式分离

### Phase 5 完成后（防御加固）

- Integration test 扫全 registry，每次 CI 校验 sanitize 合规
- 文档层 + 结构层双重防御未来动态 Tool 漏 override

### 长期外延

- 给 PluginTool 框架、CustomTool 用户脚本、HTTPTool OpenAPI 集成提供现成 schema 入口
- 给 `OutputSchemaSource` 类似改造留出对称模板（如出现 Output `Value` 问题）

---

## Out of Scope

不在本计划：

- 删除 `Tool` trait 改 codex-rs struct-based 形态（失去 typed seam）
- proc macro 隐藏 boilerplate（加基础设施，不解决架构）
- sealed marker trait（用 trait 模拟 enum，Rust 反 pattern）
- `Tool::execute` 改 `Value`-only

不在本计划但与之协同：

- Plugin Tool 框架 —— 应 Phase 4 完成后立项，consumer 视角接 `DynamicSchema::from_wire`
- `OutputSchemaSource` —— 如出现 `Output = Value` 同类 bug 再启动

---

## Decision Log

| 决策 | 选择 | 理由 |
|------|------|------|
| Schema 与 Input 的类型关系 | **解耦**：`type InputSchema: InputSchemaSource`（无 `Input = ...` bound） | 6 个静态工具的 schema 故意与 Input 解耦或更窄；type-level 绑定是错的不变量 |
| Schema 源类型 | **三种**：`TypedSchema<I>` / `ManualSchema` / `DynamicSchema` | TypedSchema 覆盖 ~35 个 trivial 工具；ManualSchema 覆盖 6 个手写 + StructuredOutputTool(lax)；DynamicSchema 覆盖 wire 来源；Plugin 走 DynamicSchema |
| Strict-subset 算法 | **照搬 codex-rs `sanitize_json_schema`** | TS-parity；保留 anyOf / $ref / type 数组归一；schemars `Option<T>` 不打死；真 MCP 不打死 |
| `SchemaError` 变体数 | **3 个**（`RootNotObject` / `RootTypeNull` / `InvalidSchema`） | 其余形状（anyOf / $ref / type arrays / type:null inside nested）全部走 sanitize 归一 |
| `ToolInputSchema` 内部存储 | **`Value`**（不是 `Map<String, Value>`） | 让 `as_value(&self) -> &Value` 可借用；修旧 plan API 内部不一致 |
| 校验失败：panic vs Result | **panic for `TypedSchema::new`, Result for `ManualSchema`/`DynamicSchema`** | 静态来源失败 = 编译/启动 bug 应立即崩；动态/手写来源失败 = 业务错误应可恢复 |
| `ToolInputSchema::sanitize` 可见性 | **`pub(crate)`** | 外部 crate 一律走 `TypedSchema` / `ManualSchema` / `DynamicSchema` —— 维持入口收敛 |
| 老 `coco_types::ToolInputSchema` 命名 | **Phase 1 用 V2 后缀，Phase 4 改回** | 避免 Phase 1-3 期间 namespace 冲突 |
| `Output: JsonSchema` bound | **不改** | Output 历史无 `Value` 问题；YAGNI |
| Migration 分阶段 | **6 phase + Phase 6 独立立项** | 41 个 Tool 一次改 review 负担过大；AgentTool DTO 拆分与主问题正交 |
| Validator cache 修复时机 | **Phase 0 独立** | 当前生产已存在的 stale-validator + schema 生产者分歧 bug，与本重构正交 |
| `StructuredOutputTool` 路径 | **`ManualSchema::lax`，保留宽松** | 它是终态 assistant 输出，不走 strict provider tool API，strict-subset 不适用；用户 array/oneOf 顶层 schema 仍合法 |
| `model_input_schema` 返回类型 | **`Cow<'_, ToolInputSchema>`** | session-aware override 需 own transformed 版本（AgentTool omit）；non-override 时仍 borrow 零开销 |
| `validation_input_schema` 返回类型 | **`&ToolInputSchema`** | validation 永远是 static schema，不需 ownership |
| Trait surface 新方法引入时机 | **Phase 1**（带默认 delegate to 老方法） | 让 Phase 2/3 真正独立 CI 绿；修旧 plan Phase 3-4 顺序错误 |
| MCP 注册改造点 | **`core/tools/src/lib.rs::register_mcp_tools`**（真正的 `McpTool::new` 调用点） | `services/mcp/src/discovery.rs::convert_server_tools` 只是 schema 抽取阶段 |
| `register_mcp_tools` 错误处理 | **skip + 返回 skipped 列表** | 单 tool 无效不应炸全套；上游决定如何呈现（warn log / init 事件） |
| AgentTool DTO 拆分时机 | **Phase 6 单独立项** | 仅 AgentTool；与主线正交；先用 `model_input_schema` 的 omit 表达 |
| 两个 model-facing schema 生产者分歧（B2） | **Phase 0 强制合并** | 当前隐性 bug；不修则 cache hash 修不对、trait surface 收紧无意义 |

---

## Implementation Schedule

| Phase | 估算工时 | 阻塞依赖 | 独立可 ship |
|-------|---------|---------|------------|
| Phase 0（合并生产者 + cache key hash）| 1 day | 无 | ✓ |
| Phase 1（新 source kind + sanitize + trait surface 带 delegate）| 1.5 day | 无（可与 Phase 0 并行）| ✓ |
| Phase 2（动态 Tool 迁移 + `register_mcp_tools` 改）| 0.5 day | Phase 1 | ✓ |
| Phase 3（41 个静态 Tool 批量迁移，6 个 sub-PR）| 1.5 day | Phase 1 | 每 sub-PR 独立 |
| Phase 4（trait surface 收紧 + 删旧类型）| 1 day | Phase 2 + Phase 3 全完 | 破坏性变更 |
| Phase 5（integration test + lint + 文档）| 0.5 day | Phase 4 | ✓ |
| Phase 6（AgentTool DTO 拆分）| 1 day | Phase 4 | 独立 PR |
| **主线总计** | **6 day** | | |

Phase 0 单独 ship 已经修两个隐性生产 bug。Phase 0 + 1 + 2 是"最小动态 Tool 收紧"。
Phase 3 全部走完前，静态工具继续走 legacy delegate，零中断风险。

---

## Review Summary

外部 review 给出 8 条 finding，全部成立并被本 plan 吸收：

| # | finding | 处理 |
|---|---------|------|
| 1 | 阶段独立性不成立（Phase 3 依赖 Phase 4 trait surface）| Phase 1 提前引入 trait surface 带 delegate；Phase 2/3 真独立 |
| 2 | 静态 Input ≠ 模型 schema（AgentTool/BashTool 反例）| 弃用 `InputSchema: InputSchemaSource<Input = Self::Input>` bound；新增 `ManualSchema` source kind |
| 3 | 嵌套 `Value` 仍然漏（AskUserQuestion 4 字段是 Value）| sanitize 算法按形状推断，嵌套 Value 自动归一；同时 AskUserQuestion 走 `ManualSchema` |
| 4 | "抄 codex-rs strict subset" 引用错误（codex-rs 是 sanitize 不是 reject）| 完整照搬真 `sanitize_json_schema` 算法；`SchemaError` 从 11 变体砍到 3 个 |
| 5 | Phase 0 cache key 仍 stale（旧 ToolInputSchema 投影丢字段）| hash 来源改 `effective_tool_schema(tool)` 的 canonical bytes |
| 6 | 旧 newtype API 自相矛盾（`Map<String,Value>` 不能 borrow `&Value`）| 改 `inner: Value` 内部存储；`Clone` derive；API 一致 |
| 7 | `coco_types::ToolInputSchema` 跨 crate 搬迁未覆盖 inference / registry，且当前两条生产路径分歧（B2 隐性 bug）| Phase 0 先强制合并两条生产路径；Phase 4 时类型搬迁只剩单点 |
| 8 | StructuredOutputTool 走 `from_wire` 会破坏 array-root user schema 合约 | 改走 `ManualSchema::lax`，明确"终态输出不走 strict provider tool API，strict-subset 不适用" |

---

## References

- TS source: `tools/SyntheticOutputTool/SyntheticOutputTool.ts`（StructuredOutputTool 的 TS 等价）
- codex-rs source: `codex-rs/tools/src/json_schema.rs::sanitize_json_schema`（**sanitize 算法** —— 本计划照搬，**非旧 plan 误读的 reject 算法**）
- codex-rs source: `codex-rs/tools/src/json_schema_tests.rs`（negative case 测试集，Phase 1 复用）
- Related: [docs/coco-rs/crate-coco-tools.md](crate-coco-tools.md)
- Related: [docs/coco-rs/crate-coco-tool.md](crate-coco-tool.md)（tool-runtime crate）
- Memory: `project_coco_rs_mcp_tool_input_json_schema.md`（McpTool override 来由）
- Memory: `project_coco_rs_turn_completed_semantics.md`（X2 wire 协议讨论上下文）
- Superseded by this doc: [tool-schema-validated-newtype-plan.md](tool-schema-validated-newtype-plan.md)
