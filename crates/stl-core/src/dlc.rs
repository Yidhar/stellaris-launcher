//! The game's DLC: the `dlc/<folder>/<id>.dlc` descriptors. Which ones are *owned* is Steam's business (the game checks); what a launcher can do
//! is list the installed ones and say which to switch off (`disabled_dlcs` of `dlc_load.json`, the same switch the official launcher uses).
//! Nothing here touches `dlc_signature`, which the game writes itself.

use crate::script;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Dlc {
    /// what `disabled_dlcs` lists: the descriptor relative to the game folder, `dlc/dlc001_symbols_of_domination/dlc001.dlc`
    pub id: String,
    pub name: String,
    /// `expansion`, `story_pack`, `species_pack`, `content_pack`, …
    pub category: String,
    pub steam_id: Option<u64>,
    pub thumbnail: Option<PathBuf>,
}

pub fn parse_descriptor(id: &str, text: &str, thumbnail: Option<PathBuf>) -> Dlc {
    let s = script::parse(text);
    Dlc {
        id: id.to_string(),
        name: script::get(&s, "name").unwrap_or(id).to_string(),
        category: script::get(&s, "category").unwrap_or("").to_string(),
        steam_id: script::get(&s, "steam_id").and_then(|v| v.parse().ok()),
        thumbnail,
    }
}

/// The installed DLC, in the order of their folder names (which is the order they were released in).
pub fn scan(game_dir: &Path) -> Vec<Dlc> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(game_dir.join("dlc")) else { return out };
    let mut folders: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).collect();
    folders.sort_by_key(|e| e.file_name());
    for f in folders {
        let folder = f.file_name().to_string_lossy().to_string();
        let Ok(inner) = std::fs::read_dir(f.path()) else { continue };
        for e in inner.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "dlc").unwrap_or(false) {
                if let Ok(bytes) = std::fs::read(&p) {
                    let id = format!("dlc/{folder}/{}", e.file_name().to_string_lossy());
                    let thumb = f.path().join("thumbnail.png");
                    out.push(parse_descriptor(&id, &String::from_utf8_lossy(&bytes), thumb.is_file().then_some(thumb)));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_descriptor() {
        let d = parse_descriptor(
            "dlc/dlc014_utopia/dlc014.dlc",
            "name = \"Utopia\"\nlocalizable_name = \"DLC_UTOPIA\"\narchive = \"dlc/dlc014_utopia/dlc014.zip\"\nsteam_id = 447700\ncategory = \"expansion\"\n",
            None,
        );
        assert_eq!(d.name, "Utopia");
        assert_eq!(d.category, "expansion");
        assert_eq!(d.steam_id, Some(447700));
        assert_eq!(parse_descriptor("x", "", None).name, "x");
    }
}
