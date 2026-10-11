//! Plugin indexes: `plugins.jsonl` files listing DLL plugins to install, one JSON object a line (the format of
//! github.com/Yidhar/stellaris-plugins, whose `schema/plugin-entry.schema.json` describes it). A line says where the plugin's zip is (a release
//! asset of its own repository) and the zip's SHA-256; installing downloads it, insists on that SHA-256, checks that the zip is that plugin,
//! and installs it like any plugin folder (the user's `config` kept). The launcher reads the indexes the store names, or the default one.

use crate::game::Game;
use crate::plugins::{self, Plugin};
use crate::{bail, net, pe, updates, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The index the launcher reads when the store names none.
pub const DEFAULT_INDEX: &str = "https://raw.githubusercontent.com/Yidhar/stellaris-plugins/main/plugins.jsonl";

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

/// One index, read now.
pub fn fetch(url: &str) -> Result<Vec<Entry>> {
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
    let bytes = net::http_get(&e.download, 120_000, 512 << 20).with_context(|| format!("cannot download {}", e.download))?;
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(&e.sha256) {
        bail!("the download of {} does not match the index's SHA-256 (expected {}, got {got})", e.id, e.sha256);
    }
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
}
