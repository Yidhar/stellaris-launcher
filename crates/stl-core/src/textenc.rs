//! Reading and writing a text file in the encoding it came in: UTF-8 (with or without a BOM), UTF-16 (with a BOM), or the system's ANSI code
//! page (GBK on a Chinese Windows) for a file that is not valid UTF-8. Line endings are kept too.

use crate::{bail, Result};
use windows_sys::Win32::Globalization::{MultiByteToWideChar, WideCharToMultiByte, CP_ACP};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// the system's ANSI code page
    Ansi,
}

impl Encoding {
    pub fn label(self) -> String {
        match self {
            Encoding::Utf8 => "UTF-8".into(),
            Encoding::Utf8Bom => "UTF-8 BOM".into(),
            Encoding::Utf16Le => "UTF-16 LE".into(),
            Encoding::Utf16Be => "UTF-16 BE".into(),
            Encoding::Ansi => format!("ANSI ({})", unsafe { windows_sys::Win32::Globalization::GetACP() }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// with `\n` line ends
    pub text: String,
    pub encoding: Encoding,
    /// the file used `\r\n`
    pub crlf: bool,
}

fn ansi_to_string(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n <= 0 {
            return None;
        }
        let mut wide = vec![0u16; n as usize];
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), n);
        Some(String::from_utf16_lossy(&wide))
    }
}

fn string_to_ansi(s: &str) -> Result<Vec<u8>> {
    let wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return Ok(Vec::new());
    }
    unsafe {
        let mut lossy = 0i32;
        let n = WideCharToMultiByte(CP_ACP, 0, wide.as_ptr(), wide.len() as i32, std::ptr::null_mut(), 0, std::ptr::null(), &mut lossy);
        if n <= 0 {
            bail!("the text cannot be written in the system code page");
        }
        if lossy != 0 {
            bail!("the text has characters the file's encoding (the system code page) cannot hold; save it as UTF-8");
        }
        let mut out = vec![0u8; n as usize];
        WideCharToMultiByte(CP_ACP, 0, wide.as_ptr(), wide.len() as i32, out.as_mut_ptr(), n, std::ptr::null(), std::ptr::null_mut());
        Ok(out)
    }
}

pub fn decode(bytes: &[u8]) -> Text {
    let (text, encoding) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (String::from_utf8_lossy(rest).to_string(), Encoding::Utf8Bom)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()), Encoding::Utf16Le)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()), Encoding::Utf16Be)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_string(), Encoding::Utf8),
            Err(_) => match ansi_to_string(bytes) {
                Some(s) => (s, Encoding::Ansi),
                None => (String::from_utf8_lossy(bytes).to_string(), Encoding::Utf8),
            },
        }
    };
    let crlf = text.contains("\r\n");
    Text { text: text.replace("\r\n", "\n"), encoding, crlf }
}

pub fn encode(t: &Text) -> Result<Vec<u8>> {
    let body = if t.crlf { t.text.replace('\n', "\r\n") } else { t.text.clone() };
    Ok(match t.encoding {
        Encoding::Utf8 => body.into_bytes(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], body.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(body.encode_utf16().flat_map(|u| u.to_le_bytes())).collect(),
        Encoding::Utf16Be => [0xFE, 0xFF].into_iter().chain(body.encode_utf16().flat_map(|u| u.to_be_bytes())).collect(),
        Encoding::Ansi => string_to_ansi(&body)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_each_encoding_and_line_ending() {
        for (bytes, enc) in [
            (b"a=1\r\nb=\xE4\xB8\xAD\r\n".to_vec(), Encoding::Utf8),
            (b"\xEF\xBB\xBFa=1\n".to_vec(), Encoding::Utf8Bom),
            (vec![0xFF, 0xFE, b'a', 0, b'=', 0, b'1', 0], Encoding::Utf16Le),
            (vec![0xFE, 0xFF, 0, b'a', 0, b'=', 0, b'1'], Encoding::Utf16Be),
        ] {
            let t = decode(&bytes);
            assert_eq!(t.encoding, enc);
            assert!(!t.text.contains('\r'));
            assert_eq!(encode(&t).unwrap(), bytes, "{enc:?}");
        }
        let t = decode(b"a=1\r\nb=2\r\n");
        assert!(t.crlf);
        assert_eq!(t.text, "a=1\nb=2\n");
    }

    #[test]
    fn invalid_utf8_is_read_in_the_system_code_page() {
        let t = decode(b"name=\xC0\xE0");
        assert_eq!(t.encoding, Encoding::Ansi);
        assert!(!t.text.contains('\u{FFFD}'));
    }
}
