# kg-extract 约定

本项目提供多机制知识图谱抽取；CLI、MCP 与各 extractor 必须产出同一套 `KnowledgeGraph` / kg protocol 语义。

- 抽取机制（prompt+parse、tool call、agentic）与 `SchemaMode`（open/fixed/evolving）是正交维度；fixed/evolving 必须有非空 seed schema。
- 所有 engine 和 MCP store 共用 `graph_build.rs` 的 ID、端点解析与 dangling 处理；类型/谓词解析保留在各调用点。
- Agentic 是顺序多轮的 consolidation/coref 路径，不得并行切片或等同于事后 merge。
- citation 由代码依据 chunk offset/range 生成，禁止让模型编坐标；预切分的 page/bbox/title/metadata 必须穿透协议。
- `SchemaMode` 留在 types 层，避免 types→extractor 环；`kg-vocab` 是实体/谓词类型的单一来源。
- community 转换保留平行边权重；并发摘要按 community key 稳定排序，失败明确降级。
- Provider capability 清单保持单一来源，协议失败与领域失败维持既定 envelope/退出语义。
- 有意保留的 Python 兼容怪异行为必须有注释和测试；不得凭直觉“修正”。
- 协议相关 Git 依赖精确 pin，并用 lockfile 验证；本地 patch 不得提交。

命令和 feature 组合以 `kg-extract --help`、Cargo.toml 与 justfile 为准。修改后运行 `just test`、lint 和相关 provider/MCP/真实 Ladybug E2E。
