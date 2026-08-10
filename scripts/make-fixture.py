#!/usr/bin/env python3
"""Generate a ladybug-e2e fixture's mock files from its entities + expected triples.

`scripts/ladybug-e2e-eval.sh` scores an extraction against `<name>_expected.json`
while feeding the engines canned replies from `<name>_simple.mock.txt` and
`<name>_schema.mock.json` (the toolcall mock is derived from the schema mock by
the eval script itself). Those three files must agree exactly: if a mock encodes
a triple the expected set does not contain — or vice versa — the run measures the
fixture's own inconsistency instead of the extractor's behaviour.

So the mocks are generated, not hand-written. Input is one spec file:

    {
      "entities": [ {"name": "Whisper", "type": "MODEL", "description": "..."} ],
      "expected": [ {"subject": "Whisper", "predicate": "DEVELOPED_BY", "object": "OpenAI"} ]
    }

    python3 scripts/make-fixture.py spec.json --name research_survey

writes `<name>_expected.json`, `<name>_simple.mock.txt` and `<name>_schema.mock.json`
into scripts/fixtures/ (`--doc FILE` also installs the source document).

It validates the spec first, because two harness constraints are silent traps:

* **Entity labels must be ASCII.** The scorer compares with
  ``re.sub(r"[^a-z0-9]+", "", s.lower())``, which maps every CJK string to `""` —
  so two *different* Chinese labels compare EQUAL and the run reports false
  passes. Chinese is fine (and wanted) in the document prose; not in labels.
* **Predicates become Cypher relationship types** (``MATCH (a)-[r:PRED]->(b)``),
  so they must be uppercase identifiers.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent / "fixtures"

#: Only these reach a Cypher query cleanly and exist in kg-vocab.
PREDICATE_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")


def validate(spec: dict) -> list[str]:
    """Return a list of problems; empty means the spec is safe to render."""
    problems: list[str] = []
    entities = spec.get("entities") or []
    expected = spec.get("expected") or []

    names = {e["name"] for e in entities}
    if not entities:
        problems.append("spec has no entities")
    if not expected:
        problems.append("spec has no expected triples")

    for e in entities:
        if not e["name"].isascii():
            problems.append(
                f"entity label {e['name']!r} is not ASCII — the scorer normalises it to "
                f'"" and it would compare equal to every other non-ASCII label'
            )
        if not e.get("description"):
            problems.append(f"entity {e['name']!r} has no description")

    seen: set[tuple[str, str, str]] = set()
    for t in expected:
        key = (t["subject"], t["predicate"], t["object"])
        if key in seen:
            problems.append(f"duplicate triple {key}")
        seen.add(key)
        if t["subject"] == t["object"]:
            problems.append(f"self-referential triple {key}")
        if not PREDICATE_RE.match(t["predicate"]):
            problems.append(
                f"predicate {t['predicate']!r} is not an uppercase identifier; it is "
                f"interpolated into a Cypher relationship type"
            )
        for role in ("subject", "object"):
            if t[role] not in names:
                problems.append(f"triple {key}: {role} {t[role]!r} is not declared in entities")
    return problems


def render_schema_mock(spec: dict) -> dict:
    """The SchemaJson engine's reply: entities keyed by name + [s, p, o] rows."""
    return {
        "entities": {
            e["name"]: {"type": e["type"], "attributes": {"description": e["description"]}}
            for e in spec["entities"]
        },
        "relationships": [[t["subject"], t["predicate"], t["object"]] for t in spec["expected"]],
    }


def render_simple_mock(spec: dict) -> str:
    """The Simple engine's delimiter reply.

    Note the field order on relationship rows is (subject, OBJECT, predicate) —
    the predicate comes third, after both endpoints. Getting this wrong silently
    produces a graph with the right nodes and wrong edges.
    """
    lines = [
        f"(entity<|>{e['name']}<|>{e['type'].lower()}<|>{e['description']}<|>)##"
        for e in spec["entities"]
    ]
    for t in spec["expected"]:
        sentence = f"{t['subject']} {t['predicate'].lower().replace('_', ' ')} {t['object']}."
        lines.append(
            f"(relationship<|>{t['subject']}<|>{t['object']}<|>"
            f"{t['predicate'].lower()}<|>{sentence}<|>0.9)##"
        )
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("spec", type=Path, help="JSON spec with `entities` and `expected`")
    ap.add_argument("--name", required=True, help="fixture name, e.g. research_survey")
    ap.add_argument("--doc", type=Path, help="source markdown to install as <name>_doc.md")
    ap.add_argument("--out-dir", type=Path, default=FIXTURES)
    ap.add_argument("--force", action="store_true", help="overwrite an existing fixture")
    args = ap.parse_args(argv)

    spec = json.loads(args.spec.read_text(encoding="utf-8"))

    problems = validate(spec)
    if problems:
        print(f"spec is not usable ({len(problems)} problem(s)):", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    args.out_dir.mkdir(parents=True, exist_ok=True)
    targets = {
        "expected": args.out_dir / f"{args.name}_expected.json",
        "schema": args.out_dir / f"{args.name}_schema.mock.json",
        "simple": args.out_dir / f"{args.name}_simple.mock.txt",
    }
    if args.doc:
        targets["doc"] = args.out_dir / f"{args.name}_doc.md"

    existing = [p for p in targets.values() if p.exists()]
    if existing and not args.force:
        print("refusing to overwrite (pass --force):", file=sys.stderr)
        for p in existing:
            print(f"  {p}", file=sys.stderr)
        return 1

    targets["expected"].write_text(
        json.dumps(spec["expected"], indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    targets["schema"].write_text(
        json.dumps(render_schema_mock(spec), indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    targets["simple"].write_text(render_simple_mock(spec), encoding="utf-8")
    if args.doc:
        targets["doc"].write_text(args.doc.read_text(encoding="utf-8"), encoding="utf-8")

    print(f"fixture '{args.name}': {len(spec['entities'])} entities, {len(spec['expected'])} triples")
    for label, path in targets.items():
        print(f"  {label:9} {path}")
    print(f"\nrun it:  FIXTURE={args.name} just ladybug-eval")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
