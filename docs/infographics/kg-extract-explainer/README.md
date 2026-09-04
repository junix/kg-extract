# kg-extract 可审计技术长图 · 重建说明

一张页面（`index.html`，1200 CSS px 宽、零 JS、零外链、自包含），把 kg-extract
引擎讲清楚：四种抽取机制、一份图谱契约、出处由代码盖章。页面上的每个数字都来自
对引擎真实二进制的离线确定性实跑（mock 后端），冻结在 `data/*.json`；逐项验收与
file:line 锚点注册表见 [VERIFICATION.md](VERIFICATION.md)。

## 管线（五步）

```sh
# 0) 引擎根一律经环境变量传入；缺失即硬失败
export KG_EXTRACT_ROOT="$HOME/projects/kg/kg-extract"

# 1) 冻结证据：真引擎实跑 + 仓库清点 → data/*.json
#    （只读引擎：要求 HEAD == e52b1f9d…且零跟踪文件改动；产物只进本树 data/）
cd <本目录>
bash freeze_evidence.sh

# 2) 平面拷贝到 /tmp 构建（交付树内不跑 python，禁 .pyc 入库）
rm -rf /tmp/ig-kgx-build && mkdir -p /tmp/ig-kgx-build
cp -R data panels.py build.py render.mjs stitch.py /tmp/ig-kgx-build/
cd /tmp/ig-kgx-build
export PYTHONDONTWRITEBYTECODE=1
python3 panels.py        # 9 张数据驱动 SVG（冻结数字断言，缺数即崩）

# 3) 渲染页面 + 六禁项门禁（引擎文件名 / file:line / 原文摘录 / 标识符 /
#    内部路径 / 生成器与重建命令——命中任何一类即 exit 4，不产出页面）
python3 build.py

# 4) 1:1 长页截图（Chrome headless + 原生 CDP，无缓存、抛弃式 profile）
node render.mjs "$PWD/index.html" "$PWD/render" 2
python3 stitch.py render   # 全宽固定高切片自 y=0 顺序拼接；位图高==CSS 高×dpr

# 5) 结构门禁：svg-linter 真二进制逐文件（rc==0 且 finding 行==0 才算过）
for f in svg/*.svg; do "$HOME/sync/macos-arm64-bin/svg-linter" check --plain "$f"; done
```

完成后把 `/tmp/ig-kgx-build/{index.html,svg/,render/}` 拷回本目录（`render/`
不带 `slice-*.png` 中间切片）。`CHROME_BIN` 可覆盖 Chrome 路径（默认 macOS
应用位置）；`KG_EXTRACT_ROOT` 与 `PYTHONDONTWRITEBYTECODE` 为必设环境变量。

## 依赖

- rust/cargo（按引擎 `justfile` 的 feature 集编译 release 二进制）
- python3（标准库 + pillow，仅在 /tmp 平面拷贝内执行）
- node ≥ 22（内置 WebSocket，零 npm 依赖）+ Chrome
- svg-linter（`svg-linter check --plain`，一次一个文件）

## 树内文件

| 路径 | 作用 |
|---|---|
| `freeze_evidence.sh` | 证据冻结（引擎实跑 + 清点 → `data/*.json`） |
| `panels.py` | 9 张 SVG 面板生成器（数字全部来自 data/） |
| `build.py` | 页面渲染 + 六禁项门禁 |
| `render.mjs` / `stitch.py` | CDP 切片截图 / 拼接与三重断言 |
| `data/` | 冻结证据（13 个 JSON，含 provenance） |
| `svg/` `index.html` `render/` | 交付物：面板、页面、位图三件 + 目检裁片 |
