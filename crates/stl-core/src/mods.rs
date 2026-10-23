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
    /// the cover picture's file name (`picture=`), when the descriptor names one
    pub picture: Option<String>,
    pub tags: Vec<String>,
    /// the mods it needs, by name (`dependencies = { "UI Overhaul Dynamic" }`)
    pub dependencies: Vec<String>,
    /// folders it replaces whole (`replace_path = "common/x"`): what was loaded before it there is not read
    pub replace_paths: Vec<String>,
    pub kind: Kind,
    /// Why the game cannot load it, if so.
    pub problem: Option<String>,
}

/// How much room a mod takes on the disk: its zip, or everything in its content folder. None when it has neither.
pub fn disk_size(m: &Mod) -> Option<u64> {
    fn walk(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    if let Some(a) = m.archive.as_ref().filter(|a| a.is_file()) {
        return std::fs::metadata(a).ok().map(|x| x.len());
    }
    m.path.as_ref().filter(|p| p.is_dir()).map(|p| walk(p))
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
        picture: script::get(&s, "picture").map(str::to_string),
        tags: script::get_list(&s, "tags").into_iter().map(str::to_string).collect(),
        dependencies: script::get_list(&s, "dependencies").into_iter().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect(),
        replace_paths: script::get_list(&s, "replace_path").into_iter().map(|p| p.replace('\\', "/").trim_matches('/').to_lowercase()).filter(|p| !p.is_empty()).collect(),
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
    // sorted by name, ignoring the punctuation modders put in front to float themselves to the top ("!!! Universal Patch")
    let key = |m: &Mod| m.name.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    out.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.id.cmp(&b.id)));
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
mod workshop_sync_tests {
    use super::*;

    #[test]
    fn writes_the_descriptor_of_a_new_item_and_refreshes_an_outdated_one() {
        let t = std::env::temp_dir().join(format!("stl-wsync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (data, content) = (t.join("data"), t.join("281990"));
        std::fs::create_dir_all(data.join("mod")).unwrap();
        for (id, desc) in [("111", "version=\"1\"\r\nname=\"A\"\r\npath=\"C:/author/a\"\r\nremote_file_id=\"111\"\r\n"), ("222", "name=\"B\"\nsupported_version=\"v4.5.*\"")] {
            std::fs::create_dir_all(content.join(id)).unwrap();
            std::fs::write(content.join(id).join("descriptor.mod"), desc).unwrap();
        }
        // being downloaded: no descriptor yet; not an item
        std::fs::create_dir_all(content.join("333")).unwrap();
        std::fs::create_dir_all(content.join("notes")).unwrap();
        let mut got = sync_workshop_from(&data, &[content.clone()]);
        got.sort();
        assert_eq!(got, vec![("mod/ugc_111.mod".to_string(), Synced::Added), ("mod/ugc_222.mod".to_string(), Synced::Added)]);
        let a = String::from_utf8(std::fs::read(data.join("mod").join("ugc_111.mod")).unwrap()).unwrap();
        let folder = content.join("111").to_string_lossy().replace('\\', "/");
        assert_eq!(a, format!("version=\"1\"\r\nname=\"A\"\npath=\"{folder}\"\nremote_file_id=\"111\""));
        let m = parse_descriptor(&data.join("mod").join("ugc_111.mod"), &a, &data);
        assert_eq!((m.kind, m.name.as_str(), m.problem.is_none()), (Kind::Workshop, "A", true));
        // nothing changed: nothing written
        assert!(sync_workshop_from(&data, &[content.clone()]).is_empty());
        // the author changes it
        std::fs::write(content.join("222").join("descriptor.mod"), "name=\"B\"\nsupported_version=\"v4.6.*\"").unwrap();
        assert_eq!(sync_workshop_from(&data, &[content.clone()]), vec![("mod/ugc_222.mod".to_string(), Synced::Updated)]);
        let _ = std::fs::remove_dir_all(&t);
    }
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

/// What became of a Workshop item's descriptor in the mod folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Synced {
    /// subscribed since: it had none
    Added,
    /// its author changed the descriptor (name, version, supported version…)
    Updated,
}

/// The `ugc_<id>.mod` of a downloaded Workshop item, as the official launcher writes it: the item's own `descriptor.mod` with `path=` (the
/// downloaded folder, with `/`) and `remote_file_id=` put last.
pub fn workshop_descriptor(descriptor: &[u8], folder: &Path, id: &str) -> Vec<u8> {
    let ours = |line: &[u8]| {
        let l = line.trim_ascii_start();
        ["path", "remote_file_id"].iter().any(|k| l.strip_prefix(k.as_bytes()).is_some_and(|rest| rest.trim_ascii_start().starts_with(b"=")))
    };
    let mut out: Vec<u8> = Vec::with_capacity(descriptor.len() + 160);
    for line in descriptor.split(|b| *b == b'\n').filter(|l| !ours(l)) {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    while out.last().is_some_and(|b| b.is_ascii_whitespace()) {
        out.pop();
    }
    if !out.is_empty() {
        out.push(b'\n');
    }
    let folder = folder.to_string_lossy().replace('\\', "/");
    out.extend_from_slice(format!("path=\"{folder}\"\nremote_file_id=\"{id}\"").as_bytes());
    out
}

/// Writes the descriptor of every downloaded Workshop item that has none in the mod folder, or an outdated one, as the official launcher does
/// when it starts: an item subscribed to since is in the mods without going through it.
pub fn sync_workshop(data_dir: &Path) -> Vec<(String, Synced)> {
    let content: Vec<PathBuf> = crate::paths::steam_libraries().into_iter().map(|lib| lib.join("workshop").join("content").join("281990")).collect();
    sync_workshop_from(data_dir, &content)
}

/// `sync_workshop` with the folders that hold the items (`…\workshop\content\281990`) given. Returns each descriptor written: its `id` as in
/// `Mod::id` and what happened.
pub fn sync_workshop_from(data_dir: &Path, content: &[PathBuf]) -> Vec<(String, Synced)> {
    // without it Stellaris has not run yet: the official launcher would not write either
    let mod_dir = data_dir.join("mod");
    if !mod_dir.is_dir() {
        return Vec::new();
    }
    let norm = |b: &[u8]| -> Vec<u8> {
        let mut v: Vec<u8> = b.iter().copied().filter(|c| *c != b'\r').collect();
        while v.last().is_some_and(|c| c.is_ascii_whitespace()) {
            v.pop();
        }
        v
    };
    let mut out = Vec::new();
    for dir in content {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().to_string();
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            // still downloading, or not a mod: nothing to load
            let Ok(desc) = std::fs::read(e.path().join("descriptor.mod")) else { continue };
            // Steam's list of libraries is lower case: the folder as the disk spells it, like the official launcher writes it
            let folder = std::fs::canonicalize(e.path())
                .ok()
                .and_then(|c| c.to_str().and_then(|s| s.strip_prefix(r"\\?\")).filter(|s| s.as_bytes().get(1) == Some(&b':')).map(PathBuf::from))
                .unwrap_or_else(|| e.path());
            let want = workshop_descriptor(&desc, &folder, &id);
            let file = mod_dir.join(format!("ugc_{id}.mod"));
            let what = match std::fs::read(&file) {
                Ok(have) if norm(&have) == norm(&want) => continue,
                Ok(_) => Synced::Updated,
                Err(_) => Synced::Added,
            };
            if std::fs::write(&file, &want).is_ok() {
                out.push((format!("mod/ugc_{id}.mod"), what));
            }
        }
    }
    out
}

/// The folders where Steam keeps the downloaded copy of a Workshop item of Stellaris.
pub fn workshop_folders(id: &str) -> Vec<PathBuf> {
    crate::paths::steam_libraries().into_iter().map(|lib| lib.join("workshop").join("content").join("281990").join(id)).filter(|d| d.is_dir()).collect()
}

/// A mod's cover: `picture=` (or `thumbnail.png`) in its content folder, or else in Steam's downloaded copy of its Workshop item. Collaborators
/// often have no cover locally (the one who uploaded it keeps it); the Workshop copy still shows it.
pub fn thumbnail(m: &Mod) -> Option<PathBuf> {
    let mut names: Vec<String> = Vec::new();
    if let Some(p) = &m.picture {
        names.push(p.clone());
    }
    names.extend(["thumbnail.png", "thumbnail.jpg"].iter().map(|s| s.to_string()));
    let mut dirs: Vec<PathBuf> = m.path.iter().cloned().collect();
    if let Some(id) = &m.remote_file_id {
        dirs.extend(workshop_folders(id));
    }
    for d in dirs {
        for n in &names {
            let f = d.join(n);
            if f.is_file() {
                return Some(f);
            }
        }
    }
    None
}

/// The cover to send with an upload: only one in the mod's own content folder (one found elsewhere is not sent again; an update without a
/// cover keeps the cover the item has).
pub fn own_thumbnail(m: &Mod) -> Option<PathBuf> {
    let p = m.path.as_ref()?;
    let mut names: Vec<String> = m.picture.iter().cloned().collect();
    names.extend(["thumbnail.png", "thumbnail.jpg"].iter().map(|s| s.to_string()));
    names.into_iter().map(|n| p.join(n)).find(|f| f.is_file())
}
