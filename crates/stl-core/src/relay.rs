//! One launcher window: a start with an `stl://` link (Windows runs `"<exe>" "%1"` for each link the site opens) hands the link to the
//! window already open, over a named pipe of this user, and exits. The window reads one link a connection; remote clients are refused.

use crate::{bail, Result};
use std::io::Write;

fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
    format!(r"\\.\pipe\stellaris-launcher-links-{user}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Gives the link to the open window. False when there is none (then this start shows the window itself).
pub fn hand_over(link: &str) -> bool {
    // the open window may come to the front (Windows lets only the foreground process allow that)
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY);
    }
    for _ in 0..40 {
        match std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            Ok(mut f) => return f.write_all(link.as_bytes()).and_then(|_| f.flush()).is_ok(),
            // busy: another link is being handed over, or the window is opening the next instance of the pipe
            Err(e) if e.raw_os_error() == Some(231) || e.raw_os_error() == Some(2) && std::path::Path::new(&pipe_name()).exists() => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Err(_) => return false,
        }
    }
    false
}

/// Takes links from later starts and gives each to `on_link`, on a thread of its own. An error when another window listens already.
pub fn listen(on_link: impl Fn(String) + Send + 'static) -> Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    let name = wide(&pipe_name());
    let make = move |first: bool| unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_INBOUND | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    let first = make(true);
    if first == INVALID_HANDLE_VALUE {
        bail!("another launcher window takes the links");
    }
    let first = first as usize;
    std::thread::Builder::new()
        .name("stl-links".into())
        .spawn(move || {
            let mut pipe = first as windows_sys::Win32::Foundation::HANDLE;
            loop {
                let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if connected {
                    let mut got = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let mut read = 0u32;
                        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) } != 0;
                        if !ok || read == 0 {
                            break;
                        }
                        got.extend_from_slice(&buf[..read as usize]);
                        if got.len() > crate::site::MAX_LINK + 16 {
                            got.clear();
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&got).trim().to_string();
                    if !text.is_empty() {
                        on_link(text);
                    }
                }
                unsafe {
                    DisconnectNamedPipe(pipe);
                    CloseHandle(pipe);
                }
                pipe = make(false);
                if pipe == INVALID_HANDLE_VALUE {
                    return;
                }
            }
        })
        .map_err(|e| crate::anyhow!("cannot start the link listener: {e}"))?;
    Ok(())
}
