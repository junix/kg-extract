#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATES_ROOT="$(cd "$ROOT/.." && pwd)"
FIXTURES="$ROOT/scripts/fixtures"
FIXTURE="${FIXTURE:-ladybug_eval}"
DOC="${DOC:-"$FIXTURES/${FIXTURE}_doc.md"}"
EXPECTED="${EXPECTED:-"$FIXTURES/${FIXTURE}_expected.json"}"
SIMPLE_MOCK_FILE="${SIMPLE_MOCK_FILE:-"$FIXTURES/${FIXTURE}_simple.mock.txt"}"
SCHEMA_MOCK_FILE="${SCHEMA_MOCK_FILE:-"$FIXTURES/${FIXTURE}_schema.mock.json"}"

for required in "$DOC" "$EXPECTED" "$SIMPLE_MOCK_FILE" "$SCHEMA_MOCK_FILE"; do
  if [[ ! -f "$required" ]]; then
    echo "required fixture file not found: $required" >&2
    exit 2
  fi
done

SIMPLE_MOCK="$(tr '\n' ' ' < "$SIMPLE_MOCK_FILE")"
SCHEMA_MOCK="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' "$SCHEMA_MOCK_FILE")"

if [[ -x "${CHONKIE:-}" ]]; then
  CHONKIE_CMD=("$CHONKIE")
elif command -v chonkie >/dev/null 2>&1; then
  CHONKIE_CMD=("$(command -v chonkie)")
elif [[ -x "$HOME/sync/bin_arm64/chonkie" ]]; then
  CHONKIE_CMD=("$HOME/sync/bin_arm64/chonkie")
else
  CHONKIE_CMD=(cargo run --quiet --manifest-path "$CRATES_ROOT/chonkie/Cargo.toml" --)
fi

KG_EXTRACT_CMD=(cargo run --quiet --manifest-path "$ROOT/Cargo.toml" --bin kg-extract --)
LBUG_CMD=(cargo run --quiet --manifest-path "$CRATES_ROOT/graphdb-ladybug/Cargo.toml" --)

TMP_ROOT="$(mktemp -d /tmp/kg-extract-lbug-eval.XXXXXX)"
REPORTS=()
FAILURES=0
TOOLCALL_MOCK_FILE="$TMP_ROOT/toolcall.mock.json"

python3 - "$SCHEMA_MOCK_FILE" "$TOOLCALL_MOCK_FILE" <<'PY'
import json
import sys

schema = json.load(open(sys.argv[1]))
calls = []
for name, info in schema.get("entities", {}).items():
    attrs = info.get("attributes") or {}
    args = {
        "name": name,
        "type": info.get("type", "OTHER"),
        "description": attrs.get("description"),
        "attributes": attrs,
    }
    calls.append({"name": "add_entity", "arguments": args})
for source, predicate, target in schema.get("relationships", []):
    calls.append({
        "name": "add_relation",
        "arguments": {
            "source": source,
            "predicate": predicate,
            "target": target,
            "strength": 0.9,
        },
    })
calls.append({"name": "finish", "arguments": {}})
json.dump(calls, open(sys.argv[2], "w"), ensure_ascii=False, indent=2)
PY

run_variant() {
  local name="$1"
  local engine="$2"
  local mode="$3"
  local mock="$4"
  local db="$TMP_ROOT/$name/db"
  local import_json="$TMP_ROOT/$name/import.json"
  mkdir -p "$TMP_ROOT/$name"

  if [[ "$mode" == "chunked" ]]; then
    "${CHONKIE_CMD[@]}" --jsonl --chunker recursive --chunk-size 48 --file "$DOC" \
      | "${KG_EXTRACT_CMD[@]}" -F chunks -e "$engine" -b mock --mock-response "$mock" -o ladybug-import \
      > "$import_json"
  else
    "${KG_EXTRACT_CMD[@]}" -f "$DOC" -e "$engine" -b mock --mock-response "$mock" -o ladybug-import \
      > "$import_json"
  fi

  validate_import "$name" "$db" "$import_json"
}

run_toolcall_variant() {
  local name="$1"
  local mode="$2"
  local db="$TMP_ROOT/$name/db"
  local import_json="$TMP_ROOT/$name/import.json"
  mkdir -p "$TMP_ROOT/$name"

  if [[ "$mode" == "chunked" ]]; then
    "${CHONKIE_CMD[@]}" --jsonl --chunker recursive --chunk-size 48 --file "$DOC" \
      | "${KG_EXTRACT_CMD[@]}" -F chunks -e toolcall -b mock --mock-tool-calls "$TOOLCALL_MOCK_FILE" -o ladybug-import \
      > "$import_json"
  else
    "${KG_EXTRACT_CMD[@]}" -f "$DOC" -e toolcall -b mock --mock-tool-calls "$TOOLCALL_MOCK_FILE" -o ladybug-import \
      > "$import_json"
  fi

  validate_import "$name" "$db" "$import_json"
}

run_live_agent_variant() {
  local agent="$1"
  local name="agent-schema-json-$agent"
  local db="$TMP_ROOT/$name/db"
  local import_json="$TMP_ROOT/$name/import.json"
  mkdir -p "$TMP_ROOT/$name"

  "${KG_EXTRACT_CMD[@]}" -f "$DOC" -e schema-json -b agent --agent "$agent" -o ladybug-import \
    > "$import_json"

  validate_import "$name" "$db" "$import_json"
}

run_live_agentic_variant() {
  local agent="$1"
  local name="agentic-$agent"
  local db="$TMP_ROOT/$name/db"
  local import_json="$TMP_ROOT/$name/import.json"
  local relation_gleaning="${AGENTIC_RELATION_GLEANING:-1}"
  mkdir -p "$TMP_ROOT/$name"

  "${KG_EXTRACT_CMD[@]}" -f "$DOC" -e agentic --agent "$agent" --relation-gleaning "$relation_gleaning" -o ladybug-import \
    > "$import_json"

  validate_import "$name" "$db" "$import_json"
}

validate_import() {
  local name="$1"
  local db="$2"
  local import_json="$3"
  local query_dir="$TMP_ROOT/$name/queries"
  local report_json="$TMP_ROOT/$name/report.json"
  mkdir -p "$query_dir"

  "${LBUG_CMD[@]}" "$db" import "$import_json" --create-tables >/dev/null

  python3 - "$EXPECTED" "$query_dir" <<'PY'
import json
import pathlib
import sys

expected = json.load(open(sys.argv[1]))
out_dir = pathlib.Path(sys.argv[2])
for i, fact in enumerate(expected):
    rel = fact["predicate"]
    query = f"MATCH (a:KgEntity)-[r:{rel}]->(b:KgEntity) RETURN a.label, r.predicate, b.label;"
    (out_dir / f"{i:02d}-{rel}.cypher").write_text(query)
PY

  local passed=0
  local total=0
  while IFS= read -r query_file; do
    local idx
    idx="$(basename "$query_file" | cut -d- -f1)"
    local result="$query_dir/$idx.json"
    local query_error="$query_dir/$idx.err"
    if ! "${LBUG_CMD[@]}" "$db" query "$(cat "$query_file")" > "$result" 2> "$query_error"; then
      python3 - "$query_error" "$result" <<'PY'
import json
import sys

error = open(sys.argv[1]).read().strip()
json.dump({"columns": [], "rows": [], "error": error}, open(sys.argv[2], "w"), ensure_ascii=False, indent=2)
PY
    fi
    if python3 - "$EXPECTED" "$idx" "$result" <<'PY'
import json
import re
import sys

expected = json.load(open(sys.argv[1]))[int(sys.argv[2])]
result = json.load(open(sys.argv[3]))

def norm(s):
    return re.sub(r"[^a-z0-9]+", "", str(s).lower())

want = (norm(expected["subject"]), expected["predicate"], norm(expected["object"]))
for row in result.get("rows", []):
    got = (norm(row.get("a.label", "")), row.get("r.predicate"), norm(row.get("b.label", "")))
    if got == want:
        sys.exit(0)
sys.exit(1)
PY
    then
      passed=$((passed + 1))
    fi
    total=$((total + 1))
  done < <(find "$query_dir" -name '*.cypher' | sort)

  python3 - "$EXPECTED" "$query_dir" "$name" "$db" "$import_json" "$report_json" <<'PY'
import json
import pathlib
import re
import sys

expected = json.load(open(sys.argv[1]))
query_dir = pathlib.Path(sys.argv[2])
variant = sys.argv[3]
db = sys.argv[4]
import_json = sys.argv[5]
report_path = sys.argv[6]

def norm(s):
    return re.sub(r"[^a-z0-9]+", "", str(s).lower())

facts = []
for i, fact in enumerate(expected):
    result_path = query_dir / f"{i:02d}.json"
    result = json.load(open(result_path))
    want = (norm(fact["subject"]), fact["predicate"], norm(fact["object"]))
    matched = False
    for row in result.get("rows", []):
        got = (norm(row.get("a.label", "")), row.get("r.predicate"), norm(row.get("b.label", "")))
        if got == want:
            matched = True
            break
    facts.append({
        "expected": fact,
        "matched": matched,
        "query_rows": result.get("rows", []),
        "query_error": result.get("error"),
    })

import_doc = json.load(open(import_json))
node_labels = {
    node.get("id"): node.get("label", "")
    for node in import_doc.get("nodes", [])
}
expected_keys = {
    (norm(f["subject"]), f["predicate"], norm(f["object"]))
    for f in expected
}
extracted = []
extra = []
for rel in import_doc.get("relationships", []):
    item = {
        "subject": node_labels.get(rel.get("_from"), rel.get("_from")),
        "predicate": rel.get("predicate") or rel.get("_type"),
        "object": node_labels.get(rel.get("_to"), rel.get("_to")),
        "table": rel.get("_type"),
        "label": rel.get("label"),
    }
    key = (norm(item["subject"]), item["predicate"], norm(item["object"]))
    item["matches_expected"] = key in expected_keys
    extracted.append(item)
    if key not in expected_keys:
        extra.append(item)

facts_found = sum(1 for f in facts if f["matched"])
facts_total = len(facts)
extracted_total = len(extracted)
report = {
    "variant": variant,
    "db": db,
    "import_json": import_json,
    "facts_found": facts_found,
    "facts_total": facts_total,
    "recall": (facts_found / facts_total) if facts_total else None,
    "extracted_relationships_total": extracted_total,
    "extra_relationships_total": len(extra),
    "facts": facts,
    "extracted_relationships": extracted,
    "extra_relationships": extra,
}
json.dump(report, open(report_path, "w"), ensure_ascii=False, indent=2)
PY

  REPORTS+=("$report_json")
  local extra_count
  extra_count="$(python3 - "$report_json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["extra_relationships_total"])
PY
)"
  printf '%s\t%s/%s\textra=%s\t%s\n' "$name" "$passed" "$total" "$extra_count" "$db"
  [[ "$passed" == "$total" ]]
}

run_agent_verifier() {
  local agent="$1"
  local context_json="$TMP_ROOT/agent-verifier-context.json"
  local verdict_txt="$TMP_ROOT/agent-verifier-verdict.txt"

  python3 - "$context_json" "${REPORTS[@]}" <<'PY'
import json
import sys

out = sys.argv[1]
reports = [json.load(open(p)) for p in sys.argv[2:]]
context = {
    "task": "Verify whether each expected Markdown fact was queryable from LadybugDB, whether subject/predicate/object direction match, and whether extra extracted relationships materially reduce accuracy.",
    "verdict_contract": {
        "overall_pass": "boolean",
        "variants": "array of {variant, facts_found, facts_total, extra_relationships_total, pass, notes}",
    },
    "reports": reports,
}
json.dump(context, open(out, "w"), ensure_ascii=False, indent=2)
PY

  local prompt
  prompt="$(cat <<EOF
You are verifying an end-to-end knowledge-graph extraction evaluation.

Read this JSON evidence. Each variant contains expected facts, actual LadybugDB query rows, and extracted relationships not present in the expected set.
Judge only from the evidence, not from prior knowledge.
Return concise JSON with:
{
  "overall_pass": true|false,
  "variants": [
    {"variant": "...", "facts_found": N, "facts_total": N, "extra_relationships_total": N, "pass": true|false, "notes": "..."}
  ]
}

Evidence:
$(cat "$context_json")
EOF
)"

  "$agent" -p "$prompt" --permission-mode dontAsk --no-session-persistence > "$verdict_txt"
  echo "agent_verifier=$agent"
  echo "agent_verdict=$verdict_txt"
}

echo -e "variant\tfacts_found\textra_relationships\tdb"
echo "fixture=$FIXTURE"

run_variant simple-plain simple plain "$SIMPLE_MOCK" || FAILURES=$((FAILURES + 1))
run_variant simple-chunked simple chunked "$SIMPLE_MOCK" || FAILURES=$((FAILURES + 1))
run_variant schema-json-plain schema-json plain "$SCHEMA_MOCK" || FAILURES=$((FAILURES + 1))
run_variant schema-json-chunked schema-json chunked "$SCHEMA_MOCK" || FAILURES=$((FAILURES + 1))
run_toolcall_variant toolcall-plain plain || FAILURES=$((FAILURES + 1))
run_toolcall_variant toolcall-chunked chunked || FAILURES=$((FAILURES + 1))

if [[ -n "${LIVE_AGENT:-}" ]]; then
  run_live_agent_variant "$LIVE_AGENT" || FAILURES=$((FAILURES + 1))
fi

if [[ -n "${LIVE_AGENTIC:-}" ]]; then
  run_live_agentic_variant "$LIVE_AGENTIC" || FAILURES=$((FAILURES + 1))
fi

if [[ -n "${VERIFY_AGENT:-}" ]]; then
  run_agent_verifier "$VERIFY_AGENT"
fi

echo "workspace=$TMP_ROOT"
if [[ "$FAILURES" -gt 0 ]]; then
  exit 1
fi
