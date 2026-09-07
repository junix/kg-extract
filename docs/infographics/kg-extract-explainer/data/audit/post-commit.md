# 交付提交后的门禁重跑记录（指纹豁免）

本文件位于 `data/audit/`，不在 VERIFICATION §4 指纹表覆盖范围内，可追加记录
而不使指纹失效（post-commit 记录若写进被指纹的文档，每次记录都要再提交再
重跑——无 fixpoint）。

## 2026-09-06 · refine 重跑（HEAD = b7a81cc + 未提交 refine 改动）

背景：交付提交 `b7a81cc`（docs: add auditable explainer infographic）落地后，
当时未留下提交后门禁重跑记录。本次 refine（行内 SVG / 字号下限 / 冻结语料
自咬修复）在 HEAD `b7a81cc` 之上以工作区改动重跑了可从冻结数据重跑的全部
门禁；refine 改动本身仍为未提交状态，由主会话统一提交。

环境：/tmp 平面拷贝全链（panels.py → build.py → render.mjs → stitch.py），
冻结数据 `data/*.json` 逐字节未动；六禁项门禁只消费冻结快照，不触碰引擎
工作区，提交前后行为不变。

| 门禁 | 结果 |
|---|---|
| build.py 六禁项（index.html + 9 svg） | PASS：0 命中（105 禁用基名 · 698 过滤标识符） |
| build.py 字号下限（2026-09-06 新增） | PASS：384 文本 run，min 11.0 px，CJK min 12.0 px |
| svg-linter `check --plain`（真二进制，逐文件） | PASS：9 × (rc=0 ∧ 0 findings) |
| render.mjs + stitch.py 三重断言 | PASS：CSS 1200×6223，7 切片自 y=0，位图 2400×12446 |
| 双跑真空 cmp（两次独立 /tmp 平面拷贝全链） | PASS：index + 9 svg + 3 位图共 13 件字节一致 |
| freeze_evidence.sh | NOT-RUN（一次性冻结，证据未变不重冻；引擎已演进至 b7a81cc，重冻须走 README 冻结 worktree 复现路径） |

refine 提交落地后的最终 post-commit 重跑：由主会话在本文件追加。
