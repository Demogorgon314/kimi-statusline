//! agent-core-v2/src/app/plugin/{store,manager,manifest}.ts (21406fb4c).
//! InstalledRecord does not persist runtime `state`. recordFrom derives it
//! from manifest errors; malformed optional hooks/MCP/commands are warnings.
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

pub fn record_active(record: &Value) -> bool {
    if record.get("enabled").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    // Also accept a runtime summary if a future upstream persists one.
    if record
        .get("state")
        .is_some_and(|s| s.as_str() != Some("ok"))
    {
        return false;
    }
    let Some(root) = record.get("root").and_then(Value::as_str).map(Path::new) else {
        return false;
    };
    let root_manifest = root.join("kimi.plugin.json");
    let manifest = if root_manifest.is_file() {
        root_manifest
    } else {
        root.join(".kimi-plugin/plugin.json")
    };
    let Some(raw) = crate::session::read_json(&manifest) else {
        return false;
    };
    let Some(name) = raw.get("name").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    let valid_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if name.is_empty()
        || name.len() > 64
        || !valid_char(name.as_bytes()[0])
        || !name
            .bytes()
            .all(|b| valid_char(b) || b == b'_' || b == b'-')
    {
        return false;
    }
    ["skills", "agents"]
        .into_iter()
        .all(|field| valid_directories(root, raw.get(field)))
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let mut result = PathBuf::new();
        for part in path.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir => {
                    result.pop();
                }
                _ => result.push(part),
            }
        }
        result
    })
}

fn valid_directories(root: &Path, field: Option<&Value>) -> bool {
    let entries = match field {
        None => return true,
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => {
            let Some(entries) = a.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                return false;
            };
            entries
        }
        _ => return false,
    };
    let root = resolved(root);
    // Missing directories are warnings in upstream, but escaping the root
    // or omitting the ./ prefix makes the entire plugin state an error.
    entries
        .into_iter()
        .all(|entry| entry.starts_with("./") && resolved(&root.join(entry)).starts_with(&root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_records_derive_state_from_manifest_errors_only() {
        let root = std::env::temp_dir().join(format!("ksl-plugin-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let record = serde_json::json!({"enabled":true,"root":root});
        assert!(!record_active(&record));
        let path = root.join("kimi.plugin.json");
        for (manifest, active) in [
            (serde_json::json!({"name":"kimi-statusline"}), true),
            (
                serde_json::json!({"name":"kimi-statusline","skills":"./missing","hooks":42}),
                true,
            ),
            (
                serde_json::json!({"name":"kimi-statusline","agents":"./../outside"}),
                false,
            ),
            (
                serde_json::json!({"name":"kimi-statusline","skills":[42]}),
                false,
            ),
            (serde_json::json!({"name":"Bad Name"}), false),
        ] {
            std::fs::write(&path, manifest.to_string()).unwrap();
            assert_eq!(record_active(&record), active, "{manifest}");
        }
        std::fs::write(&path, "{").unwrap();
        assert!(!record_active(&record));
        std::fs::write(&path, r#"{"name":"kimi-statusline"}"#).unwrap();
        assert!(!record_active(
            &serde_json::json!({"enabled":true,"root":root,"state":"error"})
        ));
        assert!(!record_active(
            &serde_json::json!({"enabled":false,"root":root})
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
