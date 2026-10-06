//! The mods the game can load: the `*.mod` descriptors in `Documents\Paradox Interactive\Stellaris\mod`.
//! Steam workshop mods are `ugc_<id>.mod`, Paradox Mods are `pdx_<id>.mod`, anything else is a local mod.

use crate::script;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kind {
    Workshop,
    ParadoxMods,
    Local,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Mod {
    /// What `dlc_load.json` lists: the descriptor path relative to the data folder, `mod/ugc_727000451.mod`.
    pub id: String,
    pub file: PathBuf,
    pub name: String,
    pub version: Option<String>,
    pub supported_version: Option<String>,
    /// The content folder (`path=`), made absolute.
    pub path: Option<PathBuf>,
    /// A zip (`archive=`), made absolute.
    pub archive: Option<PathBuf>,
    pub remote_file_id: Option<String>,
    pub tags: Vec<String>,
    pub kind: Kind,
    /// Why the game cannot load it, if so.
    pub problem: Option<String>,
}

pub fn parse_descriptor(file: &Path, text: &str, data_dir: &Path) -> Mod {
    let s = script::parse(text);
    let file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let kind = if file_name.starts_with("ugc_") {
        Kind::Workshop
    } else if file_name.starts_with("pdx_") {
        Kind::ParadoxMods
    } else {
        Kind::Local
    };
    let resolve = |p: &str| {
        let p = p.replace('/', "\\");
        let pb = PathBuf::from(&p);
        if pb.is_absolute() {
            pb
        } else {
            data_dir.join(pb)
        }
    };
    let path = script::get(&s, "path").map(resolve);
    let archive = script::get(&s, "archive").map(resolve);
    let problem = match (&path, &archive) {
        (None, None) => Some("the descriptor has neither path= nor archive=".to_string()),
        (Some(p), _) if !p.is_dir() => Some(format!("the folder {} does not exist", p.display())),
        (None, Some(a)) if !a.is_file() => Some(format!("the archive {} does not exist", a.display())),
        _ => None,
    };
    Mod {
        id: format!("mod/{file_name}"),
        file: file.to_path_buf(),
        name: script::get(&s, "name").map(str::to_string).unwrap_or_else(|| file_name.trim_end_matches(".mod").to_string()),
        version: script::get(&s, "version").map(str::to_string),
        supported_version: script::get(&s, "supported_version").map(str::to_string),
        path,
        archive,
        remote_file_id: script::get(&s, "remote_file_id").map(str::to_string),
        tags: script::get_list(&s, "tags").into_iter().map(str::to_string).collect(),
        kind,
        problem,
    }
}

/// Every descriptor of the mod folder, sorted by name.
pub fn scan(data_dir: &Path) -> Vec<Mod> {
    let mod_dir = data_dir.join("mod");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&mod_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "mod").unwrap_or(false) && p.is_file() {
                if let Ok(bytes) = std::fs::read(&p) {
                    out.push(parse_descriptor(&p, &String::from_utf8_lossy(&bytes), data_dir));
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Does a mod's `supported_version` (`v4.*`, `4.5.*`, `v4.5.1`) cover the game's version (`4.5.1`)?
pub fn supports(supported: &str, game_version: &str) -> bool {
    let p: Vec<&str> = supported.trim().trim_start_matches('v').split('.').collect();
    let g: Vec<&str> = game_version.trim().trim_start_matches('v').split('.').collect();
    for (i, part) in p.iter().enumerate() {
        if *part == "*" {
            return true;
        }
        match g.get(i) {
            Some(gp) if gp == part => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_patterns() {
        assert!(supports("v4.*", "4.5.1"));
        assert!(supports("4.5.*", "4.5.1"));
        assert!(supports("v4.5.1", "v4.5.1"));
        assert!(!supports("v3.*", "4.5.1"));
        assert!(!supports("v4.4.*", "4.5.1"));
        assert!(supports("v4.5", "4.5.1"));
    }

    #[test]
    fn descriptor_kinds_and_paths() {
        let data = Path::new(r"C:\Users\x\Documents\Paradox Interactive\Stellaris");
        let m = parse_descriptor(
            &data.join("mod").join("ugc_727000451.mod"),
            "version=\"2.20.0\"\ntags={\n\t\"Gameplay\"\n}\nname=\"Some mod\"\nsupported_version=\"v4.*\"\npath=\"Z:/does/not/exist\"\nremote_file_id=\"727000451\"\n",
            data,
        );
        assert_eq!(m.id, "mod/ugc_727000451.mod");
        assert_eq!(m.kind, Kind::Workshop);
        assert_eq!(m.name, "Some mod");
        assert_eq!(m.tags, vec!["Gameplay"]);
        assert!(m.problem.as_deref().unwrap().contains("does not exist"));
        let local = parse_descriptor(&data.join("mod").join("mine.mod"), "name=\"Mine\"\npath=\"mod/mine\"\n", data);
        assert_eq!(local.kind, Kind::Local);
        assert_eq!(local.path.as_deref(), Some(data.join(r"mod\mine").as_path()));
    }
}
