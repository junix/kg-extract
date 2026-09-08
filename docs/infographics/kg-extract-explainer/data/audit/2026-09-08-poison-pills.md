# 毒丸自证运行记录（2026-09-08，指纹豁免目录）

工具：`audit_pills.py`（树根）。运行方式（任意目录均可，绝对路径由脚本自身定位）：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 audit_pills.py
```

原则：每枚毒丸只注入 **抛弃式 /tmp 副本**（index.html / svg / build.py 整树拷贝），
冻结证据 `data/*.json` 与交付产物零触碰；副本用后即毁（失败时保留供排查）。
结论：**15/15 通过（12 枚毒丸全部 BIT，3 组对照全部干净）**，rc=0。

## 六禁项门禁（build.py code_detail_gate）

范围复核：105 个禁用基名 · 698 个过滤后标识符（与 VERIFICATION §3 一致）。

| 毒丸 | 注入内容（进 index.html 副本 `</body>` 前） | 结果 |
|---|---|---|
| P1 ban① 文件基名 | `llms_backend.rs` | BIT：1 hit，仅 `[1-filename]` |
| P2 ban② file:line | `kg_v3.md:237` | BIT：1 hit，仅 `[2-fileline]` |
| P3 ban② 第N行变体 | `第 42 行` | BIT：1 hit，仅 `[2-fileline]` |
| P4 ban③ 原文摘录 | `use chonkie::{CharChunker, …};`（冻结样本行） | BIT：1 hit，仅 `[3-verbatim]` |
| P5 ban③ rust 签名 | `fn build_graph()` / `let mut acc` | BIT：3 hit（`3-rust-sigil` + 伴生 `4-identifier`） |
| P6 ban④ 标识符 | `AgenticExtractor` | BIT：1 hit，仅 `[4-identifier]` |
| P7 ban⑤ 内部路径 | `presets/kg_demo.yaml` | BIT：1 hit，仅 `[5-dirpath]` |
| P8 ban⑥ 生成器名 | `freeze_evidence.sh` | BIT：1 hit，仅 `[6-generator]` |
| P-combined | 上述 8 枚同注一页 | BIT：10 hit，6 类 + rust-sigil 全数点名 |
| P9 ban① svg 侧 | 同 P1 文本注入 `pipeline.svg` 副本 | BIT：`[1-filename] pipeline.svg: llms_backend.rs`（证明逐文件扫描，非只扫 html） |
| 对照：干净语料 | 未污染 index.html + 9 svg | OK：**0 hit** |

## 字号下限门禁（build.py font_floor_gate）

- P10：kpi.svg 副本注入 `font-size="9.5"` 的 CJK 文本 run（主体），整跑
  `python3 build.py`（六禁项先行通过）→ **rc=5**，stderr 点名
  `FONT-FLOOR GATE FAILED: 1 run(s) under floor`。BIT。

## svg-linter 门禁（真二进制，缺二进制即硬失败）

二进制：`~/sync/macos-arm64-bin/svg-linter`（可用 `SVG_LINTER` 覆盖）。

| 步骤 | 状态 | 结果 |
|---|---|---|
| 对照：kpi.svg 未改副本 | OK | rc=0 ∧ 0 finding |
| 对照：注入 defs id + 引用俱全 | OK | rc=0 ∧ 0 finding（干净通过有意义） |
| P11：删除被引用的 defs id | BIT | **rc=1**，1 条 `finding … error … svg/dangling-reference` |

## claims 门禁：N/A

本树声明编号为「页面 colophon + VERIFICATION §2 注册表」的 present-on-page
变体，尚无机器 claims 门禁（两类绑定未建）——待后续 no-claims-binding
轮次指派后，其毒丸（删页上一条 Cxx / 注册表塞孤儿 id）随该门禁一并补入。
