//! `dlc_load.json`, the file the game reads to learn which mods to load: `{"enabled_mods": ["mod/xxx.mod", …], "disabled_dlcs": […]}`.
//! We write the mods and leave everything else (the disabled DLCs, any other key) as it was.

use crate::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// The enabled mods, in load order, and the whole document.
pub fn read(path: &Path) -> Result<(Vec<String>, Value)> {
    if !path.is_file() {
        return Ok((Vec::new(), json!({ "enabled_mods": [], "disabled_dlcs": [] })));
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{} is not valid JSON", path.display()))?;
    let mods = doc.get("enabled_mods").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok((mods, doc))
}

/// The `disabled_dlcs` of a document.
pub fn disabled_dlcs_of(doc: &Value) -> Vec<String> {
    doc.get("disabled_dlcs").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Writes `enabled_mods` (and keeps the rest). The first time it changes anything, the original is kept as `dlc_load.json.stl_backup`.
/// Returns whether the file changed.
pub fn write(path: &Path, enabled: &[String], disabled_dlcs: Option<&[String]>) -> Result<bool> {
    let (current, mut doc) = read(path)?;
    let current_dlcs = disabled_dlcs_of(&doc);
    if current == enabled && path.is_file() && disabled_dlcs.map_or(true, |d| d == &current_dlcs[..]) {
        return Ok(false);
    }
    if !doc.is_object() {
        doc = json!({});
    }
    doc["enabled_mods"] = json!(enabled);
    match disabled_dlcs {
        Some(d) => doc["disabled_dlcs"] = json!(d),
        None if doc.get("disabled_dlcs").is_none() => doc["disabled_dlcs"] = json!([]),
        None => {}
    }
    if path.is_file() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".stl_backup");
        let backup = std::path::PathBuf::from(backup);
        if !backup.exists() {
            std::fs::copy(path, &backup).with_context(|| format!("cannot back up {}", path.display()))?;
        }
    }
    std::fs::write(path, serde_json::to_string(&doc)?).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_mods_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("stl-test-dlc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("dlc_load.json");
        std::fs::write(&p, r#"{"enabled_mods":["mod/old.mod"],"disabled_dlcs":["dlc/dlc001.dlc"],"extra":1}"#).unwrap();
        assert!(write(&p, &["mod/a.mod".to_string(), "mod/b.mod".to_string()], None).unwrap());
        let (mods, doc) = read(&p).unwrap();
        assert_eq!(mods, vec!["mod/a.mod", "mod/b.mod"]);
        assert_eq!(doc["disabled_dlcs"][0], "dlc/dlc001.dlc");
        assert_eq!(doc["extra"], 1);
        assert!(!write(&p, &["mod/a.mod".to_string(), "mod/b.mod".to_string()], None).unwrap(), "no change, no write");
        // the playset's DLC list replaces the file's; None leaves it
        let two = ["mod/a.mod".to_string(), "mod/b.mod".to_string()];
        assert!(write(&p, &two, Some(&["dlc/x/x.dlc".to_string()])).unwrap());
        assert_eq!(disabled_dlcs_of(&read(&p).unwrap().1), vec!["dlc/x/x.dlc"]);
        assert!(!write(&p, &two, Some(&["dlc/x/x.dlc".to_string()])).unwrap());
        assert!(!write(&p, &two, None).unwrap());
        assert!(write(&p, &two, Some(&[])).unwrap());
        assert!(disabled_dlcs_of(&read(&p).unwrap().1).is_empty());
        let backup = std::fs::read_to_string(dir.join("dlc_load.json.stl_backup")).unwrap();
        assert!(backup.contains("mod/old.mod"));
        let fresh = dir.join("sub");
        std::fs::create_dir_all(&fresh).unwrap();
        assert!(write(&fresh.join("dlc_load.json"), &[], None).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
