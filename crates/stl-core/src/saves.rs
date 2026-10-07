//! The newest save game, for the Continue button to say what it will continue. The game finds the save itself (`--continuelastsave`); this only
//! looks: `<data folder>/save games/<empire>/<name>.sav`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

#[derive(Debug, Clone, PartialEq)]
pub struct Save {
    /// the file name without `.sav`
    pub name: String,
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// The most recently written `.sav` under `save games`, if there is one.
pub fn latest(data_dir: &Path) -> Option<Save> {
    let mut best: Option<Save> = None;
    for empire in std::fs::read_dir(data_dir.join("save games")).ok()?.flatten() {
        let Ok(files) = std::fs::read_dir(empire.path()) else { continue };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().map_or(true, |e| !e.eq_ignore_ascii_case("sav")) {
                continue;
            }
            let Ok(modified) = f.metadata().and_then(|m| m.modified()) else { continue };
            if best.as_ref().map_or(true, |b| modified > b.modified) {
                best = Some(Save { name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), path, modified });
            }
        }
    }
    best
}

/// A time as the user's clock shows it: `(year, month, day, hour, minute)`.
pub fn local_time(t: SystemTime) -> Option<(u16, u16, u16, u16, u16)> {
    let ticks = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos() / 100 + 116_444_736_000_000_000u128;
    let ft = FILETIME { dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return None;
        }
    }
    Some((local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute))
}

/// Seconds since 1970 as the user's clock shows them, `2026-10-07 14:05` (empty for 0 or a time that cannot be shown).
pub fn local_time_string(unix_secs: i64) -> String {
    if unix_secs <= 0 {
        return String::new();
    }
    match local_time(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(unix_secs as u64)) {
        Some((y, mo, d, h, mi)) => format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn finds_the_newest_save_in_any_empire_folder() {
        let dir = std::env::temp_dir().join(format!("stl-test-saves-{}", std::process::id()));
        for (empire, file, age) in [("a_1", "old.sav", 500), ("b_2", "new.sav", 10), ("b_2", "notes.txt", 1)] {
            std::fs::create_dir_all(dir.join("save games").join(empire)).unwrap();
            let p = dir.join("save games").join(empire).join(file);
            std::fs::write(&p, b"x").unwrap();
            let when = SystemTime::now() - Duration::from_secs(age);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(when).unwrap();
        }
        assert_eq!(latest(&dir).map(|s| s.name), Some("new".to_string()));
        assert_eq!(latest(&dir.join("nowhere")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_time_is_a_plausible_date() {
        let (y, mo, d, h, mi) = local_time(SystemTime::now()).unwrap();
        assert!(y >= 2024 && (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60);
        assert!(local_time(SystemTime::UNIX_EPOCH - Duration::from_secs(1)).is_none());
    }
}
