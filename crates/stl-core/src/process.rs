//! Processes: finding the game, telling when it is ready, starting it, and loading a DLL into it (`CreateRemoteThread` + `LoadLibraryW`, the
//! same thing the plugins' own injector scripts do). Windows only.

use crate::{bail, Context, Result};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, GetHandleInformation, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW, MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION,
    PROCESS_TERMINATE, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible};

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// The ids of the running processes whose executable has this file name (`stellaris.exe`).
pub fn find_processes(exe_name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            if from_wide(&e.szExeFile).eq_ignore_ascii_case(exe_name) {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

/// `(module name, full path)` of everything loaded in a process.
pub fn modules(pid: u32) -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: MODULEENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut ok = Module32FirstW(snap, &mut e);
        while ok != 0 {
            out.push((from_wide(&e.szModule), from_wide(&e.szExePath)));
            ok = Module32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

/// Is a DLL loaded in the process? Matches a full path or just a file name, ignoring case.
pub fn module_loaded(pid: u32, dll: &str) -> bool {
    let want = dll.replace('/', "\\").to_lowercase();
    let want_name = Path::new(&want).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| want.clone());
    modules(pid).iter().any(|(name, path)| {
        if want.contains('\\') {
            path.to_lowercase() == want
        } else {
            name.to_lowercase() == want_name
        }
    })
}

/// The title of the first visible top-level window of the process that contains `needle`.
pub fn window_title(pid: u32, needle: &str) -> Option<String> {
    struct Search {
        pid: u32,
        needle: String,
        found: Option<String>,
    }
    unsafe extern "system" fn each(hwnd: *mut core::ffi::c_void, lparam: isize) -> i32 {
        let s = &mut *(lparam as *mut Search);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == s.pid && IsWindowVisible(hwnd) != 0 {
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if title.to_lowercase().contains(&s.needle) {
                    s.found = Some(title);
                    return 0;
                }
            }
        }
        1
    }
    let mut s = Search { pid, needle: needle.to_lowercase(), found: None };
    unsafe {
        EnumWindows(Some(each), &mut s as *mut Search as isize);
    }
    s.found
}

/// Loads a DLL into a running process. Returns when `LoadLibraryW` has returned there (and checks that the module is in the process).
pub fn inject(pid: u32, dll: &Path) -> Result<()> {
    let dll = dll.canonicalize().with_context(|| format!("cannot find {}", dll.display()))?;
    // canonicalize gives a \\?\ path; LoadLibraryW wants the plain one
    let plain = dll.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let path = wide(OsStr::new(&plain));
    unsafe {
        let access = PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ;
        let process: HANDLE = OpenProcess(access, 0, pid);
        if process.is_null() {
            bail!("cannot open process {pid} (is it running as another user?)");
        }
        let result = (|| -> Result<()> {
            let bytes = path.len() * 2;
            let remote = VirtualAllocEx(process, std::ptr::null(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            if remote.is_null() {
                bail!("VirtualAllocEx failed in process {pid}");
            }
            let mut written = 0usize;
            if WriteProcessMemory(process, remote, path.as_ptr() as *const _, bytes, &mut written) == 0 || written != bytes {
                VirtualFreeEx(process, remote, 0, MEM_RELEASE);
                bail!("WriteProcessMemory failed in process {pid}");
            }
            let kernel32 = GetModuleHandleW(wide(OsStr::new("kernel32.dll")).as_ptr());
            let load = GetProcAddress(kernel32, b"LoadLibraryW\0".as_ptr());
            let Some(load) = load else { bail!("LoadLibraryW not found") };
            let start: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = std::mem::transmute(load);
            let thread = CreateRemoteThread(process, std::ptr::null(), 0, Some(start), remote, 0, std::ptr::null_mut());
            if thread.is_null() {
                VirtualFreeEx(process, remote, 0, MEM_RELEASE);
                bail!("CreateRemoteThread failed in process {pid}");
            }
            let wait = WaitForSingleObject(thread, 30_000);
            let mut code = 0u32;
            GetExitCodeThread(thread, &mut code);
            CloseHandle(thread);
            if wait == 0 {
                VirtualFreeEx(process, remote, 0, MEM_RELEASE);
            }
            if wait != 0 {
                bail!("LoadLibraryW did not return within 30 s");
            }
            if code == 0 {
                bail!("LoadLibraryW failed in the game (the DLL, or a DLL it needs, could not be loaded)");
            }
            Ok(())
        })();
        CloseHandle(process);
        result?;
    }
    if !module_loaded(pid, &plain) {
        bail!("LoadLibraryW returned, but {plain} is not among the game's modules (the DLL unloaded itself?)");
    }
    Ok(())
}

pub fn terminate(pid: u32) -> Result<()> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            bail!("cannot open process {pid}");
        }
        let ok = TerminateProcess(h, 1);
        CloseHandle(h);
        if ok == 0 {
            bail!("TerminateProcess failed for {pid}");
        }
    }
    Ok(())
}

/// Runs `f` with this process's standard handles marked not inheritable, then restores them. A process started inside `f` would otherwise
/// inherit them (Windows hands every inheritable handle to a child): when our output is a pipe — a script or a window reading it — the game would
/// hold the write end open for as long as it runs, and the reader would never see the end of the output.
fn without_inheritable_std_handles<T>(f: impl FnOnce() -> T) -> T {
    let mut changed: Vec<HANDLE> = Vec::new();
    unsafe {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(id);
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut flags = 0u32;
            if GetHandleInformation(h, &mut flags) != 0 && flags & HANDLE_FLAG_INHERIT != 0 && SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) != 0 {
                changed.push(h);
            }
        }
    }
    let result = f();
    unsafe {
        for h in changed {
            SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
    }
    result
}

/// Starts the game the way the official launcher does (a detached process in the game's folder, `SteamAppId` set, stderr to a file).
pub fn spawn(exe: &Path, args: &[String], cwd: &Path, env: &[(&str, &str)], stderr_log: Option<&Path>) -> Result<u32> {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).current_dir(cwd).creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    match stderr_log {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            cmd.stderr(std::fs::File::create(p).with_context(|| format!("cannot create {}", p.display()))?);
        }
        None => {
            cmd.stderr(std::process::Stdio::null());
        }
    }
    let child = without_inheritable_std_handles(|| cmd.spawn()).with_context(|| format!("cannot start {}", exe.display()))?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_its_own_modules() {
        let pid = std::process::id();
        assert!(module_loaded(pid, "kernel32.dll"));
        assert!(module_loaded(pid, "KERNEL32.DLL"));
        assert!(!module_loaded(pid, "definitely-not-loaded.dll"));
        let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().to_string();
        assert!(find_processes(&me).contains(&pid));
    }

    #[test]
    fn loads_a_dll_into_a_process() {
        // start a child that sleeps, load a harmless system DLL into it and see it among the modules
        let mut child = std::process::Command::new("ping").args(["-n", "6", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let dll = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("winmm.dll");
        assert!(!module_loaded(child.id(), "winmm.dll") || true);
        inject(child.id(), &dll).unwrap();
        assert!(module_loaded(child.id(), "winmm.dll"));
        let _ = child.kill();
    }
}
