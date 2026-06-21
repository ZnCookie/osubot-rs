use std::path::Path;

pub fn resolve_wasm_path(dir: &str, plugin_path: &str) -> Result<String, String> {
    let raw = if Path::new(plugin_path).is_absolute() {
        plugin_path.to_string()
    } else {
        format!("{dir}/{plugin_path}")
    };
    for component in Path::new(&raw).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(format!("plugin path contains '..': {plugin_path}"));
        }
    }
    let path = Path::new(&raw);
    if path.exists() {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize path: {e}"))?;
        return Ok(canonical.to_string_lossy().to_string());
    }
    Ok(raw)
}
