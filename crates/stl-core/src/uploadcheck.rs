//! Making Workshop uploads understandable: what is checked before anything is sent, what a failure means and what to do about it, and a
//! report to hand someone who helps. (The official launcher answers a failed upload with "There was an error while uploading your mod" and
//! Valve's one-line text for the code, which says neither which part nor why.)
//!
//! - `local` looks at the mod: the title, change note and preview against Steam's limits (bytes, not characters), the tags, the
//!   descriptors inside and outside the content folder, the content's size, paths that are too long, and files that should not be uploaded
//!   (`.git`, source art, archives…), which `clean_copy` leaves out.
//! - `remote` asks Steam: which account is signed in, and for an update, whose item it is and which game it belongs to (the public
//!   `GetPublishedFileDetails`, no key). A collaborator who is not a contributor of the item gets "access denied" on upload, the most puzzling
//!   failure there is; this says it before the upload.
//! - `explain` turns a failure (`workshop::UploadError`: the step, Steam's code, how far the transfer came) and the checks into what happened,
//!   the likely causes for *this* upload, and what to do.
//! - `report` writes all of it as plain text.
//! - The journal (`journal_*`) remembers an upload that did not finish, and the item it had made: the next try updates that item instead of
//!   making another, even when the launcher was closed half-way or the number could not be written into the descriptors.
//! - `verify` reads what Steam has after an upload (its client sees private items too) and says whether the new version is there.
//!
//! Texts are English here (`text`); the window has its own words for each key.

use crate::mods::Mod;
use crate::workshop::{self, Step, Upload, UploadError};
use crate::{net, script, Result};
use std::path::{Path, PathBuf};

/// Steam's limits (bytes of UTF-8): `k_cchPublishedDocumentTitleMax`, `…DescriptionMax`, `…ChangeDescriptionMax` (each without the end byte).
pub const TITLE_MAX: usize = 128;
pub const DESCRIPTION_MAX: usize = 8000;
pub const NOTE_MAX: usize = 8000;
/// what the Workshop takes for a preview
pub const PREVIEW_MAX: u64 = 1 << 20;
/// a path longer than this inside the mod may not fit under a subscriber's `steamapps\workshop\content\281990\<id>\` (260 in all)
pub const LONG_PATH: usize = 180;
const WORKSHOP_AGREEMENT: &str = "https://steamcommunity.com/sharedfiles/workshoplegalagreement";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    /// the upload would fail: it is not started
    Error,
}

/// One thing the checks found: a key (`text` gives the words, the window its own) and the values that go into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub key: &'static str,
    pub args: Vec<String>,
}

fn finding(level: Level, key: &'static str, args: &[&dyn ToString]) -> Finding {
    Finding { level, key, args: args.iter().map(|a| a.to_string()).collect() }
}

/// What the item to update is, as Steam shows it to anyone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemInfo {
    pub id: u64,
    /// false: Steam does not show it (private, friends-only, or deleted)
    pub visible: bool,
    pub creator: u64,
    pub app_id: u32,
    pub title: String,
    pub banned: bool,
    pub updated: i64,
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Preflight {
    pub findings: Vec<Finding>,
    pub files: usize,
    pub bytes: u64,
    /// files and folders left out of the upload (relative paths; folders end with `/`), and what they weigh
    pub excluded: Vec<String>,
    pub excluded_bytes: u64,
    /// the signed-in Steam account, once `remote` ran
    pub account: Option<u64>,
    pub item: Option<ItemInfo>,
    /// `remote` could not reach Steam's web API (the item was not checked)
    pub offline: bool,
}

impl Preflight {
    pub fn blocked(&self) -> bool {
        self.findings.iter().any(|f| f.level == Level::Error)
    }
    fn has(&self, key: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.key == key)
    }
}

/// Files and folders that do not belong in an upload: version control, editors, source art, archives, system litter.
pub fn is_excluded(name: &str, dir: bool) -> bool {
    let n = name.to_lowercase();
    if dir {
        return matches!(n.as_str(), ".git" | ".svn" | ".hg" | ".vs" | ".vscode" | ".idea" | ".github" | "__macosx" | "node_modules" | ".cache" | "__pycache__");
    }
    matches!(n.as_str(), "thumbs.db" | "desktop.ini" | ".ds_store" | ".gitignore" | ".gitattributes" | ".gitmodules" | ".editorconfig")
        || [".psd", ".psb", ".xcf", ".kra", ".sai", ".clip", ".pdn", ".ai", ".blend", ".blend1", ".max", ".spp", ".zip", ".7z", ".rar", ".bak", ".tmp", ".orig"]
            .iter()
            .any(|e| n.ends_with(e))
}

fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b / 1024),
        b => format!("{b} B"),
    }
}

/// The picture format by its first bytes.
fn image_kind(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG") {
        Some("png")
    } else if head.starts_with(b"\xFF\xD8\xFF") {
        Some("jpg")
    } else if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

struct Walk {
    files: usize,
    bytes: u64,
    excluded: Vec<String>,
    excluded_bytes: u64,
    long: Vec<String>,
}

fn walk(base: &Path, dir: &Path, w: &mut Walk) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        if is_excluded(&name, ft.is_dir()) {
            if ft.is_dir() {
                let mut sub = Walk { files: 0, bytes: 0, excluded: Vec::new(), excluded_bytes: 0, long: Vec::new() };
                walk(&p, &p, &mut sub);
                w.excluded_bytes += sub.bytes + sub.excluded_bytes;
                w.excluded.push(format!("{rel}/"));
            } else {
                w.excluded_bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                w.excluded.push(rel);
            }
            continue;
        }
        if ft.is_dir() {
            walk(base, &p, w);
        } else {
            w.files += 1;
            w.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            if rel.len() > LONG_PATH {
                w.long.push(rel);
            }
        }
    }
}

/// The checks that need nothing but the disk. `typed_item` is an item id the user typed (for a mod whose descriptor names none).
pub fn local(m: &Mod, u: &Upload, typed_item: Option<&str>) -> Preflight {
    let mut p = Preflight::default();
    let push = |p: &mut Preflight, f: Finding| p.findings.push(f);
    // the content
    if !u.content.is_dir() {
        push(&mut p, finding(Level::Error, "pf.content_missing", &[&u.content.display()]));
    } else {
        let mut w = Walk { files: 0, bytes: 0, excluded: Vec::new(), excluded_bytes: 0, long: Vec::new() };
        walk(&u.content, &u.content, &mut w);
        p.files = w.files;
        p.bytes = w.bytes;
        p.excluded = w.excluded;
        p.excluded_bytes = w.excluded_bytes;
        if w.files == 0 {
            push(&mut p, finding(Level::Error, "pf.content_empty", &[]));
        } else {
            push(&mut p, finding(Level::Info, "pf.summary", &[&w.files, &human(w.bytes)]));
        }
        if !p.excluded.is_empty() {
            let examples = p.excluded.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let (n, size) = (p.excluded.len(), human(p.excluded_bytes));
            push(&mut p, finding(Level::Info, "pf.excluded", &[&n, &size, &examples]));
        }
        if !w.long.is_empty() {
            push(&mut p, finding(Level::Warning, "pf.long_paths", &[&w.long.len(), &w.long[0]]));
        }
        if w.bytes > 2 << 30 {
            push(&mut p, finding(Level::Warning, "pf.big", &[&human(w.bytes)]));
        }
        // the descriptor the game reads from a Workshop download, and whether it agrees with the one in the mod folder
        let inner = u.content.join("descriptor.mod");
        match std::fs::read(&inner) {
            Err(_) => push(&mut p, finding(Level::Error, "pf.descriptor_missing", &[])),
            Ok(bytes) => {
                let s = script::parse(&String::from_utf8_lossy(&bytes));
                let get = |k: &str| script::get(&s, k).map(|v| v.trim().to_string()).unwrap_or_default();
                let mut differ = Vec::new();
                if get("name") != m.name {
                    differ.push("name");
                }
                if get("version") != m.version.clone().unwrap_or_default() {
                    differ.push("version");
                }
                if get("supported_version") != m.supported_version.clone().unwrap_or_default() {
                    differ.push("supported_version");
                }
                if !differ.is_empty() {
                    push(&mut p, finding(Level::Warning, "pf.descriptor_differs", &[&differ.join(", ")]));
                }
                let inner_id = get("remote_file_id");
                let outer_id = m.remote_file_id.clone().unwrap_or_default();
                if !inner_id.is_empty() && !outer_id.is_empty() && inner_id != outer_id {
                    push(&mut p, finding(Level::Warning, "pf.remote_id_differs", &[&outer_id, &inner_id]));
                }
                let pic = get("picture");
                if !pic.is_empty() && !u.content.join(&pic).is_file() {
                    push(&mut p, finding(Level::Warning, "pf.picture_missing", &[&pic]));
                }
            }
        }
    }
    match m.supported_version.as_deref().map(str::trim) {
        None | Some("") => push(&mut p, finding(Level::Warning, "pf.no_supported_version", &[])),
        Some(v) => {
            let ok = v.trim_start_matches('v').split('.').all(|part| part == "*" || (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit())));
            if !ok {
                push(&mut p, finding(Level::Warning, "pf.bad_supported_version", &[&v]));
            }
        }
    }
    // the title, the note, the description: Steam counts UTF-8 bytes
    let title = u.title.trim();
    if title.is_empty() {
        push(&mut p, finding(Level::Error, "pf.title_empty", &[]));
    } else if title.len() > TITLE_MAX {
        push(&mut p, finding(Level::Error, "pf.title_too_long", &[&title.len(), &(title.len() - TITLE_MAX)]));
    }
    if u.change_note.len() > NOTE_MAX {
        push(&mut p, finding(Level::Error, "pf.note_too_long", &[&u.change_note.len(), &(u.change_note.len() - NOTE_MAX)]));
    }
    if u.description.len() > DESCRIPTION_MAX {
        push(&mut p, finding(Level::Error, "pf.description_too_long", &[&u.description.len(), &(u.description.len() - DESCRIPTION_MAX)]));
    }
    // the tags the Workshop offers for Stellaris
    let unknown: Vec<&String> = u.tags.iter().filter(|t| !crate::modmake::TAGS.iter().any(|k| k.eq_ignore_ascii_case(t))).collect();
    if !unknown.is_empty() {
        push(&mut p, finding(Level::Warning, "pf.unknown_tags", &[&unknown.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")]));
    }
    // the preview
    let new_item = u.existing.is_none() && typed_item.map_or(true, |t| t.trim().is_empty());
    match &u.preview {
        None if new_item => push(&mut p, finding(Level::Warning, "pf.no_preview_new", &[])),
        None => push(&mut p, finding(Level::Info, "pf.keep_preview", &[])),
        Some(path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match std::fs::read(path) {
                Err(_) => push(&mut p, finding(Level::Error, "pf.preview_missing", &[&path.display()])),
                Ok(bytes) => {
                    if bytes.len() as u64 >= PREVIEW_MAX {
                        push(&mut p, finding(Level::Error, "pf.preview_too_big", &[&name, &(bytes.len() / 1024)]));
                    }
                    match image_kind(&bytes[..bytes.len().min(16)]) {
                        Some("png") | Some("jpg") | Some("gif") => {
                            let kind = image_kind(&bytes[..bytes.len().min(16)]).unwrap();
                            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                            let ext = if ext == "jpeg" { "jpg".to_string() } else { ext };
                            if ext != kind {
                                push(&mut p, finding(Level::Warning, "pf.preview_ext", &[&name, &kind]));
                            }
                        }
                        other => push(&mut p, finding(Level::Error, "pf.preview_format", &[&name, &other.unwrap_or("?")])),
                    }
                }
            }
        }
    }
    if let Some(t) = typed_item.map(str::trim).filter(|t| !t.is_empty()) {
        if t.parse::<u64>().is_err() {
            push(&mut p, finding(Level::Error, "pf.bad_item_id", &[&t]));
        }
    }
    if let Some(e) = journal_pending(&m.id) {
        p.findings.extend(journal_findings(&e));
    }
    p
}

/// What Steam shows anyone about an item (`ISteamRemoteStorage/GetPublishedFileDetails`, a public API that needs no key).
pub fn item_info(id: u64) -> Result<ItemInfo> {
    let form = format!("itemcount=1&publishedfileids%5B0%5D={id}");
    let body = net::http_post_form("https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/", &form, 15_000, 1 << 20)?;
    let v: serde_json::Value = serde_json::from_slice(&body)?;
    let d = &v["response"]["publishedfiledetails"][0];
    let num = |x: &serde_json::Value| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    Ok(ItemInfo {
        id,
        visible: d["result"].as_i64() == Some(1),
        creator: num(&d["creator"]),
        app_id: num(&d["consumer_app_id"]) as u32,
        title: d["title"].as_str().unwrap_or("").to_string(),
        banned: num(&d["banned"]) != 0,
        updated: num(&d["time_updated"]) as i64,
        size: num(&d["file_size"]),
    })
}

/// The checks that ask Steam: the signed-in account (through the Steam client), and for an update, the item.
pub fn remote(game_dir: &Path, p: &mut Preflight, item: Option<u64>) {
    match workshop::check(game_dir) {
        Ok((_, account)) => {
            p.account = Some(account);
            p.findings.push(finding(Level::Info, "pf.account", &[&account]));
        }
        Err(e) => p.findings.push(finding(Level::Error, "pf.steam_unavailable", &[&format!("{e:#}")])),
    }
    let Some(id) = item else { return };
    match item_info(id) {
        Err(_) => p.offline = true,
        Ok(info) => {
            if !info.visible {
                p.findings.push(finding(Level::Warning, "pf.item_not_visible", &[&id]));
            } else {
                if info.app_id != 0 && info.app_id != workshop::APP_ID {
                    p.findings.push(finding(Level::Error, "pf.item_other_game", &[&id, &info.app_id]));
                }
                if info.banned {
                    p.findings.push(finding(Level::Error, "pf.item_banned", &[&id]));
                }
                if let Some(me) = p.account {
                    if info.creator != 0 && info.creator != me {
                        p.findings.push(finding(Level::Warning, "pf.item_other_owner", &[&id, &info.creator]));
                    }
                }
                p.findings.push(finding(Level::Info, "pf.item_info", &[&info.title, &crate::saves::local_time_string(info.updated)]));
            }
            p.item = Some(info);
        }
    }
}

/// A copy of the content without what `is_excluded` names, in the temporary folder; what is uploaded when anything was left out. The caller
/// removes it (`remove_clean_copy`) after the upload.
pub fn clean_copy(content: &Path) -> Result<PathBuf> {
    let to = std::env::temp_dir().join(format!("stl-upload-{}", std::process::id())).join(content.file_name().unwrap_or_default());
    let _ = std::fs::remove_dir_all(&to);
    fn copy(from: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)?.flatten() {
            let ft = e.file_type()?;
            if is_excluded(&e.file_name().to_string_lossy(), ft.is_dir()) {
                continue;
            }
            if ft.is_dir() {
                copy(&e.path(), &to.join(e.file_name()))?;
            } else {
                std::fs::copy(e.path(), to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    copy(content, &to)?;
    Ok(to)
}

pub fn remove_clean_copy(copy: &Path) {
    if let Some(parent) = copy.parent().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("stl-upload-"))) {
        let _ = std::fs::remove_dir_all(parent);
    }
}

// ------------------------------------------------------------------ the journal: uploads that did not finish

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// the item being updated, or the one this upload made
    pub item: Option<u64>,
    /// the item was made by this upload
    #[serde(default)]
    pub created: bool,
    /// when it started (seconds since 1970)
    pub started: i64,
}

fn journal_path() -> Result<PathBuf> {
    Ok(crate::paths::app_data_dir()?.join("upload-journal.json"))
}

fn journal_load() -> std::collections::BTreeMap<String, JournalEntry> {
    journal_path().ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn journal_save(map: &std::collections::BTreeMap<String, JournalEntry>) {
    if let Ok(p) = journal_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(map).unwrap_or_default());
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// An upload of a mod (its descriptor id) starts. A number remembered from an unfinished upload is kept when none is given.
pub fn journal_start(mod_id: &str, item: Option<u64>) {
    let mut m = journal_load();
    let old = m.get(mod_id).cloned().unwrap_or_default();
    m.insert(mod_id.to_string(), JournalEntry { item: item.or(old.item), created: old.created && item.is_none(), started: now_secs() });
    journal_save(&m);
}

/// The upload made item `id` (written before anything else, so that a crash right after leaves the number behind).
pub fn journal_created(mod_id: &str, id: u64) {
    let mut m = journal_load();
    let e = m.entry(mod_id.to_string()).or_default();
    e.item = Some(id);
    e.created = true;
    journal_save(&m);
}

/// The upload went through: nothing to remember.
pub fn journal_finish(mod_id: &str) {
    let mut m = journal_load();
    if m.remove(mod_id).is_some() {
        journal_save(&m);
    }
}

/// An upload of this mod that did not finish (failed, or the launcher was closed during it).
pub fn journal_pending(mod_id: &str) -> Option<JournalEntry> {
    journal_load().get(mod_id).cloned()
}

fn journal_findings(e: &JournalEntry) -> Vec<Finding> {
    let mut v = vec![finding(Level::Warning, "j.interrupted", &[&crate::saves::local_time_string(e.started)])];
    if let (Some(id), true) = (e.item, e.created) {
        v.push(finding(Level::Info, "j.created", &[&id]));
    }
    v
}

// ------------------------------------------------------------------ after an upload

fn visibility_key(v: i32) -> &'static str {
    match v {
        0 => "vis.public",
        1 => "vis.friends",
        2 => "vis.private",
        3 => "vis.unlisted",
        _ => "vis.unknown",
    }
}

/// What Steam has after an upload, against what was sent: whether its copy is the new one, the visibility, the title, the size.
pub fn verify(u: &Upload, o: &workshop::Outcome) -> Vec<Finding> {
    let mut v = Vec::new();
    match &o.details {
        None => v.push(finding(Level::Info, "v.unchecked", &[])),
        Some(d) => {
            let when = crate::saves::local_time_string(d.updated as i64);
            // Steam's clock and ours differ a little
            if d.updated as i64 + 120 >= o.started {
                v.push(finding(Level::Info, "v.updated", &[&when]));
            } else {
                v.push(finding(Level::Warning, "v.not_updated", &[&when]));
            }
            let asked = u.visibility.map(|x| x as i32).or(if o.created { Some(workshop::Visibility::Private as i32) } else { None });
            match asked {
                Some(a) if a != d.visibility => v.push(finding(Level::Warning, "v.visibility_differs", &[&visibility_key(d.visibility), &visibility_key(a)])),
                _ => v.push(finding(Level::Info, "v.visibility", &[&visibility_key(d.visibility)])),
            }
            if d.title.trim() != u.title.trim() {
                v.push(finding(Level::Info, "v.title_differs", &[&d.title]));
            }
            if d.file_size > 0 {
                v.push(finding(Level::Info, "v.size", &[&human(d.file_size as u64)]));
            }
        }
    }
    if o.needs_agreement {
        v.push(finding(Level::Warning, "v.agreement", &[]));
    }
    v
}

// ------------------------------------------------------------------ explaining a failure

/// One sentence of an explanation: a key and its values.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub key: &'static str,
    pub args: Vec<String>,
}

fn line(key: &'static str, args: &[&dyn ToString]) -> Line {
    Line { key, args: args.iter().map(|a| a.to_string()).collect() }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Explanation {
    /// where it stopped
    pub headline: Line,
    /// what Steam's answer means
    pub meaning: Line,
    /// the likely causes for this upload, the likeliest first
    pub causes: Vec<Line>,
    /// what to do
    pub fixes: Vec<Line>,
    /// trying again as it is may well work (Steam's side was busy, the network dropped…)
    pub retry: bool,
    /// a link that helps (the Workshop agreement, the item's page)
    pub link: Option<String>,
}

fn step_key(s: Step) -> &'static str {
    match s {
        Step::Connect => "ex.at_connect",
        Step::Create => "ex.at_create",
        Step::StartUpdate => "ex.at_start",
        Step::Title => "ex.at_title",
        Step::Description => "ex.at_description",
        Step::Content => "ex.at_content",
        Step::Preview => "ex.at_preview",
        Step::Tags => "ex.at_tags",
        Step::Visibility => "ex.at_visibility",
        Step::Submit => "ex.at_submit",
    }
}

fn status_key(st: i32) -> &'static str {
    match st {
        1 => "ex.status_config",
        2 => "ex.status_preparing",
        3 => "ex.status_content",
        4 => "ex.status_preview",
        5 => "ex.status_committing",
        _ => "ex.status_unknown",
    }
}

/// What happened, why it probably happened in this upload, and what to do, from the failure and (when there are) the checks' findings.
pub fn explain(e: &UploadError, pre: Option<&Preflight>) -> Explanation {
    let mut causes = Vec::new();
    let mut fixes = Vec::new();
    let mut retry = false;
    let mut link = None;
    let headline = match e.status {
        Some(st) if e.step == Step::Submit => line("ex.headline_status", &[&step_key(e.step), &status_key(st)]),
        _ => line("ex.headline", &[&step_key(e.step)]),
    };
    let meaning = match e.result {
        Some(r) => {
            let name = workshop::eresult_name(r);
            let key = match r {
                2 => "ex.code_fail",
                3 | 35 | 36 | 37 | 38 => "ex.code_connection",
                8 => "ex.code_invalid_param",
                9 => "ex.code_file_not_found",
                10 | 20 | 33 | 84 => "ex.code_busy",
                14 => "ex.code_duplicate_name",
                15 => "ex.code_access_denied",
                16 => "ex.code_timeout",
                17 => "ex.code_banned",
                21 | 6 | 34 => "ex.code_not_logged_on",
                24 => "ex.code_insufficient_privilege",
                25 => "ex.code_limit_exceeded",
                29 => "ex.code_duplicate_request",
                44 => "ex.code_read_only",
                54 => "ex.code_disk_full",
                _ => "ex.code_other",
            };
            line(key, &[&r, &if name.is_empty() { "?" } else { name }])
        }
        None => match e.step {
            Step::Connect => line("ex.no_steam", &[]),
            Step::Create | Step::Submit => line("ex.no_answer", &[]),
            _ => line("ex.rejected_value", &[]),
        },
    };
    let pf = |key: &str| pre.and_then(|p| p.has(key));
    let item = e.item.map(|i| i.to_string()).unwrap_or_default();
    match (e.step, e.result) {
        (Step::Create, None) if e.created => {
            causes.push(line("c.descriptor_write", &[]));
            fixes.push(line("f.created_not_written", &[&item]));
        }
        (Step::Connect, _) => {
            causes.push(line("c.steam_not_running", &[]));
            fixes.push(line("f.start_steam", &[]));
            retry = true;
        }
        (_, Some(15)) => {
            if let Some(f) = pf("pf.item_other_owner") {
                causes.push(line("c.not_owner", &[&f.args[0], &f.args[1]]));
                fixes.push(line("f.ask_contributor", &[]));
            }
            causes.push(line("c.agreement", &[]));
            fixes.push(line("f.accept_agreement", &[]));
            link = Some(WORKSHOP_AGREEMENT.to_string());
            if e.step != Step::Create && pf("pf.item_other_owner").is_none() {
                causes.push(line("c.not_owner_unknown", &[&item]));
            }
        }
        (_, Some(8)) => {
            for key in ["pf.title_too_long", "pf.note_too_long", "pf.description_too_long", "pf.unknown_tags"] {
                if let Some(f) = pf(key) {
                    causes.push(Line { key: f.key, args: f.args.clone() });
                }
            }
            causes.push(line("c.invalid_field", &[]));
            fixes.push(line("f.check_fields", &[]));
        }
        (_, Some(9)) => {
            causes.push(line("c.file_missing", &[]));
            fixes.push(line("f.check_paths", &[]));
        }
        (_, Some(25)) => {
            causes.push(line("c.cloud_quota", &[]));
            fixes.push(line("f.free_cloud", &[]));
        }
        (_, Some(17)) => {
            causes.push(line("c.banned", &[]));
            fixes.push(line("f.support", &[]));
        }
        (_, Some(24)) => {
            causes.push(line("c.restricted", &[]));
            causes.push(line("c.limited_account", &[]));
            fixes.push(line("f.support", &[]));
        }
        (_, Some(44)) => {
            causes.push(line("c.recent_change", &[]));
            fixes.push(line("f.wait_days", &[]));
        }
        (_, Some(29)) => {
            causes.push(line("c.already_uploaded", &[]));
            fixes.push(line("f.open_item", &[]));
        }
        (_, Some(14)) => {
            causes.push(line("c.duplicate_name", &[]));
            fixes.push(line("f.rename", &[]));
        }
        (_, Some(21 | 6 | 34)) => {
            causes.push(line("c.not_logged_on", &[]));
            fixes.push(line("f.start_steam", &[]));
            retry = true;
        }
        (_, Some(3 | 10 | 16 | 20 | 33 | 35 | 36 | 37 | 38 | 84)) => {
            causes.push(line("c.steam_side", &[]));
            fixes.push(line("f.retry_later", &[]));
            retry = true;
        }
        (_, Some(54)) => {
            causes.push(line("c.disk_full", &[]));
            fixes.push(line("f.free_disk", &[]));
        }
        (Step::StartUpdate, None) => {
            causes.push(line("c.item_gone", &[&item]));
            if pf("pf.item_not_visible").is_some() {
                causes.push(line("c.item_hidden", &[&item]));
            }
            fixes.push(line("f.clear_id", &[]));
        }
        (Step::Title, None) => {
            if let Some(f) = pf("pf.title_too_long") {
                causes.push(Line { key: f.key, args: f.args.clone() });
            }
            causes.push(line("c.title_rejected", &[]));
            fixes.push(line("f.shorten_title", &[]));
        }
        (Step::Content, None) => {
            causes.push(line("c.content_rejected", &[]));
            fixes.push(line("f.check_paths", &[]));
        }
        (Step::Preview, None) => {
            causes.push(line("c.preview_rejected", &[]));
            fixes.push(line("f.fix_preview", &[]));
        }
        (Step::Tags | Step::Visibility | Step::Description, None) => {
            causes.push(line("c.invalid_field", &[]));
            fixes.push(line("f.check_fields", &[]));
        }
        (Step::Create | Step::Submit, None) => {
            causes.push(line("c.no_answer", &[]));
            fixes.push(line("f.retry_later", &[]));
            retry = true;
        }
        (_, Some(_)) => {
            // k_EResultFail and anything rarer: what the transfer was doing says most
            match e.status {
                Some(4) => causes.push(line("c.preview_problem", &[])),
                Some(2 | 3) => {
                    causes.push(line("c.content_problem", &[]));
                    if let Some(f) = pf("pf.long_paths") {
                        causes.push(Line { key: f.key, args: f.args.clone() });
                    }
                }
                _ => {}
            }
            causes.push(line("c.generic", &[]));
            fixes.push(line("f.retry_later", &[]));
            fixes.push(line("f.copy_report", &[]));
            retry = true;
        }
    }
    if e.created && e.step != Step::Create {
        fixes.push(line("f.created_item", &[&item]));
    }
    Explanation { headline, meaning, causes, fixes, retry, link }
}

// ------------------------------------------------------------------ the words (English; the window has its own)

/// The English template of a key (`{0}`, `{1}` for the values).
pub fn english(key: &str) -> &'static str {
    match key {
        // checks
        "pf.content_missing" => "The content folder {0} does not exist.",
        "pf.content_empty" => "The content folder has no files: Steam does not take an empty mod.",
        "pf.summary" => "{0} files, {1} will be uploaded.",
        "pf.excluded" => "{0} files or folders ({1}) are left out of the upload: {2}. They are version control, source art, archives or system files; your mod folder is not changed.",
        "pf.long_paths" => "{0} paths inside the mod are longer than 180 characters (e.g. {1}): Windows may fail to unpack them for subscribers. Shorten folder or file names.",
        "pf.big" => "The mod is {0}: large uploads take long and fail more often; check nothing is in there by mistake.",
        "pf.descriptor_missing" => "There is no descriptor.mod in the content folder: subscribers' launchers and the game read the mod's name and version from it.",
        "pf.descriptor_differs" => "descriptor.mod in the content folder and the .mod file in the mod folder disagree on: {0}. Subscribers get the one in the content folder.",
        "pf.remote_id_differs" => "The two descriptors name different Workshop items ({0} and {1}): the upload goes to {0}.",
        "pf.picture_missing" => "The descriptor names the picture {0}, which is not in the content folder.",
        "pf.no_supported_version" => "The descriptor has no supported_version: the game will mark the mod as made for another version.",
        "pf.bad_supported_version" => "supported_version \"{0}\" is not in the form the game understands (like v4.5.*).",
        "pf.title_empty" => "The mod has no name: Steam needs a title.",
        "pf.title_too_long" => "The title is {0} bytes; Steam takes at most 128 (counting UTF-8 bytes: a Chinese character is 3). Shorten it by {1} bytes.",
        "pf.note_too_long" => "The change note is {0} bytes; Steam takes at most 8000. Shorten it by {1} bytes.",
        "pf.description_too_long" => "The description is {0} bytes; Steam takes at most 8000. Shorten it by {1} bytes.",
        "pf.unknown_tags" => "Tags the Stellaris Workshop does not offer: {0}. Steam may refuse them; use the tags of the mod form.",
        "pf.no_preview_new" => "No preview picture (thumbnail.png): the new item will have no picture.",
        "pf.keep_preview" => "No preview picture here: the item keeps the picture it has on the Workshop.",
        "pf.preview_missing" => "The preview picture {0} cannot be read.",
        "pf.preview_too_big" => "The preview {0} is {1} KB; the Workshop takes less than 1024 KB. Save it smaller (JPG, or fewer pixels).",
        "pf.preview_format" => "The preview {0} is not a PNG, JPG or GIF picture (it looks like {1}): Steam refuses it.",
        "pf.preview_ext" => "The preview {0} is really a {1} picture; its file name says otherwise. Steam usually takes it, but rename it to be safe.",
        "pf.bad_item_id" => "\"{0}\" is not a Workshop item number (the digits after ?id= in the item's address).",
        "pf.steam_unavailable" => "Steam could not be reached: {0}",
        "pf.account" => "Signed in to Steam as account {0}.",
        "pf.item_not_visible" => "Steam does not show item {0} publicly: it is private, friends-only or deleted. If it was deleted, the upload fails; then clear the item number to make a new one.",
        "pf.item_other_game" => "Item {0} belongs to another game (app {1}), not Stellaris.",
        "pf.item_banned" => "Item {0} is banned on the Workshop; it cannot be updated.",
        "pf.item_other_owner" => "Item {0} was made by another account ({1}). You can update it only if its owner added you as a contributor; otherwise Steam answers \"access denied\".",
        "pf.item_info" => "On the Workshop: \"{0}\", last updated {1}.",
        // explanations
        "ex.headline" => "The upload stopped {0}.",
        "ex.headline_status" => "The upload stopped {0}, {1}.",
        "ex.at_connect" => "while starting Steam",
        "ex.at_create" => "while creating the Workshop item",
        "ex.at_start" => "while opening the item for an update",
        "ex.at_title" => "while setting the title",
        "ex.at_description" => "while setting the description",
        "ex.at_content" => "while setting the content folder",
        "ex.at_preview" => "while setting the preview picture",
        "ex.at_tags" => "while setting the tags",
        "ex.at_visibility" => "while setting the visibility",
        "ex.at_submit" => "while sending the update",
        "ex.status_config" => "preparing the item's settings",
        "ex.status_preparing" => "preparing the files",
        "ex.status_content" => "uploading the files",
        "ex.status_preview" => "uploading the preview picture",
        "ex.status_committing" => "committing the new version",
        "ex.status_unknown" => "in an unknown phase",
        "ex.no_steam" => "The Steam API did not start.",
        "ex.no_answer" => "Steam did not answer in time.",
        "ex.rejected_value" => "Steam refused the value before sending anything.",
        "ex.code_fail" => "Steam answered {0} ({1}): a general failure, without saying why.",
        "ex.code_connection" => "Steam answered {0} ({1}): the connection to Steam's servers failed.",
        "ex.code_invalid_param" => "Steam answered {0} ({1}): one of the fields (title, tags, note, visibility) has a value Steam does not accept.",
        "ex.code_file_not_found" => "Steam answered {0} ({1}): a file it should send could not be found.",
        "ex.code_busy" => "Steam answered {0} ({1}): the Workshop is busy or temporarily unavailable.",
        "ex.code_duplicate_name" => "Steam answered {0} ({1}): you already have a Workshop item with this name.",
        "ex.code_access_denied" => "Steam answered {0} ({1}): access denied.",
        "ex.code_timeout" => "Steam answered {0} ({1}): it took too long.",
        "ex.code_banned" => "Steam answered {0} ({1}): this account may not upload to this game's Workshop.",
        "ex.code_not_logged_on" => "Steam answered {0} ({1}): the Steam client is not signed in (or was signed in elsewhere).",
        "ex.code_insufficient_privilege" => "Steam answered {0} ({1}): this account is not allowed to upload right now.",
        "ex.code_limit_exceeded" => "Steam answered {0} ({1}): a limit was exceeded.",
        "ex.code_duplicate_request" => "Steam answered {0} ({1}): this was already uploaded.",
        "ex.code_read_only" => "Steam answered {0} ({1}): the account may not publish for now.",
        "ex.code_disk_full" => "Steam answered {0} ({1}): a disk is full.",
        "ex.code_other" => "Steam answered {0} ({1}).",
        // causes
        "c.steam_not_running" => "Steam is not running, not signed in, or signed in to an account that does not own Stellaris.",
        "c.not_owner" => "Item {0} belongs to account {1}, and the signed-in account is not one of its contributors.",
        "c.not_owner_unknown" => "Item {0} may belong to another account (Steam does not show its owner publicly).",
        "c.agreement" => "The Steam Workshop legal agreement has not been accepted on this account.",
        "c.invalid_field" => "A field has a value Steam refuses: too long, an unknown tag, or characters it does not take.",
        "c.file_missing" => "The content folder or the preview was moved or deleted while uploading, or the path has characters Steam cannot open.",
        "c.cloud_quota" => "The account's Steam Cloud space for Workshop files is full.",
        "c.banned" => "The account has a ban (VAC or game ban, or a Workshop ban) that stops uploads to this game.",
        "c.restricted" => "The account is restricted: a hub ban, an account lock or a community ban.",
        "c.limited_account" => "A limited Steam account (one that has not spent 5 USD on Steam) may not be allowed to publish.",
        "c.recent_change" => "The account's password or e-mail was changed recently: Steam blocks new uploads for a few days (5, up to 30 for an inactive account).",
        "c.already_uploaded" => "The same upload already went through; the item is up to date.",
        "c.duplicate_name" => "Another of your Workshop items already uses this title.",
        "c.not_logged_on" => "The Steam client lost its sign-in, or the account was signed in on another computer.",
        "c.steam_side" => "Steam's servers or the network had a problem; nothing is wrong with the mod.",
        "c.disk_full" => "The disk where Steam keeps its files is full.",
        "c.item_gone" => "Item {0} does not exist (deleted), belongs to another game, or this account cannot edit it.",
        "c.item_hidden" => "Steam does not show item {0} publicly either: it may have been deleted.",
        "c.title_rejected" => "Steam refused the title: too long (over 128 UTF-8 bytes) or with characters it does not take.",
        "c.content_rejected" => "Steam refused the content folder: its path is not a folder it can read.",
        "c.preview_rejected" => "Steam refused the preview picture's path.",
        "c.no_answer" => "Steam did not answer: its servers may be slow, or the network dropped.",
        "c.preview_problem" => "It failed while sending the preview picture: the picture is the likeliest cause (format, size, or a broken file).",
        "c.content_problem" => "It failed while preparing or sending the files: a file that cannot be read (in use, very long path, odd characters) is the likeliest cause.",
        "c.generic" => "Steam gives no reason for this code; often it is temporary.",
        // fixes
        "f.start_steam" => "Start Steam, sign in to the account that owns Stellaris, and try again.",
        "f.ask_contributor" => "Ask the item's owner to add your account as a contributor (on the item's page: Owner Controls → Add/Remove Contributors), or upload as a new item.",
        "f.accept_agreement" => "Open the Steam Workshop legal agreement and accept it (button below), then try again.",
        "f.check_fields" => "Shorten the title and change note, keep to the Workshop's tags, then try again.",
        "f.check_paths" => "Check that the mod folder and thumbnail.png are where the descriptor says, and that no other program is writing into them.",
        "f.free_cloud" => "Delete or shrink some of your Workshop items or Steam Cloud saves, then try again.",
        "f.support" => "Only Steam Support can lift this; the launcher cannot work around it.",
        "f.wait_days" => "Wait until Steam lifts the restriction (it says so on your account page), then try again.",
        "f.open_item" => "Open the item's page to see the version that is there.",
        "f.rename" => "Change the mod's name, or update the existing item instead (type its number).",
        "f.retry_later" => "Try again in a few minutes.",
        "f.clear_id" => "If the item was deleted, remove remote_file_id from the descriptors (or clear the item number) to upload as a new item.",
        "f.shorten_title" => "Shorten the mod's name and try again.",
        "f.fix_preview" => "Save thumbnail.png again as a PNG or JPG of under 1 MB and try again.",
        "f.copy_report" => "If it keeps failing, copy the diagnostic report and send it to whoever helps you.",
        "f.created_item" => "Item {0} was created before the failure (it is private) and its number is in the descriptors: trying again updates it instead of making another.",
        "c.descriptor_write" => "The Workshop item was made, but its number could not be written into the mod's descriptors (a file in use, or read-only).",
        "f.created_not_written" => "Item {0} exists (private) and the launcher remembers it: trying again updates it instead of making another.",
        "j.interrupted" => "The last upload of this mod (started {0}) did not finish.",
        "j.created" => "It had made item {0}: that number is used, so trying again updates it instead of making another.",
        "v.unchecked" => "Steam did not say what it has now; open the item's page to check.",
        "v.updated" => "Steam has the new version (updated {0}).",
        "v.not_updated" => "Steam's copy says it was last updated {0}, before this upload: the new files may not be there yet. Check the item's page in a few minutes.",
        "v.visibility" => "Visibility on Steam: {0}.",
        "v.visibility_differs" => "Visibility on Steam is {0}, not {1} as chosen; set it on the item's page.",
        "v.title_differs" => "Steam shows the title \"{0}\" (a translated title set on the item's page shows like this).",
        "v.size" => "Size on Steam: {0}.",
        "v.agreement" => "The Workshop agreement is not accepted: others cannot see the item until you accept it.",
        "vis.public" => "public",
        "vis.friends" => "friends only",
        "vis.private" => "private",
        "vis.unlisted" => "unlisted",
        "vis.unknown" => "unknown",
        _ => "",
    }
}

/// A key's English text with its values in place.
pub fn text(key: &str, args: &[String]) -> String {
    let mut s = english(key).to_string();
    if s.is_empty() {
        s = key.to_string();
    }
    for (i, a) in args.iter().enumerate() {
        // a value that is itself a key (the step, the phase) is put in as its words
        let v = if english(a).is_empty() { a.clone() } else { english(a).to_string() };
        s = s.replace(&format!("{{{i}}}"), &v);
    }
    s
}

pub fn finding_text(f: &Finding) -> String {
    text(f.key, &f.args)
}

pub fn line_text(l: &Line) -> String {
    text(l.key, &l.args)
}

/// Every key `english` knows (for the window's translations and their test).
pub const KEYS: &[&str] = &[
    "pf.content_missing", "pf.content_empty", "pf.summary", "pf.excluded", "pf.long_paths", "pf.big", "pf.descriptor_missing", "pf.descriptor_differs",
    "pf.remote_id_differs", "pf.picture_missing", "pf.no_supported_version", "pf.bad_supported_version", "pf.title_empty", "pf.title_too_long",
    "pf.note_too_long", "pf.description_too_long", "pf.unknown_tags", "pf.no_preview_new", "pf.keep_preview", "pf.preview_missing", "pf.preview_too_big",
    "pf.preview_format", "pf.preview_ext", "pf.bad_item_id", "pf.steam_unavailable", "pf.account", "pf.item_not_visible", "pf.item_other_game",
    "pf.item_banned", "pf.item_other_owner", "pf.item_info",
    "ex.headline", "ex.headline_status", "ex.at_connect", "ex.at_create", "ex.at_start", "ex.at_title", "ex.at_description", "ex.at_content",
    "ex.at_preview", "ex.at_tags", "ex.at_visibility", "ex.at_submit", "ex.status_config", "ex.status_preparing", "ex.status_content",
    "ex.status_preview", "ex.status_committing", "ex.status_unknown", "ex.no_steam", "ex.no_answer", "ex.rejected_value", "ex.code_fail",
    "ex.code_connection", "ex.code_invalid_param", "ex.code_file_not_found", "ex.code_busy", "ex.code_duplicate_name", "ex.code_access_denied",
    "ex.code_timeout", "ex.code_banned", "ex.code_not_logged_on", "ex.code_insufficient_privilege", "ex.code_limit_exceeded",
    "ex.code_duplicate_request", "ex.code_read_only", "ex.code_disk_full", "ex.code_other",
    "c.steam_not_running", "c.not_owner", "c.not_owner_unknown", "c.agreement", "c.invalid_field", "c.file_missing", "c.cloud_quota", "c.banned",
    "c.restricted", "c.limited_account", "c.recent_change", "c.already_uploaded", "c.duplicate_name", "c.not_logged_on", "c.steam_side", "c.disk_full",
    "c.item_gone", "c.item_hidden", "c.title_rejected", "c.content_rejected", "c.preview_rejected", "c.no_answer", "c.preview_problem",
    "c.content_problem", "c.generic",
    "f.start_steam", "f.ask_contributor", "f.accept_agreement", "f.check_fields", "f.check_paths", "f.free_cloud", "f.support", "f.wait_days",
    "f.open_item", "f.rename", "f.retry_later", "f.clear_id", "f.shorten_title", "f.fix_preview", "f.copy_report", "f.created_item",
    "c.descriptor_write", "f.created_not_written", "j.interrupted", "j.created", "v.unchecked", "v.updated", "v.not_updated", "v.visibility",
    "v.visibility_differs", "v.title_differs", "v.size", "v.agreement", "vis.public", "vis.friends", "vis.private", "vis.unlisted", "vis.unknown",
];

// ------------------------------------------------------------------ the report

/// The paths in a report with the user's folder written as %USERPROFILE% (the report may be posted publicly).
fn private(s: &str) -> String {
    match std::env::var("USERPROFILE") {
        Ok(home) if !home.is_empty() => s.replace(&home, "%USERPROFILE%").replace(&home.replace('\\', "/"), "%USERPROFILE%"),
        _ => s.to_string(),
    }
}

/// Everything about one upload attempt, as plain English text to copy into a message or a forum post.
pub fn report(m: &Mod, u: &Upload, pre: Option<&Preflight>, failure: Option<(&str, Option<&UploadError>)>, outcome: Option<&workshop::Outcome>, game_version: &str) -> String {
    let mut r = String::new();
    let mut add = |s: String| {
        r.push_str(&s);
        r.push('\n');
    };
    add("Stellaris Launcher — Workshop upload report".into());
    add(format!("launcher {} · Stellaris {} · {}", crate::selfupdate::current_version(), game_version, crate::saves::local_time_string(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))));
    add(String::new());
    add("[mod]".into());
    add(format!("name: {}  ({} bytes)", m.name, m.name.len()));
    add(format!("descriptor: {}", private(&m.file.display().to_string())));
    add(format!("content: {}", private(&u.content.display().to_string())));
    add(format!("version: {} · supported_version: {}", m.version.clone().unwrap_or_default(), m.supported_version.clone().unwrap_or_default()));
    add(format!("tags: {}", u.tags.join(", ")));
    add(format!("preview: {}", u.preview.as_ref().map(|p| private(&p.display().to_string())).unwrap_or("(none)".into())));
    add(format!("item: {}", u.existing.map(|i| workshop::item_url(i)).unwrap_or("(new)".into())));
    add(format!("visibility: {} · change note: {} bytes", u.visibility.map(|v| format!("{v:?}")).unwrap_or("unchanged".into()), u.change_note.len()));
    if let Some(p) = pre {
        add(String::new());
        add("[checks]".into());
        add(format!("files: {} · {} · left out: {} ({})", p.files, human(p.bytes), p.excluded.len(), human(p.excluded_bytes)));
        if let Some(a) = p.account {
            add(format!("account: {a}"));
        }
        if let Some(i) = &p.item {
            add(format!("item on Steam: visible {} · creator {} · app {} · banned {} · updated {} · {}", i.visible, i.creator, i.app_id, i.banned, crate::saves::local_time_string(i.updated), human(i.size)));
        }
        if p.offline {
            add("item on Steam: not checked (no answer from api.steampowered.com)".into());
        }
        for f in &p.findings {
            let lvl = match f.level {
                Level::Error => "ERROR",
                Level::Warning => "warning",
                Level::Info => "info",
            };
            add(format!("- {lvl}: {}", private(&finding_text(f))));
        }
    }
    add(String::new());
    match (failure, outcome) {
        (Some((message, err)), _) => {
            add("[result] FAILED".into());
            add(format!("message: {}", private(message)));
            if let Some(e) = err {
                add(format!("step: {:?} · EResult: {} · transfer phase: {} · item: {} · created now: {}", e.step, e.result.map(|r| format!("{r} {}", workshop::eresult_name(r))).unwrap_or("-".into()), e.status.map(|s| s.to_string()).unwrap_or("-".into()), e.item.map(|i| i.to_string()).unwrap_or("-".into()), e.created));
                let x = explain(e, pre);
                add(format!("what happened: {} {}", line_text(&x.headline), line_text(&x.meaning)));
                for c in &x.causes {
                    add(format!("- likely: {}", line_text(c)));
                }
                for f in &x.fixes {
                    add(format!("- to do: {}", line_text(f)));
                }
            }
        }
        (None, Some(o)) => {
            add(format!("[result] OK · item {} · {}", workshop::item_url(o.id), if o.created { "created" } else { "updated" }));
            for f in verify(u, o) {
                add(format!("- {}", finding_text(&f)));
            }
        }
        _ => add("[result] not uploaded".into()),
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods;

    fn a_mod(dir: &Path, name: &str, extra: &str) -> Mod {
        let content = dir.join("content");
        std::fs::create_dir_all(&content).unwrap();
        mods::parse_descriptor(&dir.join("m.mod"), &format!("name=\"{name}\"\nversion=\"1.0\"\nsupported_version=\"v4.5.*\"\npath=\"{}\"\n{extra}", content.to_string_lossy().replace('\\', "/")), dir)
    }

    fn upload_of(m: &Mod) -> Upload {
        Upload {
            title: m.name.clone(),
            description: String::new(),
            content: m.path.clone().unwrap(),
            preview: mods::own_thumbnail(m),
            tags: m.tags.clone(),
            visibility: None,
            change_note: String::new(),
            existing: m.remote_file_id.as_deref().and_then(|v| v.parse().ok()),
        }
    }

    #[test]
    fn checks_what_steam_would_refuse_and_what_should_not_be_sent() {
        let dir = std::env::temp_dir().join(format!("stl-test-upcheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // a title of 50 Chinese characters is 150 bytes
        let long_title: String = std::iter::repeat('星').take(50).collect();
        let m = a_mod(&dir, &long_title, "tags={\n\"Gameplay\"\n\"Not A Tag\"\n}\n");
        let c = m.path.clone().unwrap();
        std::fs::write(c.join("descriptor.mod"), "name=\"Other name\"\nversion=\"1.0\"\nsupported_version=\"v4.5.*\"\n").unwrap();
        std::fs::create_dir_all(c.join("common/x")).unwrap();
        std::fs::write(c.join("common/x/a.txt"), "a = {}").unwrap();
        std::fs::create_dir_all(c.join(".git/objects")).unwrap();
        std::fs::write(c.join(".git/objects/blob"), vec![0u8; 3000]).unwrap();
        std::fs::write(c.join("art.psd"), vec![0u8; 100]).unwrap();
        // a "PNG" that is really a JPEG
        std::fs::write(c.join("thumbnail.png"), b"\xFF\xD8\xFFrest").unwrap();
        let p = local(&m, &upload_of(&m), None);
        let keys: Vec<&str> = p.findings.iter().map(|f| f.key).collect();
        assert!(keys.contains(&"pf.title_too_long"));
        assert_eq!(p.has("pf.title_too_long").unwrap().args, vec!["150", "22"]);
        assert!(keys.contains(&"pf.unknown_tags"));
        assert!(keys.contains(&"pf.descriptor_differs"));
        assert!(keys.contains(&"pf.preview_ext"));
        assert!(keys.contains(&"pf.excluded"));
        assert!(p.blocked(), "a title over 128 bytes stops the upload");
        assert_eq!(p.files, 3, "descriptor.mod, a.txt and the thumbnail; not .git or the psd");
        assert!(p.excluded.contains(&".git/".to_string()) && p.excluded.contains(&"art.psd".to_string()));
        // the clean copy leaves them out and keeps the rest
        let copy = clean_copy(&c).unwrap();
        assert!(copy.join("common/x/a.txt").is_file() && copy.join("descriptor.mod").is_file());
        assert!(!copy.join(".git").exists() && !copy.join("art.psd").exists());
        remove_clean_copy(&copy);
        assert!(!copy.exists());
        // an empty folder without a descriptor
        let empty = a_mod(&dir.join("e"), "Fine", "");
        let pe = local(&empty, &upload_of(&empty), Some("abc"));
        let ek: Vec<&str> = pe.findings.iter().map(|f| f.key).collect();
        assert!(ek.contains(&"pf.content_empty") && ek.contains(&"pf.descriptor_missing") && ek.contains(&"pf.bad_item_id"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn checks_what_steam_has_after_an_upload() {
        let m = Mod { name: "M".into(), ..mods::parse_descriptor(Path::new("x.mod"), "name=\"M\"\npath=\"C:/x\"\n", Path::new("C:/")) };
        let up = Upload { title: "M".into(), description: String::new(), content: PathBuf::from("C:/x"), preview: None, tags: vec![], visibility: None, change_note: String::new(), existing: None };
        let details = workshop::ItemDetails { id: 5, result: 1, title: "M".into(), updated: 1_000_100, visibility: 2, file_size: 2048, ..Default::default() };
        let o = workshop::Outcome { id: 5, created: true, needs_agreement: true, details: Some(details.clone()), started: 1_000_000 };
        let keys: Vec<&str> = verify(&up, &o).iter().map(|f| f.key).collect();
        assert_eq!(keys, vec!["v.updated", "v.visibility", "v.size", "v.agreement"]);
        // an old copy, another visibility, another (translated) title
        let stale = workshop::ItemDetails { updated: 900_000, visibility: 0, title: "M (中文)".into(), ..details };
        let o2 = workshop::Outcome { details: Some(stale), needs_agreement: false, ..o };
        let keys: Vec<&str> = verify(&up, &o2).iter().map(|f| f.key).collect();
        assert_eq!(keys, vec!["v.not_updated", "v.visibility_differs", "v.title_differs", "v.size"]);
        assert!(text("v.visibility_differs", &["vis.public".into(), "vis.private".into()]).contains("public, not private"));
        let _ = m;
    }

    #[test]
    fn writes_a_report_without_the_user_folder() {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let dir = PathBuf::from(&home).join("stl-report-test-does-not-exist");
        let m = mods::parse_descriptor(&dir.join("m.mod"), &format!("name=\"My Mod\"\nversion=\"1.2\"\nsupported_version=\"v4.5.*\"\npath=\"{}\"\nremote_file_id=\"42\"\n", dir.join("content").to_string_lossy().replace('\\', "/")), &dir);
        let up = upload_of(&m);
        let mut pre = Preflight::default();
        pre.findings.push(finding(Level::Warning, "pf.item_other_owner", &[&42, &7656u64]));
        let err = UploadError { step: Step::Submit, result: Some(15), status: Some(5), item: Some(42), created: false, message: "the upload of item 42 failed".into() };
        let r = report(&m, &up, Some(&pre), Some((&err.message, Some(&err))), None, "Cygnus v4.5.2");
        assert!(r.contains("[result] FAILED"));
        assert!(r.contains("EResult: 15 k_EResultAccessDenied"));
        assert!(r.contains("likely: Item 42 belongs to account 7656"));
        assert!(r.contains("warning: Item 42 was made by another account"));
        if !home.is_empty() {
            assert!(!r.contains(&home), "the user's folder is hidden");
            assert!(r.contains("%USERPROFILE%"));
        }
    }

    #[test]
    fn explains_failures_with_what_the_checks_found() {
        let err = |step, result, status, created| UploadError { step, result, status, item: Some(42), created, message: String::new() };
        let mut pre = Preflight::default();
        pre.findings.push(finding(Level::Warning, "pf.item_other_owner", &[&42, &7656u64]));
        // access denied on an item someone else made: that comes first, with the owner
        let x = explain(&err(Step::Submit, Some(15), Some(5), false), Some(&pre));
        assert_eq!(x.causes[0].key, "c.not_owner");
        assert_eq!(x.causes[0].args, vec!["42", "7656"]);
        assert!(x.link.is_some());
        assert!(line_text(&x.headline).contains("while sending the update, committing the new version"));
        // a general failure while sending the preview points at the picture; created items are said to be kept
        let y = explain(&err(Step::Submit, Some(2), Some(4), true), None);
        assert_eq!(y.causes[0].key, "c.preview_problem");
        assert!(y.retry);
        assert!(y.fixes.iter().any(|f| f.key == "f.created_item"));
        // Steam busy: try again
        assert!(explain(&err(Step::Create, Some(16), None, false), None).retry);
        // every key has words
        for k in KEYS {
            assert!(!english(k).is_empty(), "{k}");
        }
        assert!(text("pf.summary", &["3".into(), "1 KB".into()]).starts_with("3 files"));
    }
}
