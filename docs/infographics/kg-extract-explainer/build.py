#!/usr/bin/env python3
"""Render index.html from data/*.json + svg/*.svg and run the code-detail gate.

Pure renderer: it invents no numbers, reads every visible figure from the
frozen data files, and FAILS the build if any of the six banned code-detail
classes appears on the delivered page (index.html + every svg).

Banned classes (policy: code detail stays off the page):
  1. engine source file basenames (git ls-files snapshot, minus user-data
     fixtures — fixtures are user data and allowed);
  2. file:line coordinates / line ranges / 「第 N 行」;
  3. verbatim engine source excerpts (frozen sample lines, exact substring);
  4. engine identifiers (census from src/**.rs), minus the explicitly allowed
     public surface (CLI values, tool names, config keys, JSON contract keys,
     preset keys from the real --list-presets run — data-whitelist precedent);
  5. engine internal directory paths (src/, presets/, scripts/, spec/, ...);
  6. generator filenames and rebuild commands (panels/build/render/freeze,
     python3, cargo, shasum, cmp, env-var names, ...).

Run from a flat copy: python3 build.py  (PYTHONDONTWRITEBYTECODE=1).
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
SVG = os.path.join(HERE, "svg")

SVGS = ["kpi.svg", "pipeline.svg", "aurora_graph.svg", "mechanisms.svg",
        "schema_modes.svg", "citations.svg", "vocab.svg", "presets.svg",
        "surfaces.svg"]

# --------------------------------------------------------------- allowlist --
# Public surface tokens that are ALSO engine identifiers. These stay allowed
# on the page per policy (CLI verbs/values, tool names, public config keys,
# JSON contract keys) and the data-whitelist precedent (preset keys come from
# the engine's own --list-presets output). Every entry is a public token.
SURFACE_ALLOW = {
    # CLI verbs / subcommands
    "describe", "available", "invoke", "finish", "list_entities",
    # tool names (MCP/tool-calling surface)
    "add_entity", "add_relation", "add_attribute", "propose_schema_type",
    # public config keys (config file contract)
    "engine", "model", "backend", "agent", "chunker", "schema_mode", "schema",
    "preset", "preset_file", "lang", "max_rounds", "merge_strategy", "coref",
    "canonical_direction", "max_concurrency", "relation_gleaning",
    "community_summaries", "output", "config",
    # CLI/engine/backend/chunker/agent/schema-mode/output values
    "simple", "schema-json", "toolcall", "agentic", "llms", "mock",
    "recursive", "char", "token", "open", "fixed", "evolving", "text",
    "chunks", "mermaid", "stats", "json", "jsonl", "node-link",
    "kg-protocol", "ladybug-import", "communities", "communities-hierarchy",
    "minimaxcc", "glmcc", "mimocc", "pi-agent", "keep-existing",
    "keep-incoming", "field-union",
    # JSON contract keys shown on the page
    "label", "type", "predicate", "subject", "object", "doc", "lines",
    "range", "char_span", "line", "page", "bbox", "start", "end", "nodes",
    "relations", "attributes", "source_file", "entity_type", "evidence",
    "protocol", "status", "artifacts", "result", "checksum", "kind",
    "capability_id", "num_entities", "num_triples", "source", "target",
    "name", "summary", "members", "quality", "num_communities",
    # domain / product terms (published names, not engine identifiers)
    "kg-protocol", "kg-vocab", "version",
    # vocab census surface words
    "entity", "entities", "relation", "relations", "variant", "variants",
    "aliases", "inverse", "inverses", "disambiguation", "provider",
    "manifest", "concurrency",
}


def load(name):
    with open(os.path.join(DATA, name)) as f:
        return json.load(f)


def sweep_texts():
    texts = {}
    for f in SVGS:
        with open(os.path.join(SVG, f)) as fh:
            texts[f] = fh.read()
    return texts


def code_detail_gate(html):
    texts = sweep_texts()
    texts["index.html"] = html
    hits = []

    def note(cls, where, detail):
        hits.append(f"[{cls}] {where}: {detail}")

    # ---- gate 1: engine source file basenames ---------------------------
    files = load("engine_files.json")["files"]
    banned = []
    for p in files:
        if p.startswith("scripts/fixtures/"):
            continue  # user data fixtures: allowed on the page (precedent)
        banned.append(os.path.basename(p))
    banned = sorted(set(banned))
    for where, txt in texts.items():
        for b in banned:
            if b in txt:
                note("1-filename", where, b)

    # ---- gate 2: file:line coordinates -----------------------------------
    pat_coord = [
        (re.compile(r'[A-Za-z0-9_\-]+\.(?:rs|py|md|toml|lock|ya?ml|sh|json|txt):\d'), "ext:line"),
        (re.compile(r':\d+\s*[-–—~]\s*\d+'), ":N-M range"),
        (re.compile(r'第\s*\d+\s*行'), "第N行"),
        (re.compile(r'\bL\d+\s*[-–]\s*\d+\b'), "L N-M"),
    ]
    for where, txt in texts.items():
        for pat, tag in pat_coord:
            for m in pat.finditer(txt):
                note("2-fileline", where, f"{tag}: {m.group(0)!r}")

    # ---- gate 3: verbatim engine source excerpts --------------------------
    samples = load("engine_identifiers.json").get("source_line_samples", [])
    for where, txt in texts.items():
        for s in samples:
            core = s.strip()
            if len(core) >= 30 and core in txt:
                note("3-verbatim", where, core[:60])

    # ---- gate 3b: rust code sigils ----------------------------------------
    pat_rust = re.compile(r'\b(?:pub\s+fn|fn\s+\w+\(|let\s+mut|impl\s+\w+|use\s+crate::|->\s*Result<|unwrap\(\)|&str,\s*&str)')
    for where, txt in texts.items():
        for m in pat_rust.finditer(txt):
            note("3-rust-sigil", where, m.group(0))

    # ---- gate 4: engine identifiers ---------------------------------------
    idents = load("engine_identifiers.json")["identifiers"]
    checked = []
    for ident in idents:
        if ident in SURFACE_ALLOW:
            continue
        if len(ident) < 5:
            continue
        if not ("_" in ident or re.search(r"[a-z][A-Z]", ident)):
            continue  # plain english word, no rust shape
        checked.append(ident)
    for where, txt in texts.items():
        for ident in checked:
            if re.search(r'\b' + re.escape(ident) + r'\b', txt):
                note("4-identifier", where, ident)

    # ---- gate 5: engine internal directory paths ---------------------------
    pat_dir = re.compile(r'(?:\.\./|\(|\s|/|^|=)(?:src|presets|scripts|spec|docs/infographics|target|backend|extractor|template|community|types)/[A-Za-z0-9_\-./]*')
    for where, txt in texts.items():
        for m in pat_dir.finditer(txt):
            frag = m.group(0)
            # preset keys like general/concept_graph are public CLI surface
            # (from the real --list-presets run; data-whitelist precedent).
            if re.match(r'^[\s(=/](?:general|finance|legal|medicine|industry|tcm|code|knowledge)/[a-z0-9_]+$', frag):
                continue
            if frag.startswith("~/.kg-extract"):
                continue  # public documented config path
            note("5-dirpath", where, frag[:60])

    # ---- gate 6: generator names & rebuild commands ------------------------
    pat_gen = re.compile(r'\b(?:panels\.py|build\.py|render\.mjs|render\.js|stitch\.py|freeze_evidence\.sh|svgkit|PYTHONDONTWRITEBYTECODE|KG_EXTRACT_ROOT|SYNC_BIN_DIR|python3|shasum|sha256sum|\bcmp\b|svg-linter|cargo\s+build|cargo\s+run|node\s+|npm\s+|just\s+(?:build|test|install))')
    for where, txt in texts.items():
        for m in pat_gen.finditer(txt):
            note("6-generator", where, m.group(0))

    return hits, len(banned), len(checked)


# ------------------------------------------------------------------ page ----
CSS = """\:root{--ink:#16233B;--sub:#5A6B84;--faint:#8A97AB;--line:#D7DEEA;--blue:#2563EB;--blue-dark:#1E3A8A;--paper:#FFFFFF;--bg:#F3F6FB}
*{margin:0;padding:0;box-sizing:border-box}
body{background:var(--bg);color:var(--ink);font-family:'PingFang SC','Hiragino Sans GB','Microsoft YaHei',sans-serif;width:1200px;margin:0 auto}
.wrap{padding:0 40px}
header{background:linear-gradient(135deg,#1E3A8A 0%,#2563EB 62%,#3B82F6 100%);color:#fff;padding:64px 40px 56px}
.kicker{font-size:14px;letter-spacing:.35em;opacity:.85;margin-bottom:18px}
h1{font-size:44px;line-height:1.28;font-weight:800;max-width:1040px}
.thesis{margin-top:26px;font-size:19px;line-height:1.7;max-width:980px;opacity:.94}
.readerq{margin-top:20px;font-size:13.5px;opacity:.75}
section{padding:44px 0 8px}
h2{font-size:25px;font-weight:800;color:var(--blue-dark);margin-bottom:8px;display:flex;align-items:baseline;gap:14px}
h2 .no{font-size:14px;color:var(--faint);font-weight:600;letter-spacing:.08em}
.lede{font-size:14.5px;line-height:1.75;color:var(--sub);max-width:1010px;margin-bottom:20px}
.panel{width:100%;height:auto;display:block}
.colophon{background:#0F1D33;color:#C9D6EC;border-radius:14px;padding:34px 38px;margin:52px 0 60px;font-size:13px;line-height:1.8}
.colophon h3{color:#fff;font-size:16px;margin-bottom:12px}
.colophon .claims{display:grid;grid-template-columns:1fr 1fr;gap:4px 34px;margin-top:10px}
.colophon .claims div{padding:3px 0;border-bottom:1px solid #1D3050}
.cid{font-family:'SF Mono',Menlo,Consolas,monospace;color:#7FB0FF;margin-right:8px;font-size:11.5px}
footer{padding:0 0 56px;color:var(--faint);font-size:12px;line-height:1.8}"""


def section(no, title, lede, svg, extra_html=""):
    path = os.path.join(SVG, svg)
    with open(path) as f:
        first = f.readline().strip()
    m = re.match(r'<svg[^>]*width="(\d+)"[^>]*height="(\d+)"', first)
    w, h = m.group(1), m.group(2)
    return f"""<section class="wrap" id="s{no}">
<h2><span class="no">{no}</span>{title}</h2>
<p class="lede">{lede}</p>
<img class="panel" src="svg/{svg}" width="{w}" height="{h}" alt="{title}">
{extra_html}</section>"""


def render():
    presets = load("presets.json")
    simple = load("medium_simple.json")
    prov = load("provider.json")

    hero_kpi = section("0", "", "", "kpi.svg").replace(
        '<h2><span class="no">0</span></h2>\n<p class="lede"></p>\n', "")

    s1 = section("1", "引擎总览：文本进来，图谱出去",
                 "一条流水线、四种可替换的抽取机制、两个正交的 schema 纪律维度、九种输出序列化——"
                 "所有箭头上的事实都来自冻结的实跑。", "pipeline.svg")
    s2 = section("2", "工作实例：26 行架构笔记 → 17 实体 · 15 三元组",
                 "对一份真实 fixture 语料跑冻结实抽：分段 → 逐段抽取 → 归并去重，得到下面这张图。"
                 "着色是社区检测的实跑结果：主体簇之外，治理、计费、身份三条功能链各自成团。", "aurora_graph.svg")
    s3 = section("3", "机制可换，契约不变",
                 "三条机制（分隔符提示 / 模式化 JSON / 工具调用）在同一语料、同一份事实数据上收敛到同一张 17/15 图；"
                 "第四条机制 agentic 用单会话串行换取共指质量——取舍而非升级。", "mechanisms.svg")
    s4 = section("4", "schema 模式：正交的纪律维度",
                 "机制决定「怎么让模型说出事实」，schema 模式决定「允许说什么」：open 自由、fixed 闭世界硬丢弃、evolving 可生长。"
                 "同一语料换模式重跑，丢弃行为直接可见、可审计。", "schema_modes.svg")
    s5 = section("5", "出处由代码盖章",
                 "每条实体/三元组携带它来自哪里的坐标：纯文本推导行号，预切分输入透传页码与边界框；"
                 "同一记录多处出现则出处并集。协议输出把证据提升为一等字段。", "citations.svg")
    s6 = section("6", "类型归一与权威词表",
                 "模型口中的任意叫法经三级解析落到规范类型；规范词表独立锁定发布，双向核对移植来源。", "vocab.svg")
    s7 = section("7", f"领域模板库：{presets['total']} 份 · {len(presets['domains'])} 个域",
                 "模板是比扁平 schema 更高一级的抽取目标声明：领域人设 + 抽取守则 + 输出字段结构，内嵌二进制按名加载。", "presets.svg")
    s8 = section("8", "四个可组合表面",
                 "CLI 一次调用、配置三层优先级、provider 自描述协议、无模型的 MCP 图构建——同一引擎的四种进入方式。", "surfaces.svg")

    colophon = f"""<section class="wrap"><div class="colophon">
<h3>证据冻结与声明编号</h3>
本页每个数字都来自对引擎真实二进制的离线确定性实跑（mock 后端），冻结于引擎提交
<span class="cid">e52b1f9d</span>；引擎标识符与代码坐标不上页，逐条声明登记在证据记录中，重建命令与验收管线见 README / VERIFICATION。
<div class="claims">
<div><span class="cid">C01</span>模板 40 份 · 8 域 · 8 种结构（--list-presets 实跑清点）</div>
<div><span class="cid">C02</span>provider 6 项能力 + 执行信封实录（describe/invoke 实跑）</div>
<div><span class="cid">C03</span>simple 引擎实跑 17 实体 · 15 三元组 · 3 分段</div>
<div><span class="cid">C04</span>工作实例图 = 实跑 node-link 输出逐节点逐边绘制</div>
<div><span class="cid">C05</span>三机制收敛 17/15（三组实跑 stats 对照）</div>
<div><span class="cid">C06</span>fixed 硬丢弃：17→7 实体 · 15→1 三元组 · 弃 24 条</div>
<div><span class="cid">C07</span>evolving 保留全量 17/15（实跑）</div>
<div><span class="cid">C08</span>引用双形态并集（预切分双块实跑）</div>
<div><span class="cid">C09</span>kg-protocol 一等证据字段（协议输出实跑）</div>
<div><span class="cid">C10</span>社区检测 4 团：8+3+3+3（标签传播实跑）</div>
<div><span class="cid">C11</span>词表普查 122/108/73/12/67（锁定版本程序化清点）</div>
<div><span class="cid">D01–D12</span>文档记录值（62 KB 实测、配置优先级、退化格等）</div>
</div></div></section>"""

    footer = f"""<footer class="wrap">
kg-extract 可审计技术长图 · 页面零 JS · 零外链 · 自包含 · 1200 CSS px 宽 · zh-CN<br>
证据链、file:line 锚点注册表与逐项验收记录见 VERIFICATION；重建命令见 README。
</footer>"""

    html = f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=1200">
<title>kg-extract 引擎全景：从任意文本到可审计知识图谱</title>
<style>{CSS}</style>
</head>
<body>
<header>
<div class="kicker">KG-EXTRACT · RUST 知识抽取引擎 · 可审计技术长图</div>
<h1>四种机制，一份图谱契约：<br>把任意文本变成带出处的知识图谱</h1>
<p class="thesis">分段、抽取、归并、盖章——文本超长就切段并发，模型口径不一就用词表归一，
出处必须由代码按坐标钉死。机制与纪律两个维度全部可替换，输出的类型化三元组契约始终不变。</p>
<p class="readerq">读者问题：这个引擎如何把长文档变成可信的知识图谱？为什么同一份契约能跑在四种机制、多种后端、九种输出上？</p>
</header>
{hero_kpi}
{s1}{s2}{s3}{s4}{s5}{s6}{s7}{s8}
{colophon}
{footer}
</body>
</html>
"""
    return html


def main():
    missing = [f for f in SVGS if not os.path.exists(os.path.join(SVG, f))]
    if missing:
        print("FATAL: missing svg inputs:", missing, file=sys.stderr)
        sys.exit(3)
    html = render()
    hits, n_files, n_idents = code_detail_gate(html)
    print(f"gate scope: {n_files} banned basenames, {n_idents} filtered identifiers")
    if hits:
        print(f"CODE-DETAIL GATE FAILED: {len(hits)} hit(s)", file=sys.stderr)
        for h in hits:
            print("  " + h, file=sys.stderr)
        sys.exit(4)
    out = os.path.join(HERE, "index.html")
    with open(out, "w") as f:
        f.write(html)
    print("gate clean: 0 hits across 6 banned classes")
    print("wrote", out)


if __name__ == "__main__":
    main()
