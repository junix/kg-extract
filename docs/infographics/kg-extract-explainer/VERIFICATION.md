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

- **六禁项**（`build.py` 扫 index.html + 9 svg）：范围=105 个禁用文件基名
  （引擎 git ls-files 实时快照，扣除用户 fixtures）、698 个过滤后引擎标识符；
  结果 **0 命中**，`gate clean: 0 hits across 6 banned classes`。
  白名单判例：CLI 动词/旗标值、工具名、公开配置键、JSON 契约键、
  `--list-presets` 实跑输出的模板键（数据白名单判例）。
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

## 4. 指纹表（sha256）

| 文件 | sha256 |
|---|---|
| index.html | 567f18ca70b4c712c642dc25b6a135338bcdba3e6126d57ba341d9682e2a2f68 |
| svg/kpi.svg | 9ed5ea66c58a1fa5b5f7be100d669cb21dcdb0be7b84cb37ed2e90abbfd8f1f0 |
| svg/pipeline.svg | 67a99d364f141a9cd341934b1000aecf89af89e67b41988679fe000db79400d3 |
| svg/aurora_graph.svg | ce0fe9568ae1153a1623755adc4fd0cc2fb80a87d5acc76e1351de2c1787f6a4 |
| svg/mechanisms.svg | aad29a88c523fadc04b629da467d80b4cc3f6f8612000a5d3a74082b3d47d88e |
| svg/schema_modes.svg | fef9930df73e5e7f14bc7ee84f29bcdab2b89410c6f5c72d203863af1d2d283e |
| svg/citations.svg | f1bcbb04d5313aa0551520b8f6a56b587ac2cba300309d106173318f34492ee3 |
| svg/vocab.svg | 15e8924c018f4d4280db792896f74a4eaa836cd389493e845000c93257e0867a |
| svg/presets.svg | 5dba89cbf41a729e79195b169be00e1f6a47cce5ffd5e3c8c639c6e26d8b4da2 |
| svg/surfaces.svg | 4f9e10481ef1aa65880f589fe6704fe6a784a4c695afddedf70d0c42409f7013 |
| render/full@2x.png | 864ceba4466e74ca844ce45384592a1f5f33646c933b8abdb15070460fa57b5b |
| render/full-gray.png | 85b227dd9abbeaea35cd8dd24244cbd7cfa079457e308da2393c324e6afb60d3 |
| render/thumb.png | 02f5bb6871668e63421609ea703b086f6b7cbc438e38c537eb4e0d547a4cd7e1 |

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
6. **交付不 commit**：本树留在工作区，由主会话统一提交。
