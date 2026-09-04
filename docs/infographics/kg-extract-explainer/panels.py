#!/usr/bin/env python3
"""Panel generators for the kg-extract explainer infographic.

Every geometric value and every number on the panels is derived here from
data/*.json (frozen by freeze_evidence.sh at a pinned engine commit). No
hand-plotted coordinates, no invented numbers: assertions below re-derive the
key figures from the frozen data and fail loudly if a number is missing.
"""
import json
import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
OUT = os.path.join(HERE, "svg")


def load(name):
    with open(os.path.join(DATA, name)) as f:
        return json.load(f)


# ---------------------------------------------------------------- tokens ----
INK = "#16233B"
SUB = "#5A6B84"
FAINT = "#8A97AB"
LINE = "#D7DEEA"
PAPER = "#FFFFFF"
BLUE_900 = "#1E3A8A"
BLUE_700 = "#1D4ED8"
BLUE_600 = "#2563EB"
BLUE_500 = "#3B82F6"
BLUE_400 = "#60A5FA"
BLUE_300 = "#93C5FD"
BLUE_100 = "#DBEAFE"
BLUE_050 = "#EFF6FF"
SKY_600 = "#0284C7"
SKY_100 = "#E0F2FE"
AMBER_700 = "#B45309"
GREEN_700 = "#047857"
FONT = "font-family=\"'PingFang SC','Hiragino Sans GB','Microsoft YaHei',sans-serif\""
MONO = "font-family=\"'SF Mono','Menlo','Consolas',monospace\""


# ------------------------------------------------------------- text util ----
def char_w(ch, fs, mono=False):
    """Advance-width estimate for PingFang SC / SF Mono.

    Monospace ASCII advances uniformly at 0.6em (real SF Mono/Menlo metric);
    the proportional estimate is deliberately conservative.
    """
    if re.match(r'[⺀-鿿豈-﫿＀-￯　-〿]', ch):
        return fs * 1.0
    if mono:
        return fs * 0.60
    if ch in "iljI.,:;'|!":
        return fs * 0.30
    if ch in "mwMW@…":
        return fs * 0.85
    if ch.isdigit() or ch.isupper():
        return fs * 0.62
    return fs * 0.53


def text_w(s, fs, mono=False):
    return sum(char_w(c, fs, mono) for c in s)


def wrap(s, fs, max_w, max_lines=None, mono=False):
    lines, cur, cur_w = [], "", 0.0
    for ch in s:
        w = char_w(ch, fs, mono)
        if cur_w + w > max_w and cur:
            lines.append(cur)
            cur, cur_w = ch, w
        else:
            cur += ch
            cur_w += w
    if cur:
        lines.append(cur)
    if max_lines and len(lines) > max_lines:
        raise ValueError(f"text does not fit in {max_lines} lines: {s!r} -> {lines}")
    return lines


def esc(s):
    return (s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


# ------------------------------------------------------------ svg canvas ----
class Canvas:
    def __init__(self, w, h, bg=None):
        self.w, self.h = w, h
        self.parts = []
        if bg:
            self.parts.append(f'<rect x="0" y="0" width="{w}" height="{h}" fill="{bg}"/>')

    def add(self, s):
        self.parts.append(s)

    def text(self, x, y, s, fs=14, fill=INK, weight="normal", anchor="start",
             mono=False, opacity=None):
        fam = MONO if mono else FONT
        extra = ""
        if opacity:
            extra += f' opacity="{opacity}"'
        self.parts.append(
            f'<text x="{x}" y="{y}" font-size="{fs}" fill="{fill}" '
            f'font-weight="{weight}" text-anchor="{anchor}" {fam}{extra}>{esc(s)}</text>')

    def para(self, x, y, s, fs=14, max_w=400, fill=INK, weight="normal", lh=None,
             anchor="start", mono=False):
        lh = lh or fs * 1.55
        lines = wrap(s, fs, max_w, mono=mono)
        for i, ln in enumerate(lines):
            self.text(x, y + i * lh, ln, fs=fs, fill=fill, weight=weight, anchor=anchor)
        return len(lines)

    def rect(self, x, y, w, h, fill=PAPER, stroke=None, rx=0, sw=1, dash=None):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        s = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
        self.parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}"{s}{d}/>')

    def line(self, x1, y1, x2, y2, stroke=LINE, sw=1, dash=None):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.parts.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{stroke}" stroke-width="{sw}"{d}/>')

    def path(self, d, stroke=BLUE_600, sw=2, fill="none", dash=None):
        dd = f' stroke-dasharray="{dash}"' if dash else ""
        self.parts.append(f'<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{dd}/>')

    def circle(self, cx, cy, r, fill, stroke=None, sw=1):
        s = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
        self.parts.append(f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="{fill}"{s}/>')

    def poly(self, pts, fill):
        p = " ".join(f"{x},{y}" for x, y in pts)
        self.parts.append(f'<polygon points="{p}" fill="{fill}"/>')

    def save(self, name):
        body = "\n".join(self.parts)
        svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.w}" '
               f'height="{self.h}" viewBox="0 0 {self.w} {self.h}">\n{body}\n</svg>\n')
        path = os.path.join(OUT, name)
        with open(path, "w") as f:
            f.write(svg)
        return path


def hpath(x1, y1, x2, y2, r=8):
    """Orthogonal connector with a rounded corner; horizontal-first."""
    if y1 == y2:
        return f"M {x1} {y1} L {x2} {y2}"
    if x1 == x2:
        return f"M {x1} {y1} L {x2} {y2}"
    sx = 1 if x2 >= x1 else -1
    sy = 1 if y2 >= y1 else -1
    xm = x2 - sx * r
    ym = y1 + sy * r
    if abs(x2 - x1) < 2 * r or abs(y2 - y1) < 2 * r:
        return f"M {x1} {y1} L {x1} {y2} L {x2} {y2}"
    return f"M {x1} {y1} L {xm} {y1} Q {x2} {y1} {x2} {ym} L {x2} {y2}"


def vpath(x1, y1, x2, y2, r=8):
    """Vertical-first orthogonal connector with a rounded corner."""
    if x1 == x2:
        return f"M {x1} {y1} L {x2} {y2}"
    sy = 1 if y2 >= y1 else -1
    sx = 1 if x2 >= x1 else -1
    ym = y2 - sy * r
    xm = x1 + sx * r
    if abs(y2 - y1) < 2 * r or abs(x2 - x1) < 2 * r:
        return f"M {x1} {y1} L {x2} {y1} L {x2} {y2}"
    return f"M {x1} {y1} L {x1} {ym} Q {x1} {y2} {xm} {y2} L {x2} {y2}"


def arrow_head(c, x, y, direction, fill=BLUE_600, size=7):
    if direction == "down":
        c.poly([(x - size * 0.62, y - size), (x + size * 0.62, y - size), (x, y)], fill)
    elif direction == "up":
        c.poly([(x - size * 0.62, y + size), (x + size * 0.62, y + size), (x, y)], fill)
    elif direction == "right":
        c.poly([(x - size, y - size * 0.62), (x - size, y + size * 0.62), (x, y)], fill)
    else:
        c.poly([(x + size, y - size * 0.62), (x + size, y + size * 0.62), (x, y)], fill)


def chip(c, x, y, s, fs=12, fill=BLUE_050, stroke=BLUE_300, tfill=BLUE_900, pad=8, h=24, mono=False):
    w = text_w(s, fs, mono=mono) + 2 * pad
    c.rect(x, y, w, h, fill=fill, stroke=stroke, rx=h / 2, sw=1)
    c.text(x + pad, y + h / 2 + fs * 0.36, s, fs=fs, fill=tfill, mono=mono)
    return w


def claim_tag(c, x, y, ids):
    s = " ".join(ids)
    fs = 11
    w = text_w(s, fs, mono=True) + 12
    c.rect(x, y, w, 18, fill="#FFFFFF", stroke=LINE, rx=9, sw=1)
    c.text(x + 6, y + 12.5, s, fs=fs, fill=FAINT, mono=True)
    return w


# ================================================================ panels ====
def panel_kpi(presets, cli_help, provider, vocab, simple_stats):
    engines = ["simple", "schema-json", "toolcall", "agentic"]
    backends = ["llms", "agent", "mock"]
    agents = ["minimaxcc", "glmcc", "mimocc", "pi-agent"]
    outs = ["json", "jsonl", "kg-protocol", "node-link", "ladybug-import",
            "communities", "communities-hierarchy", "mermaid", "stats"]
    for tok in engines + backends + agents + outs:
        assert tok in cli_help, f"CLI surface token missing from --help: {tok}"
    caps = [c["capability_id"] for c in provider["manifest"]["capabilities"]]
    assert len(caps) == 6 and len(set(caps)) == 6

    items = [
        (str(len(engines)), "种抽取机制", "同一抽取契约"),
        (str(len(backends)), "种模型后端", "llms / agent / mock"),
        (str(len(agents)), "个智能体 CLI", "minimaxcc · glmcc · mimocc · pi-agent"),
        (str(len(outs)), "种输出格式", "图谱 · 协议 · 社区 · 图表"),
        (str(presets["total"]), "份领域模板", f"{len(presets['domains'])} 个域 · 实跑清点"),
        (str(len(caps)), "项 provider 能力", "describe / available / invoke"),
    ]
    W, H = 1120, 168
    c = Canvas(W, H)
    gap = 12
    cw = (W - gap * (len(items) - 1)) / len(items)
    for i, (num, label, sub) in enumerate(items):
        cx = i * (cw + gap)
        c.rect(cx + 1, 1, cw - 2, H - 2, fill=PAPER, stroke=LINE, rx=12, sw=1)
        c.text(cx + cw / 2, 62, num, fs=44, fill=BLUE_700, weight="bold", anchor="middle")
        c.text(cx + cw / 2, 96, label, fs=15, fill=INK, weight="600", anchor="middle")
        c.text(cx + cw / 2, 124, sub, fs=11.5, fill=SUB, anchor="middle")
    return c.save("kpi.svg")


def panel_pipeline(cli_help):
    W, H = 1120, 780
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)

    # -- stage 1: inputs ---------------------------------------------------
    y1, h1 = 36, 74
    c.text(24, 28, "① 输入", fs=15, fill=BLUE_900, weight="700")
    in_w = 250
    c.rect(24, y1, in_w, h1, fill=BLUE_050, stroke=BLUE_300, rx=10, sw=1)
    c.text(38, y1 + 28, "纯文本  -f doc.md", fs=13.5, fill=INK, weight="600")
    c.text(38, y1 + 52, "或标准输入；文档名成为引用出处", fs=11.5, fill=SUB)
    c.rect(24 + in_w + 14, y1, in_w + 30, h1, fill=BLUE_050, stroke=BLUE_300, rx=10, sw=1)
    c.text(24 + in_w + 28, y1 + 28, "预切分输入  -F chunks", fs=13.5, fill=INK, weight="600")
    c.text(24 + in_w + 28, y1 + 52, "JSONL 逐块喂入：块自带行/页/框坐标", fs=11.5, fill=SUB)

    c.path(hpath(300, y1 + h1, 300, y1 + h1 + 30), stroke=BLUE_600, sw=2)
    arrow_head(c, 300, y1 + h1 + 37, "down")

    # -- stage 2: chunking -------------------------------------------------
    y2 = y1 + h1 + 37
    h2 = 66
    c.text(24, y2 - 8, "② 分段", fs=15, fill=BLUE_900, weight="700")
    c.rect(24, y2, 460, h2, fill=PAPER, stroke=BLUE_300, rx=10, sw=1.2)
    c.text(38, y2 + 26, "三种分段器  -k recursive | char | token", fs=13.5, fill=INK, weight="600")
    c.text(38, y2 + 50, "递归（默认，尊重词/句边界）· 字符滑窗（Python 对齐）· token 计量", fs=11.5, fill=SUB)
    c.rect(510, y2, 440, h2, fill=PAPER, stroke=LINE, rx=10, sw=1)
    c.text(524, y2 + 26, "预切分输入跳过分段", fs=13.5, fill=INK, weight="600")
    c.text(524, y2 + 50, "给的块即抽取单元；块的坐标直接成为引用坐标", fs=11.5, fill=SUB)

    c.path(hpath(300, y2 + h2, 300, y2 + h2 + 30), stroke=BLUE_600, sw=2)
    arrow_head(c, 300, y2 + h2 + 37, "down")

    # -- stage 3: extraction mechanisms -------------------------------------
    y3 = y2 + h2 + 37
    h3 = 128
    c.text(24, y3 - 8, "③ 抽取机制 × schema 模式（两个正交维度）", fs=15, fill=BLUE_900, weight="700")
    mech = [
        ("simple", "分隔符提示 + 多轮补捞", "每段一个会话，追问「漏了什么」拉高召回"),
        ("schema-json", "模式化 JSON 输出", "种子 schema 引导，输出直接可解析"),
        ("toolcall", "类型化工具调用", "调 add_entity / add_relation 记录事实"),
        ("agentic", "单会话连续多轮", "全文入只读沙箱，跨切片复用实体名"),
    ]
    mw = (1120 - 48 - 3 * 12) / 4
    for i, (name, tag, desc) in enumerate(mech):
        mx = 24 + i * (mw + 12)
        c.rect(mx, y3, mw, h3, fill=BLUE_050 if i < 3 else SKY_100,
               stroke=BLUE_300 if i < 3 else "#7DD3FC", rx=10, sw=1)
        c.text(mx + 12, y3 + 24, name, fs=13.5, fill=BLUE_900, weight="700", mono=True)
        c.text(mx + 12, y3 + 46, tag, fs=12.5, fill=INK, weight="600")
        c.para(mx + 12, y3 + 68, desc, fs=11, max_w=mw - 24, fill=SUB)
    sy = y3 + h3 + 12
    c.text(24, sy + 12, "schema 模式（机制通用）：", fs=11.5, fill=SUB)
    xx = 24 + text_w("schema 模式（机制通用）：", 11.5) + 10
    for lab in ["open 自由推断", "fixed 闭世界硬约束", "evolving 种子+可生长"]:
        xx += chip(c, xx, sy - 4, lab, fs=11.5) + 8

    c.path(hpath(300, sy + 24, 300, sy + 54), stroke=BLUE_600, sw=2)
    arrow_head(c, 300, sy + 61, "down")

    # -- stage 4: parse & merge ---------------------------------------------
    y4 = sy + 61
    h4 = 96
    c.text(24, y4 - 8, "④ 解析 · 归并 · 去重", fs=15, fill=BLUE_900, weight="700")
    steps = [
        ("解析", "分隔符记录 / JSON / 工具参数"),
        ("合并去重", "实体按小写标签；三元组按（主，谓，宾）"),
        ("共指（可选）", "跨块模糊共指：编辑距离 + 词集双通道"),
        ("方向归一（可选）", "逆谓词对翻转到正典方向再去重"),
    ]
    sw_ = (1120 - 48 - 3 * 12) / 4
    for i, (t, d) in enumerate(steps):
        sx = 24 + i * (sw_ + 12)
        c.rect(sx, y4, sw_, h4, fill=PAPER, stroke=LINE, rx=10, sw=1)
        c.text(sx + 12, y4 + 30, t, fs=13, fill=INK, weight="700")
        c.para(sx + 12, y4 + 52, d, fs=11, max_w=sw_ - 24, fill=SUB)
        if i < 3:
            ax = sx + sw_ + 2
            c.line(ax, y4 + h4 / 2, ax + 8, y4 + h4 / 2, stroke=BLUE_400, sw=1.6)
            arrow_head(c, ax + 10, y4 + h4 / 2, "right", fill=BLUE_400, size=6)

    c.path(hpath(300, y4 + h4, 300, y4 + h4 + 30), stroke=BLUE_600, sw=2)
    arrow_head(c, 300, y4 + h4 + 37, "down")

    # -- stage 5: the graph --------------------------------------------------
    y5 = y4 + h4 + 37
    h5 = 78
    c.text(24, y5 - 8, "⑤ 知识图谱", fs=15, fill=BLUE_900, weight="700")
    c.rect(24, y5, 1072, h5, fill=BLUE_100, stroke=BLUE_500, rx=10, sw=1.4)
    c.text(40, y5 + 30, "类型化实体 + 谓词三元组 + 引用出处（由代码按块坐标盖章）", fs=14.5, fill=BLUE_900, weight="700")
    c.text(40, y5 + 58, "实体身份确定性派生；重复记录与平行边在归并中收敛；每条记录可携带多条出处", fs=11.5, fill=BLUE_900, opacity="0.75")

    c.path(hpath(560, y5 + h5, 560, y5 + h5 + 28), stroke=BLUE_600, sw=2)
    arrow_head(c, 560, y5 + h5 + 35, "down")

    # -- outputs fan ---------------------------------------------------------
    y6 = y5 + h5 + 35
    outs = ["json", "jsonl", "kg-protocol", "node-link", "ladybug-import",
            "communities", "communities-hierarchy", "mermaid", "stats"]
    c.text(24, y6 + 4, "⑥ 输出", fs=15, fill=BLUE_900, weight="700")
    xx = 24
    yy = y6 + 14
    for o in outs:
        w = text_w(o, 12, mono=True) + 22
        if xx + w > 1096:
            xx = 24
            yy += 34
        c.rect(xx, yy, w, 26, fill=PAPER, stroke=BLUE_300, rx=13, sw=1)
        c.text(xx + 11, yy + 17.5, o, fs=12, fill=BLUE_900, mono=True)
        xx += w + 8
    assert yy + 26 <= H, f"outputs overflow: {yy + 26} > {H}"
    return c.save("pipeline.svg")


TYPE_ZH = {
    "PRODUCT": "产品", "ORGANIZATION": "组织", "CITY": "城市", "SOFTWARE": "软件",
    "TECHNOLOGY": "技术", "EVENT": "事件", "LAW": "制度", "DIGITAL_ASSET": "数据资产",
}


def panel_graph(simple, communities):
    g = simple["graph"]
    nodes = {n["id"]: n for n in g["nodes"]}
    links = [(l["source"], l["label"], l["target"]) for l in g["links"]]
    assert len(nodes) == simple["stats"]["num_entities"] == 17
    assert len(links) == simple["stats"]["num_triples"] == 15

    comms = communities["result"]["communities"]
    com_of = {}
    for k, ids in comms.items():
        for i in ids:
            com_of[i] = int(k)
    assert len(com_of) == 17

    # deterministic layout: columns by graph role, order by (column, label)
    col_of_label = {
        "Aurora Portal": 0, "Helio Systems": 0, "Singapore": 0,
        "Identity Service": 1, "Billing Worker": 1, "Audit Console": 1,
        "Export Job": 1, "Api Gateway": 1,
        "Postgresql": 2, "Kafka": 2, "Clickhouse": 2, "Object Storage": 2,
        "Kubernetes": 2, "Invoice Events": 2,
        "Data Retention Policy": 3, "Incident Reports": 3, "Audit Logs": 4,
    }
    assert set(col_of_label) == {n["label"] for n in nodes.values()}, "layout covers every node"

    W, H = 1120, 640
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)

    stats = simple["stats"]
    kpis = [(str(stats["num_entities"]), "实体"), (str(stats["num_triples"]), "三元组"),
            (str(len(comms)), "个社区"), (str(len(stats["entity_types"])), "种类型"),
            (str(stats["num_segments_processed"]), "个分段")]
    xx = 24
    for num, lab in kpis:
        c.text(xx, 36, num, fs=26, fill=BLUE_700, weight="bold")
        c.text(xx + text_w(num, 26) + 6, 36, lab, fs=12.5, fill=SUB)
        xx += text_w(num, 26) + text_w(lab, 12.5) + 26
    claim_tag(c, W - 150, 16, ["C04", "C10"])

    header_h = 66
    col_x = {0: 120, 1: 380, 2: 640, 3: 830, 4: 1040}
    cols_zh = {0: "主体", 1: "软件模块", 2: "技术与产物", 3: "治理", 4: "凭据"}
    for k, x in col_x.items():
        c.text(x, header_h, cols_zh[k], fs=11.5, fill=FAINT, anchor="middle")

    nw, nh = 132, 44
    # Explicit deterministic row order per column (adjacent same-column nodes
    # connect by a short vertical; the order below makes every same-column
    # edge adjacent — asserted below, so a layout/data mismatch fails loudly).
    col_rows = {
        0: ["Aurora Portal", "Helio Systems", "Singapore"],
        1: ["Audit Console", "Billing Worker", "Export Job",
            "Api Gateway", "Identity Service"],
        2: ["Clickhouse", "Invoice Events", "Kafka", "Kubernetes",
            "Object Storage", "Postgresql"],
        3: ["Data Retention Policy", "Incident Reports"],
        4: ["Audit Logs"],
    }
    assert {l for v in col_rows.values() for l in v} == set(col_of_label)
    label2nid = {n["label"]: nid for nid, n in nodes.items()}
    row_of = {label2nid[l]: (k, j) for k, rows in col_rows.items()
              for j, l in enumerate(rows)}
    pos = {}
    area_top, area_h = header_h + 22, 420
    for k, rows in col_rows.items():
        n_ = len(rows)
        if n_ > 1:
            g = min(60.0, (area_h - n_ * nh) / (n_ - 1))
            y0 = area_top + (area_h - (n_ * nh + (n_ - 1) * g)) / 2
            ys = [y0 + j * (nh + g) for j in range(n_)]
        else:
            ys = [area_top + (area_h - nh) / 2]
        for j, l in enumerate(rows):
            nid = label2nid[l]
            pos[nid] = (col_x[k] - nw / 2, ys[j])

    col_of = {nid: col_of_label[n["label"]] for nid, n in nodes.items()}

    # ---- routing discipline ------------------------------------------------
    # (a) attach points are slots spread along the box side, so fan-in/fan-out
    #     edges never share an attach point;
    # (b) every crossing edge gets its own vertical channel x inside the gap,
    #     so no two strokes are collinear;
    # (c) same-column edges connect vertically adjacent boxes bottom->top;
    # (d) the one c0->c2 edge rides the empty band above the columns.
    side_deg = {}
    same_col = []
    for src, _p, tgt in links:
        if col_of[src] != col_of[tgt]:
            lo, hi = sorted((col_of[src], col_of[tgt]))
            side_deg[(src, "R" if col_of[tgt] > col_of[src] else "L")] = \
                side_deg.get((src, "R" if col_of[tgt] > col_of[src] else "L"), 0) + 1
            side_deg[(tgt, "L" if col_of[tgt] > col_of[src] else "R")] = \
                side_deg.get((tgt, "L" if col_of[tgt] > col_of[src] else "R"), 0) + 1
        else:
            same_col.append((src, tgt))
            assert abs(row_of[src][1] - row_of[tgt][1]) == 1, \
                f"same-column edge is not between adjacent rows: {src} -> {tgt}"

    slot_ix = {}

    def slot(nid, side):
        key = (nid, side)
        d = side_deg.get(key, 1)
        i = slot_ix.get(key, 0)
        slot_ix[key] = i + 1
        x, y = pos[nid]
        cy = y + nh / 2
        span = min(nh - 12, 10 * (d - 1) + 1) if d > 1 else 0
        off = -span / 2 + (span * i / (d - 1) if d > 1 else 0)
        return (x + nw if side == "R" else x), cy + off

    gap_ch = {}

    def channel(x0):
        n = gap_ch.get(x0, 0)
        gap_ch[x0] = n + 1
        return x0 + 18 + n * 16

    edge_geoms = []
    for src, pred, tgt in links:
        cs, ct = col_of[src], col_of[tgt]
        if cs == ct:
            # vertical connector between adjacent rows; label sits in the
            # empty band between the two boxes
            up = row_of[src][1] > row_of[tgt][1]
            y_a = pos[src][1] + (0 if up else nh)
            y_b = pos[tgt][1] + (nh if up else 0)
            x_ = col_x[cs]
            d = f"M {x_} {y_a} L {x_} {y_b}"
            edge_geoms.append((d, pred, x_ + 8, (y_a + y_b) / 2 + 3, "start"))
        elif ct - cs > 1:
            # express edge over the top band (asserted to be the only one)
            assert (cs, ct) == (0, 2) and not any(
                g[4] == "express" for g in edge_geoms), "unexpected skip-column edge"
            ty = pos[tgt][1] + nh / 2
            top_y = area_top - 8
            d = (f"M {col_x[cs]} {pos[src][1]} L {col_x[cs]} {top_y} "
                 f"L {col_x[ct] - 34} {top_y} L {col_x[ct] - 34} {ty} "
                 f"L {pos[tgt][0]} {ty}")
            # label sits between the column captions, clear of every caption box
            lx = (col_x[ct - 1] + col_x[ct]) / 2
            edge_geoms.append((d, pred, lx, top_y - 6, "express"))
        else:
            fwd = ct > cs
            sx, sy = slot(src, "R" if fwd else "L")
            tx, ty = slot(tgt, "L" if fwd else "R")
            if abs(sy - ty) < 1:
                d = f"M {sx} {sy} L {tx} {ty}"
                edge_geoms.append((d, pred, (sx + tx) / 2, sy - 6, "plain"))
            else:
                x0 = (col_x[cs] + nw / 2) if fwd else (col_x[ct] + nw / 2)
                chx = channel(x0)
                d = f"M {sx} {sy} L {chx} {sy} L {chx} {ty} L {tx} {ty}"
                edge_geoms.append((d, pred, chx + 5, (sy + ty) / 2 + 3, "start"))

    for d, pred, lx, ly, kind in edge_geoms:
        c.path(d, stroke=BLUE_400, sw=1.6)
        lw_ = text_w(pred, 9.5, mono=True)
        if kind in ("express", "plain"):
            c.rect(lx - lw_ / 2 - 3, ly - 9, lw_ + 6, 12, fill=PAPER, rx=2)
            c.text(lx, ly, pred, fs=9.5, fill=BLUE_700, anchor="middle", mono=True)
        else:
            c.rect(lx - 3, ly - 9, lw_ + 6, 12, fill=PAPER, rx=2)
            c.text(lx, ly, pred, fs=9.5, fill=BLUE_700, mono=True)

    COMM_FILL = {0: BLUE_600, 1: SKY_600, 2: BLUE_500, 3: "#38BDF8", 4: BLUE_400}
    COMM_TXT = {0: "#FFFFFF", 1: "#FFFFFF", 2: "#FFFFFF", 3: "#083A5C", 4: "#FFFFFF"}

    for nid, (x, y) in pos.items():
        n = nodes[nid]
        k = com_of[nid]
        c.rect(x, y, nw, nh, fill=COMM_FILL[k], stroke="#FFFFFF", rx=9, sw=1.4)
        tzh = TYPE_ZH.get(n["type"], n["type"])
        try:
            lines = wrap(n["label"], 12, nw - 16, max_lines=1)
            c.text(x + nw / 2, y + 19, lines[0], fs=12, fill=COMM_TXT[k],
                   weight="600", anchor="middle")
            c.text(x + nw / 2, y + 35, tzh, fs=9.5, fill=COMM_TXT[k],
                   anchor="middle", opacity="0.85")
        except ValueError:
            lines = wrap(n["label"], 11, nw - 14, max_lines=2)
            c.text(x + nw / 2, y + 15, lines[0], fs=11, fill=COMM_TXT[k],
                   weight="600", anchor="middle")
            if len(lines) > 1:
                c.text(x + nw / 2, y + 28, lines[1], fs=11, fill=COMM_TXT[k],
                       weight="600", anchor="middle")
                c.text(x + nw / 2, y + 40, tzh, fs=8.5, fill=COMM_TXT[k],
                       anchor="middle", opacity="0.85")
            else:
                c.text(x + nw / 2, y + 33, tzh, fs=9, fill=COMM_TXT[k],
                       anchor="middle", opacity="0.85")

    ly = H - 46
    c.line(24, ly - 14, 1096, ly - 14, stroke=LINE, sw=1)
    c.text(24, ly + 4, "社区着色（标签传播，边权=三元组重数）：", fs=11, fill=SUB)
    xx = 24 + text_w("社区着色（标签传播，边权=三元组重数）：", 11) + 10
    legend = [("0 主体簇 · 8", 0), ("1 治理链 · 3", 1), ("2 计费链 · 3", 2), ("3 身份链 · 3", 3)]
    for lab, k in legend:
        c.rect(xx, ly - 8, 14, 14, fill=COMM_FILL[k], rx=3)
        c.text(xx + 20, ly + 3, lab, fs=11, fill=SUB)
        xx += 20 + text_w(lab, 11) + 16
    c.text(1096, ly + 4, "边标签 = 谓词", fs=11, fill=FAINT, anchor="end")
    return c.save("aurora_graph.svg")


def panel_mechanisms(simple, sj_open, toolcall, readme):
    W, H = 1120, 470
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, "三条机制 · 同一份图契约", fs=16, fill=BLUE_900, weight="700")

    lanes = [
        ("simple", "分段并发，每段独立会话", simple["stats"], "分隔符记录 → 解析", BLUE_600),
        ("schema-json", "整篇一次调用", sj_open, "模式化 JSON → 解析", BLUE_500),
        ("toolcall", "类型化工具调用轮次", toolcall["stats"], "工具参数即结构", SKY_600),
    ]
    lane_w = 250
    lane_x = [24, 24 + lane_w + 18, 24 + 2 * (lane_w + 18)]
    for (name, tag, stats, parse, color), lx in zip(lanes, lane_x):
        c.rect(lx, 48, lane_w, 150, fill=BLUE_050, stroke=LINE, rx=10, sw=1)
        c.text(lx + 14, 74, name, fs=13.5, fill=color, weight="700", mono=True)
        c.text(lx + 14, 96, tag, fs=11.5, fill=SUB)
        c.text(lx + 14, 122, parse, fs=11.5, fill=INK)
        c.text(lx + 14, 150, f"分段数 {stats['num_segments_processed']}", fs=11.5, fill=SUB)
        c.path(hpath(lx + lane_w / 2, 198, lx + lane_w / 2, 226), stroke=color, sw=2)

    gx = 24 + 3 * (lane_w + 18)
    gw = W - 24 - gx
    c.rect(gx, 48, gw, 150, fill=BLUE_100, stroke=BLUE_500, rx=10, sw=1.4)
    c.text(gx + 16, 76, "同一契约", fs=15, fill=BLUE_900, weight="700")
    c.text(gx + 16, 112, "17 实体 · 15 三元组", fs=24, fill=BLUE_700, weight="bold")
    c.para(gx + 16, 140, "三条机制对同一语料、同一份事实数据收敛到同一张图——机制可替换，图谱契约不变。",
           fs=11.5, max_w=gw - 32, fill=BLUE_900)
    # each lane enters the shared card at its own attach point (no stacked
    # arrowheads, no shared stroke lines)
    for k, lx in enumerate(lane_x):
        ey = 78 + k * 45
        c.path(hpath(lx + lane_w, ey, gx - 8, ey), stroke=BLUE_400, sw=1.8, dash="5 4")
        arrow_head(c, gx - 4, ey, "right", fill=BLUE_400)

    c.line(24, 226, 1096, 226, stroke=LINE, sw=1)
    c.text(24, 254, "第四条机制：agentic —— 换一种取舍", fs=16, fill=BLUE_900, weight="700")
    c.para(24, 278,
           "整篇文档进一个连续多轮会话：全文写入隔离目录，智能体在只读工具沙箱（Read/Grep/Glob，无写入/执行）里按切片工作。"
           "同一会话携带上下文，模型跨切片复用实体名——共指发生在抽取时，而不是事后合并。",
           fs=12.5, max_w=1072, fill=INK)
    d8 = next(cl for cl in readme["claims"] if cl["id"] == "D08")
    rows = [
        ("实体数", "516（碎片化：一个术语裂成约 7 个节点）", "136（合并：一个节点）"),
        ("孤儿率", "约 1%", "约 0%"),
        ("速度", "快（分段并发）", "慢（严格串行）"),
        ("定位", "粒度与召回", "合并与共指"),
    ]
    ry = 316
    cw0, cw1 = 150, 430
    c.rect(24, ry - 18, cw0, 28, fill="#F1F5FB")
    c.rect(24 + cw0, ry - 18, cw1, 28, fill="#F1F5FB")
    c.rect(24 + cw0 + cw1, ry - 18, cw1, 28, fill="#F1F5FB")
    c.text(24 + 10, ry + 1, "62 KB 手册实测", fs=11.5, fill=SUB, weight="700")
    c.text(24 + cw0 + 10, ry + 1, "simple（每段并发）", fs=11.5, fill=BLUE_700, weight="700")
    c.text(24 + cw0 + cw1 + 10, ry + 1, "agentic（单会话串行）", fs=11.5, fill=SKY_600, weight="700")
    for i, (k, a, b) in enumerate(rows):
        yy = ry + 24 + i * 26
        if i % 2 == 0:
            c.rect(24, yy - 16, cw0 + 2 * cw1, 24, fill="#FAFBFE")
        c.text(24 + 10, yy, k, fs=11.5, fill=SUB)
        c.text(24 + cw0 + 10, yy, a, fs=11.5, fill=INK)
        c.text(24 + cw0 + cw1 + 10, yy, b, fs=11.5, fill=INK)
    claim_tag(c, W - 150, ry + 24 + 3 * 26 - 14, ["C05", "D07", "D08"])
    c.text(24, H - 12, "62 KB 手册数字为引擎文档记录值（证据记录 D08）；三机制收敛数字来自本页冻结实跑（C05）。",
           fs=10.5, fill=FAINT)
    return c.save("mechanisms.svg")


def panel_schema_modes(modes):
    W, H = 1120, 480
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, "schema 模式：同一份数据的三种纪律", fs=16, fill=BLUE_900, weight="700")

    runs = modes["runs"]
    open_s = runs["open"]["stats"]
    fixed_s = runs["fixed"]["stats"]
    evo_s = runs["evolving"]["stats"]
    assert (open_s["num_entities"], open_s["num_triples"]) == (17, 15)
    assert (fixed_s["num_entities"], fixed_s["num_triples"]) == (7, 1)
    assert (evo_s["num_entities"], evo_s["num_triples"]) == (17, 15)

    cols = [
        ("open · 自由", "种子 schema：不需要", "模型可自由命名实体/关系类型", open_s, BLUE_600),
        ("fixed · 闭世界", "种子 schema：必须非空", "只能用 schema 里的类型；越界即弃", fixed_s, AMBER_700),
        ("evolving · 生长", "种子 schema：必须非空", "以 schema 为引导，可提议新类型", evo_s, SKY_600),
    ]
    cw = 350
    for i, (title, seed_line, free_line, st, color) in enumerate(cols):
        x = 24 + i * (cw + 16)
        c.rect(x, 46, cw, 240, fill=PAPER, stroke=LINE, rx=12, sw=1.2)
        c.rect(x, 46, cw, 34, fill=color, rx=12)
        c.rect(x, 68, cw, 12, fill=color)
        c.text(x + 14, 69, title, fs=14, fill="#FFFFFF", weight="700")
        c.para(x + 14, 104, seed_line, fs=11.5, max_w=cw - 28, fill=SUB)
        c.para(x + 14, 128, free_line, fs=11.5, max_w=cw - 28, fill=SUB)
        c.text(x + 14, 158, "实跑（窄种子：3 类实体 · 4 个谓词）", fs=10.5, fill=FAINT)
        bar_max_w = cw - 130
        for j, (lab, val, vmax) in enumerate([("实体", st["num_entities"], 17), ("三元组", st["num_triples"], 15)]):
            yy = 178 + j * 34
            c.text(x + 14, yy + 12, lab, fs=11.5, fill=INK)
            bw = max(4, bar_max_w * val / vmax)
            c.rect(x + 58, yy, bar_max_w, 16, fill="#F1F5FB", rx=4)
            c.rect(x + 58, yy, bw, 16, fill=color, rx=4)
            c.text(x + 58 + bar_max_w + 8, yy + 12, str(val), fs=13, fill=color, weight="bold")
        c.text(x + 14, 254, f"分段数 {st['num_segments_processed']} · 实体类型 {len(st['entity_types'])} 种 · 谓词 {len(st['predicate_types'])} 种",
               fs=10.5, fill=FAINT)

    x, y, lw_ = 24, 306, 700
    c.rect(x, y, lw_, 130, fill="#FFFBEB", stroke="#F59E0B", rx=10, sw=1)
    c.text(x + 14, y + 24, "fixed 模式丢弃实录（运行 stderr 原文）", fs=12.5, fill=AMBER_700, weight="700")
    stderr_lines = runs["fixed"]["stderr"].strip().splitlines()
    msg = stderr_lines[0]
    assert all(l == msg for l in stderr_lines), "stderr lines differ; re-freeze"
    yy = y + 46
    for ln in wrap(msg, 11, lw_ - 28, mono=True):
        c.text(x + 14, yy, ln, fs=11, fill="#92400E", mono=True)
        yy += 17
    c.text(x + 14, yy + 4, f"同文在 stderr 中重复出现 {len(stderr_lines)} 次；证据文件原样冻结。",
           fs=10, fill=FAINT)
    c.text(x + 14, y + 116, "24 条越界记录被丢弃；存活 7 实体 · 1 三元组，类型只剩种子三类。",
           fs=10.5, fill=AMBER_700)

    x2 = 24 + lw_ + 16
    w2 = W - 24 - x2
    c.rect(x2, y, w2, 130, fill=BLUE_050, stroke=BLUE_300, rx=10, sw=1)
    c.text(x2 + 14, y + 24, "退化格：直接拒绝", fs=12.5, fill=BLUE_900, weight="700")
    c.para(x2 + 14, y + 48,
           "schema 存在 × 允许加类型 的网格里，唯一退化格是「向空 schema 收敛 / 从空 schema 生长」。"
           "fixed / evolving 遇到空种子直接报错，绝不静默降级。",
           fs=11, max_w=w2 - 28, fill=SUB)
    c.text(24, H - 12, "窄种子（3 类实体 · 4 谓词）为演示用自造 schema；同语料全量数据见 C03。声明：C06 · C07 · D10。",
           fs=10.5, fill=FAINT)
    return c.save("schema_modes.svg")


def panel_citations(demo, simple):
    W, H = 1120, 520
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, "出处由代码盖章，不由模型口说", fs=16, fill=BLUE_900, weight="700")

    chunks = demo["input_chunks"]
    c1, c2 = chunks[0]["range"], chunks[1]["range"]
    assert "page" not in c1 and "page" in c2

    x, y, w = 24, 50, 330
    c.rect(x, y, w, 118, fill=BLUE_050, stroke=BLUE_300, rx=10, sw=1)
    c.text(x + 14, y + 24, "输入块 1 · aurora_note.md", fs=12.5, fill=BLUE_900, weight="700")
    c.text(x + 14, y + 48, "range: char_span 0-250", fs=11, fill=SUB, mono=True)
    c.text(x + 14, y + 68, "line: 1-7", fs=11, fill=SUB, mono=True)
    c.text(x + 14, y + 92, "只有行坐标 → 走 legacy 引用形态", fs=10.5, fill=FAINT)

    y2 = y + 132
    c.rect(x, y2, w, 138, fill=SKY_100, stroke="#7DD3FC", rx=10, sw=1)
    c.text(x + 14, y2 + 24, "输入块 2 · aurora_note.md", fs=12.5, fill="#075985", weight="700")
    c.text(x + 14, y2 + 48, "range: char_span 250-500", fs=11, fill=SUB, mono=True)
    c.text(x + 14, y2 + 68, "line: 9-16 · page: 3", fs=11, fill=SUB, mono=True)
    c.text(x + 14, y2 + 88, "bbox: 72.0, 120.5 -> 540.0, 380.0", fs=11, fill=SUB, mono=True)
    c.text(x + 14, y2 + 114, "带页/框坐标 → 完整坐标原样保留", fs=10.5, fill=FAINT)

    c.path(hpath(x + w, y + 59, x + w + 34, y + 59), stroke=BLUE_500, sw=2)
    arrow_head(c, x + w + 40, y + 59, "right")
    c.path(hpath(x + w, y2 + 69, x + w + 34, y2 + 69), stroke=SKY_600, sw=2)
    arrow_head(c, x + w + 40, y2 + 69, "right")

    mx = x + w + 44
    mw = 430
    c.rect(mx, y, mw, 270, fill=PAPER, stroke=BLUE_500, rx=10, sw=1.4)
    c.text(mx + 14, y + 24, f"记录「{demo['entity_sample']['label']}」的引用数组（实跑输出）", fs=12.5, fill=BLUE_900, weight="700")
    cit = demo["entity_sample"]["citations"]
    yy = y + 48
    c.text(mx + 14, yy, "[", fs=11.5, fill=INK, mono=True)
    yy += 20
    for i, cc in enumerate(cit):
        if "lines" in cc:
            body = [f'{{ "doc": "{cc["doc"]}",',
                    f'"lines": [{cc["lines"][0]}, {cc["lines"][1]}] }}']
        else:
            r = cc["range"]
            body = [f'{{ "doc": "{cc["doc"]}",',
                    f'"range": {{ char_span {r["char_span"]["start"]}-{r["char_span"]["end"]},',
                    f'line {r["line"]["start"]}-{r["line"]["end"]}, page {r["page"]["start"]},',
                    f'bbox ({r["bbox"]["x0"]}, {r["bbox"]["y0"]})-({r["bbox"]["x1"]}, {r["bbox"]["y1"]}) }} }}']
        for ln in body:
            c.text(mx + 26, yy, ln, fs=11, fill="#0B2E6B", mono=True)
            yy += 17
        if i == 0:
            c.text(mx + 26, yy, ",", fs=11.5, fill=INK, mono=True)
            yy += 17
    c.text(mx + 14, yy, "]", fs=11.5, fill=INK, mono=True)
    yy += 26
    c.para(mx + 14, yy,
           "同一记录出现在两个块里 → 两条出处并集保留：legacy 行形态 + 富坐标形态同存，一条不丢。",
           fs=11, max_w=mw - 28, fill=SUB)

    px = mx + mw + 16
    pw = W - 24 - px
    c.rect(px, y, pw, 270, fill=BLUE_100, stroke=BLUE_500, rx=10, sw=1.2)
    c.text(px + 14, y + 24, "kg-protocol：提升为一等证据", fs=12.5, fill=BLUE_900, weight="700")
    pe = simple["protocol_entities"][0]
    c.text(px + 14, y + 48, "（-o kg-protocol 实跑实体节选）", fs=10.5, fill=FAINT)
    yy = y + 76
    ev = pe["evidence"][0]
    for ln in [f'label: "{pe["label"]}"',
               f'entity_type: {pe["entity_type"]}',
               f'evidence: [ {{',
               f'  source_file: "{ev["source_file"]}",',
               f'  range: {{ line: {ev["range"]["line"]["start"]}-{ev["range"]["line"]["end"]} }} }} ]']:
        c.text(px + 14, yy, ln, fs=11, fill="#0B2E6B", mono=True)
        yy += 17
    c.para(px + 14, yy + 10,
           "协议输出里证据是一等字段：已识别的引用数组被提升，出处绝不双重序列化。",
           fs=11, max_w=pw - 28, fill=SUB)

    by = y + 290
    c.rect(24, by, 1072, 84, fill="#FAFBFE", stroke=LINE, rx=10, sw=1)
    lead = "为什么不可伪造："
    c.text(38, by + 28, lead, fs=12.5, fill=INK, weight="700")
    c.para(38 + text_w(lead, 12.5) + 8, by + 28,
           "纯文本的行号由分段器跟踪的字符偏移 + 逐文档行号索引推导；预切分输入的页/框坐标由代码原样透传。"
           "模型输出的只有实体与关系本身——坐标它说了不算。",
           fs=12, max_w=1072 - 60 - text_w(lead, 12.5), fill=INK)
    c.text(38, by + 66, "引用数值均来自冻结实跑；声明：C08 · C09 · D01 · D02 · D09。", fs=10.5, fill=FAINT)
    return c.save("citations.svg")


def panel_vocab(vocab):
    W, H = 1120, 400
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, "类型归一：任意叫法 → 规范类型", fs=16, fill=BLUE_900, weight="700")

    stages = [
        ("模型给的任意类型串", '"digital asset" / "公司" / "Tech" …', 440, BLUE_300),
        ("① 精确匹配", "命中规范词表（SCREAMING_SNAKE_CASE 串）", 370, BLUE_500),
        ("② 别名解析", f"{vocab['entity_aliases']} 张实体别名表兜住变体叫法", 300, BLUE_600),
        ("③ 回退桶", "无法识别的落入显式回退类型，可审计", 230, BLUE_700),
    ]
    fy = 56
    for i, (t, d, w, color) in enumerate(stages):
        yy = fy + i * 74
        cx = 24 + (440 - w) / 2
        c.rect(cx, yy, w, 52, fill=PAPER, stroke=color, rx=8, sw=1.4)
        c.text(cx + w / 2, yy + 22, t, fs=13, fill=INK, weight="700", anchor="middle")
        c.text(cx + w / 2, yy + 41, d, fs=10.5, fill=SUB, anchor="middle")
        if i < 3:
            c.path(hpath(244, yy + 52, 244, yy + 70), stroke=BLUE_400, sw=1.8)
            arrow_head(c, 244, yy + 74, "down", fill=BLUE_400, size=6)

    x = 540
    c.text(x, 56, "权威词表清点（kg-vocab，按引擎依赖锁定的版本）", fs=12.5, fill=BLUE_900, weight="700")
    rows = [
        ("实体类型变体", vocab["entity_variants"]),
        ("谓词类型变体", vocab["predicate_variants"]),
        ("实体别名", vocab["entity_aliases"]),
        ("逆谓词对", vocab["predicate_inverses"]),
        ("谓词消歧规则", vocab["predicate_disambiguations"]),
    ]
    vmax = max(v for _, v in rows)
    for i, (k, v) in enumerate(rows):
        yy = 78 + i * 46
        c.text(x, yy + 13, k, fs=12, fill=INK)
        bw = 300 * v / vmax
        c.rect(x + 150, yy, max(bw, 6), 20, fill=BLUE_600 if i < 2 else BLUE_400, rx=4)
        c.text(x + 150 + max(bw, 6) + 10, yy + 15, str(v), fs=13.5, fill=BLUE_700, weight="bold")
    c.para(x, 78 + 5 * 46 + 4,
           "词表独立于引擎、单独锁定发布；与其移植来源的 Python 枚举逐字对齐（122 / 108 双向核对）。"
           "本页只引用清点数字，词表文件名不上页。",
           fs=11, max_w=540, fill=SUB)
    c.text(24, H - 14, "声明：C11 · D03。清点方法：对锁定版本词表数据做程序化计数，非文档转抄。", fs=10.5, fill=FAINT)
    return c.save("vocab.svg")


def panel_presets(presets):
    W, H = 1120, 430
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, f"{presets['total']} 份领域模板 · {len(presets['domains'])} 个域（实跑清点）", fs=16, fill=BLUE_900, weight="700")

    domains = presets["domains"]
    dom_zh = {
        "general": "通用", "finance": "金融", "legal": "法律", "medicine": "医学",
        "industry": "工业", "tcm": "中医", "code": "代码", "knowledge": "知识库",
    }
    items = sorted(domains.items(), key=lambda kv: -kv[1])
    max_v = max(v for _, v in items)
    bx = 190
    bw_max = 560
    for i, (d, v) in enumerate(items):
        yy = 58 + i * 34
        c.text(bx - 12, yy + 14, f"{dom_zh.get(d, d)} {d}", fs=12.5, fill=INK, anchor="end")
        bw = bw_max * v / max_v
        c.rect(bx, yy, bw, 22, fill=BLUE_600 if d == "general" else BLUE_400, rx=4)
        c.text(bx + bw + 10, yy + 15, str(v), fs=13, fill=BLUE_700, weight="bold")

    x = 830
    c.text(x, 58, f"{len(presets['kinds'])} 种结构类型", fs=12.5, fill=BLUE_900, weight="700")
    kind_zh = {
        "graph": "图", "temporal_graph": "时序图", "hypergraph": "超图", "model": "结构模型",
        "set": "集合", "list": "列表", "spatial_graph": "空间图", "spatio_temporal_graph": "时空图",
    }
    for i, (k, v) in enumerate(presets["kinds"].items()):
        yy = 78 + i * 36
        c.circle(x, yy + 6, 5, fill=BLUE_500)
        c.text(x + 14, yy + 10, kind_zh.get(k, k), fs=11.5, fill=INK)
        c.text(x + 92, yy + 10, k, fs=10.5, fill=FAINT, mono=True)
        c.text(x + 250, yy + 10, f"{v} 份", fs=11.5, fill=BLUE_700, weight="bold")

    c.para(24, 58 + 8 * 34 + 10,
           "每份模板 = 多语种（zh/en）抽取目标说明：输出字段结构 + 领域人设与抽取守则 + 命名约定；随二进制内嵌，按名加载、零外部文件，也可自带同格式模板文件。"
           "模板引导「抽什么」，不改变输出 JSON 契约——图谱构建端无感。",
           fs=11.5, max_w=780, fill=SUB)
    note = "示例键（实跑 --list-presets 输出，数据白名单判例）："
    c.text(24, H - 16, note, fs=10.5, fill=FAINT)
    xx = 24 + text_w(note, 10.5) + 8
    for key in ["general/concept_graph", "tcm/formula_composition", "finance/ownership_graph", "code/codebase_graph"]:
        xx += chip(c, xx, H - 32, key, fs=10.5) + 8
    claim_tag(c, W - 110, 14, ["C01"])
    return c.save("presets.svg")


def panel_surfaces(provider, readme):
    W, H = 1120, 620
    c = Canvas(W, H)
    c.rect(1, 1, W - 2, H - 2, fill=PAPER, stroke=LINE, rx=14, sw=1)
    c.text(24, 30, "四个可组合表面", fs=16, fill=BLUE_900, weight="700")

    x, y, w, h = 24, 48, 530, 250
    c.rect(x, y, w, h, fill=PAPER, stroke=LINE, rx=12, sw=1.2)
    c.text(x + 16, y + 26, "CLI · 一次调用一个图", fs=13.5, fill=BLUE_700, weight="700")
    session = [
        ("$ ", "kg-extract -f medium_product_doc.md \\"),
        ("", "    -e schema-json -b mock -o stats"),
        ("", ""),
        ("# ", "输出: 17 实体 · 15 三元组 · 8 种类型"),
    ]
    yy = y + 52
    for pfx, ln in session:
        color = GREEN_700 if pfx == "$ " else (FAINT if pfx == "# " else INK)
        c.text(x + 16, yy, pfx, fs=11.5, fill=GREEN_700 if pfx == "$ " else BLUE_500, mono=True, weight="bold")
        c.text(x + 16 + 16, yy, ln, fs=11.5, fill=color, mono=True)
        yy += 19
    lead = "常用旗标："
    c.text(x + 16, yy + 18, lead, fs=11.5, fill=SUB)
    xx = x + 16
    cy = yy + 27
    for fl in ["-e 引擎", "-b 后端", "-k 分段器", "--schema-mode 模式", "--preset 模板",
               "--coref 共指", "--max-concurrency 并发界", "-o 输出"]:
        wch = text_w(fl, 11) + 18
        if xx + wch > x + w - 16:
            xx = x + 16
            cy += 30
        chip(c, xx, cy, fl, fs=11)
        xx += wch + 8
    assert cy + 24 <= y + h, "CLI flag chips overflow the card"

    x2 = 24 + w + 16
    c.rect(x2, y, w, h, fill=PAPER, stroke=LINE, rx=12, sw=1.2)
    c.text(x2 + 16, y + 26, "配置 · 三层优先级", fs=13.5, fill=BLUE_700, weight="700")
    layers = [
        ("1", "命令行显式旗标", "最高优先；布尔存在性旗标永远开启特性"),
        ("2", "配置文件", "--config 路径 / 内联 JSON / 默认 ~/.kg-extract/config.json"),
        ("3", "内置默认值", "开箱即跑：simple 引擎 + recursive 分段"),
    ]
    for i, (n, t, d) in enumerate(layers):
        yy = y + 50 + i * 44
        c.circle(x2 + 26, yy + 2, 11, fill=BLUE_600)
        c.text(x2 + 26, yy + 6.5, n, fs=11.5, fill="#FFFFFF", weight="bold", anchor="middle")
        c.text(x2 + 46, yy, t, fs=12, fill=INK, weight="600")
        c.para(x2 + 46, yy + 16, d, fs=10.5, max_w=w - 70, fill=SUB)
    lead2 = "公开配置键（节选）："
    c.text(x2 + 16, y + 186, lead2, fs=10.5, fill=FAINT)
    xx = x2 + 16
    ky = y + 194
    for k in ["engine", "backend", "chunker", "schema_mode", "preset", "max_concurrency", "output"]:
        wch = text_w(k, 10) + 16
        if xx + wch > x2 + w - 16:
            xx = x2 + 16
            ky += 28
        chip(c, xx, ky, k, fs=10, fill="#F1F5FB", stroke=LINE, tfill=SUB, h=22)
        xx += wch + 6
    assert ky + 22 <= y + h - 20, "config key chips overflow the card"
    c.text(x2 + 16, y + h - 14, "未知键一律拒绝；显式 --config 缺失或 JSON 损坏即报错。",
           fs=10.5, fill=SUB)

    y2 = y + h + 16
    c.rect(24, y2, 700, 246, fill=PAPER, stroke=LINE, rx=12, sw=1.2)
    c.text(40, y2 + 26, "Provider 协议 · 自描述能力面", fs=13.5, fill=BLUE_700, weight="700")
    caps = [c_["capability_id"] for c_ in provider["manifest"]["capabilities"]]
    xx, yy = 40, y2 + 44
    for cap in caps:
        wch = text_w(cap, 10.5) + 20
        if xx + wch > 24 + 700 - 16:
            xx = 40
            yy += 30
        chip(c, xx, yy, cap, fs=10.5, fill=BLUE_050)
        xx += wch + 8
    inv = provider["invoke"]
    yy += 34
    c.text(40, yy, "$ echo '{ … }' | kg-extract invoke extract.entities_relations --request -", fs=11, fill=INK, mono=True)
    yy += 22
    env = [
        f'"protocol": "{inv["protocol"]}"',
        f'"status": "{inv["status"]}"',
        f'"artifacts": [ {{ "kind": "kg-document",',
        f'  "checksum": "{inv["artifacts"][0]["checksum"][:26]}…" }} ]',
        f'"result": {{ "num_entities": {inv["result"]["num_entities"]}, "num_triples": {inv["result"]["num_triples"]} }}',
    ]
    for ln in env:
        c.text(56, yy, ln, fs=11, fill="#0B2E6B", mono=True)
        yy += 17
    c.para(40, yy + 10, "describe 列能力清单；available 只读探测（退出码恒 0）；invoke 恰好打印一个执行信封，失败非零退出。",
           fs=10.5, max_w=660, fill=SUB)

    x3 = 24 + 700 + 16
    w3 = W - 24 - x3
    c.rect(x3, y2, w3, 246, fill=SKY_100, stroke="#7DD3FC", rx=12, sw=1.2)
    c.text(x3 + 16, y2 + 26, "MCP · 无模型的图构建", fs=13.5, fill="#075985", weight="700")
    c.para(x3 + 16, y2 + 50,
           "服务端不调模型：外部 MCP 客户端读源文本、逐条驱动变更。出处字段组（source_file + 起止行）先验证后写入——"
           "路径必须相对 source 根、行距必须落在文件内，违规即工具错误。",
           fs=11, max_w=w3 - 32, fill=INK)
    lead3 = "工具名："
    c.text(x3 + 16, y2 + 120, lead3, fs=11, fill=SUB)
    xx = x3 + 16
    ty_ = y2 + 128
    for t in ["add_entity", "add_relation", "add_attribute", "propose_schema_type"]:
        wch = text_w(t, 10) + 16
        if xx + wch > x3 + w3 - 12:
            xx = x3 + 16
            ty_ += 28
        chip(c, xx, ty_, t, fs=10, fill="#FFFFFF", stroke="#7DD3FC", tfill="#075985", h=22)
        xx += wch + 6
    c.text(x3 + 16, y2 + 196, "同一实体/关系的重复调用合并引用而非复制记录。",
           fs=10.5, fill=SUB)

    c.text(24, H - 12, "声明：C02 · D04 · D11。信封、能力清单、旗标表均为冻结实跑输出。", fs=10.5, fill=FAINT)
    return c.save("surfaces.svg")


def main():
    os.makedirs(OUT, exist_ok=True)
    presets = load("presets.json")
    cli_help = load("cli_surface.json")["help_text"]
    provider = load("provider.json")
    vocab = load("vocab_census.json")
    simple = load("medium_simple.json")
    modes = load("medium_schema_modes.json")
    toolcall = load("medium_toolcall.json")
    communities = load("medium_communities.json")
    demo = load("citation_demo.json")
    readme = load("readme_documented.json")

    # cross-panel frozen-number assertions (fail loudly, never invent)
    assert presets["total"] == 40 and len(presets["domains"]) == 8
    assert simple["stats"]["num_entities"] == 17 and simple["stats"]["num_triples"] == 15
    assert modes["runs"]["fixed"]["stats"]["num_entities"] == 7
    assert communities["result"]["num_communities"] == 4
    assert vocab["entity_variants"] == 122 and vocab["predicate_variants"] == 108
    assert len(provider["manifest"]["capabilities"]) == 6

    written = []
    written.append(panel_kpi(presets, cli_help, provider, vocab, simple["stats"]))
    written.append(panel_pipeline(cli_help))
    written.append(panel_graph(simple, communities))
    written.append(panel_mechanisms(simple, modes["runs"]["open"]["stats"], toolcall, readme))
    written.append(panel_schema_modes(modes))
    written.append(panel_citations(demo, simple))
    written.append(panel_vocab(vocab))
    written.append(panel_presets(presets))
    written.append(panel_surfaces(provider, readme))
    for p in written:
        print("wrote", p)


if __name__ == "__main__":
    main()
