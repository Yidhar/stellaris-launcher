//! The window of the Stellaris launcher, in the manner of iOS: the game's artwork behind frosted-glass cards, a tab bar at the bottom.
//! Play (news, playset, start), Playsets (mods in order, DLC, plugins), Mods (the whole folder), Plugins (install, remove), Settings.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod i18n;
mod theme;

use assets::Assets;
use eframe::egui::{self, pos2, vec2, Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Shape, Ui, UiBuilder, Vec2};
use i18n::{tr, tr_args, Lang};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use stl_core::dlc::{self, Dlc};
use stl_core::game::Game;
use stl_core::mods::{self, Kind, Mod};
use stl_core::news::{self, Card};
use stl_core::plugins::{self, Compat, Plugin};
use stl_core::store::Store;
use stl_core::{artwork, dlcload, import, launch, official, pe, process};
use theme::{bold, chip, circle_button, glass, glass_pane, glass_rows, glass_scroll, large_title, pill_button, plain_rows, segmented, stack, switch, ButtonStyle, Icon, Rows, BLUE, GREEN, LABEL, ORANGE, PURPLE, RED, SECONDARY};

const STEAM_APP_ID: u32 = 281990;

enum Msg {
    Line(String),
    Done(Result<launch::Report, String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Play,
    Playsets,
    Mods,
    Plugins,
    Settings,
}

impl Page {
    const ALL: [Page; 5] = [Page::Play, Page::Playsets, Page::Mods, Page::Plugins, Page::Settings];

    fn icon(self) -> Icon {
        match self {
            Page::Play => Icon::Play,
            Page::Playsets => Icon::Playsets,
            Page::Mods => Icon::Mods,
            Page::Plugins => Icon::Plugins,
            Page::Settings => Icon::Settings,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Page::Play => "tab.play",
            Page::Playsets => "tab.playsets",
            Page::Mods => "tab.mods",
            Page::Plugins => "tab.plugins",
            Page::Settings => "tab.settings",
        }
    }
}

/// What the pages ask for; done after the frame is built, so that no page holds the data it changes.
enum Act {
    SetActive(usize),
    AddPlayset(String),
    DeletePlayset,
    Import,
    ModFlag(usize, bool),
    ModRemove(usize),
    ModMove(usize, i32),
    ModAdd(String),
    ModDrop(String),
    DlcFlag(String, bool),
    PluginFlag(String, bool),
    PluginRemove(String),
    PluginInstall(bool),
    Start,
    CloseGame,
    RefreshNews,
    Reload,
    SetLang(Option<Lang>),
    SetBackground(&'static str),
    SetNewsOnline(bool),
    ChangeGameDir,
    UseGameDir(String),
    Open(String),
}

/// The requests of a page; shared by the closures that build one row, so it uses a cell.
#[derive(Default)]
struct Acts(std::cell::RefCell<Vec<Act>>);

impl Acts {
    fn push(&self, a: Act) {
        self.0.borrow_mut().push(a);
    }

    fn take(&self) -> Vec<Act> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

struct News {
    cards: Vec<Card>,
    rx: Option<Receiver<Result<Vec<Card>, String>>>,
    hero: usize,
    switched: f64,
    error: Option<String>,
}

struct App {
    store: Store,
    game: Result<Game, String>,
    mods: Vec<Mod>,
    dlcs: Vec<Dlc>,
    dlc_current: Vec<String>,
    plugins: Vec<Plugin>,
    plugin_problems: Vec<String>,
    log: Vec<String>,
    running: Vec<u32>,
    last_poll: Instant,
    launching: Option<Receiver<Msg>>,
    continue_last: bool,
    use_plugins: bool,
    alternative: Option<usize>,
    lang: Lang,
    page: Page,
    seg: usize,
    filter: String,
    new_name: String,
    confirm_delete: bool,
    game_dir_text: String,
    assets: Assets,
    news: News,
    acts: Vec<Act>,
    logo: Option<PathBuf>,
    backgrounds: Vec<(&'static str, PathBuf)>,
}

fn resolve_lang(store: &Store, game: &Result<Game, String>) -> Lang {
    store
        .language
        .as_deref()
        .and_then(Lang::from_code)
        .or_else(|| game.as_ref().ok().and_then(|g| i18n::game_language(&g.data_dir)))
        .unwrap_or(Lang::En)
}

fn open_link(url: &str) {
    use std::os::windows::process::CommandExt;
    if url.starts_with("http://") || url.starts_with("https://") {
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn();
    }
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        let store = Store::load().unwrap_or_else(|e| {
            eprintln!("playsets: {e:#}");
            Store::load_from(&std::env::temp_dir().join("stl-playsets.json")).expect("a store in the temp folder")
        });
        let mut app = App {
            store,
            game: Err("not looked for yet".into()),
            mods: Vec::new(),
            dlcs: Vec::new(),
            dlc_current: Vec::new(),
            plugins: Vec::new(),
            plugin_problems: Vec::new(),
            log: Vec::new(),
            running: Vec::new(),
            last_poll: Instant::now() - Duration::from_secs(10),
            launching: None,
            continue_last: false,
            use_plugins: true,
            alternative: None,
            lang: Lang::En,
            page: Page::Play,
            seg: 0,
            filter: String::new(),
            new_name: String::new(),
            confirm_delete: false,
            game_dir_text: String::new(),
            assets: Assets::new(ctx),
            news: News { cards: Vec::new(), rx: None, hero: 0, switched: 0.0, error: None },
            acts: Vec::new(),
            logo: None,
            backgrounds: Vec::new(),
        };
        app.reload();
        app.lang = resolve_lang(&app.store, &app.game);
        // for screenshots while developing: --page=1 --seg=1 --lang=ja
        for a in std::env::args().skip(1) {
            if let Some(v) = a.strip_prefix("--page=") {
                app.page = Page::ALL.get(v.parse::<usize>().unwrap_or(0)).copied().unwrap_or(Page::Play);
            } else if let Some(v) = a.strip_prefix("--seg=") {
                app.seg = v.parse().unwrap_or(0);
            } else if let Some(v) = a.strip_prefix("--lang=") {
                app.lang = Lang::from_code(v).unwrap_or(Lang::En);
            }
        }
        theme::install_fonts(ctx, app.lang);
        app.load_news_local();
        if app.store.news_online != Some(false) {
            let stale = news::cache_dir().ok().and_then(|d| std::fs::metadata(d.join("feed.json")).ok()).and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map_or(true, |age| age > Duration::from_secs(6 * 3600));
            if stale || app.news.cards.is_empty() {
                app.refresh_news();
            }
        }
        app
    }

    fn say(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 400 {
            self.log.drain(0..100);
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.store.save() {
            self.say(format!("could not save the playsets: {e:#}"));
        }
    }

    /// Looks at the disk again: the game, the mod folder, the DLC, the plugins, the artwork.
    fn reload(&mut self) {
        let explicit = self.store.game_dir.clone().map(PathBuf::from);
        self.game = Game::open(explicit.as_deref()).map_err(|e| format!("{e:#}"));
        match &self.game {
            Ok(g) => {
                self.mods = mods::scan(&g.data_dir);
                self.dlcs = dlc::scan(&g.dir);
                self.dlc_current = dlcload::disabled_dlcs_of(&dlcload::read(&g.dlc_load_path()).map(|x| x.1).unwrap_or_default());
                self.backgrounds = artwork::backgrounds(&g.settings.game_id, STEAM_APP_ID).into_iter().map(|s| (s.id, s.path)).collect();
                self.logo = artwork::logo(&g.settings.game_id, STEAM_APP_ID);
            }
            Err(_) => {
                self.mods.clear();
                self.dlcs.clear();
                self.backgrounds.clear();
                self.logo = None;
            }
        }
        match plugins::list() {
            Ok((p, problems)) => {
                self.plugins = p;
                self.plugin_problems = problems;
            }
            Err(e) => self.plugin_problems = vec![format!("{e:#}")],
        }
    }

    fn background_path(&self) -> Option<PathBuf> {
        match self.store.background.as_deref().unwrap_or("auto") {
            "none" => None,
            "auto" => self.backgrounds.first().map(|b| b.1.clone()),
            want => self.backgrounds.iter().find(|b| b.0 == want).map(|b| b.1.clone()).or_else(|| self.backgrounds.first().map(|b| b.1.clone())),
        }
    }

    fn load_news_local(&mut self) {
        let code = self.lang.code();
        let mut cards = news::load_cached(code);
        if cards.is_empty() {
            if let Ok(g) = &self.game {
                cards = news::load_official_cache(&g.data_dir, code);
            }
        }
        self.news.cards = cards;
        self.news.hero = 0;
    }

    fn refresh_news(&mut self) {
        let Ok(g) = &self.game else { return };
        if self.news.rx.is_some() {
            return;
        }
        let (game_id, platform, code) = (g.settings.game_id.clone(), g.settings.dist_platform.clone(), self.lang.code());
        let (tx, rx) = channel();
        self.news.rx = Some(rx);
        self.news.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(news::refresh(&game_id, &platform, code).map_err(|e| format!("{e:#}")));
        });
    }

    fn drain_news(&mut self) {
        if let Some(rx) = &self.news.rx {
            if let Ok(result) = rx.try_recv() {
                self.news.rx = None;
                match result {
                    Ok(cards) if !cards.is_empty() => {
                        self.news.cards = cards;
                        self.news.hero = 0;
                    }
                    Ok(_) => {}
                    Err(e) => self.news.error = Some(e),
                }
            }
        }
    }

    fn poll_process(&mut self) {
        if self.last_poll.elapsed() > Duration::from_secs(1) {
            self.running = process::find_processes("stellaris.exe");
            self.last_poll = Instant::now();
        }
    }

    fn start(&mut self) {
        let Ok(game) = self.game.clone() else { return };
        let store = self.store.clone();
        let opts = launch::Options { use_plugins: self.use_plugins, continue_last: self.continue_last, alternative: self.alternative, ..Default::default() };
        let (tx, rx) = channel();
        self.launching = Some(rx);
        self.log.clear();
        std::thread::spawn(move || {
            let tx_line = tx.clone();
            let result = launch::launch(&game, &store, &opts, &mut |line| {
                let _ = tx_line.send(Msg::Line(line.to_string()));
            });
            let _ = tx.send(Msg::Done(result.map_err(|e| format!("{e:#}"))));
        });
    }

    fn drain_launch(&mut self) {
        let mut done = false;
        let mut lines = Vec::new();
        if let Some(rx) = &self.launching {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Line(l) => lines.push(l),
                    Msg::Done(Ok(r)) => {
                        lines.push(if r.failed_plugins.is_empty() { "the game is running".to_string() } else { format!("the game is running; {} plugin(s) failed", r.failed_plugins.len()) });
                        done = true;
                    }
                    Msg::Done(Err(e)) => {
                        lines.push(format!("could not start: {e}"));
                        done = true;
                    }
                }
            }
        }
        for l in lines {
            self.say(l);
        }
        if done {
            self.launching = None;
            self.last_poll = Instant::now() - Duration::from_secs(10);
        }
    }

    fn import_official(&mut self) {
        let Ok(game) = self.game.clone() else { return };
        let Some(db) = official::find_database(&game.data_dir) else {
            self.say("no launcher database found: the Paradox Launcher has not been used here");
            return;
        };
        match official::read_playsets(&db) {
            Ok(sets) => match import::import_official(&mut self.store, &self.mods, &sets, None, false) {
                Ok(done) => {
                    for o in done {
                        if o.skipped {
                            self.say(format!("skipped {} (already here)", o.name));
                        } else {
                            self.say(format!("imported {}: {} mods, {} enabled, {} not in the mod folder", o.name, o.mods, o.enabled, o.missing));
                        }
                    }
                    self.save();
                }
                Err(e) => self.say(format!("{e:#}")),
            },
            Err(e) => self.say(format!("{e:#}")),
        }
    }

    fn apply(&mut self, ctx: &egui::Context, act: Act) {
        let active = self.store.active_index();
        match act {
            Act::SetActive(i) => {
                if i < self.store.playsets.len() {
                    self.store.set_active(i);
                    self.confirm_delete = false;
                    self.save();
                }
            }
            Act::AddPlayset(name) => match self.store.add_playset(name.trim()) {
                Ok(i) => {
                    self.store.set_active(i);
                    self.new_name.clear();
                    self.save();
                }
                Err(e) => self.say(format!("{e:#}")),
            },
            Act::DeletePlayset => {
                let n = self.store.playsets[active].name.clone();
                if self.store.remove_playset(active).is_ok() {
                    self.save();
                    self.say(format!("deleted the playset {n}"));
                }
                self.confirm_delete = false;
            }
            Act::Import => {
                self.import_official();
            }
            Act::ModFlag(i, on) => {
                if let Some(m) = self.store.playsets[active].mods.get_mut(i) {
                    m.enabled = on;
                    self.save();
                }
            }
            Act::ModRemove(i) => {
                if i < self.store.playsets[active].mods.len() {
                    self.store.playsets[active].mods.remove(i);
                    self.save();
                }
            }
            Act::ModMove(i, by) => {
                let n = self.store.playsets[active].mods.len() as i32;
                let to = i as i32 + by;
                if i < n as usize && (0..n).contains(&to) {
                    self.store.playsets[active].mods.swap(i, to as usize);
                    self.save();
                }
            }
            Act::ModAdd(id) => {
                self.store.playsets[active].set_mod(&id, true);
                self.save();
            }
            Act::ModDrop(id) => {
                self.store.playsets[active].remove_mod(&id);
                self.save();
            }
            Act::DlcFlag(id, on) => {
                let current = self.dlc_current.clone();
                self.store.playsets[active].set_dlc_enabled(&id, on, &current);
                self.save();
            }
            Act::PluginFlag(id, on) => {
                self.store.playsets[active].set_plugin(&id, on);
                self.save();
            }
            Act::PluginRemove(id) => match plugins::remove(&id) {
                Ok(()) => {
                    self.say(format!("removed {id}"));
                    self.reload();
                }
                Err(e) => self.say(format!("{e:#}")),
            },
            Act::PluginInstall(link) => {
                if let Some(dir) = rfd::FileDialog::new().set_title("stl-plugin.json").pick_folder() {
                    match plugins::install(&dir, link) {
                        Ok(p) => {
                            self.say(format!("{} {}", if link { "linked" } else { "installed" }, p.manifest.id));
                            self.reload();
                        }
                        Err(e) => self.say(format!("{e:#}")),
                    }
                }
            }
            Act::Start => self.start(),
            Act::CloseGame => {
                for pid in self.running.clone() {
                    if let Err(e) = process::terminate(pid) {
                        self.say(format!("{e:#}"));
                    }
                }
                self.last_poll = Instant::now() - Duration::from_secs(10);
            }
            Act::RefreshNews => self.refresh_news(),
            Act::Reload => {
                self.reload();
                self.load_news_local();
            }
            Act::SetLang(l) => {
                self.store.language = l.map(|l| l.code().to_string());
                self.lang = resolve_lang(&self.store, &self.game);
                self.save();
                theme::install_fonts(ctx, self.lang);
                self.load_news_local();
            }
            Act::SetBackground(b) => {
                self.store.background = Some(b.to_string());
                self.save();
            }
            Act::SetNewsOnline(on) => {
                self.store.news_online = Some(on);
                self.save();
                if on {
                    self.refresh_news();
                }
            }
            Act::ChangeGameDir => {
                if let Some(d) = rfd::FileDialog::new().set_title(tr(self.lang, "common.folder_hint")).pick_folder() {
                    self.store.game_dir = Some(d.to_string_lossy().to_string());
                    self.save();
                    self.reload();
                    self.load_news_local();
                }
            }
            Act::UseGameDir(d) => {
                self.store.game_dir = Some(d);
                self.save();
                self.reload();
                self.lang = resolve_lang(&self.store, &self.game);
                self.load_news_local();
            }
            Act::Open(url) => open_link(&url),
        }
    }

    fn kind_chip(lang: Lang, kind: Kind) -> (&'static str, Color32) {
        match kind {
            Kind::Workshop => (tr(lang, "kind.steam"), BLUE),
            Kind::ParadoxMods => (tr(lang, "kind.pdx"), PURPLE),
            Kind::Local => (tr(lang, "kind.local"), GREEN),
        }
    }

    // ---------------------------------------------------------------- Play
    fn page_play(&mut self, ui: &mut Ui) {
        let (subtitle, can_play) = match &self.game {
            Ok(g) => (format!("{} · {}", g.settings.version, pe::describe(g.exe_timestamp).split(' ').next().unwrap_or("")), true),
            Err(_) => (String::new(), false),
        };
        let _ = can_play;
        large_title(ui, "Stellaris", Some(&subtitle), |_| {});
        let avail = ui.available_rect_before_wrap();
        let right_w = 320.0f32.min(avail.width() * 0.42);
        let gap = 22.0;
        let left_rect = Rect::from_min_max(avail.min, pos2(avail.right() - right_w - gap, avail.bottom()));
        let right_rect = Rect::from_min_max(pos2(avail.right() - right_w, avail.top()), avail.max);
        let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect));
        self.news_column(&mut left);
        let mut right = ui.new_child(UiBuilder::new().max_rect(right_rect));
        self.play_panel(&mut right);
        ui.advance_cursor_after_rect(avail);
    }

    fn news_column(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let now = ui.input(|i| i.time);
        let acts = Acts::default();
        let loading = self.news.rx.is_some();
        theme::large_title_small(ui, tr(lang, "play.news"), |ui| {
            if circle_button(ui, Icon::Refresh, theme::white(30), LABEL, !loading).clicked() {
                acts.push(Act::RefreshNews);
            }
        });
        let width = ui.available_width();
        let cards = self.news.cards.clone();
        if cards.is_empty() {
            glass(ui, 20.0, 24.0, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new(if loading { "…" } else { tr(lang, "play.news_empty") }).size(16.0).color(SECONDARY));
                    if let Some(e) = &self.news.error {
                        ui.label(RichText::new(e).size(12.0).color(SECONDARY));
                    }
                    ui.add_space(30.0);
                });
            });
        } else {
            let main: Vec<usize> = cards.iter().enumerate().filter(|(_, c)| c.slot == "main").map(|(i, _)| i).collect();
            let main = if main.is_empty() { vec![0] } else { main };
            let others: Vec<usize> = (0..cards.len()).filter(|i| !main.contains(i)).collect();
            if main.len() > 1 && now - self.news.switched > 8.0 {
                self.news.hero = (self.news.hero + 1) % main.len();
                self.news.switched = now;
            }
            let hero_i = main[self.news.hero.min(main.len() - 1)];
            // every card at the size of its picture (one pixel a point, as the official launcher shows them), left to right and then down;
            // only a card wider than the column is scaled down
            let order: Vec<usize> = std::iter::once(hero_i).chain(others.iter().copied()).collect();
            let gap = 12.0;
            let sizes: Vec<Vec2> = order
                .iter()
                .map(|&i| {
                    let s = cards[i].image.as_ref().and_then(|p| self.assets.image(p, 1400)).map(|t| t.size).unwrap_or(vec2(246.0, 230.0));
                    if s.x > width { s * (width / s.x) } else { s }
                })
                .collect();
            let mut offsets = Vec::new();
            let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
            for s in &sizes {
                if x > 0.0 && x + s.x > width + 0.5 {
                    x = 0.0;
                    y += row_h + gap;
                    row_h = 0.0;
                }
                offsets.push(vec2(x, y));
                x += s.x + gap;
                row_h = row_h.max(s.y);
            }
            let total_h = y + row_h;
            egui::ScrollArea::vertical().id_salt("news").auto_shrink([false, false]).show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(vec2(width, total_h), Sense::hover());
                for (k, &i) in order.iter().enumerate() {
                    let r = Rect::from_min_size(area.min + offsets[k], sizes[k]);
                    self.draw_card(ui, &cards[i], r, 16.0, &format!("card{k}"), &acts);
                    if k == 0 && main.len() > 1 {
                        let total = main.len() as f32 * 14.0;
                        for d in 0..main.len() {
                            let c = pos2(r.center().x - total / 2.0 + 7.0 + d as f32 * 14.0, r.bottom() - 14.0);
                            let hit = Rect::from_center_size(c, Vec2::splat(14.0));
                            if ui.interact(hit, egui::Id::new(("dot", d)), Sense::click()).clicked() {
                                self.news.hero = d;
                                self.news.switched = now;
                            }
                            ui.painter().circle_filled(c, 3.5, if d == self.news.hero { Color32::WHITE } else { theme::white(110) });
                        }
                        ui.ctx().request_repaint_after(Duration::from_secs(1));
                    }
                }
            });
        }
        if self.news.rx.is_some() || self.assets.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
        self.acts.extend(acts.take());
    }

    fn draw_card(&mut self, ui: &mut Ui, card: &Card, rect: Rect, radius: f32, id: &str, acts: &Acts) {
        let resp = ui.interact(rect, egui::Id::new(("news", id)), Sense::click());
        let now = ui.input(|i| i.time);
        match card.image.as_ref().and_then(|p| self.assets.image(p, 1400)) {
            Some(tex) => {
                theme::cover_image(ui, rect, tex.at(now).id(), tex.size, radius, Color32::WHITE);
                if tex.animated() {
                    ui.ctx().request_repaint_after(Duration::from_millis(40));
                }
            }
            None => {
                ui.painter().add(theme::glass_shapes(ui.ctx(), rect, radius));
            }
        }
        if card.link.is_some() && resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            ui.painter().add(Shape::rect_stroke(rect, egui::CornerRadius::same(radius as u8), egui::Stroke::new(2.0, Color32::WHITE.gamma_multiply(0.7)), egui::StrokeKind::Inside));
        }
        if resp.clicked() {
            if let Some(l) = &card.link {
                acts.push(Act::Open(l.clone()));
            }
        }
    }

    fn play_panel(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let rect = ui.max_rect();
        let active = self.store.active_index();
        let acts = Acts::default();
        let launching = self.launching.is_some();
        let running = !self.running.is_empty();
        let game_ok = self.game.is_ok();
        let alt_labels: Vec<String> = match &self.game {
            Ok(g) if !g.settings.alternative_executables.is_empty() => {
                let mut v = vec![tr(lang, "play.standard").to_string()];
                v.extend(g.settings.alternative_executables.iter().enumerate().map(|(i, a)| a.label.get("en").cloned().unwrap_or_else(|| format!("#{}", i + 1))));
                v
            }
            _ => Vec::new(),
        };
        let logo_path = self.logo.clone();
        glass(ui, 22.0, 18.0, |ui| {
            ui.set_min_height(rect.height() - 36.0);
            ui.vertical_centered(|ui| {
                let logo = logo_path.as_ref().and_then(|p| self.assets.image(p, 700));
                match logo {
                    Some(t) => {
                        let h = 54.0;
                        let (r, _) = ui.allocate_exact_size(vec2(h * t.size.x / t.size.y, h), Sense::hover());
                        ui.painter().image(t.handle.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.label(RichText::new("Stellaris").size(32.0).family(bold()).color(LABEL));
                    }
                }
            });
            theme::section(ui, tr(lang, "play.playset"));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for (i, p) in self.store.playsets.iter().enumerate() {
                    if theme::choice_pill(ui, &p.name, i == active).clicked() {
                        acts.push(Act::SetActive(i));
                    }
                }
            });
            let p = &self.store.playsets[active];
            let mods_on = p.mods.iter().filter(|m| m.enabled).count().to_string();
            let plugins_on = p.plugins.iter().filter(|x| x.enabled).count().to_string();
            ui.add_space(4.0);
            ui.label(RichText::new(tr_args(lang, "play.summary", &[&mods_on, &plugins_on])).size(13.0).color(SECONDARY));
            ui.add_space(12.0);
            egui::Frame::new().fill(Color32::from_black_alpha(55)).corner_radius(14).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut rows = Rows::new();
                rows.row(ui, 46.0, 46.0, false, |ui| { ui.label(tr(lang, "play.continue")); }, |ui| { switch(ui, &mut self.continue_last); });
                rows.row(ui, 46.0, 46.0, false, |ui| { ui.label(tr(lang, "play.plugins")); }, |ui| { switch(ui, &mut self.use_plugins); });
                if !alt_labels.is_empty() {
                    let cur = self.alternative.map_or(0, |i| i + 1);
                    let mut pick = None;
                    for (i, l) in alt_labels.iter().enumerate() {
                        let r = rows.row(ui, 44.0, 24.0, true, |ui| {
                            ui.add(egui::Label::new(RichText::new(l).color(if i == cur { LABEL } else { SECONDARY })).truncate());
                        }, |ui| {
                            if i == cur {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                                Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                            }
                        });
                        if r.clicked() {
                            pick = Some(i);
                        }
                    }
                    if let Some(i) = pick {
                        self.alternative = if i == 0 { None } else { Some(i - 1) };
                    }
                }
            });
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                let (text, enabled) = if launching { (tr(lang, "play.starting"), false) } else if running { (tr(lang, "play.running"), false) } else { (tr(lang, "play.button"), game_ok) };
                if theme::capsule_button(ui, text, vec2(ui.available_width(), 52.0), ButtonStyle::Filled(BLUE), enabled).clicked() {
                    acts.push(Act::Start);
                }
                ui.add_space(6.0);
                if running {
                    if pill_button(ui, tr(lang, "play.close"), ButtonStyle::Plain(RED), true).clicked() {
                        acts.push(Act::CloseGame);
                    }
                    let pid = self.running[0].to_string();
                    ui.label(RichText::new(format!("●  {}", tr_args(lang, "play.status_running", &[&pid]))).size(13.0).color(GREEN));
                } else if let Some(last) = self.log.last() {
                    let bad = last.contains("could not") || last.contains("failed");
                    ui.add(egui::Label::new(RichText::new(last).size(12.0).color(if bad { RED } else { SECONDARY })).truncate());
                }
            });
        });
        self.acts.extend(acts.take());
    }

    // ---------------------------------------------------------------- Playsets
    fn page_playsets(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let acts = Acts::default();
        large_title(ui, tr(lang, "ps.title"), None, |ui| {
            if pill_button(ui, tr(lang, "ps.import"), ButtonStyle::Tinted(BLUE), self.game.is_ok()).clicked() {
                acts.push(Act::Import);
            }
        });
        let avail = ui.available_rect_before_wrap();
        let left_w = 290.0f32.min(avail.width() * 0.32);
        let left_rect = Rect::from_min_max(avail.min, pos2(avail.left() + left_w, avail.bottom()));
        let right_rect = Rect::from_min_max(pos2(avail.left() + left_w + 20.0, avail.top()), avail.max);
        let active = self.store.active_index();

        // the list of playsets, and the field for a new one under it
        let list_rect = Rect::from_min_max(left_rect.min, pos2(left_rect.right(), left_rect.bottom() - 52.0));
        let mut left = ui.new_child(UiBuilder::new().max_rect(list_rect));
        glass_scroll(&mut left, "ps-list", |ui| {
            let mut rows = Rows::new();
            for (i, p) in self.store.playsets.iter().enumerate() {
                if i == active {
                    rows.highlight_next();
                }
                let mods_on = p.mods.iter().filter(|m| m.enabled).count().to_string();
                let plugins_on = p.plugins.iter().filter(|x| x.enabled).count().to_string();
                let r = rows.row(ui, 58.0, 24.0, true, |ui| {
                    stack(ui, 58.0, 38.0, |ui| {
                        ui.add(egui::Label::new(RichText::new(&p.name).size(15.5).family(bold())).truncate());
                        ui.label(RichText::new(tr_args(lang, "ps.counts", &[&mods_on, &plugins_on])).size(12.0).color(SECONDARY));
                    });
                }, |ui| {
                    if i == active {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                        Icon::Check.draw(ui.painter(), r.center(), 18.0, LABEL, 2.0);
                    }
                });
                if r.clicked() {
                    acts.push(Act::SetActive(i));
                }
            }
        });
        let add_rect = Rect::from_min_max(pos2(left_rect.left(), left_rect.bottom() - 40.0), left_rect.max);
        let mut add_ui = ui.new_child(UiBuilder::new().max_rect(add_rect).layout(Layout::left_to_right(Align::Center)));
        let add_w = add_ui.painter().layout_no_wrap(tr(lang, "ps.add").to_owned(), egui::FontId::new(14.5, bold()), Color32::WHITE).size().x + 28.0;
        theme::text_field(&mut add_ui, &mut self.new_name, tr(lang, "ps.new"), left_w - add_w - 10.0);
        let ok = !self.new_name.trim().is_empty();
        if pill_button(&mut add_ui, tr(lang, "ps.add"), ButtonStyle::Filled(BLUE), ok).clicked() {
            acts.push(Act::AddPlayset(self.new_name.clone()));
        }

        // the chosen playset
        glass_pane(ui, right_rect, 20.0, 18.0, |pane| self.playset_detail(pane, &acts));
        ui.advance_cursor_after_rect(avail);
        self.acts.extend(acts.take());
    }

    fn playset_detail(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let active = self.store.active_index();
        let p = &self.store.playsets[active];
        let name = p.name.clone();
        let mods_on = p.mods.iter().filter(|m| m.enabled).count().to_string();
        let plugins_on = p.plugins.iter().filter(|x| x.enabled).count().to_string();
        let can_delete = self.store.playsets.len() > 1;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(RichText::new(&name).size(24.0).family(bold())).truncate());
                ui.label(RichText::new(tr_args(lang, "ps.counts", &[&mods_on, &plugins_on])).size(13.0).color(SECONDARY));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if can_delete {
                    if self.confirm_delete {
                        if pill_button(ui, tr(lang, "ps.delete_sure"), ButtonStyle::Filled(RED), true).clicked() {
                            acts.push(Act::DeletePlayset);
                        }
                        if pill_button(ui, tr(lang, "ps.keep"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                            self.confirm_delete = false;
                        }
                    } else if pill_button(ui, tr(lang, "ps.delete"), ButtonStyle::Plain(RED), true).clicked() {
                        self.confirm_delete = true;
                    }
                }
            });
        });
        ui.add_space(10.0);
        let labels = [tr(lang, "seg.mods").to_string(), tr(lang, "seg.dlc").to_string(), tr(lang, "seg.plugins").to_string()];
        let width = ui.available_width();
        if let Some(i) = segmented(ui, &labels, self.seg, width.min(380.0)) {
            self.seg = i;
        }
        ui.add_space(10.0);
        match self.seg {
            0 => self.playset_mods(ui, acts),
            1 => self.playset_dlc(ui, acts),
            _ => self.playset_plugins(ui, acts),
        }
    }

    fn mod_sub_line(&self, ui: &mut Ui, info: Option<&Mod>, problem: Option<&str>, version: &str) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(x) = info {
                let (t, c) = Self::kind_chip(lang, x.kind);
                chip(ui, t, c);
                if let Some(sv) = x.supported_version.as_ref().filter(|sv| !mods::supports(sv, version)) {
                    chip(ui, &tr_args(lang, "mods.for_version", &[sv]), ORANGE).on_hover_text(tr(lang, "mods.mismatch"));
                }
            }
            if let Some(p) = problem {
                chip(ui, tr(lang, "mods.left_out"), RED).on_hover_text(p);
            }
        });
    }

    fn playset_mods(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let active = self.store.active_index();
        let version = self.game.as_ref().ok().map(|g| g.version().to_string()).unwrap_or_default();
        let total = self.store.playsets[active].mods.len();
        if total == 0 {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr(lang, "ps.empty")).size(16.0).color(LABEL));
                ui.label(RichText::new(tr(lang, "ps.empty_hint")).size(13.0).color(SECONDARY));
            });
            return;
        }
        ui.label(RichText::new(tr(lang, "ps.order_hint")).size(12.5).color(SECONDARY));
        ui.add_space(6.0);
        plain_rows(ui, "ps-mods", 58.0, total, |ui, range| {
            let mut rows = Rows::starting_at(range.start);
            for i in range {
                let m = &self.store.playsets[active].mods[i];
                let info = self.mods.iter().find(|x| x.id == m.id);
                let name = info.map(|x| x.name.clone()).unwrap_or_else(|| m.id.clone());
                let problem = match info {
                    Some(x) => x.problem.clone(),
                    None => Some(tr(lang, "mods.unusable").to_string()),
                };
                let enabled = m.enabled;
                rows.row(ui, 58.0, 108.0, false, |ui| {
                    let mut on = enabled;
                    if switch(ui, &mut on).changed() {
                        acts.push(Act::ModFlag(i, on));
                    }
                    ui.label(RichText::new(format!("{:>3}", i + 1)).size(12.0).color(theme::TERTIARY).monospace());
                    stack(ui, 58.0, 42.0, |ui| {
                        let color = if problem.is_some() { RED } else if enabled { LABEL } else { SECONDARY };
                        ui.add(egui::Label::new(RichText::new(&name).size(15.0).color(color)).truncate());
                        self.mod_sub_line(ui, info, problem.as_deref(), &version);
                    });
                }, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if circle_button(ui, Icon::Close, theme::white(24), SECONDARY, true).on_hover_text(tr(lang, "mods.remove")).clicked() {
                        acts.push(Act::ModRemove(i));
                    }
                    if circle_button(ui, Icon::Down, theme::white(24), LABEL, i + 1 < total).clicked() {
                        acts.push(Act::ModMove(i, 1));
                    }
                    if circle_button(ui, Icon::Up, theme::white(24), LABEL, i > 0).clicked() {
                        acts.push(Act::ModMove(i, -1));
                    }
                });
            }
        });
    }

    fn playset_dlc(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let active = self.store.active_index();
        let own = self.store.playsets[active].disabled_dlcs.is_some();
        let off: Vec<String> = self.store.playsets[active].disabled_dlcs_or(&self.dlc_current).to_vec();
        let count = self.dlcs.len().to_string();
        ui.label(RichText::new(format!("{}  ·  {}", tr(lang, "dlc.hint"), tr_args(lang, "dlc.count", &[&count]))).size(12.5).color(SECONDARY));
        if !own {
            ui.label(RichText::new(tr(lang, "dlc.inherit")).size(12.5).color(ORANGE));
        }
        ui.add_space(6.0);
        let total = self.dlcs.len();
        plain_rows(ui, "ps-dlc", 62.0, total, |ui, range| {
            let mut rows = Rows::starting_at(range.start);
            for i in range {
                let d = &self.dlcs[i];
                let on = !off.contains(&d.id);
                let cat_key = match d.category.as_str() {
                    "expansion" => "cat.expansion",
                    "story_pack" => "cat.story_pack",
                    "species_pack" => "cat.species_pack",
                    "content_pack" => "cat.content_pack",
                    "cosmetic_pack" => "cat.cosmetic_pack",
                    "music" => "cat.music",
                    _ => "cat.other",
                };
                rows.row(ui, 62.0, 46.0, false, |ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    match d.thumbnail.as_ref().and_then(|p| self.assets.image(p, 160)) {
                        Some(t) => theme::cover_image(ui, r, t.handle.id(), t.size, 9.0, if on { Color32::WHITE } else { Color32::from_gray(110) }),
                        None => {
                            ui.painter().rect_filled(r, egui::CornerRadius::same(9), theme::white(24));
                        }
                    }
                    stack(ui, 62.0, 42.0, |ui| {
                        ui.add(egui::Label::new(RichText::new(&d.name).size(15.0).color(if on { LABEL } else { SECONDARY })).truncate());
                        chip(ui, tr(lang, cat_key), if d.category == "expansion" { BLUE } else { SECONDARY });
                    });
                }, |ui| {
                    let mut v = on;
                    if switch(ui, &mut v).changed() {
                        acts.push(Act::DlcFlag(d.id.clone(), v));
                    }
                });
            }
        });
    }

    fn playset_plugins(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let active = self.store.active_index();
        if self.plugins.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr(lang, "pl.empty")).size(16.0).color(LABEL));
                ui.label(RichText::new(tr(lang, "pl.empty_hint")).size(13.0).color(SECONDARY));
            });
            return;
        }
        let game = self.game.clone().ok();
        {
            let mut rows = Rows::new();
            for p in &self.plugins {
                let on = self.store.playsets[active].plugins.iter().find(|x| x.id == p.manifest.id).map(|x| x.enabled).unwrap_or(false);
                let (status, color) = plugin_status(lang, game.as_ref(), p);
                rows.row(ui, 64.0, 46.0, false, |ui| {
                    stack(ui, 64.0, 43.0, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.manifest.name).size(15.5).family(bold()));
                            ui.label(RichText::new(&p.manifest.version).size(12.0).color(SECONDARY));
                        });
                        chip(ui, &status, color);
                    });
                }, |ui| {
                    let mut v = on;
                    if switch(ui, &mut v).changed() {
                        acts.push(Act::PluginFlag(p.manifest.id.clone(), v));
                    }
                });
            }
        }
    }

    // ---------------------------------------------------------------- Mods
    fn page_mods(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let acts = Acts::default();
        let count = self.mods.len().to_string();
        large_title(ui, tr(lang, "mods.title"), Some(&tr_args(lang, "mods.count", &[&count])), |ui| {
            theme::search_field(ui, &mut self.filter, tr(lang, "mods.search"), 280.0);
        });
        let active = self.store.active_index();
        let version = self.game.as_ref().ok().map(|g| g.version().to_string()).unwrap_or_default();
        let filter = self.filter.to_lowercase();
        let shown: Vec<usize> = (0..self.mods.len()).filter(|&i| filter.is_empty() || self.mods[i].name.to_lowercase().contains(&filter)).collect();
        glass_rows(ui, "mods-library", 58.0, shown.len(), |ui, range| {
            let mut rows = Rows::starting_at(range.start);
            for k in range {
                let m = &self.mods[shown[k]];
                let inside = self.store.playsets[active].mod_pos(&m.id).is_some();
                rows.row(ui, 58.0, 30.0, false, |ui| {
                    stack(ui, 58.0, 42.0, |ui| {
                        ui.add(egui::Label::new(RichText::new(&m.name).size(15.0).color(if m.problem.is_some() { RED } else { LABEL })).truncate());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (t, c) = Self::kind_chip(lang, m.kind);
                            chip(ui, t, c);
                            if let Some(sv) = m.supported_version.as_ref().filter(|sv| !mods::supports(sv, &version)) {
                                chip(ui, &tr_args(lang, "mods.for_version", &[sv]), ORANGE).on_hover_text(tr(lang, "mods.mismatch"));
                            }
                            if let Some(p) = &m.problem {
                                chip(ui, tr(lang, "mods.unusable"), RED).on_hover_text(p);
                            }
                        });
                    });
                }, |ui| {
                    if inside {
                        if circle_button(ui, Icon::Check, GREEN, Color32::WHITE, true).on_hover_text(tr(lang, "mods.remove")).clicked() {
                            acts.push(Act::ModDrop(m.id.clone()));
                        }
                    } else if circle_button(ui, Icon::Plus, BLUE.gamma_multiply(0.35), BLUE, true).on_hover_text(tr(lang, "mods.add")).clicked() {
                        acts.push(Act::ModAdd(m.id.clone()));
                    }
                });
            }
        });
        self.acts.extend(acts.take());
    }

    // ---------------------------------------------------------------- Plugins
    fn page_plugins(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let acts = Acts::default();
        large_title(ui, tr(lang, "pl.title"), Some(tr(lang, "pl.hint")), |ui| {
            if pill_button(ui, tr(lang, "pl.link"), ButtonStyle::Plain(BLUE), true).clicked() {
                acts.push(Act::PluginInstall(true));
            }
            if pill_button(ui, tr(lang, "pl.install"), ButtonStyle::Filled(BLUE), true).clicked() {
                acts.push(Act::PluginInstall(false));
            }
        });
        let game = self.game.clone().ok();
        let active = self.store.active_index();
        if self.plugins.is_empty() && self.plugin_problems.is_empty() {
            glass(ui, 20.0, 40.0, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(tr(lang, "pl.empty")).size(18.0).color(LABEL));
                    ui.label(RichText::new(tr(lang, "pl.empty_hint")).size(13.5).color(SECONDARY));
                });
            });
        }
        egui::ScrollArea::vertical().id_salt("plugins-page").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            for p in &self.plugins {
                let on = self.store.playsets[active].plugins.iter().find(|x| x.id == p.manifest.id).map(|x| x.enabled).unwrap_or(false);
                let (status, color) = plugin_status(lang, game.as_ref(), p);
                glass(ui, 18.0, 18.0, |ui| {
                    let w = ui.available_width();
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(vec2(w - 150.0, 0.0), Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 5.0;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&p.manifest.name).size(18.0).family(bold()));
                                ui.label(RichText::new(&p.manifest.version).size(13.0).color(SECONDARY));
                            });
                            if !p.manifest.description.is_empty() {
                                ui.label(RichText::new(&p.manifest.description).size(13.5).color(SECONDARY));
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                chip(ui, &status, color);
                                if p.linked {
                                    chip(ui, tr(lang, "pl.linked"), PURPLE);
                                }
                                ui.label(RichText::new(&p.manifest.id).size(11.5).color(theme::TERTIARY));
                            });
                        });
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                            let mut v = on;
                            if switch(ui, &mut v).changed() {
                                acts.push(Act::PluginFlag(p.manifest.id.clone(), v));
                            }
                            ui.add_space(4.0);
                            if pill_button(ui, tr(lang, "pl.remove"), ButtonStyle::Plain(RED), true).clicked() {
                                acts.push(Act::PluginRemove(p.manifest.id.clone()));
                            }
                        });
                    });
                });
            }
            for pr in &self.plugin_problems {
                ui.label(RichText::new(pr).size(12.5).color(RED));
            }
        });
        self.acts.extend(acts.take());
    }

    // ---------------------------------------------------------------- Settings
    fn page_settings(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let acts = Acts::default();
        large_title(ui, tr(lang, "set.title"), None, |ui| {
            if pill_button(ui, tr(lang, "set.reload"), ButtonStyle::Tinted(BLUE), true).clicked() {
                acts.push(Act::Reload);
            }
        });
        egui::ScrollArea::vertical().id_salt("settings").auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(820.0);
            ui.set_max_width(w);
            // language
            theme::section(ui, tr(lang, "set.language"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                let chosen = self.store.language.as_deref().and_then(Lang::from_code);
                let auto_row = rows.row(ui, 40.0, 24.0, true, |ui| { ui.label(tr(lang, "set.language_auto")); }, |ui| {
                    if chosen.is_none() {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                        Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                    }
                });
                if auto_row.clicked() {
                    acts.push(Act::SetLang(None));
                }
                for l in Lang::ALL {
                    let r = rows.row(ui, 40.0, 24.0, true, |ui| { ui.label(l.native_name()); }, |ui| {
                        if chosen == Some(l) {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                            Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                        }
                    });
                    if r.clicked() {
                        acts.push(Act::SetLang(Some(l)));
                    }
                }
            });
            // background
            theme::section(ui, tr(lang, "set.background"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                let cur = self.store.background.as_deref().unwrap_or("auto").to_string();
                for (id, key) in [("auto", "set.bg_auto"), ("launcher", "set.bg_launcher"), ("steam", "set.bg_steam"), ("none", "set.bg_none")] {
                    let present = id == "auto" || id == "none" || self.backgrounds.iter().any(|b| b.0 == id);
                    let r = rows.row(ui, 44.0, 24.0, present, |ui| {
                        ui.label(RichText::new(tr(lang, key)).color(if present { LABEL } else { theme::TERTIARY }));
                    }, |ui| {
                        if cur == id {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                            Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                        }
                    });
                    if r.clicked() && present {
                        acts.push(Act::SetBackground(match id {
                            "launcher" => "launcher",
                            "steam" => "steam",
                            "none" => "none",
                            _ => "auto",
                        }));
                    }
                }
            });
            // game
            theme::section(ui, tr(lang, "set.game"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                let (dir, version, build, data) = match &self.game {
                    Ok(g) => (g.dir.display().to_string(), g.settings.version.clone(), format!("{:#010X} · {}", g.exe_timestamp, pe::describe(g.exe_timestamp).split(' ').next().unwrap_or("")), g.data_dir.display().to_string()),
                    Err(e) => (e.clone(), String::new(), String::new(), String::new()),
                };
                rows.row(ui, 52.0, 100.0, false, |ui| {
                    stack(ui, 52.0, 38.0, |ui| {
                        ui.label(tr(lang, "set.game_folder"));
                        ui.add(egui::Label::new(RichText::new(&dir).size(12.0).color(SECONDARY)).truncate());
                    });
                }, |ui| {
                    if pill_button(ui, tr(lang, "set.change"), ButtonStyle::Tinted(BLUE), true).clicked() {
                        acts.push(Act::ChangeGameDir);
                    }
                });
                if !version.is_empty() {
                    rows.row(ui, 44.0, 220.0, false, |ui| { ui.label("Stellaris"); }, |ui| { ui.label(RichText::new(&version).color(SECONDARY)); });
                    rows.row(ui, 44.0, 220.0, false, |ui| { ui.label("Build"); }, |ui| { ui.label(RichText::new(&build).color(SECONDARY).monospace()); });
                    rows.row(ui, 52.0, 0.0, false, |ui| {
                        stack(ui, 52.0, 38.0, |ui| {
                            ui.label(tr(lang, "set.data"));
                            ui.add(egui::Label::new(RichText::new(&data).size(12.0).color(SECONDARY)).truncate());
                        });
                    }, |_| {});
                }
            });
            // account
            theme::section(ui, tr(lang, "set.account"));
            glass(ui, 16.0, 16.0, |ui| {
                ui.label(RichText::new(tr(lang, "set.account_note")).size(13.5).color(SECONDARY));
            });
            // news
            theme::section(ui, tr(lang, "set.news"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                rows.row(ui, 48.0, 46.0, false, |ui| { ui.label(tr(lang, "set.news_fetch")); }, |ui| {
                    let mut on = self.store.news_online != Some(false);
                    if switch(ui, &mut on).changed() {
                        acts.push(Act::SetNewsOnline(on));
                    }
                });
            });
            theme::footnote(ui, tr(lang, "set.news_note"));
            // about
            theme::section(ui, tr(lang, "set.about"));
            glass(ui, 16.0, 16.0, |ui| {
                ui.label(RichText::new(format!("Stellaris Launcher {}", env!("CARGO_PKG_VERSION"))).size(15.0).family(bold()));
                ui.label(RichText::new(tr(lang, "set.about_text")).size(13.5).color(SECONDARY));
            });
            // log
            theme::section(ui, tr(lang, "log.title"));
            glass(ui, 16.0, 14.0, |ui| {
                egui::ScrollArea::vertical().id_salt("log").max_height(220.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                    if self.log.is_empty() {
                        ui.label(RichText::new(tr(lang, "log.empty")).size(13.0).color(SECONDARY));
                    }
                    for l in &self.log {
                        let color = if l.contains("could not") || l.contains("failed") || l.contains("!!") {
                            RED
                        } else if l.contains("left out") || l.contains("warning") || l.contains("skipped") {
                            ORANGE
                        } else if l.contains("loaded") || l.contains("running") || l.contains("started") {
                            GREEN
                        } else {
                            SECONDARY
                        };
                        ui.label(RichText::new(l).monospace().color(color));
                    }
                });
            });
            ui.add_space(20.0);
        });
        self.acts.extend(acts.take());
    }

    /// Shown instead of the pages while the game cannot be found.
    fn page_missing(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let err = self.game.as_ref().err().cloned().unwrap_or_default();
        large_title(ui, tr(lang, "common.not_found"), Some(&err), |_| {});
        let acts = Acts::default();
        glass(ui, 18.0, 18.0, |ui| {
            ui.horizontal(|ui| {
                theme::text_field(ui, &mut self.game_dir_text, tr(lang, "common.folder_hint"), 460.0);
                if pill_button(ui, tr(lang, "common.browse"), ButtonStyle::Tinted(BLUE), true).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.game_dir_text = d.to_string_lossy().to_string();
                    }
                }
                if pill_button(ui, tr(lang, "common.use"), ButtonStyle::Filled(BLUE), !self.game_dir_text.trim().is_empty()).clicked() {
                    acts.push(Act::UseGameDir(self.game_dir_text.trim().to_string()));
                }
            });
        });
        self.acts.extend(acts.take());
    }
}

fn plugin_status(lang: Lang, game: Option<&Game>, p: &Plugin) -> (String, Color32) {
    match game.map(|g| p.compat(g)) {
        Some(Compat::Ok) => (tr(lang, "pl.build_ok").to_string(), GREEN),
        Some(Compat::Unchecked) | None => (tr(lang, "pl.build_unchecked").to_string(), SECONDARY),
        Some(Compat::Mismatch { declared, .. }) => (tr_args(lang, "pl.build_wrong", &[&declared.iter().map(|d| format!("{d:#010X}")).collect::<Vec<_>>().join(" / ")]), RED),
        Some(Compat::MissingDll(_)) => (tr(lang, "pl.dll_missing").to_string(), RED),
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.assets.poll();
        self.poll_process();
        self.drain_launch();
        self.drain_news();
        if self.launching.is_some() || !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.assets.set_background(self.background_path());
        self.paint_background(ctx);

        egui::TopBottomPanel::bottom("tabs").exact_height(66.0).frame(egui::Frame::NONE).show(ctx, |ui| {
            let items: Vec<(Icon, String)> = Page::ALL.iter().map(|p| (p.icon(), tr(self.lang, p.key()).to_string())).collect();
            let current = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0);
            if let Some(i) = theme::tab_bar(ui, &items, current) {
                self.page = Page::ALL[i];
            }
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 34, right: 34, top: 14, bottom: 14 })).show(ctx, |ui| {
            if self.game.is_err() && self.page != Page::Settings {
                self.page_missing(ui);
                return;
            }
            match self.page {
                Page::Play => self.page_play(ui),
                Page::Playsets => self.page_playsets(ui),
                Page::Mods => self.page_mods(ui),
                Page::Plugins => self.page_plugins(ui),
                Page::Settings => self.page_settings(ui),
            }
        });
        for act in std::mem::take(&mut self.acts) {
            self.apply(ctx, act);
        }
    }
}

impl App {
    /// The picture behind everything (dimmed), or a plain gradient while there is none; tells the glass cards which picture to blur.
    fn paint_background(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        let anchor = vec2(0.5, 0.35);
        let (top, bottom) = match &self.assets.background {
            Some((sharp, frost)) => {
                painter.image(sharp.handle.id(), screen, theme::cover_uv(screen.size(), sharp.size, anchor), Color32::WHITE);
                theme::set_glass(ctx, theme::GlassCtx { tex: Some(frost.handle.id()), window: screen, uv: theme::cover_uv(screen.size(), frost.size, anchor) });
                (Color32::from_black_alpha(95), Color32::from_black_alpha(205))
            }
            None => {
                theme::set_glass(ctx, theme::GlassCtx { tex: None, window: screen, uv: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) });
                (Color32::from_rgb(24, 28, 52), Color32::from_rgb(6, 7, 14))
            }
        };
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(screen.left_top(), top);
        mesh.colored_vertex(screen.right_top(), top);
        mesh.colored_vertex(screen.right_bottom(), bottom);
        mesh.colored_vertex(screen.left_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 780.0]).with_min_inner_size([980.0, 640.0]).with_title("Stellaris Launcher").with_icon(theme::icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Stellaris Launcher",
        options,
        Box::new(|cc| {
            theme::install_style(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}
