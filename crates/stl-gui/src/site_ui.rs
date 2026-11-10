//! What the window does with stellarismod.com: the `stl://` links it opens (shown in a sheet, done only after "Go ahead"), the account
//! (signing in by device code in Settings), and the wiki entry of a mod just published to the Workshop.

use super::*;
use crate::site_i18n::{t, ta};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use stl_core::pluginindex;
use stl_core::site::{self, Link, Source};

/// What a link would do, once what it names has been read.
pub enum Plan {
    Import(Box<site::Export>),
    AddMod { id: String, name: Option<String>, workshop: Option<u64> },
    Plugin(Box<pluginindex::Entry>),
    Hosted { descriptor: String, url: String, sha256: String, name: String, version: String },
    Index { url: String, count: usize },
    Account { code: String },
}

pub enum Done {
    Mod(String, Result<(), String>),
    Signed(Result<site::Poll, String>),
}

pub enum SheetState {
    Loading(Receiver<Result<Plan, String>>),
    Ready(Plan),
    Working(Receiver<Done>),
    Finished(String, bool),
}

pub enum SignMsg {
    Started(site::DeviceLink),
    Done(Result<site::Poll, String>),
}

pub struct SignIn {
    pub link: Option<site::DeviceLink>,
    rx: Receiver<SignMsg>,
    stop: Arc<AtomicBool>,
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub enum WikiMsg {
    Looked(Result<(site::Lookup, Vec<String>), String>),
    Progress(String),
    Done(Result<(site::Submitted, usize, usize), String>),
}

/// The "entry on stellarismod.com" sheet of a published mod.
pub struct WikiSheet {
    m: Mod,
    workshop: u64,
    lookup: Option<Result<site::Lookup, String>>,
    cats: Vec<String>,
    cat: usize,
    parse: bool,
    not_author: bool,
    rx: Option<Receiver<WikiMsg>>,
    progress: Option<String>,
    result: Option<Result<(site::Submitted, usize, usize), String>>,
}

#[derive(Default)]
pub struct SiteUi {
    links: Option<Receiver<String>>,
    /// links that came while a sheet was shown
    waiting: VecDeque<String>,
    sheet: Option<SheetState>,
    pub session: Option<site::Session>,
    sign_in: Option<SignIn>,
    account_msg: Option<(String, bool)>,
    wiki: Option<WikiSheet>,
}

/// A mod or plugin of a site playset the import gets besides the Workshop items.
pub enum Extra {
    Hosted { descriptor: String, url: String, sha256: String, label: String },
    Plugin(Box<pluginindex::Entry>),
}

impl Extra {
    pub fn label(&self) -> String {
        match self {
            Extra::Hosted { label, .. } => label.clone(),
            Extra::Plugin(e) => e.id.clone(),
        }
    }

    /// Done on the import's worker thread.
    pub fn get(&self, data_dir: &std::path::Path) -> Result<(), String> {
        match self {
            Extra::Hosted { descriptor, url, sha256, .. } => site::install_hosted_mod(data_dir, descriptor, url, sha256),
            Extra::Plugin(e) => pluginindex::install(e).map(|_| ()),
        }
        .map_err(|e| format!("{e:#}"))
    }
}

fn plan(link: Link, known: Vec<Mod>) -> Result<Plan, String> {
    let s = |e: anyhow::Error| format!("{e:#}");
    Ok(match link {
        Link::ImportUrl(u) => Plan::Import(Box::new(site::fetch_export(&u).map_err(s)?)),
        Link::ImportData(d) => Plan::Import(Box::new(site::parse_export(&d).map_err(s)?)),
        Link::AddMod(id) => {
            let name = known.iter().find(|m| m.id == id && m.problem.is_none()).map(|m| m.name.clone());
            Plan::AddMod { workshop: site::workshop_of(&id), id, name }
        }
        Link::InstallPlugin { id, version, sha256 } => Plan::Plugin(Box::new(site::site_plugin(&id, version.as_deref(), sha256.as_deref()).map_err(s)?)),
        Link::InstallHosted { id, version } => {
            let item = site::item(&id).map_err(s)?;
            if item["kind"].as_str() == Some("stl-plugin") {
                Plan::Plugin(Box::new(site::site_plugin(&id, version.as_deref(), None).map_err(s)?))
            } else {
                let (url, sha256, version) = site::release_of(&item, version.as_deref()).map_err(s)?;
                let descriptor = item["descriptor_id"].as_str().filter(|d| !d.is_empty()).map(String::from).unwrap_or_else(|| format!("mod/{id}.mod"));
                let descriptor = site::check_descriptor(&descriptor).map_err(s)?;
                Plan::Hosted { descriptor, url, sha256, name: item["name"].as_str().unwrap_or(&id).to_string(), version }
            }
        }
        Link::AddIndex(url) => {
            let count = pluginindex::fetch(&url).map_err(s)?.len();
            Plan::Index { url, count }
        }
        Link::Account { code } => Plan::Account { code },
    })
}

impl App {
    /// The links of later starts come here; `first` is the link this start was given.
    pub fn start_links(&mut self, ctx: &egui::Context, first: Option<String>) {
        self.site.session = site::saved();
        let (tx, rx) = channel();
        let c = ctx.clone();
        if stl_core::relay::listen(move |link| {
            let _ = tx.send(link);
            c.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            c.send_viewport_cmd(egui::ViewportCommand::Focus);
            c.request_repaint();
        })
        .is_ok()
        {
            self.site.links = Some(rx);
        }
        if let Some(l) = first {
            self.site.waiting.push_back(l);
        }
    }

    /// The next link, when no sheet is shown; and what the work of a sheet or a sign-in brought.
    pub fn drain_site(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.site.links {
            while let Ok(l) = rx.try_recv() {
                self.site.waiting.push_back(l);
            }
        }
        if self.site.sheet.is_none() {
            if let Some(link) = self.site.waiting.pop_front() {
                self.say(format!("link: {}", link.chars().take(160).collect::<String>()));
                self.site.sheet = Some(match site::parse_link(&link) {
                    Ok(l) => {
                        let (tx, rx) = channel();
                        let known = self.mods.clone();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(plan(l, known));
                            c.request_repaint();
                        });
                        SheetState::Loading(rx)
                    }
                    Err(e) => SheetState::Finished(ta(self.lang, "link.bad", &[&format!("{e:#}")]), false),
                });
            }
        }
        let mut next = None;
        match &self.site.sheet {
            Some(SheetState::Loading(rx)) => match rx.try_recv() {
                Ok(Ok(p)) => next = Some(SheetState::Ready(p)),
                Ok(Err(e)) => next = Some(SheetState::Finished(ta(self.lang, "link.bad", &[&e]), false)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => next = Some(SheetState::Finished(ta(self.lang, "link.bad", &["?"]), false)),
                Err(_) => ctx.request_repaint_after(Duration::from_millis(200)),
            },
            Some(SheetState::Working(rx)) => match rx.try_recv() {
                Ok(d) => next = Some(self.finish_link(d)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => next = Some(SheetState::Finished(ta(self.lang, "link.failed", &["?"]), false)),
                Err(_) => ctx.request_repaint_after(Duration::from_millis(200)),
            },
            _ => {}
        }
        if let Some(n) = next {
            self.site.sheet = Some(n);
        }
        self.drain_sign_in(ctx);
        self.drain_wiki(ctx);
    }

    fn finish_link(&mut self, d: Done) -> SheetState {
        let lang = self.lang;
        match d {
            Done::Mod(id, Ok(())) => {
                if let Ok(g) = &self.game {
                    mods::sync_workshop(&g.data_dir);
                    self.mods = mods::scan(&g.data_dir);
                }
                let i = self.store.active_index();
                self.store.playsets[i].set_mod(&id, true);
                self.save();
                let name = self.mods.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or(id);
                let line = ta(lang, "link.done_mod", &[&name, &self.store.playsets[i].name]);
                self.say(line.clone());
                SheetState::Finished(line, true)
            }
            Done::Mod(_, Err(e)) => SheetState::Finished(ta(lang, "link.failed", &[&e]), false),
            Done::Signed(Ok(site::Poll::Signed(s))) => {
                let line = ta(lang, "link.done_signed", &[s.shown_name()]);
                self.site.session = Some(s);
                self.say(line.clone());
                SheetState::Finished(line, true)
            }
            Done::Signed(Ok(site::Poll::Refused)) => SheetState::Finished(t(lang, "acc.refused").to_string(), false),
            Done::Signed(Ok(_)) => SheetState::Finished(t(lang, "acc.expired").to_string(), false),
            Done::Signed(Err(e)) => SheetState::Finished(ta(lang, "link.failed", &[&e]), false),
        }
    }

    /// "Go ahead" on a link's sheet.
    fn run_plan(&mut self, p: Plan, ctx: &egui::Context) -> SheetState {
        let lang = self.lang;
        let (tx, rx) = channel();
        let c = ctx.clone();
        let wake = move || c.request_repaint();
        match p {
            Plan::Import(e) => match self.import_site_playset(&e) {
                Ok(name) => SheetState::Finished(ta(lang, "link.import_started", &[&name]), true),
                Err(e) => SheetState::Finished(ta(lang, "link.failed", &[&e]), false),
            },
            Plan::AddMod { id, name: Some(_), .. } => {
                let _ = tx.send(Done::Mod(id, Ok(())));
                SheetState::Working(rx)
            }
            Plan::AddMod { id, workshop: Some(w), .. } => {
                let Ok(g) = self.game.clone() else { return SheetState::Finished(ta(lang, "link.failed", &["no game"]), false) };
                std::thread::spawn(move || {
                    let r = workshop::fetch(&g.dir, &[w], &mut |_| {}).map_err(|e| format!("{e:#}")).and_then(|failed| match failed.first() {
                        Some((_, why)) => Err(why.clone()),
                        None => Ok(()),
                    });
                    let _ = tx.send(Done::Mod(id, r));
                    wake();
                });
                SheetState::Working(rx)
            }
            Plan::AddMod { id, .. } => SheetState::Finished(ta(lang, "link.failed", &[&format!("{id}: not in the mod folder")]), false),
            Plan::Plugin(e) => {
                let id = e.id.clone();
                self.queue_plugin_job(&id, PluginJob::Get(*e));
                SheetState::Finished(ta(lang, "link.done_plugin", &[&id]), true)
            }
            Plan::Hosted { descriptor, url, sha256, .. } => {
                let Ok(g) = self.game.clone() else { return SheetState::Finished(ta(lang, "link.failed", &["no game"]), false) };
                std::thread::spawn(move || {
                    let r = site::install_hosted_mod(&g.data_dir, &descriptor, &url, &sha256).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Done::Mod(descriptor, r));
                    wake();
                });
                SheetState::Working(rx)
            }
            Plan::Index { url, .. } => {
                let mut list = pluginindex::indexes(self.store.plugin_indexes.as_ref());
                if !list.contains(&url) {
                    list.push(url);
                }
                self.acts.push(Act::SetIndexes(Some(list)));
                SheetState::Finished(t(lang, "link.done_index").to_string(), true)
            }
            Plan::Account { code } => {
                std::thread::spawn(move || {
                    let _ = tx.send(Done::Signed(site::poll(&code).map_err(|e| format!("{e:#}"))));
                    wake();
                });
                SheetState::Working(rx)
            }
        }
    }

    /// A playset of the site: made at once (its plugins switched as it says), and queued for what it lacks, like a playset file.
    fn import_site_playset(&mut self, e: &site::Export) -> Result<String, String> {
        let Ok(game) = self.game.clone() else { return Err("the game was not found".into()) };
        mods::sync_workshop(&game.data_dir);
        self.mods = mods::scan(&game.data_dir);
        let shared = e.to_shared();
        let i = import::add_shared(&mut self.store, &shared).map_err(|e| format!("{e:#}"))?;
        for p in &e.plugins {
            self.store.playsets[i].set_plugin(&p.id, p.enabled);
        }
        if e.disabled_dlcs.is_some() {
            self.store.playsets[i].disabled_dlcs = e.disabled_dlcs.clone();
        }
        self.store.set_active(i);
        self.ps_mod_filter.clear();
        self.save();
        let mut extras: Vec<Extra> = e
            .hosted_missing(&self.mods)
            .into_iter()
            .map(|m| Extra::Hosted { descriptor: m.id.clone(), url: m.download_url.clone().unwrap_or_default(), sha256: m.sha256.clone().unwrap_or_default(), label: m.label().to_string() })
            .collect();
        for p in e.plugins_missing(&self.plugins) {
            match site::export_plugin_entry(p) {
                Ok(entry) => extras.push(Extra::Plugin(Box::new(entry))),
                Err(why) => self.say(format!("{}: {why:#}", p.id)),
            }
        }
        let (playset, name) = (self.store.playsets[i].id.clone(), self.store.playsets[i].name.clone());
        let want = shared.missing_steam(&self.mods);
        self.say(format!("playset {name} made from {}: {} mods, {} to get from the Workshop, {} from the site", e.page_url.as_deref().unwrap_or("a link"), shared.mods.len(), want.len(), extras.len()));
        let job = ImportJob { playset, name: name.clone(), shared, want, extras, extra_failed: Vec::new() };
        if job.want.is_empty() && job.extras.is_empty() {
            self.finish_import(job, Vec::new());
        } else {
            self.imports.waiting.push_back(job);
        }
        self.next_import();
        Ok(name)
    }

    pub fn link_sheet(&mut self, ctx: &egui::Context) {
        let Some(state) = self.site.sheet.take() else { return };
        let lang = self.lang;
        let mut go = false;
        let mut close = false;
        let mut open = None;
        let active = self.store.active_playset().name.clone();
        let resp = egui::Modal::new(egui::Id::new("site-link")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.label(RichText::new(t(lang, "link.title")).size(22.0).family(bold()));
            ui.add_space(8.0);
            let body = |ui: &mut Ui, text: &str| {
                ui.add(egui::Label::new(RichText::new(text).size(13.0).color(LABEL)).wrap());
            };
            let note = |ui: &mut Ui, text: &str| {
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(SECONDARY)).wrap());
            };
            match &state {
                SheetState::Loading(_) | SheetState::Working(_) => {
                    ui.label(RichText::new(t(lang, if matches!(state, SheetState::Loading(_)) { "link.loading" } else { "link.working" })).size(13.0).color(SECONDARY));
                    theme::progress_bar(ui, None, ui.input(|i| i.time));
                }
                SheetState::Finished(text, ok) => {
                    ui.add(egui::Label::new(RichText::new(text).size(13.5).color(if *ok { GREEN } else { RED })).wrap());
                }
                SheetState::Ready(p) => {
                    note(ui, t(lang, "link.asks"));
                    ui.add_space(8.0);
                    glass(ui, 12.0, 12.0, |ui| match p {
                        Plan::Import(e) => {
                            ui.label(RichText::new(ta(lang, "link.import", &[&e.name])).size(15.0).family(bold()));
                            let hosted = e.count(|s| *s == Source::Hosted);
                            let local = e.count(|s| *s == Source::Local);
                            let ws = e.mods.len() - hosted - local;
                            let plugins = if e.plugins.is_empty() { "—".to_string() } else { e.plugins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ") };
                            body(ui, &ta(lang, "link.import_counts", &[&e.mods.len().to_string(), &ws.to_string(), &hosted.to_string(), &local.to_string(), &plugins]));
                            let missing: Vec<&str> = e.local_missing(&self.mods).iter().map(|m| m.label()).collect();
                            if !missing.is_empty() {
                                ui.add_space(4.0);
                                ui.add(egui::Label::new(RichText::new(ta(lang, "link.import_local", &[&missing.join(", ")])).size(12.5).color(ORANGE)).wrap());
                            }
                            ui.add_space(4.0);
                            note(ui, t(lang, "link.import_note"));
                            if let Some(page) = e.page_url.as_deref().filter(|u| u.starts_with(&format!("{}/", site::site()))) {
                                if theme::text_link(ui, t(lang, "link.page"), 12.5).clicked() {
                                    open = Some(page.to_string());
                                }
                            }
                        }
                        Plan::AddMod { id, name, workshop } => {
                            let what = name.clone().unwrap_or_else(|| workshop.map(|w| ta(lang, "link.workshop_item", &[&w.to_string()])).unwrap_or_else(|| id.clone()));
                            ui.label(RichText::new(ta(lang, "link.add_mod", &[&what, &active])).size(15.0).family(bold()));
                            if name.is_none() && workshop.is_some() {
                                note(ui, t(lang, "link.add_mod_get"));
                            }
                            if let Some(w) = workshop {
                                if theme::text_link(ui, t(lang, "link.page"), 12.5).clicked() {
                                    open = Some(workshop::item_url(*w));
                                }
                            }
                        }
                        Plan::Plugin(e) => {
                            ui.label(RichText::new(ta(lang, "link.plugin", &[&e.name, &e.version])).size(15.0).family(bold()));
                            if !e.description.is_empty() {
                                body(ui, &e.description);
                            }
                            ui.add_space(4.0);
                            note(ui, &ta(lang, "link.plugin_from", &[e.sources().first().map(String::as_str).unwrap_or(""), &e.sha256]));
                            if theme::text_link(ui, t(lang, "link.page"), 12.5).clicked() {
                                open = Some(site::item_page(&e.id));
                            }
                        }
                        Plan::Hosted { name, version, url, sha256, .. } => {
                            ui.label(RichText::new(ta(lang, "link.hosted", &[name, version, &active])).size(15.0).family(bold()));
                            note(ui, &format!("{url}\nSHA-256 {sha256}"));
                        }
                        Plan::Index { url, count } => {
                            ui.label(RichText::new(ta(lang, "link.index", &[url, &count.to_string()])).size(15.0).family(bold()));
                            note(ui, t(lang, "link.index_note"));
                        }
                        Plan::Account { .. } => {
                            ui.label(RichText::new(t(lang, "link.account")).size(15.0).family(bold()));
                            note(ui, t(lang, "link.account_note"));
                        }
                    });
                }
            }
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match &state {
                    SheetState::Ready(_) => {
                        if pill_button(ui, t(lang, "link.go"), ButtonStyle::Filled(BLUE), true).clicked() {
                            go = true;
                        }
                        if pill_button(ui, t(lang, "link.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                            close = true;
                        }
                    }
                    SheetState::Finished(..) => {
                        if pill_button(ui, t(lang, "link.close"), ButtonStyle::Filled(BLUE), true).clicked() {
                            close = true;
                        }
                    }
                    // what runs goes on; the sheet can be closed meanwhile only while reading
                    SheetState::Loading(_) => {
                        if pill_button(ui, t(lang, "link.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                            close = true;
                        }
                    }
                    SheetState::Working(_) => {}
                }
            });
        });
        if let Some(u) = open {
            open_link(&u);
        }
        let working = matches!(state, SheetState::Working(_));
        if (close || resp.should_close()) && !working {
            if let SheetState::Ready(_) = state {
                self.say("link: cancelled");
            }
            return;
        }
        self.site.sheet = Some(match state {
            SheetState::Ready(p) if go => self.run_plan(p, ctx),
            other => other,
        });
    }

    // ---- the account

    fn start_sign_in(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let halt = stop.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let d = match site::start_link(&site::device_name()) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(SignMsg::Done(Err(format!("{e:#}"))));
                    c.request_repaint();
                    return;
                }
            };
            let (code, interval, until) = (d.device_code.clone(), d.interval.clamp(1, 30), Instant::now() + Duration::from_secs(d.expires_in.min(900)));
            open_link(&d.verify_url);
            let _ = tx.send(SignMsg::Started(d));
            c.request_repaint();
            while Instant::now() < until {
                std::thread::sleep(Duration::from_secs(interval));
                if halt.load(Ordering::Relaxed) {
                    return;
                }
                match site::poll(&code) {
                    Ok(site::Poll::Pending) => continue,
                    r => {
                        let _ = tx.send(SignMsg::Done(r.map_err(|e| format!("{e:#}"))));
                        c.request_repaint();
                        return;
                    }
                }
            }
            let _ = tx.send(SignMsg::Done(Ok(site::Poll::Expired)));
            c.request_repaint();
        });
        self.site.account_msg = None;
        self.site.sign_in = Some(SignIn { link: None, rx, stop });
    }

    fn drain_sign_in(&mut self, _ctx: &egui::Context) {
        let lang = self.lang;
        let Some(s) = self.site.sign_in.as_mut() else { return };
        let mut done = None;
        while let Ok(m) = s.rx.try_recv() {
            match m {
                SignMsg::Started(d) => s.link = Some(d),
                SignMsg::Done(r) => done = Some(r),
            }
        }
        if let Some(r) = done {
            self.site.sign_in = None;
            self.site.account_msg = Some(match r {
                Ok(site::Poll::Signed(s)) => {
                    let line = ta(lang, "link.done_signed", &[s.shown_name()]);
                    self.site.session = Some(s);
                    (line, true)
                }
                Ok(site::Poll::Refused) => (t(lang, "acc.refused").to_string(), false),
                Ok(_) => (t(lang, "acc.expired").to_string(), false),
                Err(e) => (ta(lang, "link.failed", &[&e]), false),
            });
        }
    }

    /// The account's section of Settings.
    pub fn account_section(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let ctx = ui.ctx().clone();
        theme::section(ui, t(lang, "acc.section"));
        let mut sign_in = false;
        let mut sign_out = false;
        let mut cancel = false;
        let mut reopen = None;
        glass(ui, 16.0, 0.0, |ui| {
            let mut rows = Rows::new();
            match (&self.site.session, &self.site.sign_in) {
                (Some(s), _) => {
                    let line = ta(lang, "acc.signed_in", &[s.shown_name(), &s.username]);
                    rows.row(ui, 48.0, 140.0, false, |ui| {
                        ui.label(line);
                    }, |ui| {
                        if pill_button(ui, t(lang, "acc.sign_out"), ButtonStyle::Plain(RED), true).clicked() {
                            sign_out = true;
                        }
                    });
                }
                (None, Some(si)) => {
                    let text = match &si.link {
                        Some(d) => ta(lang, "acc.code", &[&d.user_code]),
                        None => t(lang, "acc.waiting").to_string(),
                    };
                    let url = si.link.as_ref().map(|d| d.verify_url.clone());
                    rows.row(ui, 56.0, 260.0, false, |ui| {
                        ui.add(egui::Label::new(RichText::new(text).size(13.0)).wrap());
                    }, |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if pill_button(ui, t(lang, "link.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                            cancel = true;
                        }
                        if let Some(u) = url {
                            if pill_button(ui, t(lang, "acc.reopen"), ButtonStyle::Tinted(BLUE), true).clicked() {
                                reopen = Some(u);
                            }
                        }
                    });
                }
                (None, None) => {
                    rows.row(ui, 48.0, 140.0, false, |ui| {
                        ui.label(RichText::new(t(lang, "acc.signed_out")).color(SECONDARY));
                    }, |ui| {
                        if pill_button(ui, t(lang, "acc.sign_in"), ButtonStyle::Tinted(BLUE), true).clicked() {
                            sign_in = true;
                        }
                    });
                }
            }
        });
        if let Some((m, ok)) = &self.site.account_msg {
            ui.label(RichText::new(m).size(12.5).color(if *ok { GREEN } else { RED }));
        }
        theme::footnote(ui, t(lang, "acc.hint"));
        if let Some(u) = reopen {
            open_link(&u);
        }
        if cancel {
            self.site.sign_in = None;
        }
        if sign_in {
            self.start_sign_in(&ctx);
        }
        if sign_out {
            if let Some(s) = self.site.session.take() {
                std::thread::spawn(move || {
                    let _ = site::logout(&s);
                });
            }
            self.site.account_msg = None;
        }
    }

    /// An error of a site call that signed the launcher out forgets the account here too.
    fn note_site_error(&mut self, e: &str) {
        if e.contains("signed this launcher out") {
            self.site.session = None;
            self.site.account_msg = Some((t(self.lang, "acc.gone").to_string(), false));
        }
    }

    // ---- the wiki entry of a published mod

    /// The sheet of the entry of a mod just published (the upload sheet offers it).
    pub fn open_wiki(&mut self, ctx: &egui::Context, m: Mod, workshop: u64) {
        let Some(s) = self.site.session.clone() else { return };
        let (tx, rx) = channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let r = site::workshop_lookup(&s, workshop).and_then(|l| Ok((l, site::categories(&s)?))).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WikiMsg::Looked(r));
            c.request_repaint();
        });
        self.site.wiki = Some(WikiSheet { m, workshop, lookup: None, cats: Vec::new(), cat: 0, parse: true, not_author: false, rx: Some(rx), progress: None, result: None });
    }

    fn drain_wiki(&mut self, ctx: &egui::Context) {
        let mut errors = Vec::new();
        if let Some(w) = self.site.wiki.as_mut() {
            if let Some(rx) = &w.rx {
                ctx.request_repaint_after(Duration::from_millis(250));
                let mut finished = false;
                while let Ok(m) = rx.try_recv() {
                    match m {
                        WikiMsg::Looked(Ok((l, cats))) => {
                            w.cat = l.suggested_category.as_ref().and_then(|c| cats.iter().position(|x| x == c)).unwrap_or(0);
                            w.cats = cats;
                            w.lookup = Some(Ok(l));
                            finished = true;
                        }
                        WikiMsg::Looked(Err(e)) => {
                            errors.push(e.clone());
                            w.lookup = Some(Err(e));
                            finished = true;
                        }
                        WikiMsg::Progress(p) => w.progress = Some(p),
                        WikiMsg::Done(r) => {
                            if let Err(e) = &r {
                                errors.push(e.clone());
                            }
                            w.result = Some(r);
                            w.progress = None;
                            finished = true;
                        }
                    }
                }
                if finished {
                    w.rx = None;
                }
            }
        }
        for e in errors {
            self.note_site_error(&e);
        }
    }

    fn submit_wiki(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let Some(s) = self.site.session.clone() else { return };
        let Some(w) = self.site.wiki.as_mut() else { return };
        let Some(Ok(lookup)) = w.lookup.clone() else { return };
        let (tx, rx) = channel();
        let c = ctx.clone();
        let (m, workshop, parse, not_author) = (w.m.clone(), w.workshop, w.parse, w.not_author);
        let category = w.cats.get(w.cat).cloned().unwrap_or_default();
        std::thread::spawn(move || {
            let say = |p: String| {
                let _ = tx.send(WikiMsg::Progress(p));
                c.request_repaint();
            };
            let r = (|| -> anyhow::Result<(site::Submitted, usize, usize)> {
                let folder = m.path.clone();
                let raw_content = folder.as_ref().and_then(|f| std::fs::read(f.join("descriptor.mod")).ok()).map(|b| String::from_utf8_lossy(&b).to_string()).filter(|t| t.len() < 64 * 1024);
                let (mut parsed, mut entries, mut pictures) = (None, 0, 0);
                if parse {
                    if let Some(folder) = &folder {
                        say(t(lang, "wiki.parsing").to_string());
                        let files = site::mod_files(folder)?;
                        let mut p = site::parse_mod(&s, &files)?;
                        entries = p["entries"].as_array().map(|e| e.len()).unwrap_or(0);
                        pictures = site::attach_art(&s, &mut p, &files, &mut |done, total| say(ta(lang, "wiki.icons", &[&done.to_string(), &total.to_string()])))?;
                        parsed = Some(p);
                    }
                }
                say(t(lang, "wiki.sending").to_string());
                let form = site::EntryForm { category, raw_content, not_author, parsed, relations: site::relations(&lookup) };
                Ok((site::submit_entry(&s, workshop, &form)?, entries, pictures))
            })();
            let _ = tx.send(WikiMsg::Done(r.map_err(|e| format!("{e:#}"))));
            c.request_repaint();
        });
        w.rx = Some(rx);
        w.result = None;
    }

    pub fn wiki_sheet(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let Some(w) = self.site.wiki.as_mut() else { return };
        let mut close = false;
        let mut submit = false;
        let mut open = None;
        let busy = w.rx.is_some();
        let resp = egui::Modal::new(egui::Id::new("site-wiki")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.label(RichText::new(t(lang, "wiki.title")).size(22.0).family(bold()));
            ui.label(RichText::new(format!("{} · {}", w.m.name, w.workshop)).size(12.5).color(SECONDARY));
            ui.add_space(10.0);
            let mut can_submit = false;
            match &w.lookup {
                None => {
                    ui.label(RichText::new(ta(lang, "wiki.looking", &[&w.workshop.to_string()])).size(13.0).color(SECONDARY));
                    theme::progress_bar(ui, None, ui.input(|i| i.time));
                }
                Some(Err(e)) => {
                    ui.add(egui::Label::new(RichText::new(ta(lang, "link.failed", &[e])).size(13.0).color(RED)).wrap());
                }
                Some(Ok(l)) if l.listed.is_some() => {
                    ui.label(RichText::new(t(lang, "wiki.listed")).size(13.5).color(GREEN));
                    if theme::text_link(ui, t(lang, "link.page"), 12.5).clicked() {
                        open = l.listed.as_deref().map(site::item_page);
                    }
                }
                Some(Ok(l)) if l.pending => {
                    ui.label(RichText::new(t(lang, "wiki.pending")).size(13.5).color(ORANGE));
                }
                Some(Ok(l)) => {
                    ui.label(RichText::new(&l.title).size(15.0).family(bold()));
                    ui.add_space(6.0);
                    let enabled = !busy && w.result.as_ref().map_or(true, |r| r.is_err());
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(t(lang, "wiki.category"));
                            egui::ComboBox::from_id_salt("wiki-cat").selected_text(w.cats.get(w.cat).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                                for (i, c) in w.cats.iter().enumerate() {
                                    ui.selectable_value(&mut w.cat, i, c);
                                }
                            });
                        });
                        ui.checkbox(&mut w.parse, t(lang, "wiki.parse"));
                        ui.checkbox(&mut w.not_author, t(lang, "wiki.not_author"));
                    });
                    can_submit = enabled && !w.cats.is_empty();
                }
            }
            if let Some(p) = &w.progress {
                ui.add_space(8.0);
                ui.label(RichText::new(p).size(12.5).color(SECONDARY));
                theme::progress_bar(ui, None, ui.input(|i| i.time));
            }
            match &w.result {
                Some(Ok((r, entries, pictures))) => {
                    ui.add_space(8.0);
                    let (text, color) = match r {
                        site::Submitted::Published(_) => (t(lang, "wiki.published").to_string(), GREEN),
                        site::Submitted::Review => (t(lang, "wiki.review").to_string(), GREEN),
                        site::Submitted::Exists(why) => (ta(lang, "wiki.exists", &[why]), ORANGE),
                    };
                    ui.add(egui::Label::new(RichText::new(text).size(13.5).color(color)).wrap());
                    if w.parse && *entries > 0 {
                        ui.label(RichText::new(ta(lang, "wiki.counts", &[&entries.to_string(), &pictures.to_string()])).size(12.5).color(SECONDARY));
                    }
                    if let site::Submitted::Published(id) = r {
                        if !id.is_empty() && theme::text_link(ui, t(lang, "link.page"), 12.5).clicked() {
                            open = Some(site::item_page(id));
                        }
                    }
                }
                Some(Err(e)) => {
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(ta(lang, "link.failed", &[e])).size(13.0).color(RED)).wrap());
                }
                None => {}
            }
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if can_submit && pill_button(ui, t(lang, "wiki.create"), ButtonStyle::Filled(BLUE), true).clicked() {
                    submit = true;
                }
                if pill_button(ui, t(lang, "link.close"), ButtonStyle::Plain(SECONDARY), !busy).clicked() {
                    close = true;
                }
            });
        });
        if let Some(u) = open {
            open_link(&u);
        }
        if (close || resp.should_close()) && !busy {
            self.site.wiki = None;
        } else if submit {
            self.submit_wiki(ctx);
        }
    }
}
