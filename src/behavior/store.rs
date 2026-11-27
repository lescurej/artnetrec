use super::types::BehaviorCatalog;
use std::path::{Path, PathBuf};

pub fn sidecar_path(show_path: &Path) -> PathBuf {
    let stem = show_path.with_extension("");
    let mut path = stem;
    path.set_extension("behaviors.json");
    path
}

pub fn load(path: &Path) -> Result<BehaviorCatalog, String> {
    if !path.exists() {
        return Ok(BehaviorCatalog::new());
    }
    let bytes =
        std::fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let catalog: BehaviorCatalog =
        serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    if catalog.version != BehaviorCatalog::VERSION {
        return Err(format!(
            "unsupported behaviors version {} in {}",
            catalog.version,
            path.display()
        ));
    }
    Ok(catalog)
}

pub fn save(path: &Path, catalog: &BehaviorCatalog) -> Result<(), String> {
    let text = serde_json::to_string_pretty(catalog)
        .map_err(|err| format!("serialize behaviors: {err}"))?;
    std::fs::write(path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))
}
