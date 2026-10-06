//! The launcher updating itself from its GitHub releases.
//!
//! 1. `check` asks GitHub for the latest release (pre-releases are not offered) and compares its tag with this build's version.
//! 2. `stage` downloads its zip (`stellaris-launcher-v<version>.zip`), checks it against the `.zip.sha256` published beside it (an update
//!    without one is refused) and unpacks it into `%APPDATA%\stellaris-launcher\update\<version>\`. The running launcher is not touched.
//! 3. `install` puts the staged files next to the running exe. Windows does not let a running exe be overwritten but does let it be renamed:
//!    each file being replaced is first renamed to `<name>.old`, then the new one is copied in; if a copy fails, everything is put back.
//!    The caller then starts the new exe and exits; the next start removes the `.old` files (`cleanup`).
//!
//! A build run from a cargo `target\` folder is never replaced (set `STL_UPDATE_FORCE=1` to test). `STL_UPDATE_REPO` and `STL_UPDATE_API`
//! point the check somewhere else (another repository, or a local server standing in for GitHub).

use crate::updates::{self, Release};
use crate::{bail, net, paths, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where the releases are published.
pub const REPO: &str = "Yidhar/stellaris-launcher";
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
    pub checksum: (String, String),
}

/// The newer release, if there is one.
pub fn check() -> Result<Option<Available>> {
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
    let checksum = release.assets.iter().find(|(n, _)| n.eq_ignore_ascii_case(&format!("{}.sha256", zip.0))).cloned()
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
    let text = String::from_utf8_lossy(&net::http_get(&a.checksum.1, 15_000, 1 << 16).with_context(|| format!("cannot download {}", a.checksum.0))?).to_string();
    let want = text.split_whitespace().next().unwrap_or("").to_lowercase();
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if want != got {
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
