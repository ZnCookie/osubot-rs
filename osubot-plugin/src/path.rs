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
        let dir_canonical = Path::new(dir)
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize dir: {e}"))?;
        if !canonical.starts_with(&dir_canonical) {
            return Err(format!(
                "plugin path escapes plugins directory: {plugin_path}"
            ));
        }
        return Ok(canonical.to_string_lossy().to_string());
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn rejects_symlink_escaping_plugins_dir() {
        let tmp = std::env::temp_dir().join("osubot_path_test");
        let plugins_dir = tmp.join("plugins");
        let outside_dir = tmp.join("outside");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("evil.wasm"), b"evil").unwrap();
        symlink(outside_dir.join("evil.wasm"), plugins_dir.join("link.wasm")).unwrap();

        let result = resolve_wasm_path(plugins_dir.to_str().unwrap(), "link.wasm");
        assert!(
            result.is_err(),
            "symlink escaping plugins dir should be rejected"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn accepts_normal_path_within_dir() {
        let tmp = std::env::temp_dir().join("osubot_path_test_ok");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("good.wasm"), b"good").unwrap();

        let result = resolve_wasm_path(tmp.to_str().unwrap(), "good.wasm");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("good.wasm"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rejects_parent_dir_component() {
        let result = resolve_wasm_path("/tmp/plugins", "../etc/passwd");
        assert!(result.is_err());
    }
}
