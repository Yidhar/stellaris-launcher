//! Just enough of the PE format to tell which build of `stellaris.exe` this is: the link timestamp is what the plugins of this
//! project (stellaris-live2d, stellaris-perf, the bridge) compare against the build they were made for.

use crate::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// `TimeDateStamp` of the COFF header of an executable or DLL.
pub fn timestamp(path: &Path) -> Result<u32> {
    let mut f = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut dos = [0u8; 0x40];
    f.read_exact(&mut dos).context("not a PE file (too short)")?;
    if &dos[0..2] != b"MZ" {
        bail!("{} is not a PE file", path.display());
    }
    let pe = u32::from_le_bytes(dos[0x3c..0x40].try_into().unwrap()) as u64;
    f.seek(SeekFrom::Start(pe))?;
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).context("not a PE file (no header)")?;
    if &hdr[0..4] != b"PE\0\0" {
        bail!("{} is not a PE file (no PE signature)", path.display());
    }
    Ok(u32::from_le_bytes(hdr[8..12].try_into().unwrap()))
}

/// `0x6AB5181D` → "2026-09-24 12:31 UTC" (civil date from days since 1970, no time library needed).
pub fn describe(ts: u32) -> String {
    let secs = ts as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60)
}

/// "0x6AB5181D" or "6ab5181d" or "1790000000" → a number.
pub fn parse_timestamp(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()
    } else if s.chars().any(|c| c.is_ascii_alphabetic()) {
        u32::from_str_radix(s, 16).ok()
    } else {
        s.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_the_known_build() {
        assert_eq!(describe(0x6AB5181D), "2026-09-24 12:31 UTC");
        assert_eq!(describe(0), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_timestamp("0x6AB5181D"), Some(0x6AB5181D));
        assert_eq!(parse_timestamp("6ab5181d"), Some(0x6AB5181D));
        assert_eq!(parse_timestamp("1000"), Some(1000));
        assert_eq!(parse_timestamp("zz"), None);
    }

    #[test]
    fn reads_a_real_exe() {
        // the test binary itself is a PE file
        let exe = std::env::current_exe().unwrap();
        assert!(timestamp(&exe).is_ok());
    }
}
