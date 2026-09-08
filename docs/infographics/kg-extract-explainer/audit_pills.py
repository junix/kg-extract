#!/usr/bin/env python3
"""Poison-pill battery: prove every gate in this tree can actually bite.

Wave-2 audit hardening (no-poison class). For each gate we inject one
positive control per banned/detected class into a THROWAWAY copy of the
corpus (never the frozen data layer, never the delivered artifacts), run
the real gate, and require it to flag. A gate that cannot bite is a bug
in the gate and fails this battery. Run record lands in
data/audit/2026-09-08-poison-pills.md (fingerprint-exempt location).

Gates covered
  1. build.py code_detail_gate (six banned classes + rust-sigil sub-check)
     - one pill per class injected into a copy of index.html
     - one svg-side pill (same corpus, copy of svg/pipeline.svg) proving
       per-file sweeping, not just the html member
     - clean control: pristine corpus must yield 0 hits
  2. build.py font_floor_gate (via full `python3 build.py` in a copy)
     - one CJK run dropped to 9.5 px must exit 5
  3. svg-linter gate (real binary; missing binary is a hard fail)
     - controls: untouched copy and defs+reference both present stay clean
     - pill: delete the referenced <defs> id -> svg/dangling-reference error

Claims gate: ABSENT in this tree (claims live in the on-page colophon +
VERIFICATION §2 registry, no machine gate yet); recorded as N/A here and
deferred to a future no-claims-binding assignment.

Usage (from anywhere, absolute paths derived from this file's location):
    PYTHONDONTWRITEBYTECODE=1 python3 audit_pills.py
Exit 0 iff every pill BIT and every control stayed clean.
"""
import importlib.util
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
BUILD = os.path.join(HERE, "build.py")
DATA = os.path.join(HERE, "data")
SVG = os.path.join(HERE, "svg")
INDEX = os.path.join(HERE, "index.html")

SVG_LINTER = os.environ.get(
    "SVG_LINTER",
    os.path.expanduser("~/sync/macos-arm64-bin/svg-linter"),
)

SIX_BAN_PILLS = [
    # (pill id, expected gate class tag, injection text)
    ("P1-ban1-filename", "1-filename",
     "引擎模块 llms_backend.rs 负责后端。"),
    ("P2-ban2-fileline-ext", "2-fileline",
     "见 kg_v3.md:237 的说明。"),
    ("P3-ban2-fileline-cn", "2-fileline",
     "配置在 第 42 行 附近。"),
    ("P4-ban3-verbatim", "3-verbatim",
     "use chonkie::{CharChunker, Chunker, RecursiveChunker, "
     "TiktokenTokenizer, TokenChunker};"),
    ("P5-ban3-rust-sigil", "3-rust-sigil",
     "fn build_graph() 每轮 let mut acc 累加。"),
    ("P6-ban4-identifier", "4-identifier",
     "核心结构 AgenticExtractor 顺序多轮。"),
    ("P7-ban5-dirpath", "5-dirpath",
     "模板文件位于 presets/kg_demo.yaml 目录下。"),
    ("P8-ban6-generator", "6-generator",
     "由 freeze_evidence.sh 重新生成。"),
]

SVG_SIDE_PILL = ("P9-ban1-filename-svg-side", "1-filename",
                 "引擎模块 llms_backend.rs 负责后端。")

FONT_PILL_TEXT = ('<text x="8" y="8" font-size="9.5" '
                  'font-family="sans-serif">主体</text>')


def load_build(copy_dir):
    """Import build.py FROM THE THROWAWAY COPY so its HERE-based paths
    (data/, svg/) resolve inside the copy, never against the tree."""
    spec = importlib.util.spec_from_file_location(
        "build_pill", os.path.join(copy_dir, "build.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def fresh_copy():
    d = tempfile.mkdtemp(prefix="kgx-pills-")
    shutil.copy2(BUILD, d)
    shutil.copytree(DATA, os.path.join(d, "data"))
    shutil.copytree(SVG, os.path.join(d, "svg"))
    shutil.copy2(INDEX, os.path.join(d, "index.html"))
    return d


def hit_classes(hits):
    return sorted({h.split("]", 1)[0][1:] for h in hits})


def run_six_ban_pills(results):
    d = fresh_copy()
    try:
        build = load_build(d)
        clean_html = open(os.path.join(d, "index.html")).read()

        # clean control: pristine corpus must be 0
        hits, n_files, n_idents = build.code_detail_gate(clean_html)
        results.append(("CONTROL six-ban clean corpus", len(hits) == 0,
                        f"{len(hits)} hits (scope {n_files} basenames / "
                        f"{n_idents} filtered identifiers)"))

        # per-class pills into the html copy
        all_classes = set()
        for pid, cls, pill in SIX_BAN_PILLS:
            poisoned = clean_html.replace(
                "</body>", f"<p>{pill}</p>\n</body>")
            hits, _, _ = build.code_detail_gate(poisoned)
            got = hit_classes(hits)
            all_classes.update(got)
            results.append((pid, cls in got,
                            f"{len(hits)} hit(s), classes={got}"))

        # combined run: all six banned classes present at once
        poisoned = clean_html
        for _, _, pill in SIX_BAN_PILLS:
            poisoned = poisoned.replace(
                "</body>", f"<p>{pill}</p>\n</body>")
        hits, _, _ = build.code_detail_gate(poisoned)
        got = hit_classes(hits)
        six = {"1-filename", "2-fileline", "3-verbatim",
               "4-identifier", "5-dirpath", "6-generator"}
        results.append(("P-combined six classes", six <= set(got),
                        f"{len(hits)} hit(s), classes={got}"))

        # svg-side pill: same corpus but poison a copied svg member
        target = os.path.join(d, "svg", "pipeline.svg")
        body = open(target).read()
        with open(target, "w") as f:
            f.write(body.replace("</svg>",
                                 f"<text>{SVG_SIDE_PILL[2]}</text></svg>"))
        hits, _, _ = build.code_detail_gate(clean_html)
        svg_hits = [h for h in hits if "pipeline.svg" in h]
        results.append((SVG_SIDE_PILL[0],
                        any(h.startswith("[1-filename]") for h in svg_hits),
                        f"{len(svg_hits)} hit(s) in pipeline.svg: "
                        f"{svg_hits[:2]}"))
        return d
    except Exception as e:  # surface as battery failure, not traceback
        results.append(("six-ban battery", False, f"exception: {e}"))
        return d


def run_font_pill(results, keep_dir):
    d = fresh_copy()
    target = os.path.join(d, "svg", "kpi.svg")
    body = open(target).read()
    with open(target, "w") as f:
        f.write(body.replace("</svg>", FONT_PILL_TEXT + "</svg>"))
    p = subprocess.run(
        [sys.executable, "build.py"], cwd=d,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
        capture_output=True, text=True)
    rc = p.returncode
    named = "FONT-FLOOR GATE FAILED" in p.stderr
    results.append(("P10-font-floor-9.5px-cjk", rc == 5 and named,
                    f"rc={rc}, gate banner={named}, "
                    f"stderr tail={p.stderr.strip().splitlines()[:1]}"))
    keep_dir.append(d)


def lint_plain(path):
    p = subprocess.run([SVG_LINTER, "check", "--plain", path],
                       capture_output=True, text=True)
    findings = [ln for ln in p.stdout.splitlines()
                if ln.startswith("finding")]
    return p.returncode, findings


def run_linter_pills(results, keep_dir):
    if not (os.path.isfile(SVG_LINTER) and os.access(SVG_LINTER, os.X_OK)):
        results.append(("svg-linter binary present", False,
                        f"missing or not executable: {SVG_LINTER}"))
        return
    d = tempfile.mkdtemp(prefix="kgx-pills-lint-")
    keep_dir.append(d)
    src = open(os.path.join(SVG, "kpi.svg")).read()
    defs = ('<defs><linearGradient id="pill-grad">'
            '<stop offset="0" stop-color="#2563EB"/>'
            '</linearGradient></defs>\n'
            '<rect x="2" y="2" width="8" height="8" '
            'fill="url(#pill-grad)"/>\n')

    # control 1: untouched copy stays clean
    untouched = os.path.join(d, "untouched.svg")
    open(untouched, "w").write(src)
    rc, findings = lint_plain(untouched)
    results.append(("CONTROL svg-linter untouched copy",
                    rc == 0 and not findings,
                    f"rc={rc}, findings={len(findings)}"))

    # control 2: defs id AND reference both present -> still clean
    with_defs = os.path.join(d, "with-defs.svg")
    open(with_defs, "w").write(src.replace("</svg>", defs + "</svg>"))
    rc, findings = lint_plain(with_defs)
    results.append(("CONTROL svg-linter defs+ref intact",
                    rc == 0 and not findings,
                    f"rc={rc}, findings={len(findings)}"))

    # pill: delete the referenced defs id (rename) -> dangling reference
    dangling = os.path.join(d, "dangling.svg")
    open(dangling, "w").write(
        src.replace("</svg>", defs + "</svg>").replace(
            'id="pill-grad"', 'id="pill-grad-gone"', 1))
    rc, findings = lint_plain(dangling)
    bit = rc != 0 and any("svg/dangling-reference" in f for f in findings)
    results.append(("P11-svg-linter-defs-id-deleted", bit,
                    f"rc={rc}, findings={len(findings)}, "
                    f"first={findings[0].split(chr(9))[:5] if findings else None}"))


def main():
    results = []
    keep = []
    d = run_six_ban_pills(results)
    run_font_pill(results, keep)
    run_linter_pills(results, keep)
    shutil.rmtree(d, ignore_errors=True)

    print("\n== poison-pill battery ==")
    bit = miss = 0
    for name, ok, detail in results:
        tag = "BIT " if ok else "MISS"
        if "CONTROL" in name:
            tag = "OK  " if ok else "MISS"
        print(f"[{tag}] {name}: {detail}")
        if ok:
            bit += 1
        else:
            miss += 1
    n_pills = sum(1 for n, _, _ in results
                  if n.startswith("P") and "CONTROL" not in n)
    n_ctrl = sum(1 for n, _, _ in results if "CONTROL" in n)
    print(f"\nsummary: {len(results) - miss}/{len(results)} checks passed "
          f"({n_pills} pills bit, {n_ctrl} controls clean, "
          f"claims gate N/A — absent in this tree)")
    if miss:
        print(f"FAILED: {miss} check(s); copies kept for inspection: {keep}")
        sys.exit(1)
    for d in keep:
        shutil.rmtree(d, ignore_errors=True)
    sys.exit(0)


if __name__ == "__main__":
    main()
