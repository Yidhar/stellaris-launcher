//! stellarismod.com, the community site the launcher works with (its API: `docs/SITE.md`, the site's `docs/LAUNCHER_API.md`):
//!
//! - `stl://` links the site opens (`parse_link`). Any web page can open one, so a link only says what the member may want: the window
//!   asks before anything is changed, and everything downloaded is checked against a SHA-256 the site or the link gave.
//! - Signing in with a site account by device code (`start_link`, `poll`): the launcher never sees the password. The token is kept
//!   encrypted for this Windows user (DPAPI) in `site-account.dat`, and forgotten when any answer is `401`.
//! - Playset exports (`parse_export`): the playset of a page, with how to get each mod (Workshop, a package the site hosts, or a local mod).
//! - Plugins the site hosts (`site_plugin`), from its `plugins.jsonl`, installed like any index's.
//! - Wiki entries for a mod published to the Workshop (`workshop_lookup`, `parse_mod`, `attach_art`, `submit_entry`).

use crate::import::{Shared, SharedMod};
use crate::{anyhow, bail, net, paths, pluginindex, updates, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The site.
pub const SITE: &str = "https://stellarismod.com";
/// The longest `stl://` link taken (the site's inline playsets are at most about 7000 characters).
pub const MAX_LINK: usize = 16 * 1024;
const TIMEOUT: i32 = 30_000;

/// The site's address: `SITE`, or for tests a local one in `STL_SITE` (`http://127.0.0.1:<port>`).
pub fn site() -> String {
    match std::env::var("STL_SITE") {
        Ok(s) if local_site(s.trim()) => s.trim().trim_end_matches('/').to_string(),
        _ => SITE.to_string(),
    }
}

fn local_site(s: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| s.strip_prefix(p).is_some_and(|port| !port.is_empty() && port.trim_end_matches('/').bytes().all(|b| b.is_ascii_digit())))
}

/// An API URL (`path` starts with `/`).
pub fn api(path: &str) -> String {
    format!("{}/api/v1{path}", site())
}

// ---------------------------------------------------------------------------------------------------------------------------------
// links
// ---------------------------------------------------------------------------------------------------------------------------------

/// What an `stl://` link asks for. Every value has been checked: ids are safe as file and URL parts, URLs are the kinds named.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    /// `stl://playset/import?url=`: a playset export of the site (`<site>/api/v1/playsets/<id>/export`)
    ImportUrl(String),
    /// `stl://playset/import?data=`: the same JSON, carried in the link
    ImportData(String),
    /// `stl://playset/add-mod?id=mod/ugc_<id>.mod`: a mod for the active playset
    AddMod(String),
    /// `stl://mod/install-r2?id=&version=`: a package the site hosts
    InstallHosted { id: String, version: Option<String> },
    /// `stl://plugin/install?id=&version=&sha256=`
    InstallPlugin { id: String, version: Option<String>, sha256: Option<String> },
    /// `stl://index/add?url=`: a plugin index (`plugins.jsonl`)
    AddIndex(String),
    /// `stl://account/link?code=&site=`: signing in, already allowed on the site
    Account { code: String },
}

/// `[A-Za-z0-9._-]`, not starting with a dot, at most `max` bytes: an id the site makes (its object keys use the same rule).
pub fn safe_segment(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Percent-decoding of a query value (`+` is a space, as browsers encode forms).
fn pct_decode(s: &str) -> Result<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = s.get(i + 1..i + 3).filter(|h| hex(h, 2)).with_context(|| format!("a broken %-escape in {s:?}"))?;
                out.push(u8::from_str_radix(h, 16)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).context("a link value that is not UTF-8")
}

/// Unpadded base64url (`=` padding is allowed too).
pub fn base64url_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => bail!("not base64url: {:?}", c as char),
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        bail!("base64url of a wrong length");
    }
    Ok(out)
}

/// Unpadded base64url of bytes (what the site's `data=` links carry).
pub fn base64url_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(b.len() * 4 / 3 + 2);
    for chunk in b.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &x)| n | (x as u32) << (16 - 8 * i));
        for i in 0..chunk.len() + 1 {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// The query of a link as pairs; a name twice is refused (which one would count?).
fn query(q: &str) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for part in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let k = pct_decode(k)?;
        if out.iter().any(|(x, _)| *x == k) {
            bail!("the link gives {k} twice");
        }
        out.push((k, pct_decode(v)?));
    }
    Ok(out)
}

/// A playset export URL of the site, exactly.
pub fn check_export_url(u: &str) -> Result<String> {
    let base = api("/playsets/");
    let id = u.strip_prefix(&base).and_then(|r| r.strip_suffix("/export")).with_context(|| format!("playsets are only imported from {base}<id>/export, not {u}"))?;
    if !safe_segment(id, 80) {
        bail!("{u} is not a playset export of the site");
    }
    Ok(u.to_string())
}

/// A descriptor id a playset can name: `mod/ugc_<number>.mod`, `mod/pdx_<number>.mod` or `mod/<safe name>.mod`.
pub fn check_descriptor(id: &str) -> Result<String> {
    let name = id.strip_prefix("mod/").and_then(|r| r.strip_suffix(".mod")).with_context(|| format!("{id:?} is not a mod descriptor (mod/<name>.mod)"))?;
    if !safe_segment(name, 120) || name.contains("..") {
        bail!("{id:?} is not a mod descriptor the launcher takes");
    }
    Ok(id.to_string())
}

/// The Workshop item of a `mod/ugc_<id>.mod` descriptor.
pub fn workshop_of(descriptor: &str) -> Option<u64> {
    let n = descriptor.strip_prefix("mod/ugc_")?.strip_suffix(".mod")?;
    n.bytes().all(|b| b.is_ascii_digit()).then(|| n.parse().ok()).flatten()
}

/// A plugin index URL: https, printable, at most 2048 bytes.
pub fn check_index_url(u: &str) -> Result<String> {
    let u = u.trim();
    if !u.starts_with("https://") || u.len() > 2048 || u.chars().any(|c| c.is_control() || c.is_whitespace()) || net::split_url(u).is_err() {
        bail!("{u:?} is not an https address of a plugin index");
    }
    Ok(u.to_string())
}

/// Reads an `stl://` link.
pub fn parse_link(link: &str) -> Result<Link> {
    let link = link.trim();
    if link.len() > MAX_LINK {
        bail!("this link is longer than {MAX_LINK} characters");
    }
    let rest = link.get(..6).filter(|p| p.eq_ignore_ascii_case("stl://")).map(|_| &link[6..]).context("not an stl:// link")?;
    let rest = rest.split_once('#').map(|(a, _)| a).unwrap_or(rest);
    let (path, q) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.trim_end_matches('/').to_ascii_lowercase();
    let params = query(q)?;
    let get = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
    let need = |k: &str| get(k).with_context(|| format!("the link has no {k}"));
    let segment = |k: &str, max: usize| -> Result<Option<String>> {
        match get(k) {
            Some(v) if safe_segment(&v, max) => Ok(Some(v)),
            Some(v) => bail!("the link's {k} {v:?} is not an id the launcher takes"),
            None => Ok(None),
        }
    };
    Ok(match path.as_str() {
        "playset/import" => match (get("url"), get("data")) {
            (Some(u), None) => Link::ImportUrl(check_export_url(&u)?),
            (None, Some(d)) => {
                if d.len() > 12 * 1024 {
                    bail!("the playset in this link is too large");
                }
                Link::ImportData(String::from_utf8(base64url_decode(&d)?).context("the playset in this link is not UTF-8 text")?)
            }
            _ => bail!("an import link names a url or carries data (one of them)"),
        },
        "playset/add-mod" => Link::AddMod(check_descriptor(&need("id")?)?),
        "mod/install-r2" => Link::InstallHosted { id: segment("id", 64)?.context("the link has no id")?, version: segment("version", 32)? },
        "plugin/install" => {
            let sha256 = match get("sha256") {
                Some(s) if hex(&s, 64) => Some(s.to_ascii_lowercase()),
                Some(_) => bail!("the link's sha256 is not 64 hex digits"),
                None => None,
            };
            Link::InstallPlugin { id: segment("id", 64)?.context("the link has no id")?, version: segment("version", 32)?, sha256 }
        }
        "index/add" => Link::AddIndex(check_index_url(&need("url")?)?),
        "account/link" => {
            let code = need("code")?;
            if !hex(&code, 64) {
                bail!("the link's sign-in code is not one the site makes");
            }
            let from = get("site").unwrap_or_else(site);
            if from.trim_end_matches('/') != site() {
                bail!("this sign-in link is for {from}, not {}", site());
            }
            Link::Account { code: code.to_ascii_lowercase() }
        }
        other => bail!("the launcher does not know the link stl://{other}"),
    })
}

/// Where a link was found among a program's arguments (Windows starts the handler with `"<exe>" "%1"`).
pub fn link_in_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter().find(|a| a.len() > 6 && a.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("stl://")))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// playset exports
// ---------------------------------------------------------------------------------------------------------------------------------

fn yes() -> bool {
    true
}

/// A mod of an export, in load order.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExportMod {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub name: Option<String>,
    /// the site's entry (`<site>/mods/<item_id>`)
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub workshop_id: Option<String>,
    /// a package the site hosts, with its SHA-256
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExportPlugin {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A playset as the site exports it (`GET /playsets/<id>/export`; a `stl_core::store::Playset` with more fields).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Export {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub format: Option<u32>,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub disabled_dlcs: Option<Vec<String>>,
    #[serde(default)]
    pub mods: Vec<ExportMod>,
    #[serde(default)]
    pub plugins: Vec<ExportPlugin>,
}

/// How an export's mod is got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Workshop(u64),
    /// a package of the site (`download_url` + `sha256`)
    Hosted,
    /// a mod the user has to have already
    Local,
}

impl ExportMod {
    pub fn source(&self) -> Source {
        if let Some(w) = self.workshop_id.as_deref().and_then(|w| w.parse().ok()) {
            Source::Workshop(w)
        } else if self.download_url.is_some() && self.sha256.is_some() {
            Source::Hosted
        } else {
            Source::Local
        }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&self.id)
    }
}

/// Reads a playset export, with the checks a file from a web page needs: ids safe as file names, downloads https with a SHA-256.
pub fn parse_export(text: &str) -> Result<Export> {
    let mut e: Export = serde_json::from_str(text.trim_start_matches('\u{feff}')).context("this is not a playset of the site")?;
    if e.format.is_some_and(|f| f > 1) {
        bail!("this playset is in a newer format ({}); update the launcher", e.format.unwrap_or(0));
    }
    e.name = e.name.trim().chars().take(120).collect();
    if e.name.is_empty() {
        bail!("this playset has no name");
    }
    if e.mods.len() > 2000 || e.plugins.len() > 200 {
        bail!("this playset lists too many mods or plugins");
    }
    let https = |u: &Option<String>| u.as_deref().map_or(true, |u| u.starts_with("https://") && net::split_url(u).is_ok());
    let sha = |s: &mut Option<String>| -> bool {
        *s = s.as_deref().map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty());
        s.as_deref().map_or(true, |x| hex(x, 64))
    };
    let mut seen = std::collections::HashSet::new();
    for m in &mut e.mods {
        m.workshop_id = m.workshop_id.as_deref().map(str::trim).filter(|w| !w.is_empty()).map(String::from);
        if let Some(w) = &m.workshop_id {
            if !w.bytes().all(|b| b.is_ascii_digit()) || w.parse::<u64>().is_err() {
                bail!("{} has the Workshop id {w}, which is not a number", m.label());
            }
            // the descriptor the Workshop item gets on any machine
            m.id = format!("mod/ugc_{w}.mod");
        }
        m.id = check_descriptor(m.id.trim())?;
        if !https(&m.download_url) || !sha(&mut m.sha256) {
            bail!("{} has a download that is not https with a SHA-256", m.label());
        }
        if !seen.insert(m.id.clone()) {
            bail!("{} is in this playset twice", m.label());
        }
    }
    let mut seen = std::collections::HashSet::new();
    for p in &mut e.plugins {
        if !safe_segment(&p.id, 64) {
            bail!("the plugin id {:?} is not one the launcher takes", p.id);
        }
        if !https(&p.download_url) || !sha(&mut p.sha256) {
            bail!("the plugin {} has a download that is not https with a SHA-256", p.id);
        }
        if !seen.insert(p.id.clone()) {
            bail!("the plugin {} is in this playset twice", p.id);
        }
    }
    Ok(e)
}

impl Export {
    /// The playset as a shared playset file (`import::Shared`): Workshop mods by item, the rest by their descriptor id. The import of a
    /// shared file (subscribing, the file's order kept) does the rest.
    pub fn to_shared(&self) -> Shared {
        let mods = self
            .mods
            .iter()
            .enumerate()
            .map(|(i, m)| SharedMod {
                display_name: m.label().to_string(),
                steam_id: m.workshop_id.clone(),
                pdx_id: None,
                position: i as f64,
                enabled: m.enabled,
                local_id: m.workshop_id.is_none().then(|| m.id.clone()),
            })
            .collect();
        Shared { game: "stellaris".into(), name: self.name.clone(), mods }
    }

    pub fn count(&self, s: fn(&Source) -> bool) -> usize {
        self.mods.iter().filter(|m| s(&m.source())).count()
    }

    /// The hosted packages not in the mod folder yet.
    pub fn hosted_missing(&self, known: &[crate::mods::Mod]) -> Vec<&ExportMod> {
        self.mods.iter().filter(|m| m.source() == Source::Hosted && !known.iter().any(|k| k.id == m.id && k.problem.is_none())).collect()
    }

    /// The local mods not in the mod folder: nobody can get those for the user.
    pub fn local_missing(&self, known: &[crate::mods::Mod]) -> Vec<&ExportMod> {
        self.mods.iter().filter(|m| m.source() == Source::Local && !known.iter().any(|k| k.id == m.id && k.problem.is_none())).collect()
    }

    /// The plugins not installed that the export says where to get.
    pub fn plugins_missing(&self, installed: &[crate::plugins::Plugin]) -> Vec<&ExportPlugin> {
        self.plugins.iter().filter(|p| p.download_url.is_some() && p.sha256.is_some() && !installed.iter().any(|i| i.manifest.id == p.id)).collect()
    }
}

/// GETs a playset export of the site (`check_export_url` first).
pub fn fetch_export(url: &str) -> Result<Export> {
    let url = check_export_url(url)?;
    let a = net::send("GET", &url, &[("Accept", "application/json")], None, TIMEOUT, 4 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    parse_export(&String::from_utf8_lossy(&a.body))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// what the site hosts
// ---------------------------------------------------------------------------------------------------------------------------------

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The folder of a mod package holding `descriptor.mod`: the top, or the one folder there.
fn mod_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("descriptor.mod").is_file() {
        return Some(dir.to_path_buf());
    }
    let subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match subs.as_slice() {
        [one] if one.join("descriptor.mod").is_file() => Some(one.clone()),
        _ => None,
    }
}

/// Installs a mod package the site hosts into the mod folder: the download must have `sha256`; it goes to `mod/<name>/` with its
/// descriptor `mod/<name>.mod` (pointing there), replacing what was there.
pub fn install_hosted_mod(data_dir: &Path, descriptor: &str, url: &str, sha256: &str) -> Result<()> {
    let descriptor = check_descriptor(descriptor)?;
    let name = descriptor.trim_start_matches("mod/").trim_end_matches(".mod").to_string();
    if !url.starts_with("https://") {
        bail!("{url} is not an https download");
    }
    let bytes = updates::download_checked(&[url], sha256, &name)?;
    let tmp = std::env::temp_dir().join(format!("stl-mod-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| {
        updates::unzip(&bytes, &tmp)?;
        let root = mod_root(&tmp).with_context(|| format!("the package of {name} holds no descriptor.mod"))?;
        let text = String::from_utf8_lossy(&std::fs::read(root.join("descriptor.mod"))?).to_string();
        let mods = data_dir.join("mod");
        let folder = mods.join(&name);
        if folder.exists() {
            std::fs::remove_dir_all(&folder).with_context(|| format!("cannot replace {}", folder.display()))?;
        }
        copy_dir(&root, &folder)?;
        let path = folder.display().to_string().replace('\\', "/");
        let text = crate::modmake::set_key(&text, "path", &path);
        std::fs::write(data_dir.join(&descriptor), text).with_context(|| format!("cannot write {descriptor}"))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// The site's item (`GET /mods/<id>`), as it is.
pub fn item(id: &str) -> Result<Value> {
    if !safe_segment(id, 64) {
        bail!("{id:?} is not an id of the site");
    }
    let a = call("GET", &format!("/mods/{id}"), None, None, 4 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    serde_json::from_slice(&a.body).context("the site's answer cannot be read")
}

/// The download and SHA-256 of an item's release (`None` = the current one).
pub fn release_of(item: &Value, version: Option<&str>) -> Result<(String, String, String)> {
    let pick = |r: &Value| -> Option<(String, String, String)> {
        Some((r["download_url"].as_str()?.to_string(), r["sha256"].as_str()?.to_ascii_lowercase(), r["version"].as_str().unwrap_or("").to_string()))
    };
    let found = match version {
        Some(v) => item["releases"].as_array().and_then(|rs| rs.iter().find(|r| r["version"].as_str() == Some(v))).and_then(pick),
        None => pick(&json!({ "download_url": item["download_url"], "sha256": item["sha256"], "version": item["version"] })),
    };
    let (url, sha, v) = found.with_context(|| format!("the site has no download of {}{}", item["id"].as_str().unwrap_or("?"), version.map(|v| format!(" {v}")).unwrap_or_default()))?;
    if !url.starts_with("https://") || !hex(&sha, 64) {
        bail!("the site's download of {} is not https with a SHA-256", item["id"].as_str().unwrap_or("?"));
    }
    Ok((url, sha, v))
}

/// A plugin the site hosts, as an index entry (its `plugins.jsonl`), at `version` when given; a `sha256` from a link must be the site's.
pub fn site_plugin(id: &str, version: Option<&str>, sha256: Option<&str>) -> Result<pluginindex::Entry> {
    let index = api("/plugins.jsonl");
    let (entries, _) = pluginindex::parse(&String::from_utf8_lossy(&net::http_get(&index, TIMEOUT, 8 << 20)?), &index);
    let mut e = entries.into_iter().find(|e| e.id == id).with_context(|| format!("the site does not host the plugin {id}"))?;
    if let Some(v) = version.filter(|v| *v != e.version) {
        let (url, sha, v) = release_of(&item(id)?, Some(v))?;
        e.download = url;
        e.sha256 = sha;
        e.version = v;
        e.fallback = None;
    }
    if let Some(s) = sha256 {
        if !s.eq_ignore_ascii_case(&e.sha256) {
            bail!("the link's SHA-256 is not the one the site gives for {id} {}", e.version);
        }
    }
    Ok(e)
}

/// An export's plugin as an index entry (for `pluginindex::install`, which checks the SHA-256 and that the zip is this plugin).
pub fn export_plugin_entry(p: &ExportPlugin) -> Result<pluginindex::Entry> {
    let (Some(url), Some(sha)) = (&p.download_url, &p.sha256) else { bail!("the playset does not say where {} is", p.id) };
    let line = json!({
        "id": p.id, "name": p.name.clone().unwrap_or_else(|| p.id.clone()), "author": "", "description": "",
        "version": p.version.clone().unwrap_or_default(), "download": url, "sha256": sha, "repository": site(),
    });
    pluginindex::parse(&line.to_string(), &site()).0.pop().with_context(|| format!("the playset's plugin {} cannot be read", p.id))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// the account
// ---------------------------------------------------------------------------------------------------------------------------------

/// A signed-in account, kept encrypted for this Windows user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub site: String,
    pub token: String,
    pub username: String,
    #[serde(default)]
    pub display_name: String,
}

impl Session {
    pub fn shown_name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

/// The site no longer takes the token (`401`): it has been forgotten.
#[derive(Debug)]
pub struct SignedOut;

impl std::fmt::Display for SignedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the site signed this launcher out (the token was removed or has run out); sign in again")
    }
}

impl std::error::Error for SignedOut {}

pub fn is_signed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SignedOut>().is_some()
}

fn account_file() -> Result<PathBuf> {
    Ok(paths::app_data_dir()?.join("site-account.dat"))
}

fn dpapi(data: &[u8], protect: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let entropy = b"stellaris-launcher site account";
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let extra = CRYPT_INTEGER_BLOB { cbData: entropy.len() as u32, pbData: entropy.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if protect {
            CryptProtectData(&input, std::ptr::null(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), &extra, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        bail!("Windows could not {} the account", if protect { "encrypt" } else { "decrypt" });
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(bytes)
}

/// The account this launcher is signed in with (for this site).
pub fn saved() -> Option<Session> {
    let bytes = std::fs::read(account_file().ok()?).ok()?;
    let s: Session = serde_json::from_slice(&dpapi(&bytes, false).ok()?).ok()?;
    (s.site == site()).then_some(s)
}

pub fn save(s: &Session) -> Result<()> {
    let path = account_file()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("dat.tmp");
    std::fs::write(&tmp, dpapi(&serde_json::to_vec(s)?, true)?)?;
    std::fs::rename(&tmp, &path).context("cannot keep the account")?;
    Ok(())
}

pub fn forget() {
    if let Ok(p) = account_file() {
        let _ = std::fs::remove_file(p);
    }
}

/// The message of an error answer (`{ "error": "…" }`, written to be shown), or its status.
pub fn api_error(a: &net::Answer) -> anyhow::Error {
    let msg = serde_json::from_slice::<Value>(&a.body).ok().and_then(|v| v["error"].as_str().map(String::from)).filter(|m| !m.trim().is_empty());
    anyhow!("{}", msg.unwrap_or_else(|| format!("{} answered {}", a.host, a.status)))
}

/// A request to the API, with the token when given (a `401` then forgets it: `SignedOut`).
fn call(method: &str, path: &str, token: Option<&str>, body: Option<(&str, &[u8])>, max: usize) -> Result<net::Answer> {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers = vec![("Accept", "application/json")];
    if let Some(a) = &auth {
        headers.push(("Authorization", a.as_str()));
    }
    let a = net::send(method, &api(path), &headers, body, TIMEOUT, max)?;
    if a.status == 401 && token.is_some() {
        forget();
        return Err(anyhow!(SignedOut));
    }
    Ok(a)
}

fn post_json(path: &str, token: Option<&str>, body: &Value) -> Result<net::Answer> {
    call("POST", path, token, Some(("application/json", body.to_string().as_bytes())), 4 << 20)
}

/// A device link being allowed on the site.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceLink {
    /// secret: whoever has it gets the token once it is allowed
    pub device_code: String,
    pub user_code: String,
    pub verify_url: String,
    #[serde(default = "ten_minutes")]
    pub expires_in: u64,
    #[serde(default = "three")]
    pub interval: u64,
}

fn ten_minutes() -> u64 {
    600
}
fn three() -> u64 {
    3
}

/// What the site shows the member: `stellaris-Launcher 0.1.13 · DESKTOP-AB12` (at most 60 characters).
pub fn device_name() -> String {
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    let name = format!("stellaris-Launcher {}{}", env!("CARGO_PKG_VERSION"), if machine.is_empty() { String::new() } else { format!(" · {machine}") });
    name.chars().take(60).collect()
}

/// Asks the site for a device link: show `user_code`, open `verify_url`, then `poll` every `interval` seconds.
pub fn start_link(device_name: &str) -> Result<DeviceLink> {
    let a = post_json("/launcher/link", None, &json!({ "device_name": device_name }))?;
    if a.status != 201 && a.status != 200 {
        return Err(api_error(&a));
    }
    let d: DeviceLink = serde_json::from_slice(&a.body).context("the site's answer cannot be read")?;
    if !d.verify_url.starts_with(&format!("{}/", site())) {
        bail!("the site gave a page elsewhere to allow the launcher on: {}", d.verify_url);
    }
    Ok(d)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    Pending,
    Signed(Session),
    /// the member refused
    Refused,
    /// ten minutes passed, or the code was used
    Expired,
}

/// Asks whether the link was allowed; a token is kept as soon as it comes (`save`).
pub fn poll(device_code: &str) -> Result<Poll> {
    let a = post_json("/launcher/token", None, &json!({ "device_code": device_code }))?;
    Ok(match a.status {
        202 => Poll::Pending,
        200 => {
            #[derive(Deserialize)]
            struct T {
                token: String,
                username: String,
                #[serde(default)]
                display_name: String,
            }
            let t: T = serde_json::from_slice(&a.body).context("the site's answer cannot be read")?;
            if !t.token.starts_with("stl_") || t.token.len() > 200 || t.token.contains(char::is_whitespace) {
                bail!("the site's token is not one the launcher takes");
            }
            let s = Session { site: site(), token: t.token, username: t.username, display_name: t.display_name };
            save(&s)?;
            Poll::Signed(s)
        }
        403 => Poll::Refused,
        410 => Poll::Expired,
        _ => return Err(api_error(&a)),
    })
}

/// The account behind the token (`GET /auth/me`): `username`, `display_name`, `role`, …
pub fn me(s: &Session) -> Result<Value> {
    let a = call("GET", "/auth/me", Some(&s.token), None, 1 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    serde_json::from_slice(&a.body).context("the site's answer cannot be read")
}

/// Signs out: the site removes the token, and it is forgotten here (also when the site cannot be reached).
pub fn logout(s: &Session) -> Result<()> {
    let r = call("POST", "/auth/logout", Some(&s.token), Some(("application/json", b"{}")), 1 << 20);
    forget();
    match r {
        Ok(a) if a.status < 300 => Ok(()),
        Ok(a) => Err(api_error(&a)),
        Err(e) if is_signed_out(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// wiki entries for a published mod
// ---------------------------------------------------------------------------------------------------------------------------------

/// What the site reads of a Workshop item (`GET /creator/workshop/<id>`).
#[derive(Debug, Clone)]
pub struct Lookup {
    pub title: String,
    pub suggested_category: Option<String>,
    /// the site's item that lists it already
    pub listed: Option<String>,
    /// someone's entry for it waits for review
    pub pending: bool,
    pub raw: Value,
}

pub fn workshop_lookup(s: &Session, workshop: u64) -> Result<Lookup> {
    let a = call("GET", &format!("/creator/workshop/{workshop}"), Some(&s.token), None, 4 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    let raw: Value = serde_json::from_slice(&a.body).context("the site's answer cannot be read")?;
    Ok(Lookup {
        title: raw["title"].as_str().unwrap_or_default().to_string(),
        suggested_category: raw["suggested_category"].as_str().map(String::from),
        listed: raw["listed"].as_str().filter(|l| !l.is_empty()).map(String::from),
        pending: raw["pending"].as_bool().unwrap_or(false),
        raw,
    })
}

/// The page of a site item.
pub fn item_page(id: &str) -> String {
    format!("{}/mods/{id}", site())
}

pub fn categories(s: &Session) -> Result<Vec<String>> {
    let a = call("GET", "/creator/categories", Some(&s.token), None, 1 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    serde_json::from_slice(&a.body).context("the site's answer cannot be read")
}

/// The texts `/creator/parse` reads (`common/**/*.txt|csv`, `events/**/*.txt`, `localisation/**/*.yml` in Simplified Chinese and
/// English, `interface/**/*.gfx`) and the textures icons may be made of (`gfx/interface/**`, `gfx/event_pictures/**`; `.dds .png .tga`),
/// by their path in the mod.
#[derive(Debug, Default)]
pub struct ModFiles {
    pub texts: Vec<(String, PathBuf)>,
    /// lower-case path → (the path as it is, the file)
    pub textures: HashMap<String, (String, PathBuf, u64)>,
    pub skipped: usize,
}

const MAX_TEXT: u64 = 8 << 20;
const MAX_TEXTURE: u64 = 4 << 20;
const PARSE_MAX: usize = 24 << 20;
const ICON_BATCH_BYTES: u64 = 12 << 20;
const ICON_BATCH_COUNT: usize = 400;

fn read_language(rel_lower: &str) -> bool {
    let tail = rel_lower.trim_start_matches("localisation/");
    ["simp_chinese", "english"].iter().any(|l| {
        tail.match_indices(l).any(|(i, _)| {
            let before = tail[..i].chars().last();
            let after = tail[i + l.len()..].chars().next();
            before.map_or(true, |c| c == '/' || c == '_') && after.map_or(true, |c| c == '/' || c == '_' || c == '.')
        })
    })
}

/// Where a file of a mod folder goes for the parse (`Some(true)` a text, `Some(false)` a texture).
fn wanted(rel: &str) -> Option<bool> {
    let l = rel.to_ascii_lowercase();
    let text = (l.starts_with("common/") && (l.ends_with(".txt") || l.ends_with(".csv")))
        || (l.starts_with("events/") && l.ends_with(".txt"))
        || (l.starts_with("localisation/") && l.ends_with(".yml") && read_language(&l))
        || (l.starts_with("interface/") && l.ends_with(".gfx"));
    if text {
        return Some(true);
    }
    let texture = (l.starts_with("gfx/interface/") || l.starts_with("gfx/event_pictures/")) && [".dds", ".png", ".tga"].iter().any(|e| l.ends_with(e));
    texture.then_some(false)
}

pub fn mod_files(folder: &Path) -> Result<ModFiles> {
    fn walk(dir: &Path, rel: &str, out: &mut ModFiles, depth: usize) {
        if depth > 24 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                walk(&e.path(), &r, out, depth + 1);
            } else if let Some(text) = wanted(&r) {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                if size > if text { MAX_TEXT } else { MAX_TEXTURE } {
                    out.skipped += 1;
                } else if text {
                    out.texts.push((r, e.path()));
                } else {
                    out.textures.insert(r.to_ascii_lowercase(), (r, e.path(), size));
                }
            }
        }
    }
    if !folder.is_dir() {
        bail!("{} is not a folder", folder.display());
    }
    let mut out = ModFiles::default();
    walk(folder, "", &mut out, 0);
    out.texts.sort();
    Ok(out)
}

fn zip_files(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        z.start_file(name.as_str(), opts)?;
        z.write_all(body)?;
    }
    Ok(z.finish()?.into_inner())
}

/// The mod's entries as the site reads them (`POST /creator/parse`): `{ entries, counts, warnings, groups, stories, pictures, resources }`.
pub fn parse_mod(s: &Session, files: &ModFiles) -> Result<Value> {
    if files.texts.is_empty() {
        bail!("the mod has no text files the site reads (common, events, localisation, interface)");
    }
    let mut list = Vec::new();
    for (rel, path) in &files.texts {
        list.push((rel.clone(), std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?));
    }
    let zipped = zip_files(&list)?;
    if zipped.len() > PARSE_MAX {
        bail!("the mod's text files are {:.1} MB packed, more than the 24 MB the site takes", zipped.len() as f64 / 1048576.0);
    }
    let a = call("POST", "/creator/parse", Some(&s.token), Some(("application/zip", &zipped)), 32 << 20)?;
    if a.status != 200 {
        return Err(api_error(&a));
    }
    serde_json::from_slice(&a.body).context("the site's answer cannot be read")
}

fn icon_key(s: &Value) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        s["path"].as_str().unwrap_or("").to_ascii_lowercase(),
        s["frames"],
        s["frame"],
        if s["square"].as_bool().unwrap_or(false) { 1 } else { 0 },
        s["kind"].as_str().unwrap_or("")
    )
}

/// The first of `sources` (`icon_from`, `pictures[x].from`) the mod has.
fn first_present<'a>(sources: &'a Value, files: &ModFiles) -> Option<&'a Value> {
    sources.as_array()?.iter().find(|s| s["path"].as_str().is_some_and(|p| files.textures.contains_key(&p.to_ascii_lowercase())))
}

/// Makes the icons and event pictures of a parse on the site (`POST /creator/icons`, in batches) and puts them in: each entry's `icon`
/// (or the game's own, `icon_site`), each resource's, and the `sprite:<name>` pictures of entries and stories. `icon_from` and
/// `icon_site` are dropped. The number of pictures made.
pub fn attach_art(s: &Session, parsed: &mut Value, files: &ModFiles, progress: &mut dyn FnMut(usize, usize)) -> Result<usize> {
    let empty = Value::Null;
    let pictures = parsed.get("pictures").cloned().unwrap_or(Value::Null);
    let mut wanted: Vec<Value> = Vec::new();
    let mut add = |src: Option<&Value>| {
        if let Some(v) = src {
            if !wanted.iter().any(|w| icon_key(w) == icon_key(v)) {
                wanted.push(v.clone());
            }
        }
    };
    for e in parsed["entries"].as_array().unwrap_or(&Vec::new()) {
        add(first_present(e.get("icon_from").unwrap_or(&empty), files));
    }
    for r in parsed["resources"].as_array().unwrap_or(&Vec::new()) {
        add(first_present(r.get("icon_from").unwrap_or(&empty), files));
    }
    if let Some(map) = pictures.as_object() {
        for p in map.values() {
            add(first_present(p.get("from").unwrap_or(&empty), files));
        }
    }
    let mut made: HashMap<String, Option<String>> = HashMap::new();
    let mut done = 0;
    progress(0, wanted.len());
    while done < wanted.len() {
        let mut batch: Vec<(Value, String, PathBuf)> = Vec::new();
        let mut bytes = 0u64;
        for w in &wanted[done..] {
            let (rel, path, size) = files.textures[&w["path"].as_str().unwrap_or("").to_ascii_lowercase()].clone();
            if !batch.is_empty() && (bytes + size > ICON_BATCH_BYTES || batch.len() >= ICON_BATCH_COUNT) {
                break;
            }
            batch.push((w.clone(), rel, path));
            bytes += size;
        }
        let mut zfiles: Vec<(String, Vec<u8>)> = Vec::new();
        let mut manifest = Vec::new();
        for (src, rel, path) in &batch {
            if !zfiles.iter().any(|(n, _)| n == rel) {
                zfiles.push((rel.clone(), std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?));
            }
            let mut m = src.clone();
            m["path"] = Value::String(rel.clone());
            manifest.push(m);
        }
        zfiles.push(("manifest.json".into(), serde_json::to_vec(&manifest)?));
        let zipped = zip_files(&zfiles)?;
        let a = call("POST", "/creator/icons", Some(&s.token), Some(("application/zip", &zipped)), 8 << 20)?;
        if a.status != 200 {
            return Err(api_error(&a));
        }
        let answer: Value = serde_json::from_slice(&a.body).context("the site's answer cannot be read")?;
        let icons = answer["icons"].as_array().cloned().unwrap_or_default();
        for (i, (src, _, _)) in batch.iter().enumerate() {
            made.insert(icon_key(src), icons.get(i).and_then(|v| v.as_str()).map(String::from));
        }
        done += batch.len();
        progress(done, wanted.len());
    }
    let made_for = |sources: Option<&Value>| -> Option<String> { first_present(sources.unwrap_or(&Value::Null), files).and_then(|s| made.get(&icon_key(s)).cloned().flatten()) };
    let fix = |p: &Value| -> Value {
        let Some(text) = p.as_str() else { return Value::Null };
        let Some(sprite) = text.strip_prefix("sprite:") else { return p.clone() };
        let pic = &pictures[sprite];
        made_for(pic.get("from")).map(Value::String).or_else(|| pic["site"].as_str().map(|s| Value::String(s.to_string()))).unwrap_or(Value::Null)
    };
    if let Some(entries) = parsed.get_mut("entries").and_then(|e| e.as_array_mut()) {
        for e in entries {
            let icon = made_for(e.get("icon_from")).or_else(|| e["icon_site"].as_str().map(String::from));
            if let Some(o) = e.as_object_mut() {
                o.remove("icon_from");
                o.remove("icon_site");
                match icon {
                    Some(i) => o.insert("icon".into(), Value::String(i)),
                    None => o.remove("icon"),
                };
                if let Some(w) = o.get_mut("wiki").and_then(|w| w.as_object_mut()) {
                    if let Some(p) = w.get("picture").cloned() {
                        w.insert("picture".into(), fix(&p));
                    }
                }
            }
        }
    }
    if let Some(res) = parsed.get_mut("resources").and_then(|e| e.as_array_mut()) {
        for r in res {
            let icon = made_for(r.get("icon_from"));
            if let Some(o) = r.as_object_mut() {
                o.remove("icon_from");
                o.insert("icon".into(), icon.map(Value::String).unwrap_or(Value::Null));
            }
        }
    }
    if let Some(stories) = parsed.get_mut("stories").and_then(|e| e.as_object_mut()) {
        for st in stories.values_mut() {
            if let Some(events) = st.get_mut("events").and_then(|e| e.as_object_mut()) {
                for ev in events.values_mut() {
                    if let Some(o) = ev.as_object_mut() {
                        if let Some(p) = o.get("picture").cloned() {
                            o.insert("picture".into(), fix(&p));
                        }
                    }
                }
            }
        }
    }
    Ok(made.values().filter(|v| v.is_some()).count())
}

/// The mod's Workshop requirements as relations of its entry.
pub fn relations(lookup: &Lookup) -> Vec<Value> {
    lookup.raw["required"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter_map(|x| {
                    let id = x["id"].as_str()?;
                    Some(json!({
                        "target_id": x["listed"].as_str().unwrap_or(""),
                        "target_name": x["title"].as_str().unwrap_or(id),
                        "relation_type": "dependency",
                        "note": format!("创意工坊 {id}"),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Submitted {
    /// an admin's entry: listed now (its item id)
    Published(String),
    /// a member's: waits for an admin
    Review,
    /// listed already, or someone's entry waits (`409`): the site's words
    Exists(String),
}

/// What `submit_entry` sends besides the Workshop item.
#[derive(Debug, Clone, Default)]
pub struct EntryForm {
    pub category: String,
    /// the mod's `descriptor.mod`
    pub raw_content: Option<String>,
    pub not_author: bool,
    /// `{ entries, groups, stories, resources }` of a parse (with `attach_art` done), if any
    pub parsed: Option<Value>,
    pub relations: Vec<Value>,
}

/// Creates the mod's entry on the site (`POST /creator/entries`).
pub fn submit_entry(s: &Session, workshop: u64, f: &EntryForm) -> Result<Submitted> {
    let mut details = json!({ "relations": f.relations });
    if let Some(p) = &f.parsed {
        for k in ["entries", "groups", "stories", "resources"] {
            if let Some(v) = p.get(k) {
                details[k] = v.clone();
            }
        }
    }
    let mut body = json!({ "workshop": workshop.to_string(), "category": f.category, "not_author": f.not_author, "details": details });
    if let Some(r) = f.raw_content.as_deref().filter(|r| !r.trim().is_empty()) {
        body["raw_content"] = Value::String(r.to_string());
    }
    let a = post_json("/creator/entries", Some(&s.token), &body)?;
    let v: Value = serde_json::from_slice(&a.body).unwrap_or(Value::Null);
    Ok(match a.status {
        201 => Submitted::Published(v["item"]["id"].as_str().unwrap_or_default().to_string()),
        202 => Submitted::Review,
        409 => Submitted::Exists(api_error(&a).to_string()),
        _ => return Err(api_error(&a)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips() {
        for s in ["", "a", "ab", "abc", "abcd", "{\"name\":\"整合包\"}", "\u{0}\u{ff}"] {
            let e = base64url_encode(s.as_bytes());
            assert!(!e.contains(['=', '+', '/']));
            assert_eq!(base64url_decode(&e).unwrap(), s.as_bytes(), "{s:?}");
        }
        assert_eq!(base64url_decode("aGk=").unwrap(), b"hi", "padding is allowed");
        assert!(base64url_decode("a+b/").is_err(), "plain base64 is not base64url");
        assert!(base64url_decode("a").is_err(), "one character is no byte");
    }

    #[test]
    fn reads_the_links_of_the_site() {
        let export = format!("{SITE}/api/v1/playsets/ps-abc123/export");
        let enc = |s: &str| crate::net::form_encode(s);
        assert_eq!(parse_link(&format!("stl://playset/import?url={}", enc(&export))).unwrap(), Link::ImportUrl(export.clone()));
        assert_eq!(parse_link(&format!("STL://Playset/Import/?url={export}")).unwrap(), Link::ImportUrl(export.clone()), "case and a trailing slash");
        let data = base64url_encode(br#"{"name":"x","mods":[]}"#);
        assert_eq!(parse_link(&format!("stl://playset/import?data={data}")).unwrap(), Link::ImportData(r#"{"name":"x","mods":[]}"#.into()));
        assert_eq!(parse_link("stl://playset/add-mod?id=mod%2Fugc_1623423360.mod").unwrap(), Link::AddMod("mod/ugc_1623423360.mod".into()));
        assert_eq!(parse_link("stl://mod/install-r2?id=gigas-cn-loc&descriptor=mod/ugc_2.mod").unwrap(), Link::InstallHosted { id: "gigas-cn-loc".into(), version: None });
        let sha = "AB".repeat(32);
        assert_eq!(
            parse_link(&format!("stl://plugin/install?id=stellaris-live2d&version=0.2.4&sha256={sha}")).unwrap(),
            Link::InstallPlugin { id: "stellaris-live2d".into(), version: Some("0.2.4".into()), sha256: Some(sha.to_ascii_lowercase()) }
        );
        assert_eq!(parse_link("stl://index/add?url=https%3A%2F%2Fstellarismod.com%2Fapi%2Fv1%2Fplugins.jsonl").unwrap(), Link::AddIndex("https://stellarismod.com/api/v1/plugins.jsonl".into()));
        let code = "0f".repeat(32);
        assert_eq!(parse_link(&format!("stl://account/link?code={code}&site=https%3A%2F%2Fstellarismod.com")).unwrap(), Link::Account { code: code.clone() });
    }

    #[test]
    fn refuses_links_a_page_could_abuse() {
        let bad = |l: &str| parse_link(l).is_err();
        assert!(bad("https://stellarismod.com"), "not stl://");
        assert!(bad("stl://playset/import?url=https://evil.example/api/v1/playsets/x/export"), "an export elsewhere");
        assert!(bad("stl://playset/import?url=https://stellarismod.com/api/v1/playsets/../../x/export"), "a path in the id");
        assert!(bad("stl://playset/import?url=https://stellarismod.com.evil.example/api/v1/playsets/x/export"), "a look-alike host");
        assert!(bad("stl://playset/import?url=x&data=e30"), "both");
        assert!(bad("stl://playset/add-mod?id=mod/../../x.mod"), "a path as a mod");
        assert!(bad("stl://playset/add-mod?id=C:\\x.mod"), "a file as a mod");
        assert!(bad("stl://plugin/install?id=..%2Fx"), "a path as a plugin");
        assert!(bad("stl://plugin/install?id=a&sha256=xyz"), "a broken sha256");
        assert!(bad("stl://plugin/install?id=a&id=b"), "a name twice");
        assert!(bad("stl://index/add?url=http://plain.example/plugins.jsonl"), "an index over plain http");
        assert!(bad("stl://index/add?url=https://x.example/a b"), "a space in an index URL");
        assert!(bad(&format!("stl://account/link?code={}&site=https://evil.example", "0f".repeat(32))), "another site");
        assert!(bad("stl://account/link?code=1234"), "a short code");
        assert!(bad("stl://launch/game"), "unknown");
        assert!(bad(&format!("stl://playset/import?data={}", "A".repeat(MAX_LINK))), "too long");
        assert!(bad("stl://playset/add-mod?id=%zz"), "a broken escape");
    }

    #[test]
    fn finds_the_link_among_arguments() {
        let args = ["C:\\x\\stellaris-launcher.exe", "--page=1", "stl://index/add?url=https://a.example/p.jsonl"].map(String::from);
        assert_eq!(link_in_args(args).as_deref(), Some("stl://index/add?url=https://a.example/p.jsonl"));
        assert_eq!(link_in_args(["a", "stl:"].map(String::from)), None);
    }

    const EXPORT: &str = r#"{
      "id": "ps-1", "name": " 整合包 ", "format": 1, "game_version": "v4.5.*", "page_url": "https://stellarismod.com/playsets/ps-1",
      "disabled_dlcs": null, "future": true,
      "mods": [
        { "id": "mod/ugc_1623423360.mod", "enabled": true, "name": "UI Overhaul Dynamic", "item_id": "ugc-1623423360",
          "workshop_id": "1623423360", "download_url": null, "sha256": null, "version": "1" },
        { "id": "mod/hosted_pack.mod", "enabled": false, "name": "Hosted", "download_url": "https://dl.stellarismod.com/x.zip",
          "sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" },
        { "id": "mod/my_local.mod", "enabled": true, "name": null }
      ],
      "plugins": [ { "id": "stellaris-mcp", "enabled": true, "name": "MCP", "version": "0.6.2",
                     "download_url": "https://plugins.stellarismod.com/x.zip", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" } ]
    }"#;

    #[test]
    fn reads_an_export_as_a_playset() {
        let e = parse_export(EXPORT).unwrap();
        assert_eq!(e.name, "整合包");
        assert_eq!(e.mods.iter().map(ExportMod::source).collect::<Vec<_>>(), vec![Source::Workshop(1623423360), Source::Hosted, Source::Local]);
        assert_eq!(e.mods[1].sha256.as_deref(), Some(&*"a".repeat(64)), "lower case");
        // it is also a playset of the store (unknown fields ignored), as the site promises
        let p: crate::store::Playset = serde_json::from_str(EXPORT).unwrap();
        assert_eq!(p.mods.len(), 3);
        let shared = e.to_shared();
        let ids: Vec<(String, bool)> = shared.mods.iter().map(|m| (m.descriptor(), m.enabled)).collect();
        assert_eq!(ids, vec![("mod/ugc_1623423360.mod".into(), true), ("mod/hosted_pack.mod".into(), false), ("mod/my_local.mod".into(), true)]);
        assert_eq!(shared.missing_steam(&[]), vec![1623423360]);
        assert!(shared.missing_pdx(&[]).is_empty(), "local mods are not Paradox mods");
        assert_eq!(e.local_missing(&[]).iter().map(|m| m.label()).collect::<Vec<_>>(), vec!["mod/my_local.mod"]);
        assert_eq!(e.hosted_missing(&[]).len(), 1);
        let entry = export_plugin_entry(&e.plugins[0]).unwrap();
        assert_eq!((entry.id.as_str(), entry.version.as_str()), ("stellaris-mcp", "0.6.2"));
    }

    #[test]
    fn refuses_an_export_a_page_could_abuse() {
        let with = |m: &str| format!(r#"{{"name":"x","mods":[{m}]}}"#);
        let bad = |m: &str| parse_export(&with(m)).is_err();
        assert!(bad(r#"{"id":"mod/../../evil.mod"}"#), "a path");
        assert!(bad(r#"{"id":"mod/x.mod","workshop_id":"12a"}"#), "a Workshop id that is no number");
        assert!(bad(r#"{"id":"mod/x.mod","download_url":"http://x.example/a.zip","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#), "plain http");
        assert!(bad(r#"{"id":"mod/x.mod","download_url":"https://x.example/a.zip","sha256":"abc"}"#), "a short sha256");
        assert!(bad(r#"{"id":"mod/x.mod"},{"id":"mod/x.mod"}"#), "twice");
        assert!(parse_export(r#"{"name":"  ","mods":[]}"#).is_err(), "no name");
        assert!(parse_export(r#"{"name":"x","format":2}"#).is_err(), "a newer format");
        assert!(parse_export(r#"{"name":"x","plugins":[{"id":"../x"}]}"#).is_err(), "a plugin path");
        // a Workshop mod's descriptor is the item's, whatever the id said
        let e = parse_export(&with(r#"{"id":"mod/whatever.mod","workshop_id":"42"}"#)).unwrap();
        assert_eq!(e.mods[0].id, "mod/ugc_42.mod");
    }

    #[test]
    fn picks_the_files_the_site_reads() {
        assert_eq!(wanted("common/technology/x.txt"), Some(true));
        assert_eq!(wanted("Common/X.CSV"), Some(true));
        assert_eq!(wanted("events/a.txt"), Some(true));
        assert_eq!(wanted("localisation/simp_chinese/x_l_simp_chinese.yml"), Some(true));
        assert_eq!(wanted("localisation/x_l_english.yml"), Some(true));
        assert_eq!(wanted("localisation/german/x_l_german.yml"), None);
        assert_eq!(wanted("localisation/replace/english/x.yml"), Some(true));
        assert_eq!(wanted("interface/x.gfx"), Some(true));
        assert_eq!(wanted("gfx/interface/icons/a.dds"), Some(false));
        assert_eq!(wanted("gfx/event_pictures/a.dds"), Some(false));
        assert_eq!(wanted("gfx/portraits/a.dds"), None);
        assert_eq!(wanted("descriptor.mod"), None);
        assert_eq!(wanted("sound/x.txt"), None);
    }

    #[test]
    fn art_goes_where_the_parse_says() {
        let dir = std::env::temp_dir().join(format!("stl-test-site-icons-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gfx/interface/icons")).unwrap();
        std::fs::write(dir.join("gfx/interface/icons/A.dds"), b"dds").unwrap();
        let files = mod_files(&dir).unwrap();
        assert!(files.textures.contains_key("gfx/interface/icons/a.dds"));
        let parsed = json!({ "entries": [
            { "id": "t1", "icon_from": [ { "path": "gfx/interface/icons/missing.dds", "frames": 1, "frame": 1, "square": false },
                                          { "path": "gfx/interface/icons/a.dds", "frames": 1, "frame": 1, "square": false } ] },
            { "id": "t2", "icon_from": [ { "path": "gfx/x.dds", "frames": 1, "frame": 1 } ], "icon_site": "/wiki-data/game/icons/x.webp" } ] });
        assert_eq!(first_present(&parsed["entries"][0]["icon_from"], &files).unwrap()["path"], "gfx/interface/icons/a.dds", "case-insensitive, first present");
        assert!(first_present(&parsed["entries"][1]["icon_from"], &files).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
