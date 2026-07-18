fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let display = match (!home.is_empty()).then(|| manifest.strip_prefix(home.as_str())).flatten() {
        Some(rel) => format!("~{}", rel),
        None => manifest,
    };
    println!("cargo:rustc-env=PROJECT_SOURCE_PATH={}", display);
}
