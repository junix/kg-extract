#!/usr/bin/env bash
# Evidence freeze for the kg-extract explainer infographic.
#
# Runs the real engine (mock backend, fully offline + deterministic) and the
# read-only repo censuses, then packages every number shown on the page into
# data/*.json plus data/provenance.json.
#
# Env:
#   KG_EXTRACT_ROOT  (required) engine repo root — no machine-absolute paths inside.
#
# Python runs ONLY inside the /tmp workdir with PYTHONDONTWRITEBYTECODE=1;
# the only files written back into the delivery tree are data/*.json.
set -euo pipefail

: "${KG_EXTRACT_ROOT:?KG_EXTRACT_ROOT must point at the kg-extract engine repo}"
export PYTHONDONTWRITEBYTECODE=1

PINNED_HEAD="e52b1f9d17abc16f3898d8e8b2779b6bef5b74db"
ENGINE_HEAD="$(git -C "$KG_EXTRACT_ROOT" rev-parse HEAD)"
if [ "$ENGINE_HEAD" != "$PINNED_HEAD" ]; then
  echo "FATAL: engine HEAD $ENGINE_HEAD != pinned $PINNED_HEAD" >&2
  exit 3
fi
# Tracked files must be untouched; untracked delivery tree is expected.
if [ -n "$(git -C "$KG_EXTRACT_ROOT" status --porcelain --untracked-files=no)" ]; then
  echo "FATAL: engine has modified tracked files; freeze requires read-only engine" >&2
  exit 3
fi

WORK="$(mktemp -d /tmp/ig-kgx-freeze.XXXXXX)"
F="$KG_EXTRACT_ROOT/scripts/fixtures"

# Build the binary exactly as the engine justfile does (idempotent; artifacts
# land only in the gitignored target/ directory).
( cd "$KG_EXTRACT_ROOT" && cargo build --release --features "llms-backend mcp community community-leiden" ) >/dev/null
BIN="$KG_EXTRACT_ROOT/target/release/kg-extract"
BIN_SHA="$(shasum -a 256 "$BIN" | cut -d' ' -f1)"

# Seed schema for the fixed/evolving demo — a deliberately narrow closed
# vocabulary (3 entity types, 4 relations) so the closed-world drop is visible.
cat > "$WORK/seed_schema.json" <<'EOF'
{ "nodes": ["PRODUCT", "ORGANIZATION", "TECHNOLOGY"], "relations": ["developed_by", "uses", "located_in", "includes"], "attributes": [] }
EOF

# Pre-chunked citation demo input: chunk 1 line-only (legacy citation form),
# chunk 2 with page + bbox (rich range form, preserved verbatim).
python3 - "$WORK/chunks_demo.jsonl" <<'PY'
import json, sys
chunks = [
  {"id": "chnk_1",
   "text": "Aurora Portal is a customer operations product developed by Helio Systems. Helio Systems is located in Singapore. Aurora Portal includes the Identity Service, the Billing Worker, the Audit Console, and the Export Job. The API Gateway requires the Identity Service.",
   "source_file": "aurora_note.md",
   "range": {"char_span": {"start": 0, "end": 250}, "line": {"start": 1, "end": 7}}},
  {"id": "chnk_2",
   "text": "The Identity Service uses PostgreSQL for user profiles. The Billing Worker uses Kafka. Aurora Portal is deployed in Kubernetes. The Data Retention Policy governs Audit Logs.",
   "source_file": "aurora_note.md",
   "range": {"char_span": {"start": 250, "end": 500}, "line": {"start": 9, "end": 16},
             "page": {"start": 3, "end": 3}, "bbox": {"x0": 72.0, "y0": 120.5, "x1": 540.0, "y1": 380.0}}},
]
with open(sys.argv[1], "w") as f:
    for c in chunks:
        f.write(json.dumps(c) + "\n")
PY

# ---------------------------------------------------------------- raw runs --
cd "$F"
SIMPLE_MOCK="$(tr '\n' ' ' < medium_product_simple.mock.txt)"
SCHEMA_MOCK="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' medium_product_schema.mock.json)"

"$BIN" --list-presets  > "$WORK/presets.txt"     2>"$WORK/presets.stderr"
"$BIN" --help > "$WORK/cli_help.txt" 2>"$WORK/cli_help.stderr"
"$BIN" describe --json > "$WORK/describe.json"   2>/dev/null
"$BIN" available --json > "$WORK/available.json" 2>/dev/null

"$BIN" -f medium_product_doc.md -e simple -b mock --mock-response "$SIMPLE_MOCK" -o stats      > "$WORK/simple.stats.json" 2>"$WORK/simple.stderr"
"$BIN" -f medium_product_doc.md -e simple -b mock --mock-response "$SIMPLE_MOCK" -o json       > "$WORK/simple.json"       2>>"$WORK/simple.stderr"
"$BIN" -f medium_product_doc.md -e simple -b mock --mock-response "$SIMPLE_MOCK" -o node-link  > "$WORK/simple.nodelink.json" 2>>"$WORK/simple.stderr"
"$BIN" -f medium_product_doc.md -e simple -b mock --mock-response "$SIMPLE_MOCK" -o kg-protocol > "$WORK/simple.protocol.json" 2>>"$WORK/simple.stderr"

"$BIN" -f medium_product_doc.md -e schema-json -b mock --mock-response "$SCHEMA_MOCK" -o stats > "$WORK/sj_open.stats.json" 2>"$WORK/sj_open.stderr"
"$BIN" -f medium_product_doc.md -e schema-json -b mock --mock-response "$SCHEMA_MOCK" --schema-mode fixed    --schema "$WORK/seed_schema.json" -o stats > "$WORK/sj_fixed.stats.json"    2>"$WORK/sj_fixed.stderr"
"$BIN" -f medium_product_doc.md -e schema-json -b mock --mock-response "$SCHEMA_MOCK" --schema-mode fixed    --schema "$WORK/seed_schema.json" -o json  > "$WORK/sj_fixed.json"        2>>"$WORK/sj_fixed.stderr"
"$BIN" -f medium_product_doc.md -e schema-json -b mock --mock-response "$SCHEMA_MOCK" --schema-mode evolving --schema "$WORK/seed_schema.json" -o stats > "$WORK/sj_evolving.stats.json" 2>"$WORK/sj_evolving.stderr"

# ToolCall engine: the same fixture facts scripted as tool invocations.
python3 - "$F/medium_product_schema.mock.json" "$WORK/toolcall.mock.json" <<'PY'
import json, sys
schema = json.load(open(sys.argv[1]))
rounds = []
for name, info in schema.get("entities", {}).items():
    rounds.append({"name": "add_entity", "arguments": {"name": name, "type": info.get("type", "OTHER"), "description": info.get("attributes", {}).get("description", "")}})
for src, pred, tgt in schema.get("relationships", []):
    rounds.append({"name": "add_relation", "arguments": {"source": src, "predicate": pred, "target": tgt}})
rounds.append({"name": "finish", "arguments": {}})
json.dump(rounds, open(sys.argv[2], "w"))
PY
"$BIN" -f medium_product_doc.md -e toolcall -b mock --mock-tool-calls "$WORK/toolcall.mock.json" -o stats > "$WORK/toolcall.stats.json" 2>"$WORK/toolcall.stderr"

# Communities over the SAME simple-engine graph (label propagation; deterministic).
"$BIN" -f medium_product_doc.md -e simple -b mock --mock-response "$SIMPLE_MOCK" -o communities > "$WORK/communities.json" 2>"$WORK/communities.stderr"

# Pre-chunked citation run.
"$BIN" -F chunks -e simple -b mock --mock-response "$SIMPLE_MOCK" -o json < "$WORK/chunks_demo.jsonl" > "$WORK/chunks_demo_out.json" 2>"$WORK/chunks_demo.stderr"

# Provider envelope: one invoke round-trip with a two-entity mock request.
echo '{"text": "OpenAI developed GPT-4.", "backend": "mock", "mock_response": "(entity<|>OpenAI<|>organization<|>An AI research lab that develops language models.<|>)##(entity<|>GPT-4<|>technology<|>A large language model developed by OpenAI.<|>)##(relationship<|>GPT-4<|>OpenAI<|>developed_by<|>GPT-4 was developed by OpenAI.<|>0.9)##"}' \
  | "$BIN" invoke extract.entities_relations --request - > "$WORK/invoke.json" 2>"$WORK/invoke.stderr"

# Read-only repo censuses (no working-tree writes).
git -C "$KG_EXTRACT_ROOT" ls-files > "$WORK/engine_files.txt"
python3 - "$KG_EXTRACT_ROOT" "$WORK" <<'PY'
import json, os, re, sys
root, work = sys.argv[1], sys.argv[2]
idents = []
samples = []
for dirpath, _dirs, files in os.walk(os.path.join(root, "src")):
    for fn in sorted(files):
        if not fn.endswith(".rs"):
            continue
        with open(os.path.join(dirpath, fn)) as f:
            src = f.read()
        for m in re.finditer(r'\b(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)', src):
            idents.append(m.group(1))
        for m in re.finditer(r'\b(?:pub\s+)?(?:enum|struct|trait|const|static|type)\s+([A-Za-z_][A-Za-z0-9_]*)', src):
            idents.append(m.group(1))
        for line in src.splitlines():
            s = line.strip()
            if len(s) >= 25 and not s.startswith("//"):
                samples.append(s)
census = sorted(set(idents))
json.dump({"count": len(census), "identifiers": census,
           "source_line_samples": samples[:400]},
          open(os.path.join(work, "engine_identifiers.raw.json"), "w"), indent=1)
PY

# kg-vocab census: count the authoritative vocab lists at the Cargo.lock pin.
VOCAB_PIN="$(python3 -c '
import tomllib,sys
with open(sys.argv[1],"rb") as f:
    lock=tomllib.load(f)
for p in lock["package"]:
    if p["name"]=="kg-vocab":
        print(p["source"].split("#")[-1]); break
' "$KG_EXTRACT_ROOT/Cargo.lock")"
VOCAB_DIR="$(python3 - "$VOCAB_PIN" <<'PY'
import os, sys
pin = sys.argv[1]
root = os.path.expanduser("~/.cargo/git/checkouts")
for entry in sorted(os.listdir(root)):
    if not entry.startswith("kg-vocab-"):
        continue
    base = os.path.join(root, entry)
    for sub in sorted(os.listdir(base)):
        if pin.startswith(sub) or sub.startswith(pin):
            print(os.path.join(base, sub))
            raise SystemExit(0)
raise SystemExit(1)
PY
)"
if [ -z "$VOCAB_DIR" ]; then
  echo "FATAL: kg-vocab checkout for pin $VOCAB_PIN not found (run cargo build once)" >&2
  exit 3
fi
python3 - "$VOCAB_DIR/vocab/vocab.json" "$WORK/vocab_census.raw.json" "$VOCAB_PIN" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
out = {
    "kg_vocab_pin": sys.argv[3],
    "entity_variants": len(v["entity"]["variants"]),
    "entity_aliases": len(v["entity"]["aliases"]),
    "predicate_variants": len(v["predicate"]["variants"]),
    "predicate_inverses": len(v["predicate"]["inverses"]),
    "predicate_disambiguations": len(v["predicate"]["disambiguations"]),
}
json.dump(out, open(sys.argv[2], "w"), indent=1)
PY

# ------------------------------------------------------------- packaging --
python3 - "$WORK" "$BIN_SHA" "$ENGINE_HEAD" <<'PY'
import json, os, sys
work, bin_sha, head = sys.argv[1], sys.argv[2], sys.argv[3]

def load(name):
    with open(os.path.join(work, name)) as f:
        return json.load(f)

def text(name):
    with open(os.path.join(work, name)) as f:
        return f.read().strip()

out = {}

presets = []
for line in text("presets.txt").splitlines():
    key, rest = line.split(None, 1)
    kind = rest[1:rest.index("]")]
    desc = rest[rest.index("]") + 1:].strip()
    presets.append({"key": key, "kind": kind, "description": desc})
domains, kinds = {}, {}
for p in presets:
    d = p["key"].split("/")[0]
    domains[d] = domains.get(d, 0) + 1
    kinds[p["kind"]] = kinds.get(p["kind"], 0) + 1
out["presets"] = {
    "source": "kg-extract --list-presets",
    "total": len(presets),
    "domains": dict(sorted(domains.items())),
    "kinds": dict(sorted(kinds.items(), key=lambda kv: -kv[1])),
    "items": presets,
}

out["provider"] = {
    "source": "kg-extract describe --json / available --json / invoke extract.entities_relations --request -",
    "manifest": load("describe.json"),
    "available": load("available.json"),
    "invoke": load("invoke.json"),
    "invoke_stderr": text("invoke.stderr"),
}

out["medium_simple"] = {
    "source": "kg-extract -f medium_product_doc.md -e simple -b mock --mock-response <medium_product_simple.mock.txt> -o stats|json|node-link|kg-protocol",
    "stats": load("simple.stats.json"),
    "graph": load("simple.nodelink.json"),
    "protocol_entities": [{"id": e["id"], "label": e["label"], "entity_type": e["entity_type"],
                           "evidence": e["evidence"]}
                          for e in load("simple.protocol.json")["entities"][:3]],
    "stderr": text("simple.stderr"),
}

sj = {
    "open":     {"stats": load("sj_open.stats.json"),     "stderr": text("sj_open.stderr")},
    "fixed":    {"stats": load("sj_fixed.stats.json"),    "stderr": text("sj_fixed.stderr")},
    "evolving": {"stats": load("sj_evolving.stats.json"), "stderr": text("sj_evolving.stderr")},
}
survivor_types = {}
for e in load("sj_fixed.json")["entities"].values():
    survivor_types[e["type"]] = survivor_types.get(e["type"], 0) + 1
sj["fixed"]["survivor_entity_types"] = dict(sorted(survivor_types.items()))
out["medium_schema_modes"] = {
    "source": "kg-extract -f medium_product_doc.md -e schema-json -b mock --mock-response <medium_product_schema.mock.json> --schema-mode {open,fixed,evolving} --schema <seed_schema.json> -o stats|json",
    "seed_schema": load("seed_schema.json"),
    "runs": sj,
}

out["medium_toolcall"] = {
    "source": "kg-extract -f medium_product_doc.md -e toolcall -b mock --mock-tool-calls <scripted add_entity/add_relation/finish rounds> -o stats",
    "stats": load("toolcall.stats.json"),
}

out["medium_communities"] = {
    "source": "kg-extract -f medium_product_doc.md -e simple -b mock --mock-response <medium_product_simple.mock.txt> -o communities",
    "result": load("communities.json"),
}

cd = load("chunks_demo_out.json")
ent0 = next(iter(cd["entities"].values()))
tri0 = cd["triples"][0]
out["citation_demo"] = {
    "source": "kg-extract -F chunks -e simple -b mock --mock-response <medium_product_simple.mock.txt> -o json < chunks_demo.jsonl",
    "input_chunks": [json.loads(l) for l in text("chunks_demo.jsonl").splitlines()],
    "entity_sample": {"label": ent0["label"], "citations": ent0["metadata"]["citations"]},
    "triple_sample": {"subject": tri0["subject"], "citations": tri0["metadata"]["citations"]},
}

files = text("engine_files.txt").splitlines()
out["engine_files"] = {"source": "git -C $KG_EXTRACT_ROOT ls-files", "count": len(files), "files": files}
out["engine_identifiers"] = load("engine_identifiers.raw.json")
out["vocab_census"] = load("vocab_census.raw.json")
out["cli_surface"] = {
    "source": "kg-extract --help (user-facing flag enumeration of the frozen binary)",
    "help_text": text("cli_help.txt"),
}

out["provenance"] = {
    "engine_commit": head,
    "engine_binary": "target/release/kg-extract",
    "engine_binary_sha256": bin_sha,
    "build_command": 'cargo build --release --features "llms-backend mcp community community-leiden"',
    "backend": "mock (deterministic, offline)",
    "frozen_at": "2026-09-04",
}

for name, payload in out.items():
    with open(os.path.join(work, f"pkg_{name}.json"), "w") as f:
        json.dump(payload, f, ensure_ascii=False, indent=1, sort_keys=True)
print("packaged:", ", ".join(sorted(out)))
PY

# Fixture input shas (read-only, hashed from the engine tree).
FIX_SHAS="$(cd "$F" && shasum -a 256 medium_product_doc.md medium_product_simple.mock.txt medium_product_schema.mock.json | awk '{print "\"" $2 "\": \"" $1 "\","}')"

python3 - "$WORK" "$FIX_SHAS" <<'PY'
import json, os, sys
work = sys.argv[1]
fix = json.loads("{" + sys.argv[2].rstrip(",") + "}")
path = os.path.join(work, "pkg_provenance.json")
with open(path) as f:
    p = json.load(f)
p["fixture_inputs_sha256"] = fix
with open(path, "w") as f:
    json.dump(p, f, ensure_ascii=False, indent=1, sort_keys=True)
print("provenance done")
PY

DEST="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/data"
mkdir -p "$DEST"
for f in "$WORK"/pkg_*.json; do
  name="${f##*/pkg_}"
  cp "$f" "$DEST/$name"
done
echo "frozen -> $DEST"
