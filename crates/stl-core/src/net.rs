//! A small HTTP GET over WinHTTP (the system's own client: it brings TLS, the user's proxy settings and certificate store, and costs no crate).
//! Used for the public news feed and its pictures, GitHub releases, Steam's public item details (a POST of the item id), and
//! stellarismod.com (`site`: the member's token, playsets, wiki entries); cookies are not kept.

use crate::{bail, Context, Result};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Networking::WinHttp::*;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// `https://host:port/path?query` → (secure, host, port, path and query)
pub fn split_url(url: &str) -> Result<(bool, String, u16, String)> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        bail!("not an http(s) URL: {url}");
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse::<u16>().context("bad port")?),
        _ => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        bail!("no host in {url}");
    }
    Ok((secure, host.to_string(), port, path.to_string()))
}

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// GETs a URL (redirects followed) and returns the body of a 200 answer. `max_bytes` bounds what is read.
pub fn http_get(url: &str, timeout_ms: i32, max_bytes: usize) -> Result<Vec<u8>> {
    request("GET", url, None, timeout_ms, max_bytes)
}

/// GETs every URL at once; the answers in the order they arrived, each with the index of its URL. For mirrors: the first good answer
/// comes from the one nearest (or least busy) for this user.
pub fn http_get_all(urls: &[String], timeout_ms: i32, max_bytes: usize) -> Vec<(usize, Result<Vec<u8>>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        for (i, url) in urls.iter().enumerate() {
            let tx = tx.clone();
            s.spawn(move || {
                let _ = tx.send((i, http_get(url, timeout_ms, max_bytes)));
            });
        }
    });
    drop(tx);
    rx.into_iter().collect()
}

/// POSTs a form (`a=1&b=2`, already encoded) and returns the body of a 200 answer.
pub fn http_post_form(url: &str, form: &str, timeout_ms: i32, max_bytes: usize) -> Result<Vec<u8>> {
    request("POST", url, Some(form.as_bytes()), timeout_ms, max_bytes)
}

/// `application/x-www-form-urlencoded` encoding of one value.
pub fn form_encode(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn request(method: &str, url: &str, form: Option<&[u8]>, timeout_ms: i32, max_bytes: usize) -> Result<Vec<u8>> {
    let body = form.map(|b| ("application/x-www-form-urlencoded", b));
    let a = send(method, url, &[], body, timeout_ms, max_bytes)?;
    if a.status != 200 {
        bail!("{} answered {}", a.host, a.status);
    }
    Ok(a.body)
}

/// An answer of any status (`send`).
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
    pub host: String,
}

/// A request with headers (`("Authorization", "Bearer …")`) and an optional body (`(content type, bytes)`), whatever the status of the
/// answer (redirects are followed). No `Origin` header and no cookies are ever sent.
pub fn send(method: &str, url: &str, headers: &[(&str, &str)], body: Option<(&str, &[u8])>, timeout_ms: i32, max_bytes: usize) -> Result<Answer> {
    let (secure, host, port, path) = split_url(url)?;
    for (k, v) in headers {
        if k.contains([':', '\r', '\n']) || v.contains(['\r', '\n']) {
            bail!("a header that cannot be sent: {k}");
        }
    }
    unsafe {
        let session = Handle(WinHttpOpen(wide(&format!("stellaris-launcher/{}", env!("CARGO_PKG_VERSION"))).as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
        if session.0.is_null() {
            bail!("WinHttpOpen failed");
        }
        WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
        let connection = Handle(WinHttpConnect(session.0, wide(&host).as_ptr(), port, 0));
        if connection.0.is_null() {
            bail!("cannot connect to {host}");
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { 0 };
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            wide(method).as_ptr(),
            wide(&path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
        ));
        if request.0.is_null() {
            bail!("WinHttpOpenRequest failed");
        }
        let mut lines: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        if let Some((ty, _)) = body {
            lines.push_str(&format!("Content-Type: {ty}\r\n"));
        }
        let wide_lines = wide(&lines);
        let (hp, hl) = if lines.is_empty() { (std::ptr::null(), 0) } else { (wide_lines.as_ptr(), u32::MAX) };
        let sent = match body {
            Some((_, b)) => WinHttpSendRequest(request.0, hp, hl, b.as_ptr() as *const _, b.len() as u32, b.len() as u32, 0),
            None => WinHttpSendRequest(request.0, hp, hl, std::ptr::null(), 0, 0, 0),
        };
        if sent == 0 {
            bail!("request to {host} failed (no network, or the host did not answer)");
        }
        if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            bail!("no answer from {host}");
        }
        let mut status = 0u32;
        let mut len = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut status as *mut u32 as *mut _, &mut len, std::ptr::null_mut()) == 0 {
            bail!("no status line from {host}");
        }
        let mut out = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                bail!("the connection to {host} broke");
            }
            if available == 0 {
                break;
            }
            let mut chunk = vec![0u8; available as usize];
            let mut read = 0u32;
            if WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, available, &mut read) == 0 {
                bail!("the connection to {host} broke");
            }
            out.extend_from_slice(&chunk[..read as usize]);
            if out.len() > max_bytes {
                bail!("the answer of {host} is larger than {max_bytes} bytes");
            }
        }
        Ok(Answer { status: status as u16, body: out, host })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        assert_eq!(split_url("https://api.example.com/a/b?c=d").unwrap(), (true, "api.example.com".into(), 443, "/a/b?c=d".into()));
        assert_eq!(split_url("http://host:8080").unwrap(), (false, "host".into(), 8080, "/".into()));
        assert!(split_url("ftp://x").is_err());
        assert!(split_url("https:///x").is_err());
        assert_eq!(form_encode("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
    }
}
