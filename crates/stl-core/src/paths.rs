//! Where things are: the user's Documents folder, our own data folder, Steam's libraries and the game.

use crate::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// `Documents`, wherever Windows has put it (it can be redirected, e.g. to OneDrive).
pub fn documents_dir() -> Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut p);
        if hr < 0 || p.is_null() {
            bail!("could not find the Documents folder (HRESULT {hr:#x})");
        }
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p as *const _);
        Ok(PathBuf::from(s))
    }
}

/// Our own data: `%APPDATA%\stellaris-launcher` (settings, playsets, plugins, logs).
pub fn app_data_dir() -> Result<PathBuf> {
    let base = std::env::var_os("APPDATA").context("%APPDATA% is not set")?;
    Ok(PathBuf::from(base).join("stellaris-launcher"))
}

/// The folders of the Steam libraries that may hold games (`...\steamapps`).
pub fn steam_libraries() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in [r"HKCU\Software\Valve\Steam", r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam"] {
        for value in ["SteamPath", "InstallPath"] {
            if let Ok(out) = std::process::Command::new("reg").args(["query", key, "/v", value]).output() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if let Some(rest) = line.split("REG_SZ").nth(1) {
                        roots.push(PathBuf::from(rest.trim().replace('/', "\\")));
                    }
                }
            }
        }
    }
    let mut libs = Vec::new();
    for root in roots {
        libs.push(root.join("steamapps"));
        if let Ok(vdf) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
            libs.extend(library_paths(&vdf));
        }
    }
    let mut seen = Vec::new();
    libs.retain(|p| {
        let key = p.to_string_lossy().to_lowercase();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    libs
}

/// The `"path"` entries of Steam's `libraryfolders.vdf`, each plus `steamapps`.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in vdf.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"path\"") {
            let value = rest.trim().trim_matches('"').replace("\\\\", "\\");
            if !value.is_empty() {
                out.push(PathBuf::from(value).join("steamapps"));
            }
        }
    }
    out
}

/// The folder of `stellaris.exe`: `explicit`, then `%STELLARIS_DIR%`, then the Steam libraries.
pub fn find_game_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(e) = explicit {
        candidates.push(e.to_path_buf());
    }
    if let Some(e) = std::env::var_os("STELLARIS_DIR") {
        candidates.push(PathBuf::from(e));
    }
    for lib in steam_libraries() {
        candidates.push(lib.join("common").join("Stellaris"));
    }
    for c in &candidates {
        if c.join("stellaris.exe").is_file() {
            return Ok(c.clone());
        }
    }
    match explicit {
        Some(e) => bail!("there is no stellaris.exe in {}", e.display()),
        None => bail!("could not find Stellaris in the Steam libraries; use --game <folder of stellaris.exe>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"E:\\\\Games\\\\SteamLibrary\"\n\t}\n}\n";
        let libs = library_paths(vdf);
        assert_eq!(libs.len(), 2);
        assert!(libs[1].ends_with("steamapps"));
        assert!(libs[1].to_string_lossy().starts_with("E:\\Games\\SteamLibrary"));
    }
}
