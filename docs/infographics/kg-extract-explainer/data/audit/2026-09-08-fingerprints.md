# 全树指纹运行记录（2026-09-08，指纹豁免目录）

工具：`fingerprint.py`（树根）；清单：`fingerprints.json`（豁免自身）。

```sh
python3 fingerprint.py                       # 生成/刷新（幂等：重复生成字节一致）
python3 fingerprint.py --check               # 机器校验（含 VERIFICATION §4 哈希对拍）
python3 fingerprint.py --check --root /tmp/<拷贝>   # detached 校验
```

## 终态清单（2026-09-08，所有文档改动落地后生成）

- **46 文件 / 8,106,347 字节**：svg 9 · data 13 · render 14 · 根 10
  （build/panels/stitch/render.mjs/freeze_evidence.sh/audit_pills/
  fingerprint/index.html/README/VERIFICATION）。
- 豁免恰两项：`fingerprints.json`（自指无 fixpoint）、`data/audit/`
  （wave-1 既定运行记录豁免；本文件即在其中）。
- 幂等实测：连续两次生成 `cmp` 字节一致。
- `--check` 实测：46/46 文件 · §4 13/13 哈希对拍 · rc=0；
  detached（/tmp 全新拷贝）rc=0。

## 篡改探针（全部 BIT，rc=1 并点名；探针均在 /tmp 拷贝上做）

| 探针 | 手法 | 结果 |
|---|---|---|
| T1 冻结数据改动 | `data/presets.json` 追加 1 空格 | `modified: data/presets.json` |
| T2 未入册文件 | 树根放 `junk.txt` | `unmanifested: junk.txt` |
| T3 §4 行形态损坏 | index.html 哈希改成 65 字符 | `§4 row not in \| path \| sha256 \| form` |
| T4 §4 哈希换错值 | 换成有效 64-hex 错值后重生成清单 | `doc-hash: index.html quoted … but manifest has …` |

T3/T4 暴露并修掉了首版缺陷：§4 行一旦脱出 `^\| \S+ \| 64hex \|$`
正则即被静默跳过（21b 自关断陷阱）——现改为按 `## 4.` 标题结构化解析
该表，行数与形态受检，任何一行损坏都硬失败。

## §4 关系

§4 仍为 13 件交付物哈希表（历史记录）；其 13 项与 `fingerprints.json`
逐条相等（`--check` 常态对拍，产物零变化故本轮无哈希迁移）。全树单一
来源是 `fingerprints.json`；任何 §4 行改动都会被 `--check` 当场拦下。
