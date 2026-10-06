//! Making a new local mod, and writing back what an upload learned (`remote_file_id`).
//!
//! A local mod is two descriptors: `mod/<folder>.mod` (what the game and `dlc_load.json` name, with `path=` to the content) and
//! `mod/<folder>/descriptor.mod` inside the content (what goes to the Workshop with it, without `path=`).

use crate::mods::{self, Mod};
use crate::{bail, Context, Result};
use std::path::Path;

/// The tags Stellaris' Workshop offers.
pub const TAGS: &[&str] = &[
    "Alternative History", "Balance", "Buildings", "Diplomacy", "Economy", "Events", "Fixes", "Font", "Galaxy Generation", "Gameplay", "Graphics",
    "Leaders", "Loading Screen", "Military", "Overhaul", "Sound", "Spaceships", "Species", "Technologies", "Total Conversion", "Translation",
    "Utilities",
];

#[derive(Debug, Clone)]
pub struct NewMod {
    pub name: String,
    pub version: String,
    /// `v4.5.*`
    pub supported_version: String,
    pub tags: Vec<String>,
}

/// A folder name for a mod name: lower-case ASCII letters, digits and `_` (other characters become `_`, runs collapse).
pub fn folder_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' };
        if c == '_' && (out.is_empty() || out.ends_with('_')) {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() { "my_mod".to_string() } else { out }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "/").replace('"', "'"))
}

/// The text of a descriptor.
pub fn descriptor_text(m: &NewMod, path: Option<&str>, remote_file_id: Option<&str>) -> String {
    let mut t = String::new();
    t.push_str(&format!("version={}\n", quote(&m.version)));
    if !m.tags.is_empty() {
        t.push_str("tags={\n");
        for tag in &m.tags {
            t.push_str(&format!("\t{}\n", quote(tag)));
        }
        t.push_str("}\n");
    }
    t.push_str(&format!("name={}\n", quote(&m.name)));
    t.push_str(&format!("supported_version={}\n", quote(&m.supported_version)));
    if let Some(p) = path {
        t.push_str(&format!("path={}\n", quote(p)));
    }
    if let Some(id) = remote_file_id {
        t.push_str(&format!("remote_file_id={}\n", quote(id)));
    }
    t
}

/// Makes the content folder and both descriptors in `<data dir>/mod`, and returns the new mod as the mod list sees it.
pub fn create(data_dir: &Path, m: &NewMod) -> Result<Mod> {
    if m.name.trim().is_empty() {
        bail!("a mod needs a name");
    }
    let mod_dir = data_dir.join("mod");
    std::fs::create_dir_all(&mod_dir)?;
    let base = folder_name(&m.name);
    let mut folder = base.clone();
    let mut n = 2;
    while mod_dir.join(&folder).exists() || mod_dir.join(format!("{folder}.mod")).exists() {
        folder = format!("{base}_{n}");
        n += 1;
    }
    let content = mod_dir.join(&folder);
    std::fs::create_dir_all(content.join("common")).with_context(|| format!("cannot create {}", content.display()))?;
    std::fs::write(content.join("descriptor.mod"), descriptor_text(m, None, None))?;
    let path = content.to_string_lossy().replace('\\', "/");
    let outer = mod_dir.join(format!("{folder}.mod"));
    std::fs::write(&outer, descriptor_text(m, Some(&path), None))?;
    let text = std::fs::read_to_string(&outer)?;
    Ok(mods::parse_descriptor(&outer, &text, data_dir))
}

/// Sets `key="value"` in a descriptor's text: the existing line is replaced, or a line is added.
pub fn set_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key}={}", quote(value));
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if !found && t.starts_with(key) && t[key.len()..].trim_start().starts_with('=') {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    out.join("\n") + "\n"
}

/// Records the Workshop item a mod was uploaded as, in both of its descriptors.
pub fn set_remote_file_id(m: &Mod, id: u64) -> Result<()> {
    let mut files = vec![m.file.clone()];
    if let Some(p) = &m.path {
        let inner = p.join("descriptor.mod");
        if inner.is_file() {
            files.push(inner);
        }
    }
    for f in files {
        let text = std::fs::read_to_string(&f).with_context(|| format!("cannot read {}", f.display()))?;
        std::fs::write(&f, set_key(&text, "remote_file_id", &id.to_string())).with_context(|| format!("cannot write {}", f.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("My Cool Mod!"), "my_cool_mod");
        assert_eq!(folder_name("  __x--y__ "), "x_y");
        assert_eq!(folder_name("中文模组"), "my_mod");
    }

    #[test]
    fn creates_a_mod_the_scanner_reads_back_and_records_its_upload() {
        let dir = std::env::temp_dir().join(format!("stl-test-modmake-{}", std::process::id()));
        let m = NewMod { name: "Test Mod".into(), version: "1.0".into(), supported_version: "v4.5.*".into(), tags: vec!["Gameplay".into(), "Fixes".into()] };
        let made = create(&dir, &m).unwrap();
        assert_eq!(made.id, "mod/test_mod.mod");
        assert_eq!(made.name, "Test Mod");
        assert_eq!(made.tags, vec!["Gameplay", "Fixes"]);
        assert!(made.problem.is_none(), "{:?}", made.problem);
        assert!(dir.join("mod/test_mod/descriptor.mod").is_file());
        // a second one with the same name gets its own folder
        assert_eq!(create(&dir, &m).unwrap().id, "mod/test_mod_2.mod");
        set_remote_file_id(&made, 1234567890).unwrap();
        let again = mods::scan(&dir).into_iter().find(|x| x.id == made.id).unwrap();
        assert_eq!(again.remote_file_id.as_deref(), Some("1234567890"));
        assert!(std::fs::read_to_string(dir.join("mod/test_mod/descriptor.mod")).unwrap().contains("remote_file_id=\"1234567890\""));
        set_remote_file_id(&again, 42).unwrap();
        let text = std::fs::read_to_string(&again.file).unwrap();
        assert_eq!(text.matches("remote_file_id").count(), 1, "replaced, not added: {text}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
