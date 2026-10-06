//! Plugin updates from GitHub releases. A manifest names its repository (`"update": {"github": "owner/repo", "asset": "name-*.zip"}`); the
//! launcher asks GitHub's public API for the latest release, compares its version with the installed one, and on request downloads the zip,
//! checks it against a `<asset>.sha256` published beside it (when there is one), and installs it like any plugin folder (keeping `config\`).

use crate::plugins::{self, Plugin};
use crate::{bail, net, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    /// the tag without a leading `v`
    pub version: String,
    pub page: String,
    /// the release notes (Markdown)
    pub notes: String,
    pub prerelease: bool,
    pub assets: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub release: Release,
    /// the zip to download, and the checksum file beside it if any
    pub asset: (String, String),
    pub checksum: Option<String>,
}

/// `*` matches any run of characters; nothing else is special.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern.eq_ignore_ascii_case(name);
    }
    let name_l = name.to_lowercase();
    let mut rest = name_l.as_str();
    for (i, part) in parts.iter().enumerate() {
        let part = part.to_lowercase();
        if i == 0 {
            if !rest.starts_with(&part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(&part);
        } else {
            match rest.find(&part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// Compares dotted versions number by number (`0.10.0` > `0.9.2`; a leading `v` and anything after `-` or `+` are ignored).
pub fn newer(candidate: &str, installed: &str) -> bool {
    let nums = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
    };
    let (a, b) = (nums(candidate), nums(installed));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

pub fn parse_release(json: &str) -> Result<Release> {
    let d: Value = serde_json::from_str(json).context("GitHub's answer is not JSON")?;
    let tag = d["tag_name"].as_str().context("the release has no tag")?.to_string();
    let assets = d["assets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: d["html_url"].as_str().unwrap_or("").to_string(),
        notes: d["body"].as_str().unwrap_or("").to_string(),
        prerelease: d["prerelease"].as_bool().unwrap_or(false),
        tag,
        assets,
    })
}

/// `owner/repo` from what a manifest may hold (`owner/repo`, or a github.com URL).
pub fn repo_of(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let s = s.strip_prefix("https://github.com/").or_else(|| s.strip_prefix("http://github.com/")).or_else(|| s.strip_prefix("github.com/")).unwrap_or(s);
    let mut parts = s.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    (!o.is_empty() && !r.is_empty()).then(|| format!("{o}/{r}"))
}

pub fn latest_release(repo: &str) -> Result<Release> {
    let body = net::http_get(&format!("https://api.github.com/repos/{repo}/releases/latest"), 15_000, 2 << 20).with_context(|| format!("no latest release of {repo}"))?;
    parse_release(&String::from_utf8_lossy(&body))
}

/// The newer release of a plugin, if there is one. None when it is up to date.
pub fn check(p: &Plugin) -> Result<Option<Available>> {
    let spec = p.manifest.update.as_ref().context("the plugin names no repository to update from")?;
    let repo = repo_of(&spec.github).with_context(|| format!("{:?} is not a GitHub repository", spec.github))?;
    let release = latest_release(&repo)?;
    if !newer(&release.version, &p.manifest.version) {
        return Ok(None);
    }
    let pattern = spec.asset.clone().unwrap_or_else(|| "*.zip".into());
    let asset = release.assets.iter().find(|(n, _)| n.to_lowercase().ends_with(".zip") && wildcard(&pattern, n)).cloned()
        .with_context(|| format!("release {} of {repo} has no zip matching {pattern}", release.tag))?;
    let checksum = release.assets.iter().find(|(n, _)| n.eq_ignore_ascii_case(&format!("{}.sha256", asset.0))).map(|(_, u)| u.clone());
    Ok(Some(Available { release, asset, checksum }))
}

/// The folder in an unpacked zip that holds `stl-plugin.json`: the root, or a single folder inside it.
fn plugin_root(dir: &Path) -> Option<PathBuf> {
    if dir.join(plugins::MANIFEST).is_file() {
        return Some(dir.to_path_buf());
    }
    let subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.into_iter().find(|s| s.join(plugins::MANIFEST).is_file())
}

pub fn unzip(bytes: &[u8], to: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("the download is not a zip")?;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { bail!("the zip holds an unsafe path") };
        let out = to.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut f, &mut w)?;
        }
    }
    Ok(())
}

/// Downloads, checks and installs an update. A linked plugin (under development) is not updated.
pub fn apply(p: &Plugin, a: &Available) -> Result<Plugin> {
    if p.linked {
        bail!("{} is linked to a folder under development; update it there", p.manifest.id);
    }
    let bytes = net::http_get(&a.asset.1, 60_000, 512 << 20).with_context(|| format!("cannot download {}", a.asset.0))?;
    if let Some(url) = &a.checksum {
        let text = String::from_utf8_lossy(&net::http_get(url, 15_000, 1 << 16)?).to_string();
        let want = text.split_whitespace().next().unwrap_or("").to_lowercase();
        let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if want != got {
            bail!("{} does not match its published SHA-256 (expected {want}, got {got})", a.asset.0);
        }
    }
    let tmp = std::env::temp_dir().join(format!("stl-update-{}-{}", p.manifest.id, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| {
        unzip(&bytes, &tmp)?;
        let root = plugin_root(&tmp).with_context(|| format!("{} holds no {}", a.asset.0, plugins::MANIFEST))?;
        let m = plugins::parse_manifest(&std::fs::read_to_string(root.join(plugins::MANIFEST))?)?;
        if m.id != p.manifest.id {
            bail!("{} is the plugin {:?}, not {:?}", a.asset.0, m.id, p.manifest.id);
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
    fn versions_and_patterns() {
        assert!(newer("0.10.0", "0.9.2"));
        assert!(newer("v1.0", "0.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.2.0-rc1", "0.2.0"));
        assert!(newer("0.2.1", "0.2"));
        assert!(wildcard("stellaris-live2d-*.zip", "stellaris-live2d-v0.2.0.zip"));
        assert!(!wildcard("stellaris-live2d-*.zip", "stellaris-live2d-v0.2.0.zip.sha256"));
        assert!(wildcard("*.zip", "a.ZIP"));
        assert_eq!(repo_of("https://github.com/Yidhar/stellaris-live2d/"), Some("Yidhar/stellaris-live2d".into()));
        assert_eq!(repo_of("Yidhar/stellaris-live2d.git"), Some("Yidhar/stellaris-live2d".into()));
        assert_eq!(repo_of("nope"), None);
    }

    #[test]
    fn reads_a_release() {
        let r = parse_release(r#"{"tag_name":"v0.2.0","html_url":"https://github.com/o/r/releases/tag/v0.2.0",
            "assets":[{"name":"p-v0.2.0.zip","browser_download_url":"https://x/p.zip"},{"name":"p-v0.2.0.zip.sha256","browser_download_url":"https://x/p.sha"}]}"#).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.assets.len(), 2);
    }

    #[test]
    fn unpacks_and_finds_the_plugin_folder() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("p/stl-plugin.json", o).unwrap();
            z.write_all(br#"{"id":"p","name":"P","dll":"p.dll"}"#).unwrap();
            z.start_file("p/p.dll", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("stl-test-unzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unzip(buf.get_ref(), &dir).unwrap();
        assert_eq!(plugin_root(&dir), Some(dir.join("p")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
