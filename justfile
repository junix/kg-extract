# kg-extract Rust 项目

set shell := ["bash", "-euo", "pipefail", "-c"]

arch_suffix := if arch() == "aarch64" { "arm64" } else { "x86" }
install_bin := home_directory() / "sync" / ("bin_" + arch_suffix)
target_dir := env("CARGO_TARGET_DIR", parent_directory(justfile_directory()) / "target")

# 构建（含 llms 后端 + mcp server）
build:
    cargo build --release --features "llms-backend mcp"

# 运行测试（含 mcp 工具与并发测试）
test:
    cargo test --features mcp

# Lint
lint:
    cargo clippy --all-targets --features "llms-backend mcp"

# 安装到 ~/sync/bin_<arch>/（含 kg-extract 与 kg-extract-mcp）
install: build
    mkdir -p {{ install_bin }}
    cp {{ target_dir }}/release/kg-extract {{ install_bin }}/kg-extract
    cp {{ target_dir }}/release/kg-extract-mcp {{ install_bin }}/kg-extract-mcp
