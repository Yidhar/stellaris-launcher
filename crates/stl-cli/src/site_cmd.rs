//! The commands for stellarismod.com: `stl open <stl://…>`, `stl import --url/--data`, `stl login/logout/account`, `stl plugin install <id>`
//! and `stl wiki-entry`. The site's dialogs show these commands for when the window did not open.

use anyhow::{bail, Context, Result};
use std::io::Write;
use stl_core::game::Game;
use stl_core::import::{self, Shared};
use stl_core::site::{self, Export, Link, Source};
use stl_core::store::Store;
use stl_core::{mods, pluginindex, plugins, process, workshop};

/// Asks on the terminal (yes skips it).
pub fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn print_progress(shared: &Shared) -> impl FnMut(workshop::FetchProgress) + '_ {
    let mut shown = None;
    move |p| {
        if shown != Some((p.downloading, p.index)) {
            shown = Some((p.downloading, p.index));
            println!("{} {}/{} {}", if p.downloading { "downloading" } else { "subscribing" }, p.index + 1, p.total, shared.name_of(p.id).unwrap_or_default());
        }
    }
}

/// Makes a playset of a shared file or a site export, gets what it lacks (Workshop items; the site's packages and plugins, checked
/// against their SHA-256), and puts the playset in the file's order.
pub fn import_playset(store: &mut Store, g: &Game, shared: &Shared, export: Option<&Export>, no_download: bool) -> Result<()> {
    mods::sync_workshop(&g.data_dir);
    let here = mods::scan(&g.data_dir);
    let i = import::add_shared(store, shared)?;
    if let Some(e) = export {
        for p in &e.plugins {
            store.playsets[i].set_plugin(&p.id, p.enabled);
        }
        if e.disabled_dlcs.is_some() {
            store.playsets[i].disabled_dlcs = e.disabled_dlcs.clone();
        }
    }
    store.set_active(i);
    store.save()?;
    let want = shared.missing_steam(&here);
    println!("{}: {} mods, {} missing from the Workshop", store.playsets[i].name, shared.mods.len(), want.len());
    if !no_download {
        if let Some(e) = export {
            for m in e.hosted_missing(&here) {
                println!("downloading {} from the site", m.label());
                if let Err(why) = site::install_hosted_mod(&g.data_dir, &m.id, m.download_url.as_deref().unwrap_or_default(), m.sha256.as_deref().unwrap_or_default()) {
                    println!("not got: {} ({why:#})", m.label());
                }
            }
            let installed = plugins::list().map(|(i, _)| i).unwrap_or_default();
            for p in e.plugins_missing(&installed) {
                println!("downloading the plugin {}", p.id);
                match site::export_plugin_entry(p).and_then(|entry| pluginindex::install(&entry)) {
                    Ok(pl) => println!("installed {} {}", pl.manifest.id, pl.manifest.version),
                    Err(why) => println!("not got: the plugin {} ({why:#})", p.id),
                }
            }
        }
        if !want.is_empty() {
            let failed = workshop::fetch(&g.dir, &want, &mut print_progress(shared))?;
            for (id, why) in failed {
                println!("not got: {} ({why})", shared.name_of(id).unwrap_or_default());
            }
        }
    }
    // what came down is a mod now; the playset is the file's, in the file's order
    mods::sync_workshop(&g.data_dir);
    let here = mods::scan(&g.data_dir);
    shared.apply(&mut store.playsets[i]);
    store.save()?;
    for m in shared.missing_pdx(&here) {
        println!("not got: {} (a Paradox mod: subscribe to it in the official launcher)", m.display_name);
    }
    for m in shared.missing_local(&here) {
        println!("missing: {} (a local mod: it has to be in the mod folder already)", m.display_name);
    }
    let lacking = shared.missing_steam(&here).len() + shared.missing_pdx(&here).len() + shared.missing_local(&here).len();
    println!("{} of {} mods are here", shared.mods.len() - lacking, shared.mods.len());
    Ok(())
}

fn describe_export(e: &Export) {
    println!("playset   {}", e.name);
    if let Some(p) = &e.page_url {
        println!("page      {p}");
    }
    if let Some(v) = &e.game_version {
        println!("game      {v}");
    }
    let hosted = e.count(|s| *s == Source::Hosted);
    let local = e.count(|s| *s == Source::Local);
    println!("mods      {} ({} from the Workshop, {hosted} hosted by the site, {local} local)", e.mods.len(), e.mods.len() - hosted - local);
    for p in &e.plugins {
        println!("plugin    {} {}", p.id, p.version.as_deref().unwrap_or(""));
    }
}

/// `stl import --url` / `--data`.
pub fn import(store: &mut Store, g: &Game, url: Option<&str>, data: Option<&str>, no_download: bool, yes: bool) -> Result<()> {
    let export = match (url, data) {
        (Some(u), None) => site::fetch_export(u)?,
        (None, Some(d)) => site::parse_export(&String::from_utf8(site::base64url_decode(d.trim())?).context("the playset is not UTF-8 text")?)?,
        _ => bail!("give --url (a playset export of the site) or --data (the playset a link carries)"),
    };
    import_export(store, g, &export, no_download, yes)
}

fn import_export(store: &mut Store, g: &Game, export: &Export, no_download: bool, yes: bool) -> Result<()> {
    describe_export(export);
    if !confirm("make this playset and get what it lacks?", yes)? {
        println!("nothing changed");
        return Ok(());
    }
    import_playset(store, g, &export.to_shared(), Some(export), no_download)
}

/// `stl login` (device code) and `stl login --code` (a sign-in the site allowed already).
pub fn login(code: Option<&str>) -> Result<()> {
    if let Some(s) = site::saved() {
        println!("signed in as {} ({}) already; stl logout first to use another account", s.shown_name(), s.username);
        return Ok(());
    }
    if let Some(code) = code {
        return match site::poll(code.trim())? {
            site::Poll::Signed(s) => {
                println!("signed in as {} ({})", s.shown_name(), s.username);
                Ok(())
            }
            site::Poll::Pending => bail!("the site has not allowed this sign-in (yet)"),
            site::Poll::Refused => bail!("the sign-in was refused on the site"),
            site::Poll::Expired => bail!("this sign-in code has run out (it is good for 2 minutes); start it again on the site"),
        };
    }
    let d = site::start_link(&site::device_name())?;
    println!("allow the launcher on the site: {}", d.verify_url);
    println!("the code it shows must be {}", d.user_code);
    let _ = std::process::Command::new("explorer").arg(&d.verify_url).spawn();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(d.expires_in.min(900));
    while std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_secs(d.interval.clamp(1, 30)));
        match site::poll(&d.device_code)? {
            site::Poll::Pending => continue,
            site::Poll::Signed(s) => {
                println!("signed in as {} ({})", s.shown_name(), s.username);
                return Ok(());
            }
            site::Poll::Refused => bail!("the sign-in was refused on the site"),
            site::Poll::Expired => break,
        }
    }
    bail!("the code ran out before it was allowed; run stl login again")
}

pub fn logout() -> Result<()> {
    match site::saved() {
        Some(s) => {
            site::logout(&s)?;
            println!("signed out of {}", s.site);
        }
        None => println!("not signed in"),
    }
    Ok(())
}

pub fn account() -> Result<()> {
    let Some(s) = site::saved() else {
        println!("not signed in (stl login)");
        return Ok(());
    };
    let me = site::me(&s)?;
    println!("{} ({}) on {}", me["display_name"].as_str().unwrap_or(s.shown_name()), me["username"].as_str().unwrap_or(&s.username), s.site);
    if let Some(r) = me["role"].as_str() {
        println!("role      {r}");
    }
    Ok(())
}

/// `stl plugin install <id>`: a plugin the site hosts.
pub fn install_site_plugin(id: &str, version: Option<&str>, sha256: Option<&str>, yes: bool) -> Result<()> {
    let e = site::site_plugin(id, version, sha256)?;
    println!("{} {} by {}: {}", e.id, e.version, if e.author.is_empty() { "?" } else { &e.author }, e.description);
    println!("from {} (SHA-256 {})", e.sources().join(", then "), e.sha256);
    if !confirm("install it?", yes)? {
        println!("nothing changed");
        return Ok(());
    }
    let p = pluginindex::install(&e)?;
    println!("installed {} {} into {}", p.manifest.id, p.manifest.version, p.dir.display());
    println!("load it with: stl plugin enable {}", p.manifest.id);
    if !process::find_processes("stellaris.exe").is_empty() {
        println!("the running game keeps what it has until it is started again");
    }
    Ok(())
}

/// Puts a mod into the active playset; a Workshop item not here is subscribed to and downloaded first.
fn add_mod(store: &mut Store, g: &Game, id: &str) -> Result<()> {
    mods::sync_workshop(&g.data_dir);
    let here = mods::scan(&g.data_dir);
    if !here.iter().any(|m| m.id == id && m.problem.is_none()) {
        match site::workshop_of(id) {
            Some(w) => {
                let failed = workshop::fetch(&g.dir, &[w], &mut |p| {
                    if p.index == 0 && p.bytes == 0 {
                        println!("{} {w}", if p.downloading { "downloading" } else { "subscribing to" });
                    }
                })?;
                if let Some((_, why)) = failed.first() {
                    bail!("the Workshop item {w} was not got: {why}");
                }
                mods::sync_workshop(&g.data_dir);
            }
            None => bail!("{id} is not in the mod folder"),
        }
    }
    let i = store.active_index();
    store.playsets[i].set_mod(id, true);
    store.save()?;
    println!("{} is in the playset {}", mods::scan(&g.data_dir).iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or_else(|| id.to_string()), store.playsets[i].name);
    Ok(())
}

/// `stl open <link>`: what an `stl://` link of the site says, after asking.
pub fn open(store: &mut Store, game: Result<&Game>, link: &str, yes: bool) -> Result<()> {
    let l = site::parse_link(link)?;
    match l {
        Link::ImportUrl(u) => import_export(store, game?, &site::fetch_export(&u)?, false, yes),
        Link::ImportData(d) => import_export(store, game?, &site::parse_export(&d)?, false, yes),
        Link::AddMod(id) => {
            let g = game?;
            let name = mods::scan(&g.data_dir).into_iter().find(|m| m.id == id).map(|m| m.name);
            let what = name.unwrap_or_else(|| match site::workshop_of(&id) {
                Some(w) => format!("the Workshop item {w}"),
                None => id.clone(),
            });
            if !confirm(&format!("add {what} to the playset {}?", store.active_playset().name), yes)? {
                println!("nothing changed");
                return Ok(());
            }
            add_mod(store, g, &id)
        }
        Link::InstallHosted { id, version } => {
            let item = site::item(&id)?;
            if item["kind"].as_str() == Some("stl-plugin") {
                return install_site_plugin(&id, version.as_deref(), None, yes);
            }
            let g = game?;
            let (url, sha, v) = site::release_of(&item, version.as_deref())?;
            let descriptor = item["descriptor_id"].as_str().filter(|d| !d.is_empty()).map(String::from).unwrap_or_else(|| format!("mod/{id}.mod"));
            println!("{} {v} ({descriptor}) from {url} (SHA-256 {sha})", item["name"].as_str().unwrap_or(&id));
            if !confirm("install it into the mod folder and add it to the active playset?", yes)? {
                println!("nothing changed");
                return Ok(());
            }
            site::install_hosted_mod(&g.data_dir, &descriptor, &url, &sha)?;
            add_mod(store, g, &descriptor)
        }
        Link::InstallPlugin { id, version, sha256 } => install_site_plugin(&id, version.as_deref(), sha256.as_deref(), yes),
        Link::AddIndex(url) => {
            let n = pluginindex::fetch(&url)?.len();
            if !confirm(&format!("read the plugin index {url} ({n} plugins) from now on?"), yes)? {
                println!("nothing changed");
                return Ok(());
            }
            let mut list = pluginindex::indexes(store.plugin_indexes.as_ref());
            if !list.contains(&url) {
                list.push(url.clone());
            }
            store.plugin_indexes = Some(list);
            store.save()?;
            println!("added {url}");
            Ok(())
        }
        Link::Account { code } => {
            if !confirm(&format!("sign the launcher in to {} with the account the site is signed in with?", site::site()), yes)? {
                println!("nothing changed");
                return Ok(());
            }
            login(Some(&code))
        }
    }
}

/// `stl wiki-entry <mod>`: creates the stellarismod.com entry of a mod published to the Workshop.
pub fn wiki_entry(m: &mods::Mod, workshop: Option<u64>, category: Option<&str>, parse: bool, not_author: bool, yes: bool) -> Result<()> {
    let s = site::saved().context("sign in first: stl login")?;
    let workshop = workshop.or_else(|| m.remote_file_id.as_deref().and_then(|r| r.parse().ok())).context("the mod has no remote_file_id: give --workshop <id>")?;
    let lookup = site::workshop_lookup(&s, workshop)?;
    println!("{} (Workshop {workshop})", lookup.title);
    if let Some(item) = &lookup.listed {
        println!("the site lists it already: {}", site::item_page(item));
        return Ok(());
    }
    if lookup.pending {
        println!("an entry for it waits for review on the site already");
        return Ok(());
    }
    let cats = site::categories(&s)?;
    let category = match category {
        Some(c) if cats.iter().any(|x| x == c) => c.to_string(),
        Some(c) => bail!("{c} is not a category of the site: {}", cats.join(", ")),
        None => lookup.suggested_category.clone().filter(|c| cats.contains(c)).or_else(|| cats.first().cloned()).context("the site gave no categories")?,
    };
    println!("category  {category} (of {})", cats.join(", "));
    let folder = m.path.clone().context("the mod has no folder")?;
    let raw_content = std::fs::read(folder.join("descriptor.mod")).ok().map(|b| String::from_utf8_lossy(&b).to_string()).filter(|t| t.len() < 64 * 1024);
    let mut parsed = None;
    if parse {
        let files = site::mod_files(&folder)?;
        println!("reading {} text files and {} textures of {}", files.texts.len(), files.textures.len(), folder.display());
        let mut p = site::parse_mod(&s, &files)?;
        println!("{} entries", p["entries"].as_array().map(|e| e.len()).unwrap_or(0));
        let made = site::attach_art(&s, &mut p, &files, &mut |done, total| {
            if total > 0 && (done == total || done == 0) {
                println!("icons {done}/{total}");
            }
        })?;
        println!("{made} pictures made");
        parsed = Some(p);
    }
    if !confirm(&format!("create the entry of {} on {}?", lookup.title, site::site()), yes)? {
        println!("nothing changed");
        return Ok(());
    }
    let form = site::EntryForm { category, raw_content, not_author, parsed, relations: site::relations(&lookup) };
    match site::submit_entry(&s, workshop, &form)? {
        site::Submitted::Published(id) => println!("listed: {}", site::item_page(&id)),
        site::Submitted::Review => println!("已提交，管理员审核后出现在百科 (sent; it shows on the site once an admin has reviewed it)"),
        site::Submitted::Exists(why) => println!("not created: {why}"),
    }
    Ok(())
}
