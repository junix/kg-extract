//! The preset gallery — the bundled extraction templates under `presets/`,
//! embedded into the binary at compile time via [`include_dir`].
//!
//! Templates are keyed `{domain}/{name}` where `domain` is the top-level preset
//! folder and `name` is the template's `name:` field (e.g. `presets/general/
//! base_graph.yaml` with `name: graph` → key `general/graph`). A bare key with no
//! `/` is resolved under `general/`, matching the Python `Gallery.get`.

use super::model::TemplateCfg;
use include_dir::{include_dir, Dir};
use std::sync::OnceLock;

/// The `presets/` directory tree, baked into the binary.
static PRESETS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/presets");

/// The bundled presets, parsed from the embedded YAML exactly once on first
/// access and cached for the lifetime of the process. The raw bytes are baked
/// in by `include_dir!`, so parsing is the only cost — paying it per `get()` /
/// `list()` call (37 YAML deserializations each time) was pure waste.
static GALLERY: OnceLock<Vec<Preset>> = OnceLock::new();

/// One entry in the gallery: its `{domain}/{name}` key and the parsed template.
#[derive(Clone)]
pub struct Preset {
    pub key: String,
    pub template: TemplateCfg,
}

/// Walk every embedded `*.yaml`, parse it, and return the keyed presets.
///
/// Malformed presets are skipped rather than aborting the whole gallery (the
/// Python gallery prints and continues); the optional `on_error` hook surfaces
/// the file + error if a caller wants to report them.
fn load_all(mut on_error: impl FnMut(&str, anyhow::Error)) -> Vec<Preset> {
    let mut out = Vec::new();
    collect(&PRESETS, &mut out, &mut on_error);
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn collect(dir: &Dir<'_>, out: &mut Vec<Preset>, on_error: &mut impl FnMut(&str, anyhow::Error)) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(d) => collect(d, out, on_error),
            include_dir::DirEntry::File(f) => {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                // domain = the first path component under presets/.
                let domain = path
                    .components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str());
                let Some(domain) = domain else { continue };
                let Some(text) = f.contents_utf8() else {
                    continue;
                };
                match TemplateCfg::from_yaml_str(text) {
                    Ok(template) => {
                        let key = format!("{domain}/{}", template.name);
                        out.push(Preset { key, template });
                    }
                    Err(e) => on_error(&path.to_string_lossy(), e),
                }
            }
        }
    }
}

/// All bundled presets, sorted by key (parsed once and cached). Silently skips
/// any that fail to parse.
pub fn list() -> &'static [Preset] {
    GALLERY.get_or_init(|| load_all(|_, _| {}))
}

/// Look up a bundled preset by key. A key without a `/` is resolved under
/// `general/` (so `graph` → `general/graph`).
pub fn get(name: &str) -> Option<TemplateCfg> {
    let key = if name.contains('/') {
        name.to_string()
    } else {
        format!("general/{name}")
    };
    list()
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.template.clone())
}

/// The bundled preset keys (sorted), e.g. for `--list-presets`.
pub fn keys() -> Vec<String> {
    list().iter().map(|p| p.key.clone()).collect()
}
