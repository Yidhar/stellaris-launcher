//! Plugin indexes: `plugins.jsonl` files listing DLL plugins to install, one JSON object a line (the format of
//! github.com/Yidhar/stellaris-plugins, whose `schema/plugin-entry.schema.json` describes it). A line says where the plugin's zip is (a release
//! asset of its own repository) and the zip's SHA-256; installing downloads it, insists on that SHA-256, checks that the zip is that plugin,
//! and installs it like any plugin folder (the user's `config` kept). The launcher reads the indexes the store names, or the default one.
//!
//! The default index is read from its mirror first (`MIRROR_INDEX`, Cloudflare R2): the index repository's CI copies every plugin zip there
//! and publishes the index with `download` pointing at the copy and `fallback` at the GitHub release asset, signed with a key only that CI
//! holds (`INDEX_KEY` is its public half). An index that does not verify is not used; then GitHub's copy is read as before. A download is
//! tried from `download`, then `fallback`, and must match the index's SHA-256 either way.

use crate::game::Game;
use crate::plugins::{self, Plugin};
use crate::{bail, net, pe, signed, updates, Context, Result};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The index the launcher reads when the store names none.
pub const DEFAULT_INDEX: &str = "https://raw.githubusercontent.com/Yidhar/stellaris-plugins/main/plugins.jsonl";
/// The mirror's signed copy of the default index.
pub const MIRROR_INDEX: &str = "https://plugins.stellarismod.com/index.json";
/// The public key the mirror's index must be signed with (the private half is a secret of the index repository's CI).
const INDEX_KEY: &str = "3d5253f5b943d0e56325af7e150e1c39d0a80982f14bc241401bb1baf224d2ef";
const INDEX_PRODUCT: &str = "stellaris-plugins";

/// The mirror's index, unless `STL_PLUGIN_MIRROR=off` (or another URL for it).
pub fn mirror_index() -> Option<String> {
    match std::env::var("STL_PLUGIN_MIRROR") {
        Ok(s) if s.trim().eq_ignore_ascii_case("off") => None,
        Ok(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => Some(MIRROR_INDEX.to_string()),
    }
}

/// The plugins of a signed mirror index (they are given `DEFAULT_INDEX` as their index: the mirror stands in for it).
fn read_mirror(text: &str, key: &ed25519_dalek::VerifyingKey) -> Result<Vec<Entry>> {
    #[derive(Deserialize)]
    struct Signed {
        product: String,
        plugins: Vec<serde_json::Value>,
    }
    let manifest = signed::open(text, key, "the mirror's plugin index")?;
    let m: Signed = serde_json::from_str(&manifest).context("the mirror's plugin index cannot be read")?;
    if m.product != INDEX_PRODUCT {
        bail!("the mirror's plugin index is for {}, not {INDEX_PRODUCT}", m.product);
    }
    let text: String = m.plugins.iter().map(|v| v.to_string() + "\n").collect();
    let (entries, _) = parse(&text, DEFAULT_INDEX);
    Ok(entries.into_iter().filter(|e| e.fallback.as_deref().map_or(true, |f| f.starts_with("https://"))).collect())
}

static MIRRORED: Mutex<Option<(Instant, String, Vec<Entry>)>> = Mutex::new(None);

/// The mirror's index, read at most every five minutes (plugin update checks ask for it once per plugin).
pub fn fetch_mirror() -> Result<Vec<Entry>> {
    let url = mirror_index().context("the plugin mirror is switched off")?;
    if let Some((at, u, entries)) = MIRRORED.lock().ok().and_then(|g| g.clone()) {
        if u == url && at.elapsed() < Duration::from_secs(300) {
            return Ok(entries);
        }
    }
    let body = net::http_get(&url, 20_000, 8 << 20).with_context(|| format!("cannot read {url}"))?;
    let entries = read_mirror(&String::from_utf8_lossy(&body), &signed::key(INDEX_KEY).context("INDEX_KEY")?)?;
    if let Ok(mut g) = MIRRORED.lock() {
        *g = Some((Instant::now(), url, entries.clone()));
    }
    Ok(entries)
}

/// The mirror's line for a plugin, when it has one from the same repository as `repo` (`owner/repo`).
pub fn mirrored(id: &str, repo: &str) -> Option<Entry> {
    fetch_mirror().ok()?.into_iter().find(|e| e.id == id && updates::repo_of(&e.repository).is_some_and(|r| r.eq_ignore_ascii_case(repo)))
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub version: String,
    pub download: String,
    pub sha256: String,
    pub repository: String,
    /// the `stellaris.exe` builds it is made for (`0x6ABEAA3F`); empty: it checks for itself
    #[serde(default)]
    pub game_builds: Vec<String>,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    /// where else the same zip is (the mirror's index: the GitHub release asset)
    #[serde(default)]
    pub fallback: Option<String>,
    /// the index it came from
    #[serde(skip)]
    pub index: String,
}

/// Whether a plugin of an index is made for the game as it is.
#[derive(Debug, Clone, PartialEq)]
pub enum Fit {
    Ok,
    /// it names no builds
    Unchecked,
    /// made for other builds (the game version they are, when the index says)
    Other(Option<String>),
}

impl Entry {
    pub fn fits(&self, game: &Game) -> Fit {
        let builds: Vec<u32> = self.game_builds.iter().filter_map(|s| pe::parse_timestamp(s)).collect();
        if builds.is_empty() {
            Fit::Unchecked
        } else if builds.contains(&game.exe_timestamp) {
            Fit::Ok
        } else {
            Fit::Other(self.game_version.clone())
        }
    }

    /// A page for people: its homepage, or its repository.
    pub fn page(&self) -> &str {
        self.homepage.as_deref().filter(|h| h.starts_with("https://")).unwrap_or(&self.repository)
    }
}

/// Where a plugin of an index stands against what is installed.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    NotInstalled,
    /// installed, at least that version
    Installed,
    /// installed, older: the version installed
    Older(String),
    /// installed as a link to a folder under development (not replaced from an index)
    Linked,
}

pub fn state(e: &Entry, installed: &[Plugin]) -> State {
    match installed.iter().find(|p| p.manifest.id == e.id) {
        None => State::NotInstalled,
        Some(p) if p.linked => State::Linked,
        Some(p) if updates::newer(&e.version, &p.manifest.version) => State::Older(p.manifest.version.clone()),
        Some(_) => State::Installed,
    }
}

/// The lines of an index; a line that is not a plugin is passed over (and counted).
pub fn parse(text: &str, index: &str) -> (Vec<Entry>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<Entry>(line) {
            Ok(mut e) if !e.id.is_empty() && e.download.starts_with("https://") && e.sha256.len() == 64 => {
                e.index = index.to_string();
                out.push(e);
            }
            _ => bad += 1,
        }
    }
    (out, bad)
}

/// One index, read now. The default one is read from its mirror when that works.
pub fn fetch(url: &str) -> Result<Vec<Entry>> {
    if url == DEFAULT_INDEX {
        if let Ok(entries) = fetch_mirror() {
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    let body = net::http_get(url, 20_000, 8 << 20).with_context(|| format!("cannot read {url}"))?;
    let (entries, bad) = parse(&String::from_utf8_lossy(&body), url);
    if entries.is_empty() && bad > 0 {
        bail!("{url} is not a plugin index ({bad} line(s) not understood)");
    }
    Ok(entries)
}

/// Every index, in order: the plugins (an id listed by more than one index is taken from the first), and the indexes that could not be read.
pub fn fetch_all(urls: &[String]) -> (Vec<Entry>, Vec<(String, String)>) {
    let mut out: Vec<Entry> = Vec::new();
    let mut failed = Vec::new();
    for url in urls {
        match fetch(url) {
            Ok(entries) => {
                for e in entries {
                    if !out.iter().any(|x| x.id == e.id) {
                        out.push(e);
                    }
                }
            }
            Err(e) => failed.push((url.clone(), format!("{e:#}"))),
        }
    }
    (out, failed)
}

/// The indexes to read: the store's, or the default one.
pub fn indexes(configured: Option<&Vec<String>>) -> Vec<String> {
    match configured {
        Some(list) => list.clone(),
        None => vec![DEFAULT_INDEX.to_string()],
    }
}

/// Downloads a plugin of an index, checks it, and installs it (over an older version: the user's settings kept).
pub fn install(e: &Entry) -> Result<Plugin> {
    let urls: Vec<&str> = std::iter::once(e.download.as_str()).chain(e.fallback.as_deref()).collect();
    let bytes = updates::download_checked(&urls, &e.sha256, &e.id)?;
    let tmp = std::env::temp_dir().join(format!("stl-get-{}-{}", e.id, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| {
        updates::unzip(&bytes, &tmp)?;
        let root = updates::plugin_root(&tmp).with_context(|| format!("the zip of {} holds no {}", e.id, plugins::MANIFEST))?;
        let m = plugins::parse_manifest(&std::fs::read_to_string(root.join(plugins::MANIFEST))?)?;
        if m.id != e.id {
            bail!("the zip listed as {:?} is the plugin {:?}", e.id, m.id);
        }
        if let Some(p) = plugins::list().ok().and_then(|(i, _)| i.into_iter().find(|p| p.manifest.id == e.id)) {
            if p.linked {
                bail!("{} is linked to a folder under development; update it there", e.id);
            }
        }
        plugins::install(&root, false)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_index() {
        let text = concat!(
            r#"{"id":"a","name":"A","author":"x","description":"d","version":"1.2.0","download":"https://github.com/x/a/releases/download/v1.2.0/a.zip","#,
            r#""sha256":"0000000000000000000000000000000000000000000000000000000000000000","repository":"https://github.com/x/a","game_builds":["0x6ABEAA3F"]}"#,
            "\n\nnot json\n",
            r#"{"id":"b","name":"B","author":"x","description":"d","version":"1","download":"http://plain/b.zip","sha256":"00","repository":"https://github.com/x/b"}"#,
            "\n"
        );
        let (entries, bad) = parse(text, "https://idx");
        assert_eq!(entries.len(), 1);
        assert_eq!(bad, 2);
        assert_eq!(entries[0].index, "https://idx");
        assert_eq!(entries[0].game_builds, vec!["0x6ABEAA3F"]);
        assert_eq!(indexes(None), vec![DEFAULT_INDEX.to_string()]);
        assert_eq!(indexes(Some(&vec![])), Vec::<String>::new());
    }

    #[test]
    fn the_index_key_is_a_valid_public_key() {
        assert!(signed::key(INDEX_KEY).is_ok());
    }

    #[test]
    fn takes_only_a_mirror_index_signed_with_its_key() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let other = ed25519_dalek::SigningKey::from_bytes(&[4u8; 32]);
        let public = key.verifying_key();
        let line = |id: &str, fallback: &str| serde_json::json!({
            "id": id, "name": "A", "author": "x", "description": "d", "version": "1.2.0",
            "download": format!("https://dl.example/plugins/{id}/v1.2.0/{id}.zip"), "fallback": fallback,
            "sha256": "0".repeat(64), "repository": format!("https://github.com/x/{id}"),
        });
        let doc = |product: &str| serde_json::json!({ "product": product, "plugins": [line("a", "https://github.com/x/a/releases/download/v1.2.0/a.zip"), line("b", "http://plain/b.zip")] }).to_string();

        let got = read_mirror(&signed::sign(&key, &doc(INDEX_PRODUCT)), &public).unwrap();
        assert_eq!(got.len(), 1, "a fallback that is not https is left out");
        assert_eq!(got[0].id, "a");
        assert_eq!(got[0].index, DEFAULT_INDEX);
        assert_eq!(got[0].fallback.as_deref(), Some("https://github.com/x/a/releases/download/v1.2.0/a.zip"));

        assert!(read_mirror(&signed::sign(&other, &doc(INDEX_PRODUCT)), &public).is_err(), "another key");
        assert!(read_mirror(&signed::sign(&key, &doc(INDEX_PRODUCT)).replace("1.2.0", "1.2.1"), &public).is_err(), "changed after signing");
        assert!(read_mirror(&signed::sign(&key, &doc("stellaris-launcher")), &public).is_err(), "another product");
        assert!(read_mirror(&doc(INDEX_PRODUCT), &public).is_err(), "not signed");
    }
}
