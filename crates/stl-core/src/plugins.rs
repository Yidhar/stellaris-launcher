//! DLL plugins ("DLL mods"): native libraries loaded into the game, described by a small manifest, `stl-plugin.json`, that sits next to the DLL.
//! Installed plugins live in `%APPDATA%\stellaris-launcher\plugins\<id>\` (a copy), or are *linked* (the folder stays where it is, which is how a
//! plugin under development is used). The game folder is not touched except for files a manifest asks to seed (an ini the plugin reads).

use crate::game::Game;
use crate::{bail, paths, pe, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "stl-plugin.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameReq {
    /// PE timestamps of the `stellaris.exe` builds the DLL was made for (`"0x6AB5181D"`); empty = it checks for itself or works with any.
    #[serde(default)]
    pub exe_timestamps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadSpec {
    /// `window` (default): when the game's window exists; `none`: as soon as the process is up.
    #[serde(default = "default_wait")]
    pub wait: String,
    /// further wait after that, in milliseconds
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_wait() -> String {
    "window".into()
}

impl Default for LoadSpec {
    fn default() -> Self {
        LoadSpec { wait: default_wait(), delay_ms: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedFile {
    /// relative to the plugin folder
    pub from: String,
    /// relative to the game folder
    pub to: String,
    /// only when `to` does not exist (default true)
    #[serde(default = "yes")]
    pub if_missing: bool,
    /// replace `{plugin_dir}` and `{game_dir}` in the text
    #[serde(default)]
    pub substitute: bool,
}

fn yes() -> bool {
    true
}

/// A settings file of the plugin, in its `config` folder. The launcher makes it from `default` when it is missing, and lets the user edit it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigFile {
    /// the file name inside `config/`
    pub file: String,
    /// a file in the plugin folder to start from (`defaults/<name>`); with none the file is only listed when it exists
    #[serde(default)]
    pub default: Option<String>,
    /// what the editor calls it; the file name when empty
    #[serde(default)]
    pub title: String,
    /// replace `{plugin_dir}` and `{config_dir}` in the default's text
    #[serde(default)]
    pub substitute: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// the DLL, relative to the plugin folder
    pub dll: String,
    #[serde(default)]
    pub game: GameReq,
    #[serde(default)]
    pub load: LoadSpec,
    /// deprecated (schema 1): files written into the game folder. Settings belong in `config` now
    #[serde(default)]
    pub seed_files: Vec<SeedFile>,
    #[serde(default)]
    pub config: Vec<ConfigFile>,
    /// where updates come from: GitHub releases of a repository
    #[serde(default)]
    pub update: Option<UpdateSpec>,
    /// a page for people (the repository, a forum thread); the GitHub repository is used when this is empty
    #[serde(default)]
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateSpec {
    /// `owner/repo` (or its github.com URL)
    pub github: String,
    /// the release asset to install, `*` as a wildcard (default `*.zip`); a `<asset>.sha256` beside it is checked when present
    #[serde(default)]
    pub asset: Option<String>,
}

/// The folder of a plugin's settings, inside its own folder.
pub const CONFIG_DIR: &str = "config";

#[derive(Debug, Clone, PartialEq)]
pub enum Compat {
    Ok,
    /// the manifest names no builds
    Unchecked,
    Mismatch { declared: Vec<u32>, game: u32 },
    MissingDll(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub linked: bool,
}

impl Plugin {
    pub fn dll_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.dll)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.dir.join(CONFIG_DIR)
    }

    /// Makes the declared settings files that are missing from their defaults. Returns what it made. An existing file is never touched.
    pub fn ensure_config(&self) -> Result<Vec<PathBuf>> {
        let mut made = Vec::new();
        for c in &self.manifest.config {
            let Some(default) = &c.default else { continue };
            let to = self.config_dir().join(&c.file);
            if to.exists() {
                continue;
            }
            let from = self.dir.join(default);
            std::fs::create_dir_all(self.config_dir())?;
            if c.substitute {
                let text = std::fs::read_to_string(&from).with_context(|| format!("cannot read {}", from.display()))?;
                let text = text.replace("{plugin_dir}", &self.dir.to_string_lossy()).replace("{config_dir}", &self.config_dir().to_string_lossy());
                std::fs::write(&to, text)?;
            } else {
                std::fs::copy(&from, &to).with_context(|| format!("cannot copy {}", from.display()))?;
            }
            made.push(to);
        }
        Ok(made)
    }

    /// The settings files to offer for editing: the declared ones that exist, then any other file in `config/`.
    pub fn config_files(&self) -> Vec<PathBuf> {
        let dir = self.config_dir();
        let mut out: Vec<PathBuf> = self.manifest.config.iter().map(|c| dir.join(&c.file)).filter(|p| p.is_file()).collect();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let mut extra: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && !out.contains(p)).collect();
            extra.sort();
            out.extend(extra);
        }
        out
    }

    /// The default a settings file was made from, if the manifest names one.
    pub fn config_default(&self, file: &Path) -> Option<PathBuf> {
        let name = file.file_name()?.to_string_lossy().to_string();
        self.manifest.config.iter().find(|c| c.file == name).and_then(|c| c.default.as_ref()).map(|d| self.dir.join(d))
    }

    pub fn compat(&self, game: &Game) -> Compat {
        let dll = self.dll_path();
        if !dll.is_file() {
            return Compat::MissingDll(dll);
        }
        let declared: Vec<u32> = self.manifest.game.exe_timestamps.iter().filter_map(|s| pe::parse_timestamp(s)).collect();
        if declared.is_empty() {
            Compat::Unchecked
        } else if declared.contains(&game.exe_timestamp) {
            Compat::Ok
        } else {
            Compat::Mismatch { declared, game: game.exe_timestamp }
        }
    }

    pub fn sha256(&self) -> Result<String> {
        let bytes = std::fs::read(self.dll_path())?;
        Ok(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
    }
}

pub fn parse_manifest(text: &str) -> Result<Manifest> {
    let m: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}')).context("the manifest is not valid")?;
    if m.id.is_empty() || !m.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        bail!("the plugin id {:?} must be letters, digits, - _ and .", m.id);
    }
    if m.dll.is_empty() || m.dll.contains("..") || Path::new(&m.dll).is_absolute() {
        bail!("`dll` must be a path inside the plugin folder");
    }
    for s in &m.seed_files {
        if s.from.contains("..") || s.to.contains("..") || Path::new(&s.from).is_absolute() || Path::new(&s.to).is_absolute() {
            bail!("seed file paths must stay inside the plugin folder and the game folder");
        }
    }
    for c in &m.config {
        let bad = |p: &str| p.is_empty() || p.contains("..") || Path::new(p).is_absolute();
        if bad(&c.file) || c.file.contains('/') || c.file.contains('\\') || c.default.as_deref().is_some_and(bad) {
            bail!("config entries name a file in config/ and a default inside the plugin folder");
        }
    }
    Ok(m)
}

/// Where plugins live: `Documents\Paradox Interactive\Stellaris\plugins\<id>\`, next to the game's own `mod` folder, so that a plugin (or its
/// loader) finds itself and its settings without the launcher.
pub fn plugins_dir() -> Result<PathBuf> {
    Ok(paths::documents_dir()?.join("Paradox Interactive").join("Stellaris").join("plugins"))
}

/// The launcher's list of plugins that are used where they are (linked, for development); this is the launcher's own state.
fn links_path() -> Result<PathBuf> {
    Ok(paths::app_data_dir()?.join("plugin-links.json"))
}

/// Moves what an earlier version kept in `%APPDATA%\stellaris-launcher\plugins` to the plugins folder (once; a plugin already there wins).
fn migrate_legacy() {
    let (Ok(old), Ok(new)) = (paths::app_data_dir().map(|d| d.join("plugins")), plugins_dir()) else { return };
    if !old.is_dir() {
        return;
    }
    if let Ok(links) = std::fs::read_to_string(old.join("links.json")) {
        if let Ok(to) = links_path() {
            if !to.exists() {
                let _ = std::fs::write(to, links);
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&old) {
        for e in rd.flatten() {
            let to = new.join(e.file_name());
            if e.path().is_dir() && !to.exists() {
                let _ = copy_dir(&e.path(), &to);
            }
        }
    }
    let _ = std::fs::rename(&old, old.with_file_name("plugins.migrated"));
}

fn read_links() -> Vec<PathBuf> {
    links_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn write_links(links: &[PathBuf]) -> Result<()> {
    let p = links_path()?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, serde_json::to_string_pretty(&links.iter().map(|l| l.to_string_lossy().to_string()).collect::<Vec<_>>())?)?;
    Ok(())
}

fn load_dir(dir: &Path, linked: bool) -> Result<Plugin> {
    let text = std::fs::read_to_string(dir.join(MANIFEST)).with_context(|| format!("no {MANIFEST} in {}", dir.display()))?;
    let manifest = parse_manifest(&text).with_context(|| format!("{}", dir.join(MANIFEST).display()))?;
    Ok(Plugin { manifest, dir: dir.to_path_buf(), linked })
}

/// Installed and linked plugins; a folder that cannot be read is reported in the second list.
pub fn list() -> Result<(Vec<Plugin>, Vec<String>)> {
    migrate_legacy();
    let mut plugins = Vec::new();
    let mut problems = Vec::new();
    let dir = plugins_dir()?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                match load_dir(&e.path(), false) {
                    Ok(p) => plugins.push(p),
                    Err(err) => problems.push(format!("{}: {err:#}", e.path().display())),
                }
            }
        }
    }
    for l in read_links() {
        match load_dir(&l, true) {
            Ok(p) => {
                if !plugins.iter().any(|x| x.manifest.id == p.manifest.id) {
                    plugins.push(p);
                }
            }
            Err(err) => problems.push(format!("{}: {err:#}", l.display())),
        }
    }
    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    Ok((plugins, problems))
}

pub fn find(id: &str) -> Result<Plugin> {
    let (plugins, _) = list()?;
    plugins.into_iter().find(|p| p.manifest.id.eq_ignore_ascii_case(id)).with_context(|| format!("no plugin called {id} is installed (stl plugins)"))
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), &to).with_context(|| format!("cannot copy {}", e.path().display()))?;
        }
    }
    Ok(())
}

/// Installs a plugin from a folder holding `stl-plugin.json`: a copy, or with `link` a reference to the folder.
pub fn install(src: &Path, link: bool) -> Result<Plugin> {
    let src = src.canonicalize().with_context(|| format!("cannot find {}", src.display()))?;
    let src = PathBuf::from(src.to_string_lossy().trim_start_matches(r"\\?\"));
    let probe = load_dir(&src, link)?;
    if !src.join(&probe.manifest.dll).is_file() {
        bail!("{} does not contain {}", src.display(), probe.manifest.dll);
    }
    if link {
        let mut links = read_links();
        if !links.contains(&src) {
            links.push(src.clone());
        }
        write_links(&links)?;
        return Ok(probe);
    }
    let dst = plugins_dir()?.join(&probe.manifest.id);
    // an update keeps the user's settings: config/ is set aside, the new version copied, and the old files put back over the new defaults
    let mut kept: Vec<(std::ffi::OsString, Vec<u8>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dst.join(CONFIG_DIR)) {
        for e in rd.flatten() {
            if e.path().is_file() {
                if let Ok(bytes) = std::fs::read(e.path()) {
                    kept.push((e.file_name(), bytes));
                }
            }
        }
    }
    if dst.exists() {
        std::fs::remove_dir_all(&dst).with_context(|| format!("cannot replace {} (is the game running with it loaded?)", dst.display()))?;
    }
    copy_dir(&src, &dst)?;
    if !kept.is_empty() {
        std::fs::create_dir_all(dst.join(CONFIG_DIR))?;
        for (name, bytes) in kept {
            std::fs::write(dst.join(CONFIG_DIR).join(name), bytes)?;
        }
    }
    let p = load_dir(&dst, false)?;
    p.ensure_config()?;
    Ok(p)
}

pub fn remove(id: &str) -> Result<()> {
    let p = find(id)?;
    if p.linked {
        let links: Vec<PathBuf> = read_links().into_iter().filter(|l| *l != p.dir).collect();
        write_links(&links)
    } else {
        std::fs::remove_dir_all(&p.dir).with_context(|| format!("cannot remove {} (is the game running with it loaded?)", p.dir.display()))
    }
}

/// Creates the files the manifest wants in the game folder (an ini the plugin reads, say). Returns what it created.
pub fn seed(plugin: &Plugin, game: &Game) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    for s in &plugin.manifest.seed_files {
        let from = plugin.dir.join(&s.from);
        let to = game.dir.join(&s.to);
        if s.if_missing && to.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if s.substitute {
            let text = std::fs::read_to_string(&from).with_context(|| format!("cannot read {}", from.display()))?;
            let text = text.replace("{plugin_dir}", &plugin.dir.to_string_lossy()).replace("{game_dir}", &game.dir.to_string_lossy());
            std::fs::write(&to, text)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("cannot copy {}", from.display()))?;
        }
        created.push(to);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{ "schema": 1, "id": "stellaris-live2d", "name": "Live2D portraits", "version": "0.1.0", "dll": "stellaris_live2d.dll",
        "game": { "exe_timestamps": ["0x6AB5181D"] }, "load": { "wait": "window", "delay_ms": 3000 },
        "seed_files": [ { "from": "stellaris_live2d.default.ini", "to": "stellaris_live2d.ini", "substitute": true } ] }"#;

    #[test]
    fn parses_a_manifest() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!(m.id, "stellaris-live2d");
        assert_eq!(m.load.delay_ms, 3000);
        assert!(m.seed_files[0].if_missing, "if_missing defaults to true");
        assert_eq!(parse_manifest(r#"{"id":"x","name":"x","dll":"x.dll"}"#).unwrap().load.wait, "window");
    }

    #[test]
    fn config_files_are_made_from_defaults_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("stl-test-plugin-config-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("defaults")).unwrap();
        std::fs::write(dir.join("defaults/p.ini"), "path={plugin_dir}\n").unwrap();
        std::fs::write(dir.join("p.dll"), b"x").unwrap();
        let m = parse_manifest(r#"{"schema":2,"id":"p","name":"P","dll":"p.dll","config":[{"file":"p.ini","default":"defaults/p.ini","substitute":true}]}"#).unwrap();
        let p = Plugin { manifest: m, dir: dir.clone(), linked: false };
        assert_eq!(p.ensure_config().unwrap(), vec![dir.join("config/p.ini")]);
        assert!(std::fs::read_to_string(dir.join("config/p.ini")).unwrap().contains(&dir.to_string_lossy().to_string()));
        std::fs::write(dir.join("config/p.ini"), "mine").unwrap();
        std::fs::write(dir.join("config/extra.json"), "{}").unwrap();
        assert!(p.ensure_config().unwrap().is_empty(), "an existing file is kept");
        assert_eq!(std::fs::read_to_string(dir.join("config/p.ini")).unwrap(), "mine");
        assert_eq!(p.config_files(), vec![dir.join("config/p.ini"), dir.join("config/extra.json")]);
        assert_eq!(p.config_default(&dir.join("config/p.ini")), Some(dir.join("defaults/p.ini")));
        assert!(parse_manifest(r#"{"id":"p","name":"P","dll":"p.dll","config":[{"file":"../x.ini"}]}"#).is_err());
        assert!(parse_manifest(r#"{"id":"p","name":"P","dll":"p.dll","config":[{"file":"sub/x.ini"}]}"#).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_unsafe_manifests() {
        assert!(parse_manifest(r#"{"id":"../evil","name":"x","dll":"x.dll"}"#).is_err());
        assert!(parse_manifest(r#"{"id":"x","name":"x","dll":"..\\x.dll"}"#).is_err());
        assert!(parse_manifest(r#"{"id":"x","name":"x","dll":"C:\\x.dll"}"#).is_err());
        assert!(parse_manifest(r#"{"id":"x","name":"x","dll":"x.dll","seed_files":[{"from":"a","to":"..\\b"}]}"#).is_err());
        assert!(parse_manifest("not json").is_err());
    }

    #[test]
    fn compat_checks_the_build() {
        let game = crate::game::Game {
            dir: PathBuf::new(),
            exe: PathBuf::new(),
            exe_args: vec![],
            settings: Default::default(),
            data_dir: PathBuf::new(),
            mod_dir: PathBuf::new(),
            exe_timestamp: 0x6AB5181D,
        };
        let dir = std::env::temp_dir().join(format!("stl-test-plugin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.dll"), b"MZ").unwrap();
        let mut p = Plugin { manifest: parse_manifest(r#"{"id":"p","name":"p","dll":"p.dll","game":{"exe_timestamps":["0x6AB5181D"]}}"#).unwrap(), dir: dir.clone(), linked: false };
        assert_eq!(p.compat(&game), Compat::Ok);
        p.manifest.game.exe_timestamps = vec!["0x11111111".into()];
        assert!(matches!(p.compat(&game), Compat::Mismatch { .. }));
        p.manifest.game.exe_timestamps.clear();
        assert_eq!(p.compat(&game), Compat::Unchecked);
        p.manifest.dll = "gone.dll".into();
        assert!(matches!(p.compat(&game), Compat::MissingDll(_)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
