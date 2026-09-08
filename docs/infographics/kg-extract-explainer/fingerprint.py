#!/usr/bin/env python3
"""Whole-tree fingerprints + machine verification (wave-2, fingerprint-gaps).

Coverage: EVERY file under this tree (data, tools, svg, render, docs,
index.html) with exactly two named exemptions:

  1. fingerprints.json — the manifest itself (a self-hash has no fixpoint);
  2. data/audit/       — run-record directory (wave-1 established: records
     are appendable without invalidating the fingerprint; writing a run
     into a fingerprinted doc would force another commit + another run).

Manifest carries only stable fields (sorted relative paths, sha256, byte
size; NO timestamps, NO live HEAD) so regeneration is idempotent and
post-commit re-runs change nothing. Every file hash quoted in
VERIFICATION.md (the §4 artifact table, `| path | sha256 |` rows) is
cross-checked against this manifest by --check, so the docs cannot drift
from the single source of truth.

Usage:
    PYTHONDONTWRITEBYTECODE=1 python3 fingerprint.py            # regenerate
    PYTHONDONTWRITEBYTECODE=1 python3 fingerprint.py --check    # verify in place
    PYTHONDONTWRITEBYTECODE=1 python3 fingerprint.py --check --root /tmp/copy
                                                                 # detached verify
Exit codes: 0 ok; 1 verification failure; 2 usage/manifest error.
"""
import hashlib
import json
import os
import re
import sys

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
MANIFEST_NAME = "fingerprints.json"
EXEMPT_FILES = {MANIFEST_NAME}
EXEMPT_DIRS = {os.path.join("data", "audit")}
EXEMPTIONS_DOC = {
    MANIFEST_NAME: "清单自身——自指哈希无 fixpoint（audit-batteries 规则）",
    os.path.join("data", "audit") + "/":
        "运行记录目录（2026-09-06 wave-1 既定豁免；可追加记录不使指纹失效）",
}

def sha256_file(path):
    h = hashlib.sha256()
    n = 0
    with open(path, "rb") as f:
        while True:
            chunk = f.read(1 << 16)
            if not chunk:
                break
            h.update(chunk)
            n += len(chunk)
    return h.hexdigest(), n


def scan_tree(root):
    """Return {relpath: {"sha256":…, "bytes":…}} for every non-exempt file."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == ".":
            dirnames[:] = sorted(dirnames)
        else:
            dirnames[:] = sorted(
                d for d in dirnames
                if os.path.join(rel_dir, d).replace(os.sep, "/")
                not in EXEMPT_DIRS)
        for name in sorted(filenames):
            if name in EXEMPT_FILES:
                continue
            rel = name if rel_dir == "." else os.path.join(rel_dir, name)
            digest, size = sha256_file(os.path.join(dirpath, name))
            out[rel] = {"sha256": digest, "bytes": size}
    return out


def build_manifest(root):
    files = scan_tree(root)
    return {
        "generator": "fingerprint.py",
        "exemptions": EXEMPTIONS_DOC,
        "files": {k: files[k] for k in sorted(files)},
        "total_files": len(files),
        "total_bytes": sum(v["bytes"] for v in files.values()),
    }


def write_manifest(root, manifest):
    out = os.path.join(root, MANIFEST_NAME)
    with open(out, "w") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2, sort_keys=True)
        f.write("\n")
    return out


def doc_hash_check(root, manifest):
    """Every row of the VERIFICATION.md §4 table must be `| path | sha256 |`
    and equal the manifest entry for that path (single source of truth).
    The table is located by its `## 4.` heading and parsed structurally, so
    a corrupted row cannot silently fall outside the regex and skip the
    binding (allow-list-that-switches-the-gate-off trap)."""
    problems = []
    quoted = 0
    doc = os.path.join(root, "VERIFICATION.md")
    if not os.path.isfile(doc):
        return ["doc-hash: VERIFICATION.md missing"], 0
    lines = open(doc).read().splitlines()
    sec = end = None
    for i, line in enumerate(lines):
        if sec is None and line.startswith("## 4."):
            sec = i
        elif sec is not None and i > sec and line.startswith("## "):
            end = i
            break
    if sec is None:
        return ["doc-hash: VERIFICATION §4 heading not found"], 0
    for line in lines[sec + 1:end]:
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if all(set(c) <= set("-: ") for c in cells):
            continue  # separator row
        if cells and cells[0] == "文件":
            continue  # header row
        quoted += 1
        if len(cells) != 2 or not re.fullmatch(r"[0-9a-f]{64}", cells[1]):
            problems.append("doc-hash: §4 row not in `| path | sha256 |` "
                            f"form: {line.strip()[:70]}")
            continue
        path, digest = cells
        entry = manifest["files"].get(path)
        if entry is None:
            problems.append(f"doc-hash: {path} quoted but not in manifest")
        elif entry["sha256"] != digest:
            problems.append(
                f"doc-hash: {path} quoted {digest[:12]}… but manifest has "
                f"{entry['sha256'][:12]}…")
    if quoted == 0:
        problems.append("doc-hash: §4 table has no data rows")
    return problems, quoted


def check(root):
    man_path = os.path.join(root, MANIFEST_NAME)
    if not os.path.isfile(man_path):
        return 2, [f"manifest missing: {man_path} (run without --check first)"]
    with open(man_path) as f:
        manifest = json.load(f)

    problems = []
    on_disk = scan_tree(root)
    listed = manifest["files"]

    for rel in sorted(set(listed) - set(on_disk)):
        problems.append(f"missing: {rel}")
    for rel in sorted(set(on_disk) - set(listed)):
        problems.append(f"unmanifested: {rel}")
    for rel in sorted(set(listed) & set(on_disk)):
        if listed[rel]["sha256"] != on_disk[rel]["sha256"]:
            problems.append(
                f"modified: {rel} "
                f"(manifest {listed[rel]['sha256'][:12]}…, "
                f"disk {on_disk[rel]['sha256'][:12]}…)")

    dh_problems, quoted = doc_hash_check(root, manifest)
    problems.extend(dh_problems)

    print(f"fingerprint --check: {len(listed)} files listed, "
          f"{len(on_disk)} on disk (exempt: {MANIFEST_NAME}, data/audit/), "
          f"{quoted} doc hashes cross-checked vs VERIFICATION.md")
    if problems:
        for p in problems:
            print("  " + p)
        return 1, problems
    print("fingerprint --check: OK")
    return 0, []


def main():
    args = sys.argv[1:]
    if args and args[0] == "--check":
        root = HERE
        i = 1
        while i < len(args):
            if args[i] == "--root" and i + 1 < len(args):
                root = os.path.abspath(args[i + 1])
                i += 2
            else:
                print(__doc__)
                sys.exit(2)
        rc, _ = check(root)
        sys.exit(rc)
    if args:
        print(__doc__)
        sys.exit(2)
    manifest = build_manifest(HERE)
    out = write_manifest(HERE, manifest)
    print(f"wrote {out}: {manifest['total_files']} files, "
          f"{manifest['total_bytes']} bytes (exempt: "
          f"{MANIFEST_NAME}, data/audit/)")


if __name__ == "__main__":
    main()
