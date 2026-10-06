//! Pictures for the window's background and logo, found at run time in what is already on the machine: the Paradox Launcher's cached theme of
//! the game and Steam's library cache. Nothing is copied or shipped; the files are only read.

use crate::paths;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// `launcher` or `steam`
    pub id: &'static str,
    pub path: PathBuf,
}

fn launcher_cache() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Paradox Interactive").join("launcher-v2").join("cache"))
}

fn steam_app_cache(app_id: u32) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for lib in paths::steam_libraries() {
        if let Some(root) = lib.parent() {
            let d = root.join("appcache").join("librarycache").join(app_id.to_string());
            if d.is_dir() {
                dirs.push(d);
            }
        }
    }
    dirs
}

/// First file called `name` in a Steam app's cache folder, which keeps its pictures in hash-named sub folders.
fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path().join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Background pictures that exist, the best first: the launcher's theme art of the game, then Steam's hero image.
pub fn backgrounds(game_id: &str, steam_app_id: u32) -> Vec<Source> {
    let mut out = Vec::new();
    if let Some(c) = launcher_cache() {
        let p = c.join(format!("{game_id}-background"));
        if p.is_file() {
            out.push(Source { id: "launcher", path: p });
        }
    }
    for d in steam_app_cache(steam_app_id) {
        if let Some(p) = find_in(&d, "library_hero.jpg") {
            out.push(Source { id: "steam", path: p });
            break;
        }
    }
    out
}

/// The game's logo, if one is cached (the launcher's theme logo, or Steam's).
pub fn logo(game_id: &str, steam_app_id: u32) -> Option<PathBuf> {
    if let Some(c) = launcher_cache() {
        let p = c.join(format!("{game_id}-logo"));
        if p.is_file() {
            return Some(p);
        }
    }
    steam_app_cache(steam_app_id).iter().find_map(|d| find_in(d, "logo.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_files_in_hash_folders() {
        let dir = std::env::temp_dir().join(format!("stl-test-art-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("abc123")).unwrap();
        std::fs::write(dir.join("abc123").join("library_hero.jpg"), b"x").unwrap();
        assert_eq!(find_in(&dir, "library_hero.jpg"), Some(dir.join("abc123").join("library_hero.jpg")));
        assert_eq!(find_in(&dir, "logo.png"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
