//! The launcher as an installed Windows program: its entry in "Apps & features" and in App Paths (Win+R `stellaris-launcher`), for the
//! current user only (`HKEY_CURRENT_USER`, no administrator rights).
//!
//! - Installed with the setup program (Inno Setup, `installer/stellaris-launcher.iss`): the setup writes the entry
//!   (`…\Uninstall\StellarisLauncher_is1`) and brings its own uninstaller; the launcher only keeps the version shown there current after a
//!   self-update.
//! - Unpacked from the zip: the first start writes an entry of its own (`…\Uninstall\StellarisLauncher`) whose uninstall command is
//!   `stellaris-launcher.exe --uninstall`; later starts refresh it when the folder or the version changed.
//!
//! A build run from a cargo `target\` folder is never registered.

use crate::{bail, Context, Result};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
/// the setup's entry (Inno Setup names it after the AppId, plus `_is1`)
pub const SETUP_KEY: &str = "StellarisLauncher_is1";
/// the entry the zip version writes for itself
pub const OWN_KEY: &str = "StellarisLauncher";
const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\stellaris-launcher.exe";
pub const HOMEPAGE: &str = "https://github.com/Yidhar/stellaris-launcher";
/// The files a release brings next to the exe: what the zip version's uninstall removes (nothing else in that folder is touched).
pub const SHIPPED: &[&str] = &["stellaris-launcher.exe", "stl.exe", "README.md", "LICENSE", "docs", "examples"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read(key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 2048];
    let mut bytes = (buf.len() * 2) as u32;
    let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = (bytes as usize / 2).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn create(path: &str) -> Result<Key> {
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, wide(path).as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut()) };
        if rc != ERROR_SUCCESS {
            bail!("cannot write HKCU\\{path} (error {rc})");
        }
        Ok(Key(h))
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        let data = wide(value);
        let rc = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        if rc != ERROR_SUCCESS {
            bail!("cannot write the registry value {name} (error {rc})");
        }
        Ok(())
    }

    fn set_dword(&self, name: &str, value: u32) -> Result<()> {
        let rc = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_DWORD, (&value as *const u32).cast(), 4) };
        if rc != ERROR_SUCCESS {
            bail!("cannot write the registry value {name} (error {rc})");
        }
        Ok(())
    }
}

fn delete_tree(path: &str) {
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(path).as_ptr()) };
}

fn same_dir(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}

/// The folder of the running exe, unless it is a development build.
fn program_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let s = dir.to_string_lossy().to_lowercase().replace('/', "\\");
    if s.contains("\\target\\debug") || s.contains("\\target\\release") || s.contains("\\target\\x86_64") {
        return None;
    }
    Some(dir)
}

/// Was this copy installed by the setup program?
pub fn installed_by_setup() -> bool {
    let Some(dir) = program_dir() else { return false };
    read(&format!(r"{UNINSTALL}\{SETUP_KEY}"), "InstallLocation").is_some_and(|l| same_dir(&l, &dir))
}

fn size_kb(dir: &Path) -> u32 {
    let mut total = 0u64;
    for name in SHIPPED {
        let p = dir.join(name);
        if let Ok(m) = std::fs::metadata(&p) {
            if m.is_file() {
                total += m.len();
            } else {
                total += walk_size(&p);
            }
        }
    }
    (total / 1024) as u32
}

fn walk_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_dir() => walk_size(&e.path()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }).sum()).unwrap_or(0)
}

/// What the start of the launcher does: registers a zip copy (or refreshes its entry when the folder or the version changed); for a copy the
/// setup installed, keeps the version in the setup's entry current. Returns whether anything was written.
pub fn register(version: &str) -> Result<bool> {
    let Some(dir) = program_dir() else { return Ok(false) };
    let exe = dir.join("stellaris-launcher.exe");
    if installed_by_setup() {
        let key = format!(r"{UNINSTALL}\{SETUP_KEY}");
        if read(&key, "DisplayVersion").as_deref() == Some(version) {
            return Ok(false);
        }
        Key::create(&key)?.set("DisplayVersion", version)?;
        return Ok(true);
    }
    let key = format!(r"{UNINSTALL}\{OWN_KEY}");
    let current = read(&key, "InstallLocation").is_some_and(|l| same_dir(&l, &dir)) && read(&key, "DisplayVersion").as_deref() == Some(version);
    if current {
        return Ok(false);
    }
    let exe_s = exe.to_string_lossy().to_string();
    let k = Key::create(&key)?;
    k.set("DisplayName", "Stellaris Launcher")?;
    k.set("DisplayVersion", version)?;
    k.set("Publisher", "Yidhar")?;
    k.set("DisplayIcon", &exe_s)?;
    k.set("InstallLocation", &dir.to_string_lossy())?;
    k.set("UninstallString", &format!("\"{exe_s}\" --uninstall"))?;
    k.set("QuietUninstallString", &format!("\"{exe_s}\" --uninstall --quiet"))?;
    k.set("URLInfoAbout", HOMEPAGE)?;
    k.set("HelpLink", HOMEPAGE)?;
    k.set("InstallDate", &today())?;
    k.set_dword("NoModify", 1)?;
    k.set_dword("NoRepair", 1)?;
    k.set_dword("EstimatedSize", size_kb(&dir))?;
    let a = Key::create(APP_PATHS)?;
    a.set("", &exe_s)?;
    a.set("Path", &dir.to_string_lossy())?;
    Ok(true)
}

fn today() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

/// Removes the zip copy's registry entries.
pub fn unregister() {
    delete_tree(&format!(r"{UNINSTALL}\{OWN_KEY}"));
    if read(APP_PATHS, "Path").is_some_and(|p| program_dir().is_some_and(|d| same_dir(&p, &d))) {
        delete_tree(APP_PATHS);
    }
}

/// The setup's own uninstaller, for a copy the setup installed (`--uninstall` hands over to it).
pub fn setup_uninstaller() -> Option<PathBuf> {
    let s = read(&format!(r"{UNINSTALL}\{SETUP_KEY}"), "UninstallString")?;
    Some(PathBuf::from(s.trim().trim_matches('"')))
}

/// Uninstalls the zip copy: removes its registry entries and, once this process has exited, the files a release brings (only those: a
/// folder that holds other files keeps them, and is removed only when that leaves it empty). `with_data` also removes the playsets and
/// settings (`%APPDATA%\stellaris-launcher`).
pub fn uninstall_portable(with_data: bool) -> Result<()> {
    let dir = program_dir().context("a development build is not uninstalled")?;
    unregister();
    if with_data {
        if let Ok(d) = crate::paths::app_data_dir() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
    // the running exe cannot delete itself: a hidden cmd waits for this process to end, then deletes
    let mut script = String::from("@echo off\r\nping -n 3 127.0.0.1 >nul\r\n");
    let d = dir.to_string_lossy();
    for name in SHIPPED {
        let p = dir.join(name);
        if p.is_dir() {
            script.push_str(&format!("rmdir /s /q \"{}\"\r\n", p.display()));
        } else {
            script.push_str(&format!("del /f /q \"{}\"\r\n", p.display()));
        }
    }
    script.push_str(&format!("del /f /q \"{d}\\*.old\" 2>nul\r\n"));
    script.push_str(&format!("rmdir \"{d}\" 2>nul\r\n"));
    script.push_str("del /f /q \"%~f0\"\r\n");
    let bat = std::env::temp_dir().join(format!("stellaris-launcher-uninstall-{}.cmd", std::process::id()));
    std::fs::write(&bat, script)?;
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd").args(["/c", &bat.to_string_lossy()]).creation_flags(0x0800_0000).spawn().context("cannot start the removal")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_folders_loosely() {
        assert!(same_dir(r"C:\Games\Launcher\", Path::new(r"c:\games\launcher")));
        assert!(same_dir("C:/Games/Launcher", Path::new(r"C:\Games\Launcher")));
        assert!(!same_dir(r"C:\Games\Launcher2", Path::new(r"C:\Games\Launcher")));
    }
}
