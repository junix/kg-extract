#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATES_ROOT="$(cd "$ROOT/.." && pwd)"
DOC="${1:-"$ROOT/README.md"}"

platform="$(uname -s):$(uname -m)"
case "$platform" in
  Darwin:arm64|Darwin:aarch64) native_dir=macos-arm64-bin ;;
  Darwin:x86_64|Darwin:amd64)  native_dir=macos-x86-bin ;;
  Linux:arm64|Linux:aarch64)    native_dir=linux-arm64-bin ;;
  Linux:x86_64|Linux:amd64)     native_dir=linux-x86-bin ;;
  *) native_dir= ;;
esac
NATIVE_BIN_DIR="${SYNC_BIN_DIR:-${native_dir:+$HOME/sync/$native_dir}}"

if [[ ! -f "$DOC" ]]; then
  echo "input markdown not found: $DOC" >&2
  exit 2
fi

if [[ -x "${CHONKIE:-}" ]]; then
  CHONKIE_CMD=("$CHONKIE")
elif command -v chonkie >/dev/null 2>&1; then
  CHONKIE_CMD=("$(command -v chonkie)")
elif [[ -n "$NATIVE_BIN_DIR" && -x "$NATIVE_BIN_DIR/chonkie" ]]; then
  CHONKIE_CMD=("$NATIVE_BIN_DIR/chonkie")
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
