//! Whether a mod keeps Ironman (and achievements): it does when it changes nothing the game checksums. Which files those are, the game says
//! itself in `checksum_manifest.txt` (`directory { name = common sub_directories = yes file_extension = .txt }` …); a mod that has any such
//! file, in its folder or in its zip, changes the checksum.

use crate::mods::Mod;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub dir: String,
    pub sub_directories: bool,
    /// with its dot, lower case: `.txt`
    pub extension: String,
}

pub fn parse_manifest(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut cur: Option<Rule> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("directory") {
            if let Some(r) = cur.take() {
                rules.push(r);
            }
            cur = Some(Rule { dir: String::new(), sub_directories: false, extension: String::new() });
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        if let Some(r) = cur.as_mut() {
            match k {
                "name" => r.dir = v.replace('\\', "/").trim_matches('/').to_lowercase(),
                "sub_directories" => r.sub_directories = v == "yes",
                "file_extension" => r.extension = v.to_lowercase(),
                _ => {}
            }
        }
    }
    if let Some(r) = cur {
        rules.push(r);
    }
    rules.retain(|r| !r.dir.is_empty() && !r.extension.is_empty());
    rules
}

/// The rules of the installed game; the ones of Stellaris 4.5 when the file cannot be read.
pub fn rules(game_dir: &Path) -> Vec<Rule> {
    let r = std::fs::read_to_string(game_dir.join("checksum_manifest.txt")).map(|t| parse_manifest(&t)).unwrap_or_default();
    if !r.is_empty() {
        return r;
    }
    [("common", ".txt"), ("common", ".shader"), ("common", ".csv"), ("events", ".txt"), ("map", ".shader"), ("map", ".txt")]
        .iter()
        .map(|(d, e)| Rule { dir: d.to_string(), sub_directories: true, extension: e.to_string() })
        .collect()
}

/// Does a path inside a mod (`common/buildings/x.txt`, with `/`) fall under a rule?
pub fn matches(rules: &[Rule], rel: &str) -> bool {
    let rel = rel.replace('\\', "/").to_lowercase();
    let rel = rel.trim_start_matches('/');
    rules.iter().any(|r| {
        let Some(rest) = rel.strip_prefix(&r.dir).and_then(|x| x.strip_prefix('/')) else { return false };
        rest.ends_with(&r.extension) && (r.sub_directories || !rest.contains('/'))
    })
}

fn walk(dir: &Path, prefix: &str, rules: &[Rule]) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            // only the folders some rule can reach are entered
            let lower = rel.to_lowercase();
            if rules.iter().any(|r| r.dir.starts_with(&lower) || lower.starts_with(&r.dir)) && walk(&e.path(), &rel, rules) {
                return true;
            }
        } else if matches(rules, &rel) {
            return true;
        }
    }
    false
}

/// Whether the mod changes the checksum (so Ironman and achievements are off with it). None when its files cannot be read.
pub fn affects_checksum(m: &Mod, rules: &[Rule]) -> Option<bool> {
    if let Some(p) = m.path.as_ref().filter(|p| p.is_dir()) {
        return Some(walk(p, "", rules));
    }
    let a = m.archive.as_ref().filter(|a| a.is_file())?;
    let file = std::fs::File::open(a).ok()?;
    let zip = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let hit = zip.file_names().any(|n| matches(rules, n));
    Some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "directory \nname = common\nsub_directories = yes\nfile_extension = .txt\n\ndirectory\nname = events\nsub_directories = yes\nfile_extension = .txt\n\ndirectory\nname = map\nsub_directories = no\nfile_extension = .shader\n";

    #[test]
    fn reads_the_manifest_and_matches_paths() {
        let r = parse_manifest(MANIFEST);
        assert_eq!(r.len(), 3);
        assert!(matches(&r, "common/buildings/00_x.txt"));
        assert!(matches(&r, "Common\\Traits\\a.TXT"));
        assert!(matches(&r, "events/e.txt"));
        assert!(matches(&r, "map/a.shader"));
        assert!(!matches(&r, "map/sub/a.shader"), "sub_directories = no");
        assert!(!matches(&r, "gfx/portraits/x.txt"));
        assert!(!matches(&r, "localisation/x.yml"));
        assert!(!matches(&r, "interface/x.gui"));
        assert!(!matches(&r, "commonish/x.txt"));
    }

    #[test]
    fn a_graphics_only_mod_keeps_ironman() {
        let dir = std::env::temp_dir().join(format!("stl-test-ironman-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gfx = dir.join("gfx_mod");
        std::fs::create_dir_all(gfx.join("gfx/portraits")).unwrap();
        std::fs::create_dir_all(gfx.join("localisation")).unwrap();
        std::fs::write(gfx.join("gfx/portraits/a.txt"), "x").unwrap();
        std::fs::write(gfx.join("localisation/a_l_english.yml"), "x").unwrap();
        let game = dir.join("game_mod");
        std::fs::create_dir_all(game.join("common/traits")).unwrap();
        std::fs::write(game.join("common/traits/t.txt"), "x").unwrap();
        let rules = parse_manifest(MANIFEST);
        let m = |p: &Path| crate::mods::parse_descriptor(&dir.join("x.mod"), &format!("name=\"x\"\npath=\"{}\"\n", p.to_string_lossy().replace('\\', "/")), &dir);
        assert_eq!(affects_checksum(&m(&gfx), &rules), Some(false));
        assert_eq!(affects_checksum(&m(&game), &rules), Some(true));
        assert_eq!(affects_checksum(&m(&dir.join("missing")), &rules), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
