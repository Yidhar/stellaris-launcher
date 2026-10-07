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
    /// what Steam says about the item right after the upload (None when it would not say)
    pub details: Option<ItemDetails>,
    /// when the upload started (seconds since 1970), to tell whether Steam's copy is the new one
    pub started: i64,
}

/// An item as Steam's client shows it to its owner (private items too): from `SteamUGCDetails_t`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemDetails {
    pub id: u64,
    pub result: i32,
    pub app_id: u32,
    pub title: String,
    pub owner: u64,
    pub created: u32,
    pub updated: u32,
    /// `ERemoteStoragePublishedFileVisibility`: 0 public, 1 friends, 2 private, 3 unlisted
    pub visibility: i32,
    pub banned: bool,
    pub file_size: i32,
}

// `SteamUGCDetails_t` (Steamworks, 8-byte packing on Windows): the offsets of the fields read. The struct only ever grew at its end, so a
// buffer far larger than it is filled and these are read from it.
const D_ID: usize = 0;
const D_RESULT: usize = 8;
const D_CONSUMER_APP: usize = 20;
const D_TITLE: usize = 24; // char[129]
const D_OWNER: usize = 8160; // after char[8000] of description, aligned to 8
const D_CREATED: usize = 8168;
const D_UPDATED: usize = 8172;
const D_VISIBILITY: usize = 8180;
const D_BANNED: usize = 8184;
const D_FILE_SIZE: usize = 9492;
const UGC_QUERY_COMPLETED: i32 = 3401;

#[repr(C)]
#[derive(Default)]
struct QueryCompleted {
    handle: u64,
    result: i32,
    returned: u32,
    total: u32,
    cached: bool,
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

/// The step of an upload that went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// starting the Steam API
    Connect,
    /// `CreateItem`
    Create,
    /// `StartItemUpdate`
    StartUpdate,
    Title,
    Description,
    Content,
    Preview,
    Tags,
    Visibility,
    /// `SubmitItemUpdate` and the transfer
    Submit,
}

/// What went wrong in an upload, for an explanation (`uploadcheck::explain`) and a report: the step, Steam's `EResult` if it gave one, how far
/// the transfer had come (`EItemUpdateStatus`), and whether the item had already been made.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadError {
    pub step: Step,
    pub result: Option<i32>,
    /// the last `EItemUpdateStatus` seen: 1 preparing config, 2 preparing content, 3 uploading content, 4 uploading preview, 5 committing
    pub status: Option<i32>,
    /// the item (made now, or the one being updated)
    pub item: Option<u64>,
    pub created: bool,
    pub message: String,
    /// why Steam's own log (`logs\workshop_log.txt`) says the upload failed: the `EResult` is often only "failed"
    pub steam_log: Option<String>,
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UploadError {}

fn steam_log_path() -> Option<PathBuf> {
    crate::paths::steam_root().map(|r| r.join("logs").join("workshop_log.txt"))
}

fn steam_log_len() -> u64 {
    steam_log_path().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0)
}

/// Why Steam's log says the upload of item `id` failed, among what it wrote after byte `since` (Steam may write it a moment after it
/// answers, so it is waited for a little).
pub fn steam_log_reason(id: u64, since: u64) -> Option<String> {
    let path = steam_log_path()?;
    for _ in 0..10 {
        if let Ok(bytes) = std::fs::read(&path) {
            // a log Steam started over is read whole
            let from = if (since as usize) <= bytes.len() { since as usize } else { 0 };
            if let Some(r) = log_reason(&String::from_utf8_lossy(&bytes[from..]), id) {
                return Some(r);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

/// `[time] [AppID 281990] Upload workshop item 3815197025 failed (Timeout uploading manifest (size 773))` gives what is in the brackets.
fn log_reason(text: &str, id: u64) -> Option<String> {
    let needle = format!("workshop item {id} failed (");
    let line = text.lines().rev().find(|l| l.contains(&needle))?;
    let rest = &line[line.find(&needle)? + needle.len()..];
    let reason = rest.trim_end().strip_suffix(')').unwrap_or(rest).trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

/// The name Valve gives an `EResult` (`k_EResultInvalidParam`), for reports.
pub fn eresult_name(r: i32) -> &'static str {
    match r {
        1 => "k_EResultOK",
        2 => "k_EResultFail",
        3 => "k_EResultNoConnection",
        5 => "k_EResultInvalidPassword",
        6 => "k_EResultLoggedInElsewhere",
        8 => "k_EResultInvalidParam",
        9 => "k_EResultFileNotFound",
        10 => "k_EResultBusy",
        11 => "k_EResultInvalidState",
        14 => "k_EResultDuplicateName",
        15 => "k_EResultAccessDenied",
        16 => "k_EResultTimeout",
        17 => "k_EResultBanned",
        20 => "k_EResultServiceUnavailable",
        21 => "k_EResultNotLoggedOn",
        24 => "k_EResultInsufficientPrivilege",
        25 => "k_EResultLimitExceeded",
        29 => "k_EResultDuplicateRequest",
        33 => "k_EResultLockingFailed",
        34 => "k_EResultLogonSessionReplaced",
        35 => "k_EResultConnectFailed",
        36 => "k_EResultHandshakeFailed",
        37 => "k_EResultIOFailure",
        38 => "k_EResultRemoteDisconnect",
        44 => "k_EResultServiceReadOnly",
        53 => "k_EResultDataCorruption",
        54 => "k_EResultDiskFull",
        84 => "k_EResultRateLimitExceeded",
        _ => "",
    }
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
///
/// A failure is an `UploadError` (inside the `anyhow` error): the step, Steam's answer, how far it came.
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
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut item = u.existing;
    let mut created = false;
    // what Steam's log says about this upload is what it wrote after this point
    let log_mark = steam_log_len();
    let fail = |step: Step, result: Option<i32>, status: Option<i32>, item: Option<u64>, created: bool, message: String| -> anyhow::Error {
        let steam_log = if step == Step::Submit { item.and_then(|id| steam_log_reason(id, log_mark)) } else { None };
        anyhow::Error::new(UploadError { step, result, status, item, created, message, steam_log })
    };
    let s = Session::open(game_dir).map_err(|e| fail(Step::Connect, None, None, item, false, format!("{e:#}")))?;
    let mut needs_agreement = false;
    let id = match u.existing {
        Some(id) => id,
        None => {
            progress(Stage::Creating, 0, 0);
            let r: CreateItemResult = unsafe {
                let create: unsafe extern "C" fn(*mut c_void, u32, i32) -> u64 = s.api.f("SteamAPI_ISteamUGC_CreateItem")?;
                s.wait(create(s.ugc, APP_ID, 0), CREATE_ITEM_RESULT, Duration::from_secs(90), || {}).map_err(|e| fail(Step::Create, None, None, None, false, format!("{e:#}")))?
            };
            if r.result != 1 {
                return Err(fail(Step::Create, Some(r.result), None, None, false, format!("the Workshop item could not be created: {}", result_text(r.result))));
            }
            created = true;
            item = Some(r.id);
            needs_agreement |= r.needs_agreement;
            if let Err(e) = on_created(r.id) {
                return Err(fail(Step::Create, None, None, item, true, format!("item {} was made, but its number could not be recorded: {e:#}", r.id)));
            }
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
            return Err(fail(Step::StartUpdate, None, None, item, created, format!("Steam would not start an update of item {id}")));
        }
        let title = cstr(&u.title);
        if !set_str("SteamAPI_ISteamUGC_SetItemTitle")?(s.ugc, handle, title.as_ptr()) {
            return Err(fail(Step::Title, None, None, item, created, "Steam did not take the title".into()));
        }
        if created && !u.description.trim().is_empty() {
            let d = cstr(&u.description);
            if !set_str("SteamAPI_ISteamUGC_SetItemDescription")?(s.ugc, handle, d.as_ptr()) {
                return Err(fail(Step::Description, None, None, item, created, "Steam did not take the description".into()));
            }
        }
        let content = path_str(&u.content);
        if !set_str("SteamAPI_ISteamUGC_SetItemContent")?(s.ugc, handle, content.as_ptr()) {
            return Err(fail(Step::Content, None, None, item, created, "Steam did not take the content folder".into()));
        }
        if let Some(p) = &u.preview {
            let preview = path_str(p);
            if !set_str("SteamAPI_ISteamUGC_SetItemPreview")?(s.ugc, handle, preview.as_ptr()) {
                return Err(fail(Step::Preview, None, None, item, created, "Steam did not take the preview picture".into()));
            }
        }
        if !u.tags.is_empty() {
            let owned: Vec<CString> = u.tags.iter().map(|t| cstr(t)).collect();
            let ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
            let arr = ParamStringArray { strings: ptrs.as_ptr(), count: ptrs.len() as i32 };
            let tags: unsafe extern "C" fn(*mut c_void, u64, *const ParamStringArray) -> bool = s.api.f("SteamAPI_ISteamUGC_SetItemTags")?;
            if !tags(s.ugc, handle, &arr) {
                return Err(fail(Step::Tags, None, None, item, created, "Steam did not take the tags".into()));
            }
        }
        let visibility = u.visibility.or(if created { Some(Visibility::Private) } else { None });
        if let Some(v) = visibility {
            let vis: unsafe extern "C" fn(*mut c_void, u64, i32) -> bool = s.api.f("SteamAPI_ISteamUGC_SetItemVisibility")?;
            if !vis(s.ugc, handle, v as i32) {
                return Err(fail(Step::Visibility, None, None, item, created, "Steam did not take the visibility".into()));
            }
        }
        let submit: unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> u64 = s.api.f("SteamAPI_ISteamUGC_SubmitItemUpdate")?;
        let note = cstr(&u.change_note);
        let call = submit(s.ugc, handle, if u.change_note.is_empty() { std::ptr::null() } else { note.as_ptr() });
        let status: unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut u64) -> i32 = s.api.f("SteamAPI_ISteamUGC_GetItemUpdateProgress")?;
        let mut status_seen: Option<i32> = None;
        let waited: Result<SubmitItemUpdateResult> = s.wait(call, SUBMIT_ITEM_UPDATE_RESULT, Duration::from_secs(3600), || {
            let (mut done, mut total) = (0u64, 0u64);
            let st = status(s.ugc, handle, &mut done, &mut total);
            if st > 0 {
                status_seen = Some(st);
            }
            progress(Stage::Uploading(st), done, total);
        });
        let r = waited.map_err(|e| fail(Step::Submit, None, status_seen, item, created, format!("{e:#}")))?;
        if r.result != 1 {
            return Err(fail(Step::Submit, Some(r.result), status_seen, item, created, format!("the upload of item {id} failed: {}", result_text(r.result))));
        }
        needs_agreement |= r.needs_agreement;
    }
    progress(Stage::Done, 0, 0);
    // what Steam has now, to check the upload landed (its own client sees private items too)
    let details = query_details(&s, id).ok();
    Ok(Outcome { id, created, needs_agreement, details, started })
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn query_details(s: &Session, id: u64) -> Result<ItemDetails> {
    unsafe {
        let create: unsafe extern "C" fn(*mut c_void, *const u64, u32) -> u64 = s.api.f("SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest")?;
        let cached: unsafe extern "C" fn(*mut c_void, u64, u32) -> bool = s.api.f("SteamAPI_ISteamUGC_SetAllowCachedResponse")?;
        let send: unsafe extern "C" fn(*mut c_void, u64) -> u64 = s.api.f("SteamAPI_ISteamUGC_SendQueryUGCRequest")?;
        let get: unsafe extern "C" fn(*mut c_void, u64, u32, *mut u8) -> bool = s.api.f("SteamAPI_ISteamUGC_GetQueryUGCResult")?;
        let release: unsafe extern "C" fn(*mut c_void, u64) -> bool = s.api.f("SteamAPI_ISteamUGC_ReleaseQueryUGCRequest")?;
        let ids = [id];
        let handle = create(s.ugc, ids.as_ptr(), 1);
        if handle == u64::MAX {
            bail!("Steam would not make the query");
        }
        cached(s.ugc, handle, 0);
        let done: Result<QueryCompleted> = s.wait(send(s.ugc, handle), UGC_QUERY_COMPLETED, Duration::from_secs(30), || {});
        let out = (|| {
            let q = done?;
            if q.result != 1 || q.returned == 0 {
                bail!("Steam answered the query with EResult {}", q.result);
            }
            let mut buf = vec![0u8; 32 * 1024];
            if !get(s.ugc, handle, 0, buf.as_mut_ptr()) {
                bail!("Steam gave no details of item {id}");
            }
            let title_bytes = &buf[D_TITLE..D_TITLE + 129];
            let end = title_bytes.iter().position(|&c| c == 0).unwrap_or(129);
            Ok(ItemDetails {
                id: u64::from_le_bytes(buf[D_ID..D_ID + 8].try_into().unwrap()),
                result: read_u32(&buf, D_RESULT) as i32,
                app_id: read_u32(&buf, D_CONSUMER_APP),
                title: String::from_utf8_lossy(&title_bytes[..end]).to_string(),
                owner: u64::from_le_bytes(buf[D_OWNER..D_OWNER + 8].try_into().unwrap()),
                created: read_u32(&buf, D_CREATED),
                updated: read_u32(&buf, D_UPDATED),
                visibility: read_u32(&buf, D_VISIBILITY) as i32,
                banned: buf[D_BANNED] != 0,
                file_size: read_u32(&buf, D_FILE_SIZE) as i32,
            })
        })();
        release(s.ugc, handle);
        out
    }
}

/// What Steam's client says about an item (the owner sees private items too).
pub fn details(game_dir: &Path, id: u64) -> Result<ItemDetails> {
    let _one = STEAM.lock().unwrap_or_else(|e| e.into_inner());
    let s = Session::open(game_dir)?;
    query_details(&s, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_why_steam_says_an_upload_failed() {
        let log = "[2026-10-07 19:40:25] [AppID 281990] Upload starting for workshop item 3815197025 by AppID 281990\n\
                   [2026-10-07 19:40:32] [AppID 281990] Upload workshop item 3815197025 failed (Timeout uploading manifest (size 773))\n";
        assert_eq!(log_reason(log, 3815197025).as_deref(), Some("Timeout uploading manifest (size 773)"));
        assert_eq!(log_reason(log, 1), None);
    }

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
