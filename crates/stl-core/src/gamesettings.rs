//! The game's graphics settings, kept in two files of the data folder, both edited in place (every other line is left as it is):
//!
//! - `pdx_settings.txt`, the one the official launcher edits: `"Graphics"={ "display_mode"={ value="windowed" } "fullscreen_resolution"=…
//!   "windowed_resolution"=… "vsync"={ enabled=yes } "display_index"=… }`
//! - `settings.txt`, the game's own: `graphics={ size={ x= y= } gui_scale=1.000000 refreshRate=60 fullScreen=no borderless=no vsync=yes
//!   multi_sampling=4 maxanisotropy=16 … }` — the interface scale and anti-aliasing live only here.
//!
//! The game reads them at start and writes them when it closes, so a change made while it runs is lost: callers should say so.

use crate::{Context, Result};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Graphics {
    /// `fullscreen`, `borderless_fullscreen` or `windowed`
    pub display_mode: String,
    pub display_index: u32,
    pub fullscreen_resolution: (u32, u32),
    pub windowed_resolution: (u32, u32),
    pub refresh_rate: u32,
    pub vsync: bool,
    /// the interface scale, 1.0 = 100 %
    pub gui_scale: f32,
    /// MSAA samples: 0 (off), 2, 4, 8
    pub multi_sampling: u32,
}

impl Default for Graphics {
    fn default() -> Self {
        Graphics {
            display_mode: "borderless_fullscreen".into(),
            display_index: 0,
            fullscreen_resolution: (1920, 1080),
            windowed_resolution: (1920, 1080),
            refresh_rate: 60,
            vsync: true,
            gui_scale: 1.0,
            multi_sampling: 4,
        }
    }
}

// ------------------------------------------------------------------ a small in-place editor of Paradox script blocks

/// The byte range of the body of `name={ … }` (between the braces) inside `range` of `text`, at the top level of that range.
fn block(text: &str, range: (usize, usize), name: &str) -> Option<(usize, usize)> {
    let body = &text[range.0..range.1];
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        // a name (which may itself be quoted) at the top level, followed by `= {`
        if depth == 0 && body[i..].starts_with(name) && (i == 0 || !is_word(bytes[i - 1])) {
            let after = &body[i + name.len()..];
            let after_trim = after.trim_start();
            if let Some(after_eq) = after_trim.strip_prefix('=').map(str::trim_start) {
                if after_eq.starts_with('{') {
                    let open = range.0 + i + name.len() + (after.len() - after_eq.len());
                    let mut d = 0i32;
                    for (k, c) in text[open..range.1].char_indices() {
                        match c {
                            '{' => d += 1,
                            '}' => {
                                d -= 1;
                                if d == 0 {
                                    return Some((open + 1, open + k));
                                }
                            }
                            _ => {}
                        }
                    }
                    return None;
                }
            }
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'"' => {
                // skip a quoted string
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'"'
}

/// The value of `key=value` at the top level of a block body.
fn get(text: &str, range: (usize, usize), key: &str) -> Option<String> {
    let mut depth = 0;
    for line in text[range.0..range.1].lines() {
        let t = line.trim();
        if depth == 0 {
            if let Some(rest) = t.strip_prefix(key) {
                if let Some(v) = rest.trim_start().strip_prefix('=') {
                    let v = v.trim();
                    if !v.starts_with('{') {
                        return Some(v.trim_matches('"').to_string());
                    }
                }
            }
        }
        depth += t.matches('{').count() as i32 - t.matches('}').count() as i32;
    }
    None
}

/// Sets `key=value` at the top level of the block body `range`: the line is replaced, or added before the closing brace.
fn set(text: &mut String, range: (usize, usize), key: &str, value: &str, indent: &str) {
    let mut depth = 0;
    let mut offset = range.0;
    for line in text[range.0..range.1].split_inclusive('\n') {
        let t = line.trim();
        if depth == 0 {
            if let Some(rest) = t.strip_prefix(key) {
                if let Some(v) = rest.trim_start().strip_prefix('=') {
                    if !v.trim().starts_with('{') {
                        let lead = &line[..line.len() - line.trim_start().len()];
                        let ending = if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" };
                        text.replace_range(offset..offset + line.len(), &format!("{lead}{key}={value}{ending}"));
                        return;
                    }
                }
            }
        }
        depth += t.matches('{').count() as i32 - t.matches('}').count() as i32;
        offset += line.len();
    }
    // not there: add a line before the closing brace
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let insert_at = range.1;
    let before = &text[range.0..insert_at];
    let prefix = if before.ends_with('\n') { "" } else { nl };
    text.insert_str(insert_at, &format!("{prefix}{indent}{key}={value}{nl}"));
}

fn whole(text: &str) -> (usize, usize) {
    (0, text.len())
}

fn parse_res(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

// ------------------------------------------------------------------ the two files

/// The current settings (the defaults for what neither file says).
pub fn read(data_dir: &Path) -> Graphics {
    let mut g = Graphics::default();
    if let Ok(t) = std::fs::read_to_string(data_dir.join("settings.txt")) {
        if let Some(gr) = block(&t, whole(&t), "graphics") {
            if let Some(size) = block(&t, gr, "size") {
                if let (Some(x), Some(y)) = (get(&t, size, "x"), get(&t, size, "y")) {
                    if let (Ok(x), Ok(y)) = (x.parse(), y.parse()) {
                        g.windowed_resolution = (x, y);
                        g.fullscreen_resolution = (x, y);
                    }
                }
            }
            let yes = |k: &str| get(&t, gr, k).map(|v| v == "yes");
            if let Some(v) = get(&t, gr, "gui_scale").and_then(|v| v.parse().ok()) {
                g.gui_scale = v;
            }
            if let Some(v) = get(&t, gr, "refreshRate").and_then(|v| v.parse().ok()) {
                g.refresh_rate = v;
            }
            if let Some(v) = get(&t, gr, "multi_sampling").and_then(|v| v.parse().ok()) {
                g.multi_sampling = v;
            }
            if let Some(v) = get(&t, gr, "display_index").and_then(|v| v.parse().ok()) {
                g.display_index = v;
            }
            if let Some(v) = yes("vsync") {
                g.vsync = v;
            }
            match (yes("fullScreen"), yes("borderless")) {
                (Some(true), Some(true)) => g.display_mode = "borderless_fullscreen".into(),
                (Some(true), _) => g.display_mode = "fullscreen".into(),
                (Some(false), _) => g.display_mode = "windowed".into(),
                _ => {}
            }
        }
    }
    // the launcher's file is the newer word on what both hold
    if let Ok(t) = std::fs::read_to_string(data_dir.join("pdx_settings.txt")) {
        if let Some(gr) = block(&t, whole(&t), "\"Graphics\"") {
            let val = |k: &str| block(&t, gr, &format!("\"{k}\"")).and_then(|b| get(&t, b, "value").or_else(|| get(&t, b, "enabled")));
            if let Some(v) = val("display_mode") {
                g.display_mode = v;
            }
            if let Some(v) = val("display_index").and_then(|v| v.parse().ok()) {
                g.display_index = v;
            }
            if let Some(v) = val("fullscreen_resolution").and_then(|v| parse_res(&v)) {
                g.fullscreen_resolution = v;
            }
            if let Some(v) = val("windowed_resolution").and_then(|v| parse_res(&v)) {
                g.windowed_resolution = v;
            }
            if let Some(v) = val("vsync") {
                g.vsync = v == "yes";
            }
        }
    }
    g
}

fn backup_once(path: &Path) -> Result<()> {
    if path.is_file() {
        let mut b = path.as_os_str().to_owned();
        b.push(".stl_backup");
        let b = std::path::PathBuf::from(b);
        if !b.exists() {
            std::fs::copy(path, &b).with_context(|| format!("cannot back up {}", path.display()))?;
        }
    }
    Ok(())
}

/// Writes the settings into both files (each backed up once as `<file>.stl_backup`), touching only these keys.
pub fn write(data_dir: &Path, g: &Graphics) -> Result<()> {
    let active = if g.display_mode == "windowed" { g.windowed_resolution } else { g.fullscreen_resolution };
    // settings.txt
    let path = data_dir.join("settings.txt");
    let mut t = std::fs::read_to_string(&path).unwrap_or_default();
    if block(&t, whole(&t), "graphics").is_none() {
        let nl = if t.contains("\r\n") { "\r\n" } else { "\n" };
        t.push_str(&format!("graphics={nl}{{{nl}}}{nl}"));
    }
    let gr = block(&t, whole(&t), "graphics").unwrap();
    if block(&t, gr, "size").is_none() {
        set(&mut t, gr, "size", "{\n\t\tx=0\n\t\ty=0\n\t}", "\t");
    }
    let size = block(&t, block(&t, whole(&t), "graphics").unwrap(), "size").unwrap();
    set(&mut t, size, "x", &active.0.to_string(), "\t\t");
    let size = block(&t, block(&t, whole(&t), "graphics").unwrap(), "size").unwrap();
    set(&mut t, size, "y", &active.1.to_string(), "\t\t");
    let yn = |b: bool| if b { "yes" } else { "no" };
    let pairs = [
        ("gui_scale", format!("{:.6}", g.gui_scale)),
        ("refreshRate", g.refresh_rate.to_string()),
        ("fullScreen", yn(g.display_mode != "windowed").to_string()),
        ("borderless", yn(g.display_mode == "borderless_fullscreen").to_string()),
        ("display_index", g.display_index.to_string()),
        ("multi_sampling", g.multi_sampling.to_string()),
        ("vsync", yn(g.vsync).to_string()),
    ];
    for (k, v) in pairs {
        let gr = block(&t, whole(&t), "graphics").unwrap();
        set(&mut t, gr, k, &v, "\t");
    }
    backup_once(&path)?;
    std::fs::write(&path, &t).with_context(|| format!("cannot write {}", path.display()))?;

    // pdx_settings.txt
    let path = data_dir.join("pdx_settings.txt");
    let mut t = std::fs::read_to_string(&path).unwrap_or_default();
    if block(&t, whole(&t), "\"Graphics\"").is_none() {
        let nl = if t.contains("\r\n") { "\r\n" } else { "\n" };
        t.insert_str(0, &format!("\"Graphics\"={nl}{{{nl}}}{nl}"));
    }
    let entries = [
        ("display_mode", "value", format!("\"{}\"", g.display_mode)),
        ("display_index", "value", format!("\"{}\"", g.display_index)),
        ("fullscreen_resolution", "value", format!("\"{}x{}\"", g.fullscreen_resolution.0, g.fullscreen_resolution.1)),
        ("windowed_resolution", "value", format!("\"{}x{}\"", g.windowed_resolution.0, g.windowed_resolution.1)),
        ("vsync", "enabled", yn(g.vsync).to_string()),
    ];
    for (k, field, v) in entries {
        let gr = block(&t, whole(&t), "\"Graphics\"").unwrap();
        let quoted = format!("\"{k}\"");
        if block(&t, gr, &quoted).is_none() {
            set(&mut t, gr, &quoted, &format!("\n\t{{\n\t\t{field}=x\n\t\tversion=0\n\t}}"), "\t");
        }
        let gr = block(&t, whole(&t), "\"Graphics\"").unwrap();
        let entry = block(&t, gr, &quoted).unwrap();
        set(&mut t, entry, field, &v, "\t\t");
    }
    backup_once(&path)?;
    std::fs::write(&path, &t).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

// ------------------------------------------------------------------ the monitors and their modes

#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub name: String,
    /// (width, height, refresh rate) the monitor offers, largest first
    pub modes: Vec<(u32, u32, u32)>,
    pub current: (u32, u32, u32),
}

/// The attached monitors in system order (the game's `display_index` counts them the same way), with the modes each offers.
pub fn displays() -> Vec<Display> {
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS};
    const ATTACHED: u32 = 0x1;
    const PRIMARY: u32 = 0x4;
    let mut out: Vec<(bool, Display)> = Vec::new();
    unsafe {
        let mut i = 0u32;
        loop {
            let mut dev: DISPLAY_DEVICEW = std::mem::zeroed();
            dev.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(std::ptr::null(), i, &mut dev, 0) == 0 {
                break;
            }
            i += 1;
            if dev.StateFlags & ATTACHED == 0 {
                continue;
            }
            let name_w = dev.DeviceName;
            let mut modes = Vec::new();
            let mut m = 0u32;
            loop {
                let mut dm: DEVMODEW = std::mem::zeroed();
                dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
                if EnumDisplaySettingsW(name_w.as_ptr(), m, &mut dm) == 0 {
                    break;
                }
                m += 1;
                if dm.dmBitsPerPel >= 24 && dm.dmPelsWidth >= 1024 {
                    let mode = (dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency);
                    if !modes.contains(&mode) {
                        modes.push(mode);
                    }
                }
            }
            let mut cur: DEVMODEW = std::mem::zeroed();
            cur.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            EnumDisplaySettingsW(name_w.as_ptr(), ENUM_CURRENT_SETTINGS, &mut cur);
            modes.sort_by(|a, b| (b.0 * b.1, b.2).cmp(&(a.0 * a.1, a.2)));
            let label = String::from_utf16_lossy(&dev.DeviceString).trim_end_matches('\0').to_string();
            out.push((dev.StateFlags & PRIMARY != 0, Display { name: label, modes, current: (cur.dmPelsWidth, cur.dmPelsHeight, cur.dmDisplayFrequency) }));
        }
    }
    // SDL (which the game uses) lists the primary monitor first
    out.sort_by_key(|(primary, _)| !*primary);
    out.into_iter().map(|(_, d)| d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = "force_pow2_textures=no\r\ngraphics=\r\n{\r\n\tsize=\r\n\t{\r\n\t\tx=1920\r\n\t\ty=1080\r\n\t}\r\n\r\n\tgui_scale=1.000000\r\n\trefreshRate=60\r\n\tfullScreen=no\r\n\tborderless=no\r\n\tdisplay_index=0\r\n\tmulti_sampling=4\r\n\tvsync=yes\r\n}\r\nmusic_volume=50.000000\r\nbloom=\r\n{\r\n\tquality=2\r\n}\r\n";
    const PDX: &str = "\"Graphics\"=\n{\n\t\"display_mode\"=\n\t{\n\t\tvalue=\"windowed\"\n\t\tversion=0\n\t}\n\t\"vsync\"=\n\t{\n\t\tenabled=yes\n\t\tversion=0\n\t}\n}\n\"System\"=\n{\n\t\"language\"=\n\t{\n\t\tvalue=\"l_simp_chinese\"\n\t\tversion=0\n\t}\n}\n";

    #[test]
    fn reads_edits_and_keeps_everything_else() {
        let dir = std::env::temp_dir().join(format!("stl-test-gfx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.txt"), SETTINGS).unwrap();
        std::fs::write(dir.join("pdx_settings.txt"), PDX).unwrap();
        let mut g = read(&dir);
        assert_eq!(g.display_mode, "windowed");
        assert_eq!(g.windowed_resolution, (1920, 1080));
        assert_eq!(g.gui_scale, 1.0);
        g.display_mode = "borderless_fullscreen".into();
        g.fullscreen_resolution = (3840, 2160);
        g.gui_scale = 1.5;
        g.multi_sampling = 8;
        g.vsync = false;
        write(&dir, &g).unwrap();
        let again = read(&dir);
        assert_eq!(again, g);
        let s = std::fs::read_to_string(dir.join("settings.txt")).unwrap();
        assert!(s.contains("\tgui_scale=1.500000\r\n") && s.contains("\t\tx=3840\r\n") && s.contains("\tborderless=yes\r\n"), "{s}");
        assert!(s.contains("music_volume=50.000000") && s.contains("quality=2"), "the rest stays");
        assert_eq!(s.matches("gui_scale").count(), 1);
        let p = std::fs::read_to_string(dir.join("pdx_settings.txt")).unwrap();
        assert!(p.contains("value=\"borderless_fullscreen\"") && p.contains("enabled=no") && p.contains("value=\"3840x2160\""), "{p}");
        assert!(p.contains("l_simp_chinese"));
        assert!(dir.join("settings.txt.stl_backup").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_files_get_the_blocks() {
        let dir = std::env::temp_dir().join(format!("stl-test-gfx-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let g = Graphics { gui_scale: 1.25, ..Default::default() };
        write(&dir, &g).unwrap();
        assert_eq!(read(&dir), g);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lists_monitors() {
        let d = displays();
        assert!(!d.is_empty());
        assert!(d.iter().all(|x| !x.modes.is_empty()));
    }
}
