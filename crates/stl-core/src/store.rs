//! Our own settings and playsets, in `%APPDATA%\stellaris-launcher\playsets.json`: a playset is an ordered list of mods (by descriptor id) with an
//! enabled flag each, plus the DLL plugins that go with it.

use crate::{bail, paths, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaysetMod {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaysetPlugin {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub mods: Vec<PlaysetMod>,
    #[serde(default)]
    pub plugins: Vec<PlaysetPlugin>,
    /// DLC switched off in this playset (`dlc/dlc001_x/dlc001.dlc`); None = leave whatever `dlc_load.json` says
    #[serde(default)]
    pub disabled_dlcs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Store {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub playsets: Vec<Playset>,
    /// the game folder the user chose, if it is not found by itself
    #[serde(default)]
    pub game_dir: Option<String>,
    /// the window's language (`en`, `zh-Hans`, …); None = the game's language
    #[serde(default)]
    pub language: Option<String>,
    /// `auto`, `launcher`, `steam` or `none`
    #[serde(default)]
    pub background: Option<String>,
    /// fetch the public news feed; None = yes
    #[serde(default)]
    pub news_online: Option<bool>,
    /// the Play page's news strip is folded away to the side
    #[serde(default)]
    pub news_folded: Option<bool>,
    /// look for a newer launcher on start and download it in the background; None = yes
    #[serde(default)]
    pub auto_update: Option<bool>,
    /// the Mods page: `name`, `updated` or `source`; and `list` or `compact`
    #[serde(default)]
    pub mods_sort: Option<String>,
    #[serde(default)]
    pub mods_view: Option<String>,
    /// load the playset's DLL plugins when starting; None = yes
    #[serde(default)]
    pub use_plugins: Option<bool>,
    /// the alternative executable of `launcher-settings.json` to start (None = the standard one)
    #[serde(default)]
    pub alternative: Option<usize>,
    #[serde(skip)]
    path: PathBuf,
}

fn new_id(seed: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
}

impl Store {
    pub fn default_path() -> Result<PathBuf> {
        Ok(paths::app_data_dir()?.join("playsets.json"))
    }

    pub fn load() -> Result<Store> {
        Store::load_from(&Store::default_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Store> {
        let mut s = if path.is_file() {
            let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
            serde_json::from_str::<Store>(&text).with_context(|| format!("{} is not valid", path.display()))?
        } else {
            Store::default()
        };
        s.version = 1;
        s.path = path.to_path_buf();
        if s.playsets.is_empty() {
            let p = Playset { id: new_id("default"), name: "Default".into(), mods: Vec::new(), plugins: Vec::new(), disabled_dlcs: None };
            s.active = Some(p.id.clone());
            s.playsets.push(p);
        }
        if s.active.as_ref().map_or(true, |a| !s.playsets.iter().any(|p| &p.id == a)) {
            s.active = s.playsets.first().map(|p| p.id.clone());
        }
        Ok(s)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("cannot write {}", self.path.display()))?;
        Ok(())
    }

    /// A playset by name (case-insensitive) or by the start of its id.
    pub fn find(&self, name_or_id: &str) -> Option<usize> {
        let n = name_or_id.to_lowercase();
        self.playsets.iter().position(|p| p.name.to_lowercase() == n).or_else(|| self.playsets.iter().position(|p| p.id.starts_with(&n)))
    }

    pub fn active_index(&self) -> usize {
        self.active.as_ref().and_then(|a| self.playsets.iter().position(|p| &p.id == a)).unwrap_or(0)
    }

    pub fn active_playset(&self) -> &Playset {
        &self.playsets[self.active_index()]
    }

    pub fn add_playset(&mut self, name: &str) -> Result<usize> {
        if self.find(name).is_some() {
            bail!("there is already a playset called {name}");
        }
        self.playsets.push(Playset { id: new_id(name), name: name.to_string(), mods: Vec::new(), plugins: Vec::new(), disabled_dlcs: None });
        Ok(self.playsets.len() - 1)
    }

    pub fn set_active(&mut self, index: usize) {
        self.active = Some(self.playsets[index].id.clone());
    }

    pub fn remove_playset(&mut self, index: usize) -> Result<()> {
        if self.playsets.len() == 1 {
            bail!("cannot remove the only playset");
        }
        let was_active = index == self.active_index();
        self.playsets.remove(index);
        if was_active {
            self.active = self.playsets.first().map(|p| p.id.clone());
        }
        Ok(())
    }
}

impl Playset {
    pub fn mod_pos(&self, id: &str) -> Option<usize> {
        self.mods.iter().position(|m| m.id == id)
    }

    /// Adds a mod at the end (or sets its flag when it is there).
    pub fn set_mod(&mut self, id: &str, enabled: bool) {
        match self.mod_pos(id) {
            Some(i) => self.mods[i].enabled = enabled,
            None => self.mods.push(PlaysetMod { id: id.to_string(), enabled }),
        }
    }

    pub fn remove_mod(&mut self, id: &str) -> bool {
        match self.mod_pos(id) {
            Some(i) => {
                self.mods.remove(i);
                true
            }
            None => false,
        }
    }

    /// Moves a mod to a position (0 = loaded first); positions past the end mean last.
    pub fn move_mod(&mut self, id: &str, to: usize) -> bool {
        let Some(i) = self.mod_pos(id) else { return false };
        let m = self.mods.remove(i);
        let to = to.min(self.mods.len());
        self.mods.insert(to, m);
        true
    }

    /// The DLC this playset switches off: its own list, or `fallback` (the current `dlc_load.json`) while it has none.
    pub fn disabled_dlcs_or<'a>(&'a self, fallback: &'a [String]) -> &'a [String] {
        self.disabled_dlcs.as_deref().unwrap_or(fallback)
    }

    pub fn set_dlc_enabled(&mut self, id: &str, enabled: bool, fallback: &[String]) {
        let list = self.disabled_dlcs.get_or_insert_with(|| fallback.to_vec());
        list.retain(|d| d != id);
        if !enabled {
            list.push(id.to_string());
        }
    }

    pub fn set_plugin(&mut self, id: &str, enabled: bool) {
        match self.plugins.iter().position(|p| p.id == id) {
            Some(i) => self.plugins[i].enabled = enabled,
            None => self.plugins.push(PlaysetPlugin { id: id.to_string(), enabled }),
        }
    }

    /// The enabled mods, in load order.
    pub fn enabled_mods(&self) -> Vec<String> {
        self.mods.iter().filter(|m| m.enabled).map(|m| m.id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("stl-test-store-{}-{}", std::process::id(), new_id("t")));
        Store::load_from(&dir.join("playsets.json")).unwrap()
    }

    #[test]
    fn starts_with_a_default_playset_and_round_trips() {
        let mut s = temp_store();
        assert_eq!(s.playsets.len(), 1);
        assert_eq!(s.active_playset().name, "Default");
        let i = s.add_playset("Ironman").unwrap();
        s.set_active(i);
        s.playsets[i].set_mod("mod/a.mod", true);
        s.playsets[i].set_mod("mod/b.mod", false);
        s.playsets[i].set_plugin("stellaris-live2d", true);
        s.save().unwrap();
        let again = Store::load_from(&s.path).unwrap();
        assert_eq!(again.active_playset().name, "Ironman");
        assert_eq!(again.active_playset().enabled_mods(), vec!["mod/a.mod"]);
        assert_eq!(again.find("iron"), None, "names match whole, ids by prefix");
        assert_eq!(again.find("IRONMAN"), Some(i));
        assert!(s.add_playset("ironman").is_err());
        let _ = std::fs::remove_dir_all(s.path.parent().unwrap());
    }

    #[test]
    fn mod_order() {
        let mut p = Playset { id: "x".into(), name: "x".into(), mods: vec![], plugins: vec![], disabled_dlcs: None };
        for m in ["a", "b", "c", "d"] {
            p.set_mod(m, true);
        }
        assert!(p.move_mod("d", 0));
        assert!(p.move_mod("a", 99));
        assert_eq!(p.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["d", "b", "c", "a"]);
        assert!(p.remove_mod("b"));
        assert!(!p.remove_mod("b"));
        p.set_mod("c", false);
        assert_eq!(p.enabled_mods(), vec!["d", "a"]);
    }

    #[test]
    fn dlc_switches_start_from_the_current_file() {
        let mut p = Playset { id: "x".into(), name: "x".into(), mods: vec![], plugins: vec![], disabled_dlcs: None };
        let current = vec!["dlc/a/a.dlc".to_string()];
        assert_eq!(p.disabled_dlcs_or(&current), &current[..], "no list of its own: the file's");
        p.set_dlc_enabled("dlc/b/b.dlc", false, &current);
        assert_eq!(p.disabled_dlcs.as_deref().unwrap(), &["dlc/a/a.dlc".to_string(), "dlc/b/b.dlc".to_string()]);
        p.set_dlc_enabled("dlc/a/a.dlc", true, &current);
        p.set_dlc_enabled("dlc/b/b.dlc", false, &current);
        assert_eq!(p.disabled_dlcs.as_deref().unwrap(), &["dlc/b/b.dlc".to_string()], "no duplicates");
    }
}
