# kg-extract Rust 项目

set shell := ["bash", "-euo", "pipefail", "-c"]

arch_suffix := if arch() == "aarch64" { "arm64" } else { "x86" }
install_bin := home_directory() / "sync" / ("bin_" + arch_suffix)
target_dir := env("CARGO_TARGET_DIR", justfile_directory() / ".." / "target")

# 构建（含 llms 后端 + mcp server）
build:
    cargo build --release --features "llms-backend mcp"

# 运行测试（含 mcp 工具与并发测试）
test:
    cargo test --features mcp

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
    cargo clippy --all-targets --features "llms-backend mcp"

# 安装到 ~/sync/bin_<arch>/（含 kg-extract 与 kg-extract-mcp）
install: build
    mkdir -p {{ install_bin }}
    cp {{ target_dir }}/release/kg-extract {{ install_bin }}/kg-extract
    cp {{ target_dir }}/release/kg-extract-mcp {{ install_bin }}/kg-extract-mcp
