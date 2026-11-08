//! The launcher updating itself from its mirror, or from its GitHub releases when the mirror cannot be reached.
//!
//! 1. `check` reads the mirror's `latest.json` (`MIRROR`, Cloudflare R2). It holds the release's manifest and an Ed25519 signature of it,
//!    which must verify against `MIRROR_KEY`: the private half stays on the release machine (`scripts/publish_mirror.py`), so whoever
//!    can write to the bucket still cannot ship an update. A mirror that cannot be read or whose manifest does not verify is passed over
//!    for GitHub's latest release (pre-releases are not offered). Either way the version is compared with this build's.
//! 2. `stage` downloads the zip (`stellaris-launcher-v<version>.zip`), checks it against the SHA-256 in the signed manifest, or for a
//!    GitHub release the `.zip.sha256` published beside it (an update without one is refused), and unpacks it into
//!    `%APPDATA%\stellaris-launcher\update\<version>\`. The running launcher is not touched.
//! 3. `install` puts the staged files next to the running exe. Windows does not let a running exe be overwritten but does let it be renamed:
//!    each file being replaced is first renamed to `<name>.old`, then the new one is copied in; if a copy fails, everything is put back.
//!    The caller then starts the new exe and exits; the next start removes the `.old` files (`cleanup`).
//!
//! A build run from a cargo `target\` folder is never replaced (set `STL_UPDATE_FORCE=1` to test). `STL_UPDATE_MIRROR` (a URL, or `off`),
//! `STL_UPDATE_REPO` and `STL_UPDATE_API` point the check somewhere else (a local server standing in for the mirror or GitHub).

use crate::updates::{self, Release};
use crate::{anyhow, bail, net, paths, signed, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where the releases are published.
pub const REPO: &str = "Yidhar/stellaris-launcher";
/// The launcher's own copy of each release: readable by anyone, written only when releasing.
pub const MIRROR: &str = "https://dl.stellarismod.com/launcher";
/// The public key the mirror's manifest must be signed with (`scripts/update_key.py --public`).
const MIRROR_KEY: &str = "7f7d933edf9acc4354e06350e99e80ab9f31d2aae6fd723ab3f63c1eec8a0c51";
/// What a manifest names itself, so that one signed for something else is not taken for the launcher's.
const PRODUCT: &str = "stellaris-launcher";

/// The mirror, unless `STL_UPDATE_MIRROR=off`.
pub fn mirror() -> Option<String> {
    match std::env::var("STL_UPDATE_MIRROR") {
        Ok(s) if s.trim().eq_ignore_ascii_case("off") => None,
        Ok(s) if !s.trim().is_empty() => Some(s.trim().trim_end_matches('/').to_string()),
        _ => Some(MIRROR.to_string()),
    }
}

/// The mirror's host, for telling the user where updates come from.
pub fn mirror_host() -> String {
    mirror().map(|m| m.split("://").nth(1).unwrap_or(&m).split('/').next().unwrap_or("").to_string()).unwrap_or_else(|| "-".into())
}
/// The files of a release that make up the program (the rest of the zip is documentation and examples, copied when present).
const PROGRAM: [&str; 2] = ["stellaris-launcher.exe", "stl.exe"];

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn repo() -> String {
    std::env::var("STL_UPDATE_REPO").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| REPO.to_string())
}

fn api() -> String {
    std::env::var("STL_UPDATE_API").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://api.github.com".into()).trim_end_matches('/').to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub release: Release,
    pub zip: (String, String),
    pub checksum: Checksum,
}

/// What the zip is checked against.
#[derive(Debug, Clone, PartialEq)]
pub enum Checksum {
    /// a `.sha256` file to download (name, URL)
    File(String, String),
    /// the SHA-256 in the mirror's signed manifest
    Signed(String),
}

/// The newer release, if there is one: from the mirror, or from GitHub when the mirror fails.
pub fn check() -> Result<Option<Available>> {
    let Some(base) = mirror() else { return check_github() };
    match check_mirror(&base) {
        Ok(found) => Ok(found),
        Err(m) => check_github().map_err(|g| anyhow!("{g:#} (and the mirror {base}: {m:#})")),
    }
}

fn check_mirror(base: &str) -> Result<Option<Available>> {
    let body = net::http_get(&format!("{base}/latest.json"), 15_000, 1 << 20).with_context(|| format!("cannot read {base}/latest.json"))?;
    let key = signed::key(MIRROR_KEY).context("MIRROR_KEY")?;
    let a = read_mirror(&String::from_utf8_lossy(&body), base, &key)?;
    if last_installed().is_some_and(|v| v == a.release.version) || !updates::newer(&a.release.version, current_version()) {
        return Ok(None);
    }
    Ok(Some(a))
}

/// The release in a mirror's `latest.json`, once its signature verifies against `key`.
fn read_mirror(text: &str, base: &str, key: &ed25519_dalek::VerifyingKey) -> Result<Available> {
    #[derive(serde::Deserialize)]
    struct File {
        path: String,
        sha256: String,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        product: String,
        version: String,
        tag: String,
        #[serde(default)]
        page: String,
        #[serde(default)]
        notes: String,
        zip: File,
    }
    let manifest = signed::open(text, key, "the mirror's latest.json")?;
    let m: Manifest = serde_json::from_str(&manifest).context("the mirror's manifest cannot be read")?;
    if m.product != PRODUCT {
        bail!("the mirror's manifest is for {}, not {PRODUCT}", m.product);
    }
    if m.tag != format!("v{}", m.version) {
        bail!("the mirror's manifest says version {} but tag {}", m.version, m.tag);
    }
    let path = m.zip.path.trim();
    let name = path.rsplit('/').next().unwrap_or("").to_string();
    let safe = !path.is_empty() && !path.starts_with('/') && !path.contains(['\\', ':', '?', '#']) && !path.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    if !safe || !updates::wildcard("stellaris-launcher-*.zip", &name) {
        bail!("the mirror's manifest names an unexpected file ({path})");
    }
    let sha = m.zip.sha256.trim().to_lowercase();
    if signed::hex::<32>(&sha).is_none() {
        bail!("the mirror's manifest has no valid SHA-256 for {name}");
    }
    let release = Release { tag: m.tag, version: m.version, page: m.page, notes: m.notes, prerelease: false, assets: Vec::new() };
    Ok(Available { release, zip: (name, format!("{}/{path}", base.trim_end_matches('/'))), checksum: Checksum::Signed(sha) })
}

fn check_github() -> Result<Option<Available>> {
    let repo = repo();
    let body = match net::http_get(&format!("{}/repos/{repo}/releases/latest", api()), 15_000, 2 << 20) {
        Ok(b) => b,
        // GitHub answers 404 both for a repository that does not exist (or is private) and for one without a release
        Err(e) if format!("{e:#}").contains("404") => bail!("github.com/{repo} has no published release (or cannot be seen)"),
        Err(e) => return Err(e).with_context(|| format!("cannot ask GitHub about {repo}")),
    };
    let release = updates::parse_release(&String::from_utf8_lossy(&body))?;
    // a release installed before whose program still calls itself older (its tag and its build disagree) is not offered again,
    // or every start would install it once more
    if last_installed().is_some_and(|v| v == release.version) {
        return Ok(None);
    }
    pick(release, current_version())
}

fn installed_marker() -> Option<PathBuf> {
    staging_root().ok().map(|r| r.join("installed.txt"))
}

/// The version the last update installed.
pub fn last_installed() -> Option<String> {
    std::fs::read_to_string(installed_marker()?).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn pick(release: Release, installed: &str) -> Result<Option<Available>> {
    if release.prerelease || !updates::newer(&release.version, installed) {
        return Ok(None);
    }
    let zip = release.assets.iter().find(|(n, _)| updates::wildcard("stellaris-launcher-*.zip", n)).cloned()
        .with_context(|| format!("release {} has no stellaris-launcher-*.zip", release.tag))?;
    let checksum = release.assets.iter().find(|(n, _)| n.eq_ignore_ascii_case(&format!("{}.sha256", zip.0))).map(|(n, u)| Checksum::File(n.clone(), u.clone()))
        .with_context(|| format!("release {} publishes no {}.sha256, so its download cannot be checked", release.tag, zip.0))?;
    Ok(Some(Available { release, zip, checksum }))
}

pub fn staging_root() -> Result<PathBuf> {
    Ok(paths::app_data_dir()?.join("update"))
}

/// A release unpacked and waiting to be installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Staged {
    pub version: String,
    /// the folder holding `stellaris-launcher.exe`
    pub dir: PathBuf,
    pub notes: String,
    pub page: String,
}

/// The folder in an unpacked release that holds the program: the root, or a single folder inside it.
fn program_root(dir: &Path) -> Option<PathBuf> {
    if dir.join(PROGRAM[0]).is_file() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.join(PROGRAM[0]).is_file())
}

/// Downloads, checks and unpacks a release. Safe to run while the launcher runs; nothing next to the exe changes.
pub fn stage(a: &Available) -> Result<Staged> {
    let bytes = net::http_get(&a.zip.1, 120_000, 256 << 20).with_context(|| format!("cannot download {}", a.zip.0))?;
    let want = match &a.checksum {
        Checksum::Signed(h) => h.clone(),
        Checksum::File(name, url) => {
            let text = String::from_utf8_lossy(&net::http_get(url, 15_000, 1 << 16).with_context(|| format!("cannot download {name}"))?).to_string();
            text.split_whitespace().next().unwrap_or("").to_lowercase()
        }
    };
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if want.is_empty() || want != got {
        bail!("{} does not match its published SHA-256 (expected {want}, got {got})", a.zip.0);
    }
    let root = staging_root()?;
    let dir = root.join(&a.release.version);
    let tmp = root.join(format!(".{}.part", a.release.version));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    if let Err(e) = updates::unzip(&bytes, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if program_root(&tmp).is_none() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("{} holds no {}", a.zip.0, PROGRAM[0]);
    }
    std::fs::write(tmp.join("release.json"), serde_json::json!({ "version": a.release.version, "notes": a.release.notes, "page": a.release.page }).to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).context("cannot finish the staged update")?;
    staged_in(&dir).context("the staged update cannot be read back")
}

fn staged_in(dir: &Path) -> Option<Staged> {
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("release.json")).ok()?).ok()?;
    Some(Staged {
        version: meta["version"].as_str()?.to_string(),
        dir: program_root(dir)?,
        notes: meta["notes"].as_str().unwrap_or("").to_string(),
        page: meta["page"].as_str().unwrap_or("").to_string(),
    })
}

/// The newest staged release that is newer than this build.
pub fn staged() -> Option<Staged> {
    let root = staging_root().ok()?;
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| staged_in(&e.path()))
        .filter(|s| updates::newer(&s.version, current_version()) && last_installed().as_deref() != Some(s.version.as_str()))
        .max_by(|a, b| if updates::newer(&a.version, &b.version) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less })
}

/// The folder of the running exe, if it is one we may replace.
pub fn install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("the exe has no folder")?.to_path_buf();
    let s = dir.to_string_lossy().to_lowercase().replace('/', "\\");
    if (s.contains("\\target\\debug") || s.contains("\\target\\release")) && std::env::var("STL_UPDATE_FORCE").ok().as_deref() != Some("1") {
        bail!("this is a development build ({}); it is not updated", dir.display());
    }
    Ok(dir)
}

/// Every file of the staged release, as (path inside it, path in the install folder).
fn files(from: &Path, to: &Path) -> Vec<(PathBuf, PathBuf)> {
    fn walk(base: &Path, dir: &Path, to: &Path, out: &mut Vec<(PathBuf, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(rel) = p.strip_prefix(base) else { continue };
            if rel == Path::new("release.json") {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, to, out);
            } else {
                out.push((p.clone(), to.join(rel)));
            }
        }
    }
    let mut out = Vec::new();
    walk(from, from, to, &mut out);
    out
}

fn old_name(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Puts a staged release into `to` (the folder of the running exe). On success the staged copy is removed; on failure `to` is as it was.
pub fn install_into(s: &Staged, to: &Path) -> Result<()> {
    let list = files(&s.dir, to);
    if !list.iter().any(|(_, d)| d.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PROGRAM[0]))) {
        bail!("the staged update has no {}", PROGRAM[0]);
    }
    // (target, whether an old file was moved aside)
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let mut result = Ok(());
    for (src, dst) in &list {
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let old = old_name(dst);
        let moved = if dst.exists() {
            let _ = std::fs::remove_file(&old);
            if let Err(e) = std::fs::rename(dst, &old) {
                result = Err(e).with_context(|| format!("cannot move {} aside", dst.display()));
                break;
            }
            true
        } else {
            false
        };
        done.push((dst.clone(), moved));
        if let Err(e) = std::fs::copy(src, dst) {
            result = Err(e).with_context(|| format!("cannot write {}", dst.display()));
            break;
        }
    }
    if result.is_err() {
        for (dst, moved) in done.iter().rev() {
            let _ = std::fs::remove_file(dst);
            if *moved {
                let _ = std::fs::rename(old_name(dst), dst);
            }
        }
        return result;
    }
    if let Some(release_dir) = s.dir.ancestors().find(|a| a.parent().map(|p| p.file_name().is_some_and(|n| n == "update")).unwrap_or(false)) {
        let _ = std::fs::remove_dir_all(release_dir);
    }
    Ok(())
}

/// Installs a staged release next to the running exe.
pub fn install(s: &Staged) -> Result<PathBuf> {
    let dir = install_dir()?;
    install_into(s, &dir)?;
    if let Some(m) = installed_marker() {
        let _ = std::fs::write(m, &s.version);
    }
    Ok(dir.join(PROGRAM[0]))
}

/// After an update: removes the `.old` files next to the exe and staged releases that are not newer than this build.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            remove_old(dir);
        }
    }
    if let Ok(root) = staging_root() {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                if e.path().is_file() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_string();
                let stale = name.starts_with('.') || staged_in(&e.path()).map_or(true, |s| !updates::newer(&s.version, current_version()) || last_installed().as_deref() == Some(s.version.as_str()));
                if stale {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
}

fn remove_old(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_old(&p);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("old")) {
            // still in use by an instance that has not exited yet: try again next time
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str], pre: bool) -> Release {
        Release {
            tag: tag.into(),
            version: tag.trim_start_matches('v').into(),
            page: String::new(),
            notes: String::new(),
            prerelease: pre,
            assets: assets.iter().map(|n| (n.to_string(), format!("https://x/{n}"))).collect(),
        }
    }

    #[test]
    fn offers_only_a_newer_checked_release() {
        let zip = "stellaris-launcher-v9.0.0.zip";
        let sha = "stellaris-launcher-v9.0.0.zip.sha256";
        assert!(pick(release("v9.0.0", &[zip, sha], false), "0.1.0").unwrap().is_some());
        assert!(pick(release("v0.1.0", &[zip, sha], false), "0.1.0").unwrap().is_none(), "the same version");
        assert!(pick(release("v9.0.0", &[zip, sha], true), "0.1.0").unwrap().is_none(), "a pre-release");
        assert!(pick(release("v9.0.0", &[zip], false), "0.1.0").is_err(), "no checksum");
        assert!(pick(release("v9.0.0", &["other.zip"], false), "0.1.0").is_err(), "no zip");
    }

    use crate::signed::sign as signed;

    fn manifest(product: &str, tag: &str, path: &str) -> String {
        serde_json::json!({
            "product": product, "version": tag.trim_start_matches('v'), "tag": tag, "page": "https://x/p", "notes": "n",
            "zip": { "path": path, "sha256": "AB".repeat(32), "size": 1 },
        })
        .to_string()
    }

    #[test]
    fn the_release_key_is_a_valid_public_key() {
        assert!(signed::key(MIRROR_KEY).is_ok());
    }

    #[test]
    fn takes_only_a_mirror_manifest_signed_with_the_release_key() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let other = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
        let public = key.verifying_key();
        let good = manifest(PRODUCT, "v9.0.0", "v9.0.0/stellaris-launcher-v9.0.0.zip");

        let a = read_mirror(&signed(&key, &good), "https://m/launcher/", &public).unwrap();
        assert_eq!(a.release.version, "9.0.0");
        assert_eq!(a.zip, ("stellaris-launcher-v9.0.0.zip".to_string(), "https://m/launcher/v9.0.0/stellaris-launcher-v9.0.0.zip".to_string()));
        assert_eq!(a.checksum, Checksum::Signed("ab".repeat(32)));

        assert!(read_mirror(&signed(&other, &good), "https://m", &public).is_err(), "another key");
        let tampered = signed(&key, &good).replace("9.0.0", "9.0.1");
        assert!(read_mirror(&tampered, "https://m", &public).is_err(), "changed after signing");
        assert!(read_mirror(&good, "https://m", &public).is_err(), "not signed at all");
        assert!(read_mirror(&signed(&key, &manifest("something-else", "v9.0.0", "v9.0.0/stellaris-launcher-v9.0.0.zip")), "https://m", &public).is_err(), "another product");
        for path in ["../x/stellaris-launcher-v9.0.0.zip", "/v9/stellaris-launcher-v9.0.0.zip", "https://evil/stellaris-launcher-v9.0.0.zip", "v9.0.0/other.zip", "v9.0.0//stellaris-launcher-v9.0.0.zip"] {
            assert!(read_mirror(&signed(&key, &manifest(PRODUCT, "v9.0.0", path)), "https://m", &public).is_err(), "{path}");
        }
        let mismatched = manifest(PRODUCT, "v9.0.0", "v9.0.0/stellaris-launcher-v9.0.0.zip").replace("\"tag\":\"v9.0.0\"", "\"tag\":\"v9.0.1\"");
        assert!(read_mirror(&signed(&key, &mismatched), "https://m", &public).is_err(), "tag and version disagree");
    }

    #[test]
    fn installs_by_moving_the_running_files_aside_and_rolls_back() {
        let base = std::env::temp_dir().join(format!("stl-test-selfupdate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let staged_dir = base.join("update").join("9.0.0").join("stellaris-launcher-v9.0.0");
        std::fs::create_dir_all(staged_dir.join("docs")).unwrap();
        std::fs::write(staged_dir.join("stellaris-launcher.exe"), "new gui").unwrap();
        std::fs::write(staged_dir.join("stl.exe"), "new cli").unwrap();
        std::fs::write(staged_dir.join("docs").join("DESIGN.md"), "new doc").unwrap();
        let install = base.join("app");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("stellaris-launcher.exe"), "old gui").unwrap();
        std::fs::write(install.join("stl.exe"), "old cli").unwrap();
        let s = Staged { version: "9.0.0".into(), dir: staged_dir.clone(), notes: String::new(), page: String::new() };

        // a file that cannot be written (here: a file where its folder should be) puts everything back
        std::fs::write(install.join("docs"), "in the way").unwrap();
        assert!(install_into(&s, &install).is_err());
        assert_eq!(std::fs::read_to_string(install.join("stellaris-launcher.exe")).unwrap(), "old gui");
        assert_eq!(std::fs::read_to_string(install.join("stl.exe")).unwrap(), "old cli");
        assert!(!install.join("stl.exe.old").exists());
        std::fs::remove_file(install.join("docs")).unwrap();

        install_into(&s, &install).unwrap();
        assert_eq!(std::fs::read_to_string(install.join("stellaris-launcher.exe")).unwrap(), "new gui");
        assert_eq!(std::fs::read_to_string(install.join("stellaris-launcher.exe.old")).unwrap(), "old gui");
        assert_eq!(std::fs::read_to_string(install.join("docs").join("DESIGN.md")).unwrap(), "new doc");
        assert!(!base.join("update").join("9.0.0").exists(), "the staged copy is removed");
        remove_old(&install);
        assert!(!install.join("stellaris-launcher.exe.old").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
