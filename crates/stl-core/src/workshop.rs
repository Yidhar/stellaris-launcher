//! Uploading a mod to the Steam Workshop, the way the official launcher and the game's own uploader do it: through the Steamworks API
//! (`steam_api64.dll`, which the game ships), as the Steam user who is logged in. Nothing here sees a password: Steam must be running and
//! signed in, and it does the talking.
//!
//! The library is loaded from the game folder and its flat C functions are called by name; the interface versions are the ones that library
//! exports (`SteamUGC016`, `SteamUtils010`, `SteamUser021`). Calls that answer later (create, submit) are polled with `IsAPICallCompleted`.

use crate::{bail, Context, Result};
use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

pub const APP_ID: u32 = 281990;

/// Who may see the item (`ERemoteStoragePublishedFileVisibility`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public = 0,
    FriendsOnly = 1,
    Private = 2,
    Unlisted = 3,
}

impl Visibility {
    pub fn parse(s: &str) -> Option<Visibility> {
        Some(match s {
            "public" => Visibility::Public,
            "friends" => Visibility::FriendsOnly,
            "private" => Visibility::Private,
            "unlisted" => Visibility::Unlisted,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Upload {
    pub title: String,
    /// set only when the item is created (later, the Workshop page is where it is edited)
    pub description: String,
    /// the content folder, sent whole
    pub content: PathBuf,
    /// a picture of under 1 MB, usually `thumbnail.png` of the content
    pub preview: Option<PathBuf>,
    pub tags: Vec<String>,
    /// None: leave it as it is (a new item is made private)
    pub visibility: Option<Visibility>,
    pub change_note: String,
    /// the item to update; None makes a new one
    pub existing: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: u64,
    pub created: bool,
    /// the user has not accepted the Workshop agreement yet: the item stays hidden until they do (on its page)
    pub needs_agreement: bool,
}

pub fn item_url(id: u64) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

/// What an upload is doing, for a progress line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Connecting,
    Creating,
    /// `EItemUpdateStatus`: 1 preparing config, 2 preparing content, 3 uploading content, 4 uploading preview, 5 committing
    Uploading(i32),
    Done,
}

#[repr(C)]
#[derive(Default)]
struct CreateItemResult {
    result: i32,
    id: u64,
    needs_agreement: bool,
}

#[repr(C)]
#[derive(Default)]
struct SubmitItemUpdateResult {
    result: i32,
    needs_agreement: bool,
    id: u64,
}

#[repr(C)]
struct ParamStringArray {
    strings: *const *const c_char,
    count: i32,
}

const CREATE_ITEM_RESULT: i32 = 3403;
const SUBMIT_ITEM_UPDATE_RESULT: i32 = 3404;

fn result_text(r: i32) -> String {
    let what = match r {
        2 => "failed",
        3 => "no connection to Steam",
        8 => "invalid parameter (the title, a tag or a path)",
        9 => "a file was not found (the content folder or the preview)",
        10 => "Steam is busy",
        15 => "access denied (is this your item? is the Workshop agreement accepted?)",
        16 => "timed out",
        17 => "this account is banned from the Workshop",
        21 => "not logged on to Steam",
        25 => "a limit was exceeded (the preview must be under 1 MB)",
        _ => "",
    };
    if what.is_empty() { format!("Steam answered EResult {r}") } else { format!("Steam answered EResult {r}: {what}") }
}

static STEAM: Mutex<()> = Mutex::new(());

struct Api {
    lib: HMODULE,
}

impl Api {
    fn load(game_dir: &Path) -> Result<Api> {
        let dll = game_dir.join("steam_api64.dll");
        if !dll.is_file() {
            bail!("{} is missing (the Workshop needs the Steam version of the game)", dll.display());
        }
        let wide: Vec<u16> = dll.as_os_str().encode_wide_nul();
        let lib = unsafe { LoadLibraryW(wide.as_ptr()) };
        if lib.is_null() {
            bail!("cannot load {}", dll.display());
        }
        Ok(Api { lib })
    }

    /// A function of the library, as the type `F` (a `extern "C" fn` type).
    unsafe fn f<F: Copy>(&self, name: &str) -> Result<F> {
        let c = CString::new(name).unwrap();
        let p = unsafe { GetProcAddress(self.lib, c.as_ptr() as *const u8) };
        match p {
            Some(p) => Ok(unsafe { std::mem::transmute_copy::<_, F>(&p) }),
            None => bail!("steam_api64.dll has no {name}: this game build's Steam library is not the one this launcher knows"),
        }
    }
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// An initialised Steam API; shut down when dropped.
struct Session {
    api: Api,
    utils: *mut c_void,
    ugc: *mut c_void,
    user: *mut c_void,
    run_callbacks: unsafe extern "C" fn(),
}

impl Session {
    fn open(game_dir: &Path) -> Result<Session> {
        let api = Api::load(game_dir)?;
        // the API learns which game it is from these (or from a steam_appid.txt in the working folder)
        std::env::set_var("SteamAppId", APP_ID.to_string());
        std::env::set_var("SteamGameId", APP_ID.to_string());
        unsafe {
            let init: unsafe extern "C" fn() -> bool = api.f("SteamAPI_Init")?;
            if !init() {
                bail!("the Steam API did not start: Steam must be running and signed in to an account that owns Stellaris");
            }
            let utils: unsafe extern "C" fn() -> *mut c_void = api.f("SteamAPI_SteamUtils_v010")?;
            let ugc: unsafe extern "C" fn() -> *mut c_void = api.f("SteamAPI_SteamUGC_v016")?;
            let user: unsafe extern "C" fn() -> *mut c_void = api.f("SteamAPI_SteamUser_v021")?;
            let run_callbacks = api.f("SteamAPI_RunCallbacks")?;
            let s = Session { utils: utils(), ugc: ugc(), user: user(), run_callbacks, api };
            if s.utils.is_null() || s.ugc.is_null() || s.user.is_null() {
                bail!("the Steam API started but did not give its interfaces");
            }
            Ok(s)
        }
    }

    fn app_id(&self) -> Result<u32> {
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u32 = self.api.f("SteamAPI_ISteamUtils_GetAppID")?;
            Ok(f(self.utils))
        }
    }

    fn steam_id(&self) -> Result<u64> {
        unsafe {
            let f: unsafe extern "C" fn(*mut c_void) -> u64 = self.api.f("SteamAPI_ISteamUser_GetSteamID")?;
            Ok(f(self.user))
        }
    }

    /// Waits for an asynchronous call and reads its result into `T`.
    fn wait<T: Default>(&self, call: u64, callback: i32, timeout: Duration, mut tick: impl FnMut()) -> Result<T> {
        if call == 0 {
            bail!("Steam refused the request");
        }
        unsafe {
            let done: unsafe extern "C" fn(*mut c_void, u64, *mut bool) -> bool = self.api.f("SteamAPI_ISteamUtils_IsAPICallCompleted")?;
            let get: unsafe extern "C" fn(*mut c_void, u64, *mut c_void, i32, i32, *mut bool) -> bool = self.api.f("SteamAPI_ISteamUtils_GetAPICallResult")?;
            let until = Instant::now() + timeout;
            loop {
                (self.run_callbacks)();
                let mut failed = false;
                if done(self.utils, call, &mut failed) {
                    let mut out = T::default();
                    let mut io_failed = false;
                    if !get(self.utils, call, &mut out as *mut T as *mut c_void, std::mem::size_of::<T>() as i32, callback, &mut io_failed) || io_failed || failed {
                        bail!("the answer from Steam could not be read");
                    }
                    return Ok(out);
                }
                if Instant::now() > until {
                    bail!("Steam did not answer in time");
                }
                tick();
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            if let Ok(shutdown) = self.api.f::<unsafe extern "C" fn()>("SteamAPI_Shutdown") {
                shutdown();
            }
        }
    }
}

/// Starts the Steam API, reads which game and account it sees, and stops it again. Changes nothing.
pub fn check(game_dir: &Path) -> Result<(u32, u64)> {
    let _one = STEAM.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::open(game_dir)?;
    Ok((s.app_id()?, s.steam_id()?))
}

/// Creates the Workshop item (when `u.existing` is None, after which `on_created` is told its id, so that the caller can write it into the
/// mod's descriptors before the content is sent) and uploads the content folder, preview, title, tags and change note.
pub fn upload(game_dir: &Path, u: &Upload, on_created: &mut dyn FnMut(u64) -> Result<()>, progress: &mut dyn FnMut(Stage, u64, u64)) -> Result<Outcome> {
    if !u.content.is_dir() {
        bail!("the content folder {} does not exist", u.content.display());
    }
    if let Some(p) = &u.preview {
        let size = std::fs::metadata(p).with_context(|| format!("the preview {} cannot be read", p.display()))?.len();
        if size >= 1 << 20 {
            bail!("the preview {} is {} KB; the Workshop takes less than 1024 KB", p.display(), size / 1024);
        }
    }
    let _one = STEAM.lock().unwrap_or_else(|e| e.into_inner());
    progress(Stage::Connecting, 0, 0);
    let s = Session::open(game_dir)?;
    let mut created = false;
    let mut needs_agreement = false;
    let id = match u.existing {
        Some(id) => id,
        None => {
            progress(Stage::Creating, 0, 0);
            let r: CreateItemResult = unsafe {
                let create: unsafe extern "C" fn(*mut c_void, u32, i32) -> u64 = s.api.f("SteamAPI_ISteamUGC_CreateItem")?;
                s.wait(create(s.ugc, APP_ID, 0), CREATE_ITEM_RESULT, Duration::from_secs(90), || {})?
            };
            if r.result != 1 {
                bail!("the Workshop item could not be created: {}", result_text(r.result));
            }
            created = true;
            needs_agreement |= r.needs_agreement;
            on_created(r.id)?;
            r.id
        }
    };
    let cstr = |s: &str| CString::new(s.replace('\0', "")).unwrap();
    let path_str = |p: &Path| cstr(&p.to_string_lossy());
    unsafe {
        let start: unsafe extern "C" fn(*mut c_void, u32, u64) -> u64 = s.api.f("SteamAPI_ISteamUGC_StartItemUpdate")?;
        let set_str = |name: &str| s.api.f::<unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> bool>(name);
        let handle = start(s.ugc, APP_ID, id);
        if handle == u64::MAX || handle == 0 {
            bail!("Steam would not start an update of item {id}");
        }
        let title = cstr(&u.title);
        if !set_str("SteamAPI_ISteamUGC_SetItemTitle")?(s.ugc, handle, title.as_ptr()) {
            bail!("Steam did not take the title");
        }
        if created && !u.description.trim().is_empty() {
            let d = cstr(&u.description);
            set_str("SteamAPI_ISteamUGC_SetItemDescription")?(s.ugc, handle, d.as_ptr());
        }
        let content = path_str(&u.content);
        if !set_str("SteamAPI_ISteamUGC_SetItemContent")?(s.ugc, handle, content.as_ptr()) {
            bail!("Steam did not take the content folder");
        }
        if let Some(p) = &u.preview {
            let preview = path_str(p);
            if !set_str("SteamAPI_ISteamUGC_SetItemPreview")?(s.ugc, handle, preview.as_ptr()) {
                bail!("Steam did not take the preview picture");
            }
        }
        if !u.tags.is_empty() {
            let owned: Vec<CString> = u.tags.iter().map(|t| cstr(t)).collect();
            let ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
            let arr = ParamStringArray { strings: ptrs.as_ptr(), count: ptrs.len() as i32 };
            let tags: unsafe extern "C" fn(*mut c_void, u64, *const ParamStringArray) -> bool = s.api.f("SteamAPI_ISteamUGC_SetItemTags")?;
            tags(s.ugc, handle, &arr);
        }
        let visibility = u.visibility.or(if created { Some(Visibility::Private) } else { None });
        if let Some(v) = visibility {
            let vis: unsafe extern "C" fn(*mut c_void, u64, i32) -> bool = s.api.f("SteamAPI_ISteamUGC_SetItemVisibility")?;
            vis(s.ugc, handle, v as i32);
        }
        let submit: unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> u64 = s.api.f("SteamAPI_ISteamUGC_SubmitItemUpdate")?;
        let note = cstr(&u.change_note);
        let call = submit(s.ugc, handle, if u.change_note.is_empty() { std::ptr::null() } else { note.as_ptr() });
        let status: unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut u64) -> i32 = s.api.f("SteamAPI_ISteamUGC_GetItemUpdateProgress")?;
        let r: SubmitItemUpdateResult = s.wait(call, SUBMIT_ITEM_UPDATE_RESULT, Duration::from_secs(3600), || {
            let (mut done, mut total) = (0u64, 0u64);
            let st = status(s.ugc, handle, &mut done, &mut total);
            progress(Stage::Uploading(st), done, total);
        })?;
        if r.result != 1 {
            bail!("the upload of item {id} failed: {}", result_text(r.result));
        }
        needs_agreement |= r.needs_agreement;
    }
    progress(Stage::Done, 0, 0);
    Ok(Outcome { id, created, needs_agreement })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_layouts_are_the_steam_ones() {
        // Steam packs its callback structs to 8 on Windows
        assert_eq!(std::mem::size_of::<CreateItemResult>(), 24);
        assert_eq!(std::mem::size_of::<SubmitItemUpdateResult>(), 16);
        assert_eq!(std::mem::offset_of!(SubmitItemUpdateResult, id), 8);
        assert_eq!(std::mem::offset_of!(CreateItemResult, needs_agreement), 16);
    }

    #[test]
    fn visibility_names_and_urls() {
        assert_eq!(Visibility::parse("friends"), Some(Visibility::FriendsOnly));
        assert_eq!(Visibility::parse("x"), None);
        assert_eq!(item_url(7), "https://steamcommunity.com/sharedfiles/filedetails/?id=7");
        assert!(result_text(25).contains("1 MB"));
    }
}
