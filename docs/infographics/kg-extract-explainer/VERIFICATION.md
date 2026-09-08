# VERIFICATION · kg-extract 可审计技术长图验收记录

策略：**代码细节天生下页**——引擎源文件名、file:line、原文摘录、标识符、内部路径、
生成器与重建命令，六类一律不上页面（index.html + 全部 svg + 位图零容忍）；锚点与
命令只登记在本记录。重建命令见 [README.md](README.md)。

## 1. 冻结与环境

| 项 | 值 |
|---|---|
| 引擎提交 | `e52b1f9d17abc16f3898d8e8b2779b6bef5b74db`（开工/收工双查：HEAD 匹配，`git status --porcelain --untracked-files=no` 为空） |
| 引擎二进制 | `target/release/kg-extract`，feature 集 `llms-backend mcp community community-leiden`，sha256 见 `data/provenance.json` |
| 后端 | mock（离线、确定性：`--mock-response` / `--mock-tool-calls`） |
| 语料 | 引擎自带 fixtures：`medium_product_doc.md` 及配套 mock（sha256 见 `data/provenance.json`） |
| 证据 | `data/*.json` 13 个文件，152 KB，全部由 `freeze_evidence.sh` 产出 |

## 2. 声明编号 ↔ 证据 ↔ 锚点注册表（file:line 只出现在本表）

实跑冻结声明（C 系列；证据=冻结数据文件，无需源码锚点）：

| 声明 | 内容 | 证据文件 |
|---|---|---|
| C01 | 模板 40 份 · 8 域 · 8 种结构 | `data/presets.json`（`--list-presets` 实跑解析） |
| C02 | provider 6 项能力 + 执行信封实录 | `data/provider.json`（describe/available/invoke） |
| C03 | simple 引擎 17 实体 · 15 三元组 · 3 分段 | `data/medium_simple.json` stats |
| C04 | 工作实例图=实跑 node-link 逐节点逐边绘制 | `data/medium_simple.json` graph |
| C05 | 三机制收敛 17/15 | `data/medium_simple.json` + `data/medium_schema_modes.json`(open) + `data/medium_toolcall.json` stats |
| C06 | fixed 硬丢弃 17→7 / 15→1、弃 24 条 | `data/medium_schema_modes.json` fixed（stderr 原文冻结） |
| C07 | evolving 保留全量 17/15 | `data/medium_schema_modes.json` evolving |
| C08 | 引用双形态并集（legacy 行 + 富坐标） | `data/citation_demo.json`（预切分双块实跑） |
| C09 | kg-protocol 一等证据字段 | `data/medium_simple.json` protocol_entities |
| C10 | 社区检测 4 团 8+3+3+3 | `data/medium_communities.json` |
| C11 | 词表清点 122/108/73/12/67 | `data/vocab_census.json`（锁定版本程序化计数） |

文档记录值（D 系列；锚点=引擎 README.md 行号，引文原文存 `data/readme_documented.json`）：

| 声明 | 锚点 | 页面形态 |
|---|---|---|
| D01 出处由代码计算 | README.md:237-239 | 文字结论（引用面板） |
| D02 多处出现出处并集 | README.md:250-254 | 文字结论（引用面板） |
| D03 类型词表 122/108 变体 | README.md:258-261 | 计数条形（词表面板） |
| D04 配置三层优先级 | README.md:327-333 | 三层卡片（表面面板） |
| D05 社区=标签传播、边权=三元组重数 | README.md:399-423 | 图例文字（图谱面板） |
| D06 摘要界值 32/24/120/1024 | README.md:477-485 | 未上页（登记备查） |
| D07 agentic 只读沙箱 | README.md:592-598 | 机制面板文字 |
| D08 62 KB 实测 516/136 | README.md:604-617 | 对照表（标注「文档记录值」） |
| D09 协议提升 Evidence、不双写 | README.md:374-380 | 协议卡（引用面板） |
| D10 空种子=退化格直接报错 | README.md:87-90 | 退化格卡（schema 面板） |
| D11 类型化工具集（参数以文字形态） | README.md:198-208 | 工具名 chips（表面面板） |
| D12 机制×schema 正交 | README.md:16-17,63-67 | 流水线③与 KPI 卡 |

## 3. 门禁结果

- **六禁项**（`build.py` 扫 index.html + 9 svg；index.html 自 2026-09-06 起
  行内内嵌 9 张面板，扫描面即页面真实字节）：范围=105 个禁用文件基名
  （引擎 ls-files 冻结快照，扣除用户 fixtures 与本树自身路径）、698 个过滤后
  引擎标识符；结果 **0 命中**，`gate clean: 0 hits across 6 banned classes`。
  白名单判例：CLI 动词/旗标值、工具名、公开配置键、JSON 契约键、
  `--list-presets` 实跑输出的模板键（数据白名单判例）。
- **字号下限**（2026-09-06 新增，`build.py` 构建期断言）：384 个 SVG 文本 run，
  任意文本 ≥11 px、CJK ≥12 px，`font-floor gate clean: min 11.0 px, cjk min 12.0 px`。
- **svg-linter**（真二进制 `check --plain`，一次一文件）：
  **9 × (rc=0 ∧ finding 行=0)**。
  首轮 29 findings（描边越界 / 扇出共线 / 文本重叠 / 等宽宽度低估）→ 根因修复
  （描边内缩、槽位布线+独立通道、等宽 0.6em 估计）→ 复验全零。
- **渲染三重断言**：页面 CSS 1200×6223；切片 7 张全宽固定高自 y=0 顺序拼接；
  位图 2400×12446 == 6223×2（逐切片尺寸亦断言）；空白页守卫通过；
  `full@2x.png` / `full-gray.png` / `thumb.png` 三件齐全。
- **双跑真空 cmp**：全新 /tmp 平面拷贝全链重跑（冻结数据相同输入），index.html、
  9 张 svg、3 张位图 **全部字节一致**（Chrome 渲染确定性成立）。
- **无缓存目检**：抛弃式 user-data-dir 启动（零缓存），等 `document.fonts.ready`
  + 双 rAF 后截图；全页缩略 + 逐面板裁片（`render/crops/`，9 张）人工核查：
  无重叠、无裁切、断行正常、伪代码块可读。

## 4. 指纹表（sha256，2026-09-06 refine 后重算）

| 文件 | sha256 |
|---|---|
| index.html | 03d3d9cefd992e289ddef31fac5167580463341498626fd06c56ba1cc446cff6 |
| svg/kpi.svg | 09dde85453e6c627d3e07287deeee3153f22e517044ad4f8497228da4a5246b8 |
| svg/pipeline.svg | aee66fcd1c4cc25ee1a853c0947499c96e7412c75a42dd9277ac931f57ed1b5b |
| svg/aurora_graph.svg | 77a5b18d493b2bd5621c3433fc1f9c59fb2fdbc49fccf96b5dbc84657a8bef9c |
| svg/mechanisms.svg | 1959885d11a46a6b84e4b479b36bff9c875e7970c783d668f3d925fa37265d67 |
| svg/schema_modes.svg | e809fbc5f2bf611c8d42eff2ecca85406b1940f9ab260878dc67da18c8a4a8d9 |
| svg/citations.svg | 1de015e3fc584b1fca1be1addd035fca1d0c18a12f4c18c8086b84f4140a6203 |
| svg/vocab.svg | 6251bd6c38611df9a4fafb1fa48f4f3aba0d761df461dc92aeb02d5a66c78a5a |
| svg/presets.svg | c200c1f5dd0cd5fd5c8835402492c4a9e5e110cdc497250d42e934d1cfc9caed |
| svg/surfaces.svg | 108a43fd2a43434591a2b0afc9b7904c8a913a44d72b68bf1eecf17119398d4c |
| render/full@2x.png | 67a439f4dcb1705dfafa19a19d0e47ca1e9c7cf636aebee940b5d8d5d20e8b5a |
| render/full-gray.png | bb81c54792ff5b356b9e5348d486d3f56a748f119eeca8db4cb261dc2b377c29 |
| render/thumb.png | 69c29af30220d1c55ba5cec59dd69a46024ce0b5ed2e85875971a32852d39772 |

## 5. 偏差与判例记录（如实）

1. **README 37/6 vs 实跑 40/8**：引擎 README.md:155-156 记「37 份模板 · 6 个域」，
   冻结实跑 `--list-presets` 为 **40 份 · 8 域**（多出 code、knowledge 域）。
   页面采用实跑值；差异登记于 `data/readme_documented.json`。
2. **kg-vocab 清点方法**：取 Cargo.lock 锁定的 kg-vocab 版本（pin
   `2084415…`）的词表数据做程序化计数，得 122/108/73/12/67，与 README D03 的
   122/108 双向一致；清点在引擎只读前提下进行（cargo git checkout，不写引擎树）。
3. **模板键白名单判例**：`general/concept_graph` 等模板键来自引擎自身
   `--list-presets` 输出（公开 CLI 面），按数据白名单判例上页并记录于此。
4. **fixed stderr 重复行**：冻结 stderr 中丢弃报告同文出现 2 次（schema-json
   整篇单调用、segments=0）。页面展示一次原文 + 事实性注明「重复出现 2 次」，
   全文原样冻结在证据文件，不解读重复原因。
5. **中间切片不入库**：`render/slice-*.png` 为拼接中间物，验收后从交付树剔除
   （双跑 cmp 已覆盖其内容；manifest.json 保留切片清单）。
6. **交付不 commit**：本树留在工作区，由主会话统一提交。【后证不实，已修正】
   主会话已以 `b7a81cc` 提交交付版；当时未留下提交后的门禁重跑记录，2026-09-06
   补记于 [`data/audit/post-commit.md`](data/audit/post-commit.md)（指纹豁免，
   可追加不破坏 §4）。2026-09-06 refine 改动同样不自行提交，仍由主会话统一提交。

## 6. 2026-09-06 refine

按 survey（img-external-panel / doc-drift / cjk-small 三项 high 优先）做最小
修复；冻结证据 `data/*.json`（13 个 JSON）逐字节未动。

**改动清单**

1. **img-external-panel（high，已修）**：`build.py` `section()` 由
   `<img src="svg/…">` 改为把 SVG 文件逐字节内联进 `index.html`
   （`<figure class="panel" data-svg=…>`），页面 9 处外链清零，「零外链 ·
   自包含」声明自此为真。`render.mjs` 面板探测同步从 `document.images`
   改为 `figure.panel`（含每 figure 恰一 svg 断言），crop 清单沿用
   `data-svg` 命名；另补 `mkdirSync(outDir)`（原版在新平拷贝里首次运行会
   ENOENT）。
2. **doc-drift（high，已修）**：README 首段「零外链、自包含」原与 img 外链
   构造不符【后证不实，已修正】——修法是内联 SVG 让声明成立（见上），
   并在 README 管线注释中写明面板为行内内嵌。
3. **cjk-small（high，已修）+ svg-text-small（med，一并修）**：`panels.py`
   新增 `floor_fs`（CJK ≥12 px、任意文本 ≥11 px），在 `text_w` /
   `Canvas.text` / `Canvas.para` 三个入口统一生效，chip 宽度与折行随之下
   沉，几何无需重排（各面板既有溢出断言全部原样通过）；`build.py` 新增
   `font_floor_gate()` 构建期断言（violation 即 exit 5），本次实测 384 run、
   min 11.0 px、CJK min 12.0 px（修复前 161 个 CJK run <12 px、最小 9.0、
   ≥11 px 占比 76.8%）。
4. **gate-selfbite（加固）**：`freeze_evidence.sh` 冻结语料（`git ls-files`）
   显式排除本树自身路径（交付提交后重冻不再自吞）；HEAD 前移时的失败信息
   区分「引擎演进」并给出冻结 worktree 复现命令（证据漂移仍硬失败）；
   README 补同款复现配方；新增 `data/audit/post-commit.md`（指纹豁免）记录
   `b7a81cc` 之后的门禁重跑。
5. **width-rule**：`body` 由 `width:1200px` 改为 `max-width:1200px;
   margin:0 auto`（1200 视口下渲染不变，宽屏居中不再依赖固定宽块）。

**重建与门禁复跑**（/tmp 平面拷贝全链，双跑 cmp 13 件产物字节一致）：
六禁项 PASS（0 命中）· 字号下限 PASS · svg-linter 9×PASS · 渲染三重断言
PASS（CSS 1200×6223、7 切片、位图 2400×12446，与 refine 前同尺寸）·
9 张目检裁片人工复核无重叠/无裁切。§4 指纹表已按新产物重算。

**本轮明确不做（登记待后续）**：hero 主面板改版（survey no-hero，med）、
sidenote 侧注轨（no-sidenote-track，med）、全树指纹 + 机器校验
（fingerprint-gaps，med，仅按规则刷新了被重建波及的 §4 指纹）、六禁项
毒丸自证（no-poison，low）、contract.md（other，low）。

## 7. 2026-09-07 refine（审计硬化，wave 2；实际运行 2026-09-08）

本轮 AUDIT-HARDENING wave 2 指派两类：**no-poison** 与 **fingerprint-gaps**
（no-claims-binding / no-reverse-sweep / no-vacuum 未指派，见本节末）。
冻结证据 `data/*.json` 逐字节未动；交付产物（index.html / 9 svg / 3 位图）
零变化（/tmp 平面拷贝全链重建 cmp 10/10 字节一致复核）。

### 7.1 no-poison（毒丸自证）——已修

新工具 `audit_pills.py`（树根）：对每个既有门禁注入阳性对照到 **抛弃式
/tmp 副本**（整树拷贝 build.py+data+svg+index.html），门禁必须咬人；
冻结层与交付物零触碰，副本用后即毁。实测 **15/15**（12 枚毒丸全 BIT、
3 组对照全干净），rc=0。逐项：

- **六禁项门禁 6/6**：每类一枚（文件基名/file:line/原文摘录/标识符/
  内部路径/生成器名）注入 index.html 副本，各类只命中本类；另加
  「第 N 行」变体、rust 签名子检（伴生 4 类命中已如实记录）、8 枚合注
  一页（6 类全点名）、**svg 侧**注入（`[1-filename] pipeline.svg`，
  证明逐文件扫描）。干净语料对照 = **0 hit**（范围复核 105 基名 /
  698 标识符，与 §3 一致）。
- **字号下限门禁**：kpi.svg 副本注入 9.5 px CJK run，整跑
  `python3 build.py` → **rc=5**，stderr 点名 `FONT-FLOOR GATE FAILED`。
- **svg-linter 门禁**：面板本无 defs id（生成器产出无 id SVG），故按
  删除自测同构做法——副本先注入 defs id+引用（对照：rc=0 ∧ 0 finding，
  干净通过有意义），再删除被引用 id → **rc=1**，1 条 `error/high/
  svg/dangling-reference`。真二进制缺失即硬失败。
- **claims 门禁：N/A**——本树为 present-on-page 变体（colophon +
  §2 注册表），无机器门禁；待 no-claims-binding 轮次指派后随建随补毒丸。

运行记录（命令 + 逐枚计数）：[`data/audit/2026-09-08-poison-pills.md`](data/audit/2026-09-08-poison-pills.md)
（指纹豁免目录）。

### 7.2 fingerprint-gaps（全树指纹 + 机器校验）——已修

新工具 `fingerprint.py` + 清单 `fingerprints.json`（树根）：

- **覆盖**：全树 **46 文件**（svg 9 · data 13 · render 14 · 根 10——
  含工具、docs、index.html），sha256+字节数；对照 §4 旧表仅 13 件交付物
  ——覆盖增量 13→46，工具/数据/文档不再豁免于指纹。
- **豁免恰两项（具名）**：`fingerprints.json` 自身（自指无 fixpoint）、
  `data/audit/`（2026-09-06 wave-1 既定：运行记录可追加不使指纹失效）。
- **稳定字段**：清单只含排序相对路径 + sha256 + 字节数，无时间戳、无
  live HEAD——重复生成字节一致（幂等实测通过），提交后重跑不改字节。
- **`--check` 机器校验**：清单逐文件对拍（missing/modified/unmanifested
  三向）+ **§4 表结构化对拍**——§4 每行必须为 `| 路径 | sha256 |` 且与
  清单逐条相等（本轮实测 13/13 一致，产物零变化故无哈希迁移）；§4 行
  被改成非 64-hex 形态或换成错误值均硬失败（两种篡改探针均 rc=1，修掉
  了首版「行脱出正则即静默跳过」的 21b 类自关断缺陷）。`--check --root
  <目录>` 支持 detached 校验（/tmp 拷贝实测 rc=0）。
- **篡改探针 4/4 咬人**：冻结数据字节改动 / 未入册文件 / §4 行形态损坏 /
  §4 哈希换错值——全部 rc=1 并点名。

运行记录：`data/audit/2026-09-08-fingerprints.md`（指纹豁免目录）。

### 7.3 门禁复跑（本轮改动后）

| 门禁 | 结果 |
|---|---|
| build.py 六禁项 + 字号下限（/tmp 平面拷贝整链） | PASS：0 命中 · 384 run min 11.0/CJK 12.0 · 产物 cmp 10/10 一致 |
| svg-linter 9 面板（真二进制逐文件） | PASS：9 × (rc=0 ∧ 0 finding) |
| audit_pills.py 毒丸电池 | PASS：15/15（12 毒丸 BIT + 3 对照干净） |
| fingerprint.py --check（本树 + /tmp detached） | PASS：46/46 文件 · §4 13/13 对拍 · 幂等 |
| render.mjs/stitch.py 位图链 | NOT-RUN（页面零变化，位图不涉及；§3 记录仍有效） |
| freeze_evidence.sh | NOT-RUN（一次性冻结，证据未变不重冻） |

### 7.4 本轮未指派、明确不做（登记待后续）

- **no-claims-binding**：colophon C01–C11+D01–D12 与 §2 注册表的双向
  机器绑定未建（指派给后续轮次）。
- **no-reverse-sweep**：页面数字反向清扫门禁未建。
- **no-vacuum**：真空重建仍为「双跑 cmp」弱形态（同冻结输入两次全链
  字节一致），未做删除-再生长快照对账。
- 提交沿用 §5 偏差 6 惯例：本轮改动不自行 commit，由主会话统一提交；
  提交后按 `data/audit/post-commit.md` 槽位补记。

