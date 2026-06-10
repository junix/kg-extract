#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATES_ROOT="$(cd "$ROOT/.." && pwd)"
DOC="${1:-"$ROOT/README.md"}"

if [[ ! -f "$DOC" ]]; then
  echo "input markdown not found: $DOC" >&2
  exit 2
fi

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

TMP_ROOT="$(mktemp -d /tmp/kg-extract-lbug-e2e.XXXXXX)"
DB="$TMP_ROOT/db"

MOCK="(entity<|>OpenAI<|>organization<|>An AI research lab that develops language models.<|>)##(entity<|>GPT-4<|>technology<|>A large language model developed by OpenAI.<|>)##(relationship<|>GPT-4<|>OpenAI<|>developed_by<|>GPT-4 was developed by OpenAI.<|>0.9)##"

"${CHONKIE_CMD[@]}" --jsonl --chunker recursive --chunk-size 512 --limit 1 --file "$DOC" \
  | "${KG_EXTRACT_CMD[@]}" -F chunks -e simple -b mock --mock-response "$MOCK" -o ladybug-import \
  | "${LBUG_CMD[@]}" "$DB" import - --create-tables

"${LBUG_CMD[@]}" "$DB" query \
  "MATCH (a:KgEntity)-[r:DEVELOPED_BY]->(b:KgEntity) RETURN a.label, r.predicate, b.label;"

echo "DB=$DB"
