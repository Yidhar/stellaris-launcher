//! `stl_notice.dll`: the launcher loads it into the game after the playset's DLL plugins. From inside the game it checks that each of them
//! is loaded and says how many are, in a small capsule at the top right of the game window for a few seconds (not their names: there may be
//! many). While the game shows its title screen, the bottom-right corner of the game window opens the plugin list: hovering the corner fades
//! in a small button, which opens a panel in the middle of the game with every installed plugin — name, version, description, whether it is
//! loaded — and a switch for it in this playset, taking effect at the next start. Once a game is loaded it goes away and unloads itself.
//!
//! - What to show comes from `%APPDATA%\stellaris-launcher\notice\<pid>.txt`, written by the launcher just before (one line each, fields
//!   split by tabs): words (`all`, `some` — `{0}` loaded, `{1}` switched on —, `missing`, `button`, `heading`, `footer`, `loaded`, `off`),
//!   `gamelog <path>`, `toggles <path>`, `playset <id>`, then `plugin <id> <name> <dll> <version> <description> <on 1|0> <why left out>`.
//! - A switch flipped here appends `<playset id>\t<plugin id>\t1|0` to the `toggles` file; the launcher takes it into the playset (it does
//!   not touch the launcher's own files, which a running launcher would write over). A plugin in the game stays there until the game ends.
//! - A loaded game: the game writes `gamestate… Recalc cached data` into its `time.log` when a game is loaded or started (the log starts
//!   empty with each run of the game). Going back to the title screen writes nothing, so the list does not come back until the next run.
//! - Its windows are its own (layered, never activated; the capsule lets clicks through), drawn with GDI on this DLL's own thread: nothing
//!   of the game is called. They show only while the game (or one of them) is the foreground window; a game in exclusive full screen hides
//!   them (the launcher does not load this DLL then).
//! - The release profile aborts on panic, which would take the game down: nothing here may panic (no unwrap, no indexing).

#![cfg(windows)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, POINT, RECT, SIZE, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, GetDC, GetDIBits,
    GetStockObject, Polygon, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLENDFUNCTION, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, NULL_PEN, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, FreeLibraryAndExitThread, GetModuleHandleW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Threading::{CreateThread, GetCurrentProcessId, Sleep};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, LoadCursorW, PeekMessageW, RegisterClassW, ShowWindow, TranslateMessage,
    UnregisterClassW, UpdateLayeredWindow, GW_OWNER, IDC_ARROW, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_LBUTTONUP, WM_MOUSEWHEEL,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const FADE_IN_MS: f32 = 220.0;
const HOLD_MS: f32 = 6000.0;
const FADE_OUT_MS: f32 = 500.0;
/// the corner button and the list fade in and out this fast
const FADE_MS: f32 = 140.0;
/// how long to wait for the game's window before giving up
const WAIT_FOR_GAME_MS: u32 = 180_000;
const TICK_MS: u32 = 16;
/// while the game is not in front: looked at this often
const IDLE_TICK_MS: u32 = 200;
/// the line the game writes into its time.log when a game is loaded or started
const GAME_LOADED: &str = "Recalc cached data";

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

// ------------------------------------------------------------------ what the launcher says

struct Plugin {
    id: String,
    name: String,
    version: String,
    description: String,
    /// switched on in the playset when the game started
    was_on: bool,
    /// its switch now (flipped in the list)
    on: bool,
    loaded: bool,
    /// why it is not loaded although switched on
    why: String,
}

struct Notice {
    words: std::collections::HashMap<String, String>,
    plugins: Vec<Plugin>,
    gamelog: Option<std::path::PathBuf>,
    toggles: Option<std::path::PathBuf>,
    playset: String,
}

impl Notice {
    fn word(&self, k: &str) -> String {
        self.words.get(k).cloned().unwrap_or_default()
    }

    fn loaded(&self) -> usize {
        self.plugins.iter().filter(|p| p.loaded).count()
    }

    /// switched on when the game started
    fn wanted(&self) -> usize {
        self.plugins.iter().filter(|p| p.was_on).count()
    }

    fn all(&self) -> bool {
        self.loaded() >= self.wanted()
    }

    fn summary(&self) -> String {
        let template = if self.all() { self.word("all") } else { self.word("some") };
        template.replace("{0}", &self.loaded().to_string()).replace("{1}", &self.wanted().to_string())
    }

    /// The game has loaded a game (its time.log says so).
    fn in_game(&self) -> bool {
        self.gamelog.as_ref().and_then(|p| std::fs::read(p).ok()).is_some_and(|b| String::from_utf8_lossy(&b).contains(GAME_LOADED))
    }

    /// Flips a plugin's switch, and tells the launcher.
    fn flip(&mut self, i: usize) {
        let Some(p) = self.plugins.get_mut(i) else { return };
        p.on = !p.on;
        let line = format!("{}\t{}\t{}\n", self.playset, p.id, p.on as u8);
        if let Some(path) = &self.toggles {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The launcher's file for this game, read and removed.
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
    let mut n = Notice { words: Default::default(), plugins: Vec::new(), gamelog: None, toggles: None, playset: String::new() };
    let path = |s: String| Some(std::path::PathBuf::from(s)).filter(|p| !p.as_os_str().is_empty());
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
        match f.first().copied() {
            Some("plugin") => {
                let dll = get(3);
                let on = get(6) == "1";
                let loaded = !dll.is_empty() && unsafe { !GetModuleHandleW(wide(&dll).as_ptr()).is_null() };
                n.plugins.push(Plugin { id: get(1), name: get(2), version: get(4), description: get(5), was_on: on, on, loaded, why: get(7) });
            }
            Some("gamelog") => n.gamelog = path(get(1)),
            Some("toggles") => n.toggles = path(get(1)),
            Some("playset") => n.playset = get(1),
            Some(k) if !k.is_empty() => {
                n.words.insert(k.to_string(), get(1));
            }
            _ => {}
        }
    }
    let missing = n.word("missing");
    for p in n.plugins.iter_mut().filter(|p| p.was_on && !p.loaded && p.why.is_empty()) {
        p.why = missing.clone();
    }
    (!n.plugins.is_empty()).then_some(n)
}

// ------------------------------------------------------------------ the game's window

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

/// The game's client area on the screen.
unsafe fn client_on_screen(game: HWND) -> RECT {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(game, &mut r);
    let mut a = POINT { x: r.left, y: r.top };
    let mut b = POINT { x: r.right, y: r.bottom };
    ClientToScreen(game, &mut a);
    ClientToScreen(game, &mut b);
    RECT { left: a.x, top: a.y, right: b.x, bottom: b.y }
}

fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

// ------------------------------------------------------------------ drawing

fn font(px: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe { CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr()) }
}

/// A GDI colour (0x00BBGGRR).
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

unsafe fn text_width(dc: HDC, f: HFONT, s: &str) -> i32 {
    let old = SelectObject(dc, f);
    let w = wide(s);
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    DrawTextW(dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, old);
    r.right - r.left
}

unsafe fn text(dc: HDC, f: HFONT, s: &str, color: u32, mut r: RECT, right: bool) {
    let old = SelectObject(dc, f);
    SetTextColor(dc, color);
    let w = wide(s);
    let align = if right { DT_RIGHT } else { DT_LEFT };
    DrawTextW(dc, w.as_ptr(), -1, &mut r, align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(dc, old);
}

/// The mark of the title bar: a four-pointed star filled as a fan of triangles from its right-hand tip (as egui fills it there).
unsafe fn mark(dc: HDC, cx: i32, cy: i32, r: f32) {
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

/// A picture for a layered window: a 32-bit bitmap in a DC; colours premultiplied by its alpha once `finish` has run.
struct Card {
    dc: HDC,
    bitmap: *mut c_void,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Drop for Card {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.bitmap);
        }
    }
}

impl Card {
    /// A card of a size, filled with a colour (r, g, b).
    unsafe fn new(w: i32, h: i32, fill: (u8, u8, u8)) -> Option<Card> {
        let (w, h) = (w.max(1), h.max(1));
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        ReleaseDC(null_mut(), screen);
        if dc.is_null() {
            return None;
        }
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
            DeleteDC(dc);
            return None;
        }
        SelectObject(dc, bitmap);
        SetBkMode(dc, TRANSPARENT as i32);
        let card = Card { dc, bitmap, bits: bits as *mut u32, w, h };
        let c = (fill.0 as u32) << 16 | (fill.1 as u32) << 8 | fill.2 as u32;
        for p in card.pixels().iter_mut() {
            *p = c;
        }
        Some(card)
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn pixels(&self) -> &mut [u32] {
        std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize)
    }

    /// Fills a rectangle with rounded corners in a colour laid over what is there, `amount` of it (before `finish`).
    unsafe fn round_rect(&self, r: RECT, radius: f32, color: (u8, u8, u8), amount: f32) {
        let px = self.pixels();
        for y in r.top.max(0)..r.bottom.min(self.h) {
            for x in r.left.max(0)..r.right.min(self.w) {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let dx = (r.left as f32 + radius - fx).max(fx - (r.right as f32 - radius)).max(0.0);
                let dy = (r.top as f32 + radius - fy).max(fy - (r.bottom as f32 - radius)).max(0.0);
                let cover = (0.5 - ((dx * dx + dy * dy).sqrt() - radius)).clamp(0.0, 1.0) * amount;
                let Some(p) = px.get_mut((y * self.w + x) as usize) else { continue };
                let mix = |old: u32, new: u8| ((old as f32) * (1.0 - cover) + new as f32 * cover).round() as u32;
                *p = mix((*p >> 16) & 255, color.0) << 16 | mix((*p >> 8) & 255, color.1) << 8 | mix(*p & 255, color.2);
            }
        }
    }

    /// A round dot.
    unsafe fn dot(&self, cx: i32, cy: i32, r: i32, color: (u8, u8, u8)) {
        self.round_rect(RECT { left: cx - r, top: cy - r, right: cx + r, bottom: cy + r }, r as f32, color, 1.0);
    }

    /// The card's shape into the alpha channel (rounded corners of `radius`, a lighter hairline rim), colours premultiplied.
    unsafe fn finish(&self, radius: f32, opacity: f32, scale: f32) {
        let (w, h) = (self.w as f32, self.h as f32);
        let px = self.pixels();
        for y in 0..self.h {
            for x in 0..self.w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let dx = (radius - fx).max(fx - (w - radius)).max(0.0);
                let dy = (radius - fy).max(fy - (h - radius)).max(0.0);
                let dist = (dx * dx + dy * dy).sqrt() - radius;
                let edge = dist.max(-fx.min(w - fx).min(fy).min(h - fy));
                let coverage = (0.5 - dist).clamp(0.0, 1.0);
                let Some(p) = px.get_mut((y * self.w + x) as usize) else { continue };
                let (mut r, mut g, mut b) = ((*p >> 16) & 255, (*p >> 8) & 255, *p & 255);
                if edge > -1.0 * scale {
                    r = (r + 46).min(255);
                    g = (g + 46).min(255);
                    b = (b + 56).min(255);
                }
                let a = coverage * opacity;
                let pm = |c: u32| ((c as f32) * a).round() as u32;
                *p = ((a * 255.0).round() as u32) << 24 | pm(r) << 16 | pm(g) << 8 | pm(b);
            }
        }
    }
}

const CARD_BG: (u8, u8, u8) = (24, 26, 36);
const GREEN: (u8, u8, u8) = (52, 199, 89);
const ORANGE: (u8, u8, u8) = (255, 159, 10);

/// The capsule that says how many plugins are loaded: the mark, one line, an orange dot when not all are.
unsafe fn toast_card(n: &Notice, scale: f32) -> Option<Card> {
    let px = |v: f32| (v * scale).round() as i32;
    let probe = Card::new(1, 1, CARD_BG)?;
    let f = font(px(14.0), FW_SEMIBOLD as i32);
    let summary = n.summary();
    let all = n.all();
    let (pad, icon, gap, dot) = (px(16.0), px(18.0), px(10.0), if all { 0 } else { px(16.0) });
    let h = px(40.0);
    let w = pad + icon + gap + text_width(probe.dc, f, &summary) + dot + pad + px(4.0);
    drop(probe);
    let card = Card::new(w, h, CARD_BG)?;
    mark(card.dc, pad + icon / 2, h / 2, icon as f32 / 2.0);
    text(card.dc, f, &summary, rgb(245, 245, 250), RECT { left: pad + icon + gap, top: 0, right: w - pad - dot, bottom: h }, false);
    if !all {
        card.dot(w - pad - px(4.0), h / 2, px(4.0), ORANGE);
    }
    DeleteObject(f);
    card.finish(h as f32 / 2.0, 0.92, scale);
    Some(card)
}

/// The corner button of the title screen: the mark and a word, lighter under the pointer.
unsafe fn button_card(n: &Notice, scale: f32, hot: bool) -> Option<Card> {
    let px = |v: f32| (v * scale).round() as i32;
    let probe = Card::new(1, 1, CARD_BG)?;
    let f = font(px(13.0), FW_SEMIBOLD as i32);
    let label = format!("{}  {}", n.word("button"), n.plugins.len());
    let (pad, icon, gap) = (px(12.0), px(14.0), px(7.0));
    let h = px(32.0);
    let w = pad + icon + gap + text_width(probe.dc, f, &label) + pad + px(2.0);
    drop(probe);
    let card = Card::new(w, h, if hot { (44, 47, 62) } else { CARD_BG })?;
    mark(card.dc, pad + icon / 2, h / 2, icon as f32 / 2.0);
    text(card.dc, f, &label, rgb(240, 240, 245), RECT { left: pad + icon + gap, top: 0, right: w - pad, bottom: h }, false);
    DeleteObject(f);
    card.finish(h as f32 / 2.0, 0.94, scale);
    Some(card)
}

/// What is under the pointer in the list.
#[derive(Clone, Copy, PartialEq)]
enum Hot {
    Nothing,
    Close,
    Switch(usize),
}

/// Where things are in the list's card.
#[derive(Default)]
struct PanelHits {
    close: Option<RECT>,
    /// (plugin, its switch)
    switches: Vec<(usize, RECT)>,
    /// how far the rows can scroll
    max_scroll: i32,
}

/// The list: the mark and a heading, a close button; a row per plugin (its name and version, its description, whether it is loaded, and
/// its switch); what the switches do, at the bottom. `scroll` moves the rows up.
unsafe fn panel_card(n: &Notice, scale: f32, w: i32, max_h: i32, hot: Hot, scroll: i32) -> Option<(Card, PanelHits)> {
    let px = |v: f32| (v * scale).round() as i32;
    let f_head = font(px(18.0), FW_SEMIBOLD as i32);
    let f_name = font(px(15.0), FW_SEMIBOLD as i32);
    let f_small = font(px(12.5), FW_NORMAL as i32);
    let fonts = [f_head, f_name, f_small];
    let (pad, head_h, row_h, foot_h) = (px(24.0), px(68.0), px(76.0), px(48.0));
    let rows_h = row_h * n.plugins.len() as i32;
    let h = (head_h + rows_h + foot_h).min(max_h).max(head_h + row_h + foot_h);
    let view_h = h - head_h - foot_h;
    let max_scroll = (rows_h - view_h).max(0);
    let scroll = scroll.clamp(0, max_scroll);
    let card = Card::new(w, h, CARD_BG)?;
    let mut hits = PanelHits { max_scroll, ..Default::default() };
    // the heading
    mark(card.dc, pad + px(11.0), head_h / 2, px(11.0) as f32);
    let title_x = pad + px(32.0);
    text(card.dc, f_head, &n.word("heading"), rgb(250, 250, 252), RECT { left: title_x, top: 0, right: w - pad - px(120.0), bottom: head_h }, false);
    let count = format!("{} / {}  {}", n.loaded(), n.wanted(), n.word("loaded"));
    let close = RECT { left: w - pad - px(32.0), top: (head_h - px(32.0)) / 2, right: w - pad, bottom: (head_h + px(32.0)) / 2 };
    text(card.dc, f_small, &count, rgb(150, 152, 165), RECT { left: title_x, top: 0, right: close.left - px(14.0), bottom: head_h }, true);
    card.dot(close.left + px(16.0), close.top + px(16.0), px(16.0), if hot == Hot::Close { (62, 65, 82) } else { (40, 43, 56) });
    // the ×, as two strokes of dots
    for k in -5..=5 {
        let d = px(k as f32 * 0.9);
        let (cx, cy) = (close.left + px(16.0), close.top + px(16.0));
        card.dot(cx + d, cy + d, px(1.1).max(1), (210, 212, 220));
        card.dot(cx + d, cy - d, px(1.1).max(1), (210, 212, 220));
    }
    hits.close = Some(close);
    card.round_rect(RECT { left: 0, top: head_h - 1, right: w, bottom: head_h }, 0.0, (52, 55, 70), 1.0);
    // the rows, clipped to their area
    for (i, p) in n.plugins.iter().enumerate() {
        let top = head_h + row_h * i as i32 - scroll;
        if top + row_h <= head_h || top >= head_h + view_h {
            continue;
        }
        let row = RECT { left: 0, top: top.max(head_h), right: w, bottom: (top + row_h).min(head_h + view_h) };
        if i > 0 && top > head_h {
            card.round_rect(RECT { left: pad, top, right: w - pad, bottom: top + 1 }, 0.0, (44, 47, 60), 1.0);
        }
        let sw = (px(46.0), px(28.0));
        let switch = RECT { left: w - pad - sw.0, top: top + (row_h - sw.1) / 2, right: w - pad, bottom: top + (row_h + sw.1) / 2 };
        let text_right = switch.left - px(20.0);
        // name and version
        let name_w = text_width(card.dc, f_name, &p.name);
        let name_rect = RECT { left: pad, top: top + px(10.0), right: text_right, bottom: top + px(32.0) };
        if name_rect.top >= row.top && name_rect.bottom <= row.bottom {
            text(card.dc, f_name, &p.name, rgb(245, 245, 250), name_rect, false);
            if !p.version.is_empty() && pad + name_w + px(10.0) < text_right {
                let v = format!("v{}", p.version.trim_start_matches('v'));
                text(card.dc, f_small, &v, rgb(130, 133, 148), RECT { left: pad + name_w + px(10.0), ..name_rect }, false);
            }
        }
        // the description
        let desc_rect = RECT { left: pad, top: top + px(32.0), right: text_right, bottom: top + px(51.0) };
        if desc_rect.top >= row.top && desc_rect.bottom <= row.bottom {
            text(card.dc, f_small, &p.description, rgb(160, 163, 178), desc_rect, false);
        }
        // its state: loaded, switched off, or why it is not loaded
        let (state, color, dot) = if !p.was_on {
            (n.word("off"), rgb(130, 133, 148), (90, 93, 108))
        } else if p.loaded {
            (n.word("loaded"), rgb(120, 214, 140), GREEN)
        } else {
            (p.why.clone(), rgb(255, 179, 64), ORANGE)
        };
        let state_rect = RECT { left: pad + px(14.0), top: top + px(52.0), right: text_right, bottom: top + px(68.0) };
        if state_rect.top >= row.top && state_rect.bottom <= row.bottom {
            card.dot(pad + px(4.0), (state_rect.top + state_rect.bottom) / 2, px(3.5), dot);
            text(card.dc, f_small, &state, color, state_rect, false);
        }
        // the switch: green with the knob at the right when on
        if switch.top >= row.top && switch.bottom <= row.bottom {
            let lit = hot == Hot::Switch(i);
            let track = if p.on { (48, 190, 84) } else if lit { (92, 95, 112) } else { (72, 75, 90) };
            card.round_rect(switch, sw.1 as f32 / 2.0, track, 1.0);
            let r = sw.1 / 2 - px(3.0);
            let kx = if p.on { switch.right - sw.1 / 2 } else { switch.left + sw.1 / 2 };
            card.dot(kx, (switch.top + switch.bottom) / 2, r, (250, 250, 252));
            hits.switches.push((i, switch));
        }
    }
    // what the switches do
    card.round_rect(RECT { left: 0, top: h - foot_h, right: w, bottom: h - foot_h + 1 }, 0.0, (52, 55, 70), 1.0);
    let changed = n.plugins.iter().any(|p| p.on != p.was_on);
    let foot_color = if changed { rgb(255, 196, 92) } else { rgb(130, 133, 148) };
    text(card.dc, f_small, &n.word("footer"), foot_color, RECT { left: pad, top: h - foot_h, right: w - pad, bottom: h }, false);
    text(card.dc, f_small, "Stellaris Launcher", rgb(96, 99, 114), RECT { left: pad, top: h - foot_h, right: w - pad, bottom: h }, true);
    // a hint of more rows below or above
    if scroll < max_scroll {
        card.round_rect(RECT { left: w / 2 - px(18.0), top: h - foot_h - px(6.0), right: w / 2 + px(18.0), bottom: h - foot_h - px(3.0) }, px(1.5) as f32, (90, 93, 108), 1.0);
    }
    for f in fonts {
        DeleteObject(f);
    }
    card.finish(px(18.0) as f32, 0.97, scale);
    Some((card, hits))
}

// ------------------------------------------------------------------ the windows

thread_local! {
    /// clicks on our windows: (window, x, y in it)
    static CLICKS: RefCell<Vec<(HWND, i32, i32)>> = const { RefCell::new(Vec::new()) };
    /// the wheel turned over our windows, in notches (120 a notch)
    static WHEEL: RefCell<i32> = const { RefCell::new(0) };
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    if msg == WM_LBUTTONUP {
        let (x, y) = ((l & 0xffff) as i16 as i32, ((l >> 16) & 0xffff) as i16 as i32);
        CLICKS.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                c.push((hwnd, x, y));
            }
        });
        return 0;
    }
    if msg == WM_MOUSEWHEEL {
        let delta = ((w >> 16) & 0xffff) as i16 as i32;
        WHEEL.with(|d| {
            if let Ok(mut d) = d.try_borrow_mut() {
                *d += delta;
            }
        });
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}

/// A layered window and what it shows.
struct Layer {
    hwnd: HWND,
    card: Option<Card>,
    /// what is on the screen: (alpha, x, y)
    shown: Option<(u8, i32, i32)>,
}

impl Layer {
    unsafe fn new(module: HMODULE, class: &[u16], click_through: bool) -> Layer {
        let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        let hwnd = CreateWindowExW(ex, class.as_ptr(), null(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), module, null());
        Layer { hwnd, card: None, shown: None }
    }

    fn set(&mut self, card: Option<Card>) {
        self.card = card;
        self.shown = None;
    }

    fn size(&self) -> (i32, i32) {
        self.card.as_ref().map(|c| (c.w, c.h)).unwrap_or((0, 0))
    }

    fn rect(&self, at: POINT) -> RECT {
        let (w, h) = self.size();
        RECT { left: at.x, top: at.y, right: at.x + w, bottom: at.y + h }
    }

    /// Shows the card at a place with an opacity; only when that changes.
    unsafe fn present(&mut self, at: POINT, opacity: f32) {
        let (Some(card), false) = (&self.card, self.hwnd.is_null()) else { return };
        let alpha = (opacity.clamp(0.0, 1.0) * 255.0) as u8;
        if self.shown == Some((alpha, at.x, at.y)) {
            return;
        }
        if self.shown.is_none() {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        let size = SIZE { cx: card.w, cy: card.h };
        let from = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(self.hwnd, null_mut(), &at, &size, card.dc, &from, 0, &blend, ULW_ALPHA);
        self.shown = Some((alpha, at.x, at.y));
    }

    unsafe fn destroy(&mut self) {
        if !self.hwnd.is_null() {
            DestroyWindow(self.hwnd);
            self.hwnd = null_mut();
        }
        self.card = None;
    }
}

fn approach(now: f32, target: f32, step: f32) -> f32 {
    if now < target {
        (now + step).min(target)
    } else {
        (now - step).max(target)
    }
}

/// The list's size for the game's client area: wide and tall enough to read, within the game.
fn panel_size(client: &RECT, scale: f32) -> (i32, i32) {
    let (cw, ch) = (client.right - client.left, client.bottom - client.top);
    let w = ((cw as f32 * 0.5) as i32).clamp((560.0 * scale) as i32, (760.0 * scale) as i32).min(cw - (40.0 * scale) as i32);
    let h = ((ch as f32 * 0.78) as i32).max((240.0 * scale) as i32);
    (w.max(1), h.max(1))
}

fn run(module: HMODULE) {
    let Some(mut notice) = read_notice() else { return };
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // the game's window
        let mut waited = 0;
        let game = loop {
            let g = game_window();
            if !g.is_null() {
                break g;
            }
            if waited >= WAIT_FOR_GAME_MS {
                return;
            }
            Sleep(250);
            waited += 250;
        };
        let scale = (GetDpiForWindow(game).max(96) as f32) / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let class = wide("StellarisLauncherNotice");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let mut toast = Layer::new(module, &class, true);
        // the capsule when any plugin was to be loaded
        if notice.wanted() > 0 {
            toast.set(toast_card(&notice, scale));
        }
        let mut button = Layer::new(module, &class, false);
        let mut panel = Layer::new(module, &class, false);
        // the list is for the title screen: not when a game is loaded already (the game was started into a save)
        let mut title_screen = !notice.in_game();
        let mut button_hot = false;
        button.set(button_card(&notice, scale, false));
        let mut hits = PanelHits::default();
        let mut panel_open = false;
        let mut hot = Hot::Nothing;
        let mut scroll = 0;
        let mut panel_dirty = false;
        let (mut toast_ms, mut button_t, mut panel_t, mut was_down) = (0.0f32, 0.0f32, 0.0f32, false);
        let mut last = Instant::now();
        let mut last_log_check = Instant::now();
        let mut msg: MSG = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if IsWindow(game) == 0 {
                break;
            }
            let dt = last.elapsed().as_secs_f32() * 1000.0;
            last = Instant::now();
            if title_screen && last_log_check.elapsed().as_millis() >= 1000 {
                last_log_check = Instant::now();
                if notice.in_game() {
                    title_screen = false;
                }
            }
            let ours = [toast.hwnd, button.hwnd, panel.hwnd];
            let front_window = GetForegroundWindow();
            let front = !front_window.is_null() && (front_window == game || ours.contains(&front_window));
            let client = client_on_screen(game);
            let mut cursor = POINT { x: 0, y: 0 };
            GetCursorPos(&mut cursor);

            // the capsule: fades in, stays, fades out (the time runs only while the game is in front)
            if toast.card.is_some() {
                if front {
                    toast_ms += dt;
                }
                let t = if toast_ms < FADE_IN_MS {
                    toast_ms / FADE_IN_MS
                } else if toast_ms > FADE_IN_MS + HOLD_MS {
                    1.0 - (toast_ms - FADE_IN_MS - HOLD_MS) / FADE_OUT_MS
                } else {
                    1.0
                };
                let (w, _) = toast.size();
                toast.present(POINT { x: client.right - w - px(24.0), y: client.top + px(24.0) }, if front { t } else { 0.0 });
                if toast_ms >= FADE_IN_MS + HOLD_MS + FADE_OUT_MS {
                    toast.destroy();
                }
            }

            // the corner of the title screen: hovering it shows the button, the button opens the list
            let (bw, bh) = button.size();
            let margin = px(16.0);
            let button_at = POINT { x: client.right - bw - margin, y: client.bottom - bh - margin };
            let button_rect = button.rect(button_at);
            let zone = RECT { left: client.right - px(96.0), top: client.bottom - px(80.0), right: client.right, bottom: client.bottom };
            if !title_screen || !front {
                panel_open = false;
            }
            let want = title_screen && front && (panel_open || contains(&zone, cursor) || contains(&button_rect, cursor));
            button_t = approach(button_t, if want { 1.0 } else { 0.0 }, dt / FADE_MS);
            let now_hot = want && (contains(&button_rect, cursor) || panel_open);
            if now_hot != button_hot {
                button_hot = now_hot;
                button.set(button_card(&notice, scale, now_hot));
            }
            button.present(button_at, button_t);

            // the list: in the middle of the game
            let (pw, ph) = panel_size(&client, scale);
            let (cur_w, cur_h) = panel.size();
            let panel_at = POINT { x: (client.left + client.right - cur_w) / 2, y: (client.top + client.bottom - cur_h) / 2 };
            let panel_rect = panel.rect(panel_at);
            let local = POINT { x: cursor.x - panel_at.x, y: cursor.y - panel_at.y };

            // clicks on our windows
            let clicks: Vec<(HWND, i32, i32)> = CLICKS.with(|c| c.try_borrow_mut().map(|mut c| std::mem::take(&mut *c)).unwrap_or_default());
            for (hwnd, x, y) in clicks {
                let at = POINT { x, y };
                if hwnd == button.hwnd && button_t > 0.5 {
                    panel_open = !panel_open;
                    if panel_open {
                        scroll = 0;
                        hot = Hot::Nothing;
                        panel_dirty = true;
                    }
                } else if hwnd == panel.hwnd && panel_open {
                    if hits.close.as_ref().is_some_and(|r| contains(r, at)) {
                        panel_open = false;
                    }
                    let flip = hits.switches.iter().find(|(_, r)| contains(r, at)).map(|(i, _)| *i);
                    if let Some(i) = flip {
                        notice.flip(i);
                        panel_dirty = true;
                    }
                }
            }
            // the wheel scrolls the rows
            let wheel = WHEEL.with(|d| d.try_borrow_mut().map(|mut d| std::mem::take(&mut *d)).unwrap_or(0));
            if wheel != 0 && panel_open {
                scroll = (scroll - wheel * px(76.0) / 120).clamp(0, hits.max_scroll);
                panel_dirty = true;
            }
            // a click anywhere else closes the list
            let down = (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0;
            if down && !was_down && panel_open && !contains(&panel_rect, cursor) && !contains(&button_rect, cursor) {
                panel_open = false;
            }
            was_down = down;

            if panel_open {
                let now_hot = if hits.close.as_ref().is_some_and(|r| contains(r, local)) {
                    Hot::Close
                } else if let Some((i, _)) = hits.switches.iter().find(|(_, r)| contains(r, local)) {
                    Hot::Switch(*i)
                } else {
                    Hot::Nothing
                };
                if now_hot != hot || panel_dirty || (pw, ph) != (cur_w, cur_h.max(ph)) && panel.card.is_none() {
                    hot = now_hot;
                    panel_dirty = false;
                    if let Some((card, h)) = panel_card(&notice, scale, pw, ph, hot, scroll) {
                        panel.set(Some(card));
                        hits = h;
                    }
                }
            }
            panel_t = approach(panel_t, if panel_open { 1.0 } else { 0.0 }, dt / FADE_MS);
            if panel.card.is_some() {
                let (w, h) = panel.size();
                panel.present(POINT { x: (client.left + client.right - w) / 2, y: (client.top + client.bottom - h) / 2 }, panel_t);
            }

            // done: the capsule has been shown and the title screen is over
            if toast.card.is_none() && !title_screen && button_t <= 0.0 && panel_t <= 0.0 {
                break;
            }
            Sleep(if front { TICK_MS } else { IDLE_TICK_MS });
        }
        for l in [&mut toast, &mut button, &mut panel] {
            l.destroy();
        }
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            DispatchMessageW(&msg);
        }
        UnregisterClassW(class.as_ptr(), module);
    }
}

// ------------------------------------------------------------------ a look without the game

/// For a look at the pictures without the game (development): draws the capsule (`what` 0), the corner button (1, 2 under the pointer)
/// or the list (3, 4 with the second switch under the pointer and the first one flipped) of a notice file into a 32-bit BMP
/// (premultiplied alpha) at `scale_percent`, the list for a game area of `area_w` × `area_h`. Returns 1 when written. The caller holds its
/// own reference to the DLL (its loading thread lets go of one).
#[no_mangle]
pub unsafe extern "C" fn stl_notice_preview(notice: *const u16, out: *const u16, scale_percent: u32, what: u32, area_w: i32, area_h: i32) -> i32 {
    let read = |p: *const u16| {
        if p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
    };
    let (Some(notice), Some(out)) = (read(notice), read(out)) else { return 0 };
    let Some(mut n) = std::fs::read_to_string(notice).ok().and_then(|t| parse(&t)) else { return 0 };
    let scale = scale_percent.max(50) as f32 / 100.0;
    let card = match what {
        0 => toast_card(&n, scale),
        1 | 2 => button_card(&n, scale, what == 2),
        _ => {
            n.toggles = None;
            if what == 4 {
                n.flip(0);
            }
            let (w, h) = panel_size(&RECT { left: 0, top: 0, right: area_w, bottom: area_h }, scale);
            panel_card(&n, scale, w, h, if what == 4 { Hot::Switch(1) } else { Hot::Nothing }, 0).map(|(c, _)| c)
        }
    };
    let Some(card) = card else { return 0 };
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
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader = BITMAPINFOHEADER { biSize: 40, biWidth: card.w, biHeight: -card.h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
    GetDIBits(card.dc, card.bitmap, 0, card.h as u32, px.as_mut_ptr() as *mut c_void, &mut info, DIB_RGB_COLORS);
    for p in &px {
        bytes.extend_from_slice(&p.to_le_bytes());
    }
    std::fs::write(out, bytes).is_ok() as i32
}
