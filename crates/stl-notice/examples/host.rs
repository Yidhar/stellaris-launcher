//! A stand-in for the game, to see the notice without starting Stellaris: a dark window, the launcher's notice file for this process, then
//! `stl_notice.dll` loaded into it. `cargo run -p stl-notice --example host -- <path to stl_notice.dll> [seconds]`
//! The file lists this example's own exe as a loaded plugin (it is in the process) and a missing one, plus one the launcher skipped.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, Sleep};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage, MSG,
    PM_REMOVE, SW_SHOW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn proc_(h: *mut core::ffi::c_void, m: u32, w: usize, l: isize) -> isize {
    DefWindowProcW(h, m, w, l)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dll = args.next().expect("the path of stl_notice.dll");
    let seconds: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(9);
    unsafe {
        let class = wide("StlNoticeHost");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: GetModuleHandleW(null()),
            lpszClassName: class.as_ptr(),
            hbrBackground: GetStockObject(BLACK_BRUSH),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("Stellaris (notice test)");
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW, 120, 120, 1280, 760, null_mut(), null_mut(), wc.hInstance, null());
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        println!("hwnd {}", hwnd as usize);
        // the launcher's file for this process
        let me = std::env::current_exe().unwrap();
        let dir = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("stellaris-launcher").join("notice");
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!(
            "title\tStellaris Launcher\nall\t{{0}} 个 DLL 插件已在游戏内加载\nsome\t{{0}} / {{1}} 个 DLL 插件已在游戏内加载\nmissing\t没有在游戏中找到\nplugin\tLive2D 立绘\t{}\nplugin\tStellaris MCP\tC:\\nowhere\\stellaris_bridge.dll\nskipped\tPerf HUD\t为另一个游戏版本制作\n",
            me.display()
        );
        std::fs::write(dir.join(format!("{}.txt", GetCurrentProcessId())), text).unwrap();
        let module = LoadLibraryW(wide(&dll).as_ptr());
        println!("loaded {}", !module.is_null());
        let mut msg: MSG = std::mem::zeroed();
        for _ in 0..seconds * 60 {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            Sleep(16);
        }
    }
}
