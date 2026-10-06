//! The game installation: its `launcher-settings.json` (the same file the official launcher reads), the data folder and the exe.

use crate::{paths, pe, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlternativeExecutable {
    #[serde(default)]
    pub exe_path: String,
    #[serde(default)]
    pub exe_args: Vec<String>,
    #[serde(default)]
    pub label: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSettings {
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub raw_version: String,
    #[serde(default)]
    pub mods_compatibility_version: String,
    #[serde(default)]
    pub dist_platform: String,
    #[serde(default)]
    pub game_data_path: String,
    #[serde(default)]
    pub exe_path: String,
    #[serde(default)]
    pub exe_args: Vec<String>,
    #[serde(default)]
    pub alternative_executables: Vec<AlternativeExecutable>,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub dir: PathBuf,
    pub exe: PathBuf,
    pub exe_args: Vec<String>,
    pub settings: LauncherSettings,
    /// `Documents\Paradox Interactive\Stellaris`
    pub data_dir: PathBuf,
    /// `<data_dir>\mod`
    pub mod_dir: PathBuf,
    pub exe_timestamp: u32,
}

impl Game {
    pub fn open(explicit: Option<&Path>) -> Result<Game> {
        let dir = paths::find_game_dir(explicit)?;
        let settings_path = dir.join("launcher-settings.json");
        let text = std::fs::read_to_string(&settings_path).with_context(|| format!("cannot read {}", settings_path.display()))?;
        let settings: LauncherSettings = serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{} is not valid", settings_path.display()))?;
        let documents = paths::documents_dir()?;
        let data_dir = if settings.game_data_path.is_empty() {
            documents.join("Paradox Interactive").join("Stellaris")
        } else {
            PathBuf::from(settings.game_data_path.replace("%USER_DOCUMENTS%", &documents.to_string_lossy()).replace('/', "\\"))
        };
        let exe_rel = if settings.exe_path.is_empty() { "stellaris.exe" } else { settings.exe_path.trim_start_matches("./") };
        let exe = dir.join(exe_rel);
        let exe_timestamp = pe::timestamp(&exe)?;
        Ok(Game { exe_args: settings.exe_args.clone(), mod_dir: data_dir.join("mod"), data_dir, dir, exe, settings, exe_timestamp })
    }

    /// "v4.5.1" → "4.5.1"
    pub fn version(&self) -> &str {
        self.settings.raw_version.trim_start_matches('v')
    }

    pub fn dlc_load_path(&self) -> PathBuf {
        self.data_dir.join("dlc_load.json")
    }

    /// The executable and arguments of the alternative with this number (0-based), or the normal one.
    pub fn executable(&self, alternative: Option<usize>) -> Result<(PathBuf, Vec<String>)> {
        match alternative {
            None => Ok((self.exe.clone(), self.exe_args.clone())),
            Some(i) => {
                let a = self.settings.alternative_executables.get(i).with_context(|| format!("the game has no alternative executable {i}"))?;
                Ok((self.dir.join(a.exe_path.trim_start_matches("./")), a.exe_args.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_games_launcher_settings() {
        let text = r#"{ "gameId": "stellaris", "version": "Cygnus v4.5.1 (358e)", "rawVersion": "v4.5.1", "modsCompatibilityVersion": "4.5",
            "gameDataPath": "%USER_DOCUMENTS%/Paradox Interactive/Stellaris", "exePath": "./stellaris.exe", "exeArgs": [ "-gdpr-compliant" ],
            "alternativeExecutables": [ { "exePath": "./stellaris.exe", "exeArgs": ["-gdpr-compliant", "-nakama"], "label": { "en": "Cross-Store Multiplayer" } } ] }"#;
        let s: LauncherSettings = serde_json::from_str(text).unwrap();
        assert_eq!(s.raw_version, "v4.5.1");
        assert_eq!(s.exe_args, vec!["-gdpr-compliant"]);
        assert_eq!(s.alternative_executables[0].exe_args[1], "-nakama");
        assert_eq!(s.alternative_executables[0].label["en"], "Cross-Store Multiplayer");
    }
}
