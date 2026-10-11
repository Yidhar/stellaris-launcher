//! `stl_notice.dll`: the launcher loads it into the game after the playset's DLL plugins. From inside the game it checks that each of them
//! is loaded, then says how many are in a small capsule at the top right of the game window for a few seconds, and unloads itself. It does
//! not name them: there may be many.
//!
//! - What to check and the words to show come from `%APPDATA%\stellaris-launcher\notice\<pid>.txt`, written by the launcher just before (one
//!   line each, fields split by tabs): `all` and `some` (the summary when all / not all are loaded: `{0}` loaded, `{1}` in all),
//!   `missing` (the reason for a plugin not found in the game), then `plugin <name> <dll path>` and `skipped <name> <reason>` lines.
//! - The card is its own window (layered, click-through, never activated), drawn with GDI on this DLL's own thread: nothing of the game is
//!   called. It shows only while the game is the foreground window; a game in exclusive full screen hides it (the launcher does not load
//!   this DLL then).
//! - The release profile aborts on panic, which would take the game down: nothing here may panic (no unwrap, no indexing).

#![cfg(windows)]

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, POINT, RECT, SIZE, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    CLEARTYPE_QUALITY, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    FW_SEMIBOLD, HDC, HFONT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, FreeLibraryAndExitThread, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Threading::{CreateThread, GetCurrentProcessId, Sleep};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClientRect, GetForegroundWindow, GetWindow,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PeekMessageW, RegisterClassW, ShowWindow, TranslateMessage, UnregisterClassW,
    UpdateLayeredWindow, GW_OWNER, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const FADE_IN_MS: u32 = 220;
const HOLD_MS: u32 = 6000;
const FADE_OUT_MS: u32 = 500;
/// how long to wait for the game's window to be in front before giving up
const WAIT_FOR_GAME_MS: u32 = 120_000;
const TICK_MS: u32 = 16;
/// while the game is not in front: looked at this often, and given up after this long
const IDLE_TICK_MS: u32 = 200;
const GIVE_UP_MS: u32 = 600_000;

#[no_mangle]
pub extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(module);
            // the work is on a thread of its own: nothing may wait inside DllMain (the loader lock is held)
            let thread = CreateThread(null(), 0, Some(worker), module as *const c_void, 0, null_mut());
            if !thread.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(thread);
            }
        }
    }
    TRUE
}

unsafe extern "system" fn worker(param: *mut c_void) -> u32 {
    let module = param as HMODULE;
    run(module);
    // done: out of the game again
    FreeLibraryAndExitThread(module, 0)
}

/// What the card says.
struct Notice {
    summary: String,
    rows: Vec<(bool, String, String)>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The launcher's file for this game, read and removed; with each plugin checked here, in the game.
fn read_notice() -> Option<Notice> {
    let pid = unsafe { GetCurrentProcessId() };
    let dir = std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("stellaris-launcher").join("notice");
    let path = dir.join(format!("{pid}.txt"));
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    parse(&text)
}

/// A notice file's lines, each plugin checked in this process.
fn parse(text: &str) -> Option<Notice> {
    let mut words = std::collections::HashMap::new();
    let mut rows = Vec::new();
    let (mut loaded, mut total) = (0usize, 0usize);
    for line in text.lines() {
        let mut f = line.split('\t');
        let (Some(kind), Some(a)) = (f.next(), f.next()) else { continue };
        let b = f.next().unwrap_or("");
        match kind {
            "plugin" => {
                total += 1;
                let there = unsafe { !GetModuleHandleW(wide(b).as_ptr()).is_null() };
                if there {
                    loaded += 1;
                }
                rows.push((there, a.to_string(), if there { String::new() } else { "\u{1}missing".to_string() }));
            }
            "skipped" => {
                total += 1;
                rows.push((false, a.to_string(), b.to_string()));
            }
            _ => {
                words.insert(kind.to_string(), a.to_string());
            }
        }
    }
    if rows.is_empty() {
        return None;
    }
    let word = |k: &str| words.get(k).cloned().unwrap_or_default();
    let missing = word("missing");
    for r in rows.iter_mut() {
        if r.2 == "\u{1}missing" {
            r.2 = missing.clone();
        }
    }
    let template = if loaded == total { word("all") } else { word("some") };
    let summary = template.replace("{0}", &loaded.to_string()).replace("{1}", &total.to_string());
    Some(Notice { summary, rows })
}

struct Find {
    pid: u32,
    best: HWND,
    area: i64,
}

unsafe extern "system" fn each_window(hwnd: HWND, data: LPARAM) -> BOOL {
    let find = &mut *(data as *mut Find);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == find.pid && IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > find.area {
            find.area = area;
            find.best = hwnd;
        }
    }
    TRUE
}

/// The game's main window: its largest visible top-level window.
fn game_window() -> HWND {
    let mut find = Find { pid: unsafe { GetCurrentProcessId() }, best: null_mut(), area: 0 };
    unsafe { EnumWindows(Some(each_window), &mut find as *mut Find as LPARAM) };
    find.best
}

fn in_front(game: HWND) -> bool {
    let front = unsafe { GetForegroundWindow() };
    !front.is_null() && front == game
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    DefWindowProcW(hwnd, msg, w, l)
}

fn font(px: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe { CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr()) }
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// The width a line of text takes.
unsafe fn text_width(dc: HDC, f: HFONT, s: &str) -> i32 {
    let old = SelectObject(dc, f);
    let w = wide(s);
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    DrawTextW(dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, old);
    r.right - r.left
}

unsafe fn text(dc: HDC, f: HFONT, s: &str, color: u32, mut r: RECT) {
    let old = SelectObject(dc, f);
    SetTextColor(dc, color);
    let w = wide(s);
    DrawTextW(dc, w.as_ptr(), -1, &mut r, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(dc, old);
}

/// The capsule, drawn once into a 32-bit bitmap with its rounded ends in the alpha channel; shown with a fading opacity.
struct Card {
    dc: HDC,
    bitmap: *mut c_void,
    w: i32,
    h: i32,
}

/// The mark of the title bar: a four-pointed star filled as a fan of triangles from its right-hand tip (as egui fills it there).
unsafe fn mark(dc: HDC, cx: i32, cy: i32, r: f32) {
    use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, Polygon, GetStockObject, NULL_PEN};
    let pts: Vec<POINT> = (0..16)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let rr = if k % 4 == 0 { r } else { r * 3.2 / 9.0 };
            POINT { x: cx + (a.cos() * rr).round() as i32, y: cy + (a.sin() * rr).round() as i32 }
        })
        .collect();
    let brush = CreateSolidBrush(rgb(255, 255, 255));
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    if let Some(&first) = pts.first() {
        for pair in pts.windows(2).skip(1) {
            if let [a, b] = pair {
                let tri = [first, *a, *b];
                Polygon(dc, tri.as_ptr(), 3);
            }
        }
    }
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(brush);
}

/// A capsule: the mark, then one line (how many plugins are loaded); an orange dot when not all are. The plugins are not named: there may be
/// many of them.
unsafe fn draw(n: &Notice, scale: f32) -> Option<Card> {
    let px = |v: f32| (v * scale).round() as i32;
    let screen = GetDC(null_mut());
    let dc = CreateCompatibleDC(screen);
    ReleaseDC(null_mut(), screen);
    if dc.is_null() {
        return None;
    }
    let all = n.rows.iter().all(|r| r.0);
    let f_summary = font(px(14.0), FW_SEMIBOLD as i32);
    let (pad, icon, gap, dot) = (px(16.0), px(18.0), px(10.0), if all { 0 } else { px(16.0) });
    let h = px(40.0);
    let w = pad + icon + gap + text_width(dc, f_summary, &n.summary) + dot + pad + px(4.0);
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut bits: *mut c_void = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if bitmap.is_null() || bits.is_null() {
        DeleteObject(f_summary);
        DeleteDC(dc);
        return None;
    }
    SelectObject(dc, bitmap);
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
    // the capsule's colour (0x00RRGGBB in the bitmap)
    let (br, bg, bb) = (24u32, 26u32, 36u32);
    for p in pixels.iter_mut() {
        *p = br << 16 | bg << 8 | bb;
    }
    SetBkMode(dc, TRANSPARENT as i32);
    mark(dc, pad + icon / 2, h / 2, icon as f32 / 2.0);
    let tx = pad + icon + gap;
    text(dc, f_summary, &n.summary, rgb(245, 245, 250), RECT { left: tx, top: 0, right: w - pad - dot, bottom: h });
    if !all {
        use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, Ellipse, GetStockObject, NULL_PEN};
        let brush = CreateSolidBrush(rgb(255, 159, 10));
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let (cx, cy, r) = (w - pad - px(4.0), h / 2, px(4.0));
        Ellipse(dc, cx - r, cy - r, cx + r + 1, cy + r + 1);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        DeleteObject(brush);
    }
    DeleteObject(f_summary);
    // fully rounded ends and a hairline rim into the alpha channel, colours premultiplied (as UpdateLayeredWindow wants them)
    let radius = h as f32 / 2.0;
    let opacity = 0.92f32;
    for yy in 0..h {
        for xx in 0..w {
            let (fx, fy) = (xx as f32 + 0.5, yy as f32 + 0.5);
            let dx = (radius - fx).max(fx - (w as f32 - radius)).max(0.0);
            let dy = (radius - fy).max(fy - (h as f32 - radius)).max(0.0);
            let dist = (dx * dx + dy * dy).sqrt() - radius;
            let coverage = (0.5 - dist).clamp(0.0, 1.0);
            let Some(p) = pixels.get_mut((yy * w + xx) as usize) else { continue };
            let (mut r, mut g, mut b) = ((*p >> 16) & 255, (*p >> 8) & 255, *p & 255);
            if dist > -1.0 * scale {
                // the rim: a little lighter
                r = (r + 46).min(255);
                g = (g + 46).min(255);
                b = (b + 56).min(255);
            }
            let a = coverage * opacity;
            let pm = |c: u32| ((c as f32) * a).round() as u32;
            *p = ((a * 255.0).round() as u32) << 24 | pm(r) << 16 | pm(g) << 8 | pm(b);
        }
    }
    Some(Card { dc, bitmap, w, h })
}

/// For a look at the card without the game (development): draws the card of a notice file into a 32-bit BMP, premultiplied alpha, at
/// `scale_percent`. Returns 1 when written. The caller holds its own reference to the DLL (its loading thread lets go of one).
#[no_mangle]
pub unsafe extern "C" fn stl_notice_preview(notice: *const u16, out: *const u16, scale_percent: u32) -> i32 {
    let read = |p: *const u16| {
        if p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
    };
    let (Some(notice), Some(out)) = (read(notice), read(out)) else { return 0 };
    let Some(n) = std::fs::read_to_string(notice).ok().and_then(|t| parse(&t)) else { return 0 };
    let Some(card) = draw(&n, scale_percent.max(50) as f32 / 100.0) else { return 0 };
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    let size = (card.w * card.h * 4) as usize;
    let mut bytes = Vec::with_capacity(54 + size);
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&((54 + size) as u32).to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&card.w.to_le_bytes());
    bytes.extend_from_slice(&(-card.h).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 24]);
    let mut px: Vec<u32> = vec![0; (card.w * card.h) as usize];
    let header = BITMAPINFOHEADER { biSize: 40, biWidth: card.w, biHeight: -card.h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
    info.bmiHeader = header;
    windows_sys::Win32::Graphics::Gdi::GetDIBits(card.dc, card.bitmap, 0, card.h as u32, px.as_mut_ptr() as *mut c_void, &mut info, DIB_RGB_COLORS);
    for p in &px {
        bytes.extend_from_slice(&p.to_le_bytes());
    }
    DeleteDC(card.dc);
    DeleteObject(card.bitmap);
    std::fs::write(out, bytes).is_ok() as i32
}

unsafe fn show(window: HWND, card: &Card, at: POINT, alpha: u8) {
    let size = SIZE { cx: card.w, cy: card.h };
    let from = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
    UpdateLayeredWindow(window, null_mut(), &at, &size, card.dc, &from, 0, &blend, ULW_ALPHA);
}

/// Where the card goes: the game's top right, inside its client area.
unsafe fn place(game: HWND, card: &Card, scale: f32) -> POINT {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(game, &mut r);
    let mut corner = POINT { x: r.right, y: r.top };
    ClientToScreen(game, &mut corner);
    let margin = (24.0 * scale) as i32;
    POINT { x: corner.x - card.w - margin, y: corner.y + margin }
}

fn run(module: HMODULE) {
    let Some(notice) = read_notice() else { return };
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // the game's window, in front
        let mut waited = 0;
        let game = loop {
            let g = game_window();
            if !g.is_null() && in_front(g) {
                break g;
            }
            if waited >= WAIT_FOR_GAME_MS {
                return;
            }
            Sleep(250);
            waited += 250;
        };
        let scale = (GetDpiForWindow(game).max(96) as f32) / 96.0;
        let Some(card) = draw(&notice, scale) else { return };
        let class = wide("StellarisLauncherNotice");
        let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: module, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let window = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            card.w,
            card.h,
            null_mut(),
            null_mut(),
            module,
            null(),
        );
        if !window.is_null() {
            show(window, &card, place(game, &card, scale), 0);
            ShowWindow(window, SW_SHOWNOACTIVATE);
            // the time it has been shown (it stops while the game is not in front, when it is looked at less often)
            let mut shown = 0u32;
            let mut elapsed = 0u32;
            let total = FADE_IN_MS + HOLD_MS + FADE_OUT_MS;
            let mut last: Option<(u8, i32, i32)> = None;
            let mut msg: MSG = std::mem::zeroed();
            while shown < total && elapsed < GIVE_UP_MS && IsWindow(game) != 0 {
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let front = in_front(game);
                let alpha = if front {
                    shown += TICK_MS;
                    let t = if shown < FADE_IN_MS {
                        shown as f32 / FADE_IN_MS as f32
                    } else if shown > FADE_IN_MS + HOLD_MS {
                        1.0 - (shown - FADE_IN_MS - HOLD_MS) as f32 / FADE_OUT_MS as f32
                    } else {
                        1.0
                    };
                    (t.clamp(0.0, 1.0) * 255.0) as u8
                } else {
                    0
                };
                let at = place(game, &card, scale);
                // drawn again only when it changes
                if last != Some((alpha, at.x, at.y)) {
                    show(window, &card, at, alpha);
                    last = Some((alpha, at.x, at.y));
                }
                let tick = if front { TICK_MS } else { IDLE_TICK_MS };
                Sleep(tick);
                elapsed += tick;
            }
            DestroyWindow(window);
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
        }
        UnregisterClassW(class.as_ptr(), module);
        DeleteDC(card.dc);
        DeleteObject(card.bitmap);
    }
}
