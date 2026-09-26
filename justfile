# kg-extract Rust 项目

set shell := ["bash", "-euo", "pipefail", "-c"]

os_suffix := if os() == "macos" { "macos" } else { "linux" }
arch_suffix := if arch() == "aarch64" { "arm64" } else { "x86" }
install_bin := env("SYNC_BIN_DIR", home_directory() / "sync" / (os_suffix + "-" + arch_suffix + "-bin"))
target_dir := `cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'`
# 构建印章（ADR-1168）：git 短 sha + 脏标记，经 PM_BUILD_SHA 嵌入 --version。
stamp := `git rev-parse --short HEAD` + `(git diff --quiet && git diff --cached --quiet) >/dev/null 2>&1 || printf .dirty`

# 构建（含 llms 后端 + mcp server + 社区检测）
build:
    PM_BUILD_SHA=g{{stamp}} cargo build --release --features "llms-backend mcp community community-leiden"

# 运行测试。feature 集必须与 build/lint 一致：否则 community.rs（ADR-987 Step 5
# 适配器）、provider.rs 与 bin 测试全部不参与编译，静默跳过 25 个测试。
test:
    cargo test --features "llms-backend mcp community community-leiden"

# Markdown -> chonkie -> kg-extract -> graphdb-ladybug -> query smoke
ladybug-smoke:
    bash scripts/ladybug-e2e-smoke.sh

# Markdown facts -> extraction variants -> graphdb-ladybug -> query coverage
ladybug-eval fixture="ladybug_eval":
    FIXTURE={{ fixture }} bash scripts/ladybug-e2e-eval.sh

# Same eval, plus one live schema-json extraction through an agent backend
ladybug-eval-live agent="minimaxcc" fixture="ladybug_eval":
    FIXTURE={{ fixture }} LIVE_AGENT={{ agent }} bash scripts/ladybug-e2e-eval.sh

# Same eval, plus one live agentic extraction through an agent backend
ladybug-eval-agentic agent="minimaxcc" fixture="ladybug_eval":
    FIXTURE={{ fixture }} LIVE_AGENTIC={{ agent }} bash scripts/ladybug-e2e-eval.sh

# Same eval, then ask an agent to judge the query evidence
ladybug-eval-verify agent="minimaxcc" fixture="ladybug_eval":
    FIXTURE={{ fixture }} VERIFY_AGENT={{ agent }} bash scripts/ladybug-e2e-eval.sh

# Live extraction plus agent verification over the query evidence
ladybug-eval-live-verify agent="minimaxcc" fixture="ladybug_eval":
    FIXTURE={{ fixture }} LIVE_AGENT={{ agent }} VERIFY_AGENT={{ agent }} bash scripts/ladybug-e2e-eval.sh

# Deterministic variants + live schema-json + live agentic + agent verification
ladybug-eval-full-verify agent="minimaxcc" fixture="ladybug_eval":
    FIXTURE={{ fixture }} LIVE_AGENT={{ agent }} LIVE_AGENTIC={{ agent }} VERIFY_AGENT={{ agent }} bash scripts/ladybug-e2e-eval.sh

# Lint
lint:
    cargo clippy --all-targets --features "llms-backend mcp community community-leiden"

# 安装到 ~/sync/<os>-<arch>-bin/（含 kg-extract 与 kg-extract-mcp）
install: build
    mkdir -p {{ install_bin }}
    @set -eu; dest="{{ install_bin }}/kg-extract"; mkdir -p "$(dirname "$dest")"; tmp="$(mktemp "{{ install_bin }}/.kg-extract.XXXXXX")"; trap 'rm -f "$tmp"' EXIT; cp "{{ target_dir }}/release/kg-extract" "$tmp"; chmod 755 "$tmp"; if [ "$(uname -s)" = "Darwin" ]; then xattr -c "$tmp" 2>/dev/null || true; codesign --force --sign - "$tmp"; fi; mv -f "$tmp" "$dest"
    @set -eu; dest="{{ install_bin }}/kg-extract-mcp"; mkdir -p "$(dirname "$dest")"; tmp="$(mktemp "{{ install_bin }}/.kg-extract-mcp.XXXXXX")"; trap 'rm -f "$tmp"' EXIT; cp "{{ target_dir }}/release/kg-extract-mcp" "$tmp"; chmod 755 "$tmp"; if [ "$(uname -s)" = "Darwin" ]; then xattr -c "$tmp" 2>/dev/null || true; codesign --force --sign - "$tmp"; fi; mv -f "$tmp" "$dest"
