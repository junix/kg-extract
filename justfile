# kg-extract Rust 项目

set shell := ["bash", "-euo", "pipefail", "-c"]

arch_suffix := if arch() == "aarch64" { "arm64" } else { "x86" }
install_bin := home_directory() / "sync" / ("bin_" + arch_suffix)
target_dir := env("CARGO_TARGET_DIR", parent_directory(justfile_directory()) / "target")

# 构建（含 llms 后端）
build:
    cargo build --release --features llms-backend

# 运行测试
test:
    cargo test

# Lint
lint:
    cargo clippy --all-targets --features llms-backend

# 安装到 ~/sync/bin_<arch>/
install: build
    mkdir -p {{ install_bin }}
    cp {{ target_dir }}/release/kg-extract {{ install_bin }}/kg-extract
