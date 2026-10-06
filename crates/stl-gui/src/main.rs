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
use stl_core::saves::{self, Save};
use stl_core::{modmake, workshop};
use stl_core::{artwork, dlcload, import, launch, official, pe, process};
use theme::{bold, chip, circle_button, glass, glass_pane, large_title, pill_button, plain_rows, segmented, stack, switch, ButtonStyle, Icon, Rows, BLUE, GREEN, LABEL, ORANGE, PURPLE, RED, SECONDARY};

const STEAM_APP_ID: u32 = 281990;

/// The "new mod" sheet.
struct MakeForm {
    name: String,
    version: String,
    tags: Vec<String>,
    add_to_playset: bool,
    error: Option<String>,
}

/// The settings editor of a plugin: its files in `config/`, one open at a time.
struct ConfigEditor {
    plugin: Plugin,
    files: Vec<PathBuf>,
    index: usize,
    text: String,
    saved: String,
    status: Option<(String, bool)>,
}

impl ConfigEditor {
    fn open(plugin: Plugin) -> ConfigEditor {
        let _ = plugin.ensure_config();
        let files = plugin.config_files();
        let mut e = ConfigEditor { plugin, files, index: 0, text: String::new(), saved: String::new(), status: None };
        e.load(0);
        e
    }

    fn load(&mut self, i: usize) {
        self.index = i;
        self.text = self.files.get(i).and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        self.saved = self.text.clone();
        self.status = None;
    }
}

enum UpMsg {
    Progress(workshop::Stage, u64, u64),
    Done(Result<workshop::Outcome, String>),
}

/// The "upload to the Workshop" sheet, and the upload once it runs.
struct UploadForm {
    m: Mod,
    /// 0 unchanged, 1 private, 2 friends, 3 unlisted, 4 public
    visibility: usize,
    note: String,
    /// a Workshop item to update, typed in for a mod whose descriptor does not name one
    item_id: String,
    rx: Option<Receiver<UpMsg>>,
    stage: Option<(workshop::Stage, u64, u64)>,
    result: Option<Result<workshop::Outcome, String>>,
}

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
    /// start the game; true = continue the last save
    Start(bool),
    SetUsePlugins(bool),
    SetAlternative(Option<usize>),
    CloseGame,
    RefreshNews,
    Reload,
    SetLang(Option<Lang>),
    SetBackground(&'static str),
    SetNewsOnline(bool),
    ChangeGameDir,
    UseGameDir(String),
    Open(String),
    CreateMod,
    RescanMods,
    RescanPlugins,
    EditConfig(String),
    SetModsSort(&'static str),
    SetModsView(&'static str),
    OpenUpload(String),
    StartUpload,
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
    /// the page of the strip shown, and when the wheel last turned it
    page: usize,
    wheel: f64,
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
    lang: Lang,
    page: Page,
    seg: usize,
    filter: String,
    new_name: String,
    confirm_delete: bool,
    /// the search in the playset drop-down (shown when there are many) and in the list of the Playsets page
    playset_filter: String,
    ps_filter: String,
    focus_playset_filter: bool,
    /// development: open the playset drop-down on the first frame
    dev_popup: bool,
    make: Option<MakeForm>,
    config: Option<ConfigEditor>,
    /// the "Upload mod" picker is open
    pick_upload: bool,
    /// when each mod was last changed (for the sort by date), filled when needed
    mod_times: std::collections::HashMap<String, Option<std::time::SystemTime>>,
    /// where each mod's cover is, looked up once
    mod_covers: std::collections::HashMap<String, Option<PathBuf>>,
    upload: Option<UploadForm>,
    /// the newest save, for the Continue button
    last_save: Option<Save>,
    /// page transition: the page on show, when it came, and from which side
    shown_page: Page,
    page_t0: f64,
    page_dir: f32,
    /// the same for the Mods | DLC | Plugins switch of a playset
    seg_shown: usize,
    seg_t0: f64,
    seg_dir: f32,
    /// when the pointer was last near the tab bar
    bar_active: f64,
    /// what was on show before the change, drawn fading out while the new one comes in
    prev_page: Option<Page>,
    prev_seg: Option<usize>,
    /// development: slow the transitions down, and switch to a page two seconds after the start
    dev_slow: f64,
    dev_then: Option<Page>,
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

/// "Cygnus v4.5.1 (358e)" -> ("Cygnus", "4.5.1", "358e"); a string of another shape comes back whole as the number.
fn version_parts(v: &str) -> (String, String, String) {
    let (mut name, mut number, mut build) = (Vec::new(), String::new(), String::new());
    for t in v.split_whitespace() {
        if number.is_empty() && t.len() > 1 && t.starts_with('v') && t[1..].starts_with(|c: char| c.is_ascii_digit()) {
            number = t[1..].to_string();
        } else if t.len() > 2 && t.starts_with('(') && t.ends_with(')') {
            build = t[1..t.len() - 1].to_string();
        } else if number.is_empty() {
            name.push(t);
        }
    }
    if number.is_empty() {
        return (String::new(), v.trim().to_string(), build);
    }
    (name.join(" "), number, build)
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
            lang: Lang::En,
            page: Page::Play,
            seg: 0,
            filter: String::new(),
            new_name: String::new(),
            confirm_delete: false,
            playset_filter: String::new(),
            ps_filter: String::new(),
            focus_playset_filter: false,
            dev_popup: false,
            make: None,
            config: None,
            pick_upload: false,
            mod_times: std::collections::HashMap::new(),
            mod_covers: std::collections::HashMap::new(),
            upload: None,
            last_save: None,
            shown_page: Page::Play,
            page_t0: -10.0,
            page_dir: 1.0,
            seg_shown: 0,
            seg_t0: -10.0,
            seg_dir: 1.0,
            bar_active: 0.0,
            prev_page: None,
            prev_seg: None,
            dev_slow: 1.0,
            dev_then: None,
            game_dir_text: String::new(),
            assets: Assets::new(ctx),
            news: News { cards: Vec::new(), rx: None, hero: 0, switched: 0.0, error: None, page: 0, wheel: 0.0 },
            acts: Vec::new(),
            logo: None,
            backgrounds: Vec::new(),
        };
        app.reload();
        app.lang = resolve_lang(&app.store, &app.game);
        // for screenshots while developing: --page=1 --seg=1 --lang=ja --popup --many=30 --make --upload=<part of a mod name>
        for a in std::env::args().skip(1) {
            if let Some(v) = a.strip_prefix("--page=") {
                app.page = Page::ALL.get(v.parse::<usize>().unwrap_or(0)).copied().unwrap_or(Page::Play);
            } else if let Some(v) = a.strip_prefix("--seg=") {
                app.seg = v.parse().unwrap_or(0);
            } else if let Some(v) = a.strip_prefix("--lang=") {
                app.lang = Lang::from_code(v).unwrap_or(Lang::En);
            } else if let Some(v) = a.strip_prefix("--slow=") {
                app.dev_slow = v.parse().unwrap_or(1.0);
            } else if let Some(v) = a.strip_prefix("--then=") {
                app.dev_then = Page::ALL.get(v.parse::<usize>().unwrap_or(0)).copied();
            } else if let Some(v) = a.strip_prefix("--config=") {
                app.acts.push(Act::EditConfig(v.to_string()));
            } else if a == "--pick" {
                app.pick_upload = true;
            } else if a == "--make" {
                app.make = Some(MakeForm { name: "My New Mod".into(), version: "1.0.0".into(), tags: vec!["Gameplay".into()], add_to_playset: true, error: None });
            } else if let Some(v) = a.strip_prefix("--upload=") {
                if let Some(m) = app.mods.iter().find(|m| m.name.contains(v)) {
                    app.acts.push(Act::OpenUpload(m.id.clone()));
                }
            } else if a == "--popup" {
                app.dev_popup = true;
            } else if let Some(v) = a.strip_prefix("--many=") {
                // this many extra playsets, in memory only (nothing is saved unless something is changed)
                for i in 1..=v.parse::<usize>().unwrap_or(0) {
                    let _ = app.store.add_playset(&format!("Test playset {i:02}"));
                }
            }
        }
        app.shown_page = app.page;
        app.seg_shown = app.seg;
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
                self.last_save = saves::latest(&g.data_dir);
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
        self.news.page = 0;
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
            let was_running = !self.running.is_empty();
            self.running = process::find_processes("stellaris.exe");
            self.last_poll = Instant::now();
            if was_running && self.running.is_empty() {
                // the game has just closed: it may have written a newer save
                if let Ok(g) = &self.game {
                    self.last_save = saves::latest(&g.data_dir);
                }
            }
        }
    }

    fn start(&mut self, continue_last: bool) {
        let Ok(game) = self.game.clone() else { return };
        let store = self.store.clone();
        let alternative = self.store.alternative.filter(|&i| game.settings.alternative_executables.len() > i);
        let opts = launch::Options { use_plugins: self.store.use_plugins != Some(false), continue_last, alternative, ..Default::default() };
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

    fn start_upload(&mut self) {
        let (Ok(game), Some(f)) = (self.game.clone(), self.upload.as_mut()) else { return };
        if f.rx.is_some() {
            return;
        }
        let Some(content) = f.m.path.clone() else {
            f.result = Some(Err("the mod has no content folder (path=)".into()));
            return;
        };
        let visibility = match f.visibility {
            1 => Some(workshop::Visibility::Private),
            2 => Some(workshop::Visibility::FriendsOnly),
            3 => Some(workshop::Visibility::Unlisted),
            4 => Some(workshop::Visibility::Public),
            _ => None,
        };
        let up = workshop::Upload {
            title: f.m.name.clone(),
            description: String::new(),
            preview: mods::own_thumbnail(&f.m),
            content,
            tags: f.m.tags.clone(),
            visibility,
            change_note: f.note.trim().to_string(),
            existing: f.m.remote_file_id.as_deref().and_then(|v| v.parse().ok()),
        };
        let mut up = up;
        let typed = f.item_id.trim().to_string();
        let mut record_typed = None;
        if up.existing.is_none() && !typed.is_empty() {
            match typed.parse::<u64>() {
                Ok(id) => {
                    up.existing = Some(id);
                    record_typed = Some(id);
                }
                Err(_) => {
                    f.result = Some(Err(tr(self.lang, "up.bad_id").to_string()));
                    return;
                }
            }
        }
        let m = f.m.clone();
        let (tx, rx) = channel();
        f.rx = Some(rx);
        f.result = None;
        std::thread::spawn(move || {
            let tx_p = tx.clone();
            let r = workshop::upload(&game.dir, &up, &mut |id| modmake::set_remote_file_id(&m, id), &mut |stage, done, total| {
                let _ = tx_p.send(UpMsg::Progress(stage, done, total));
            });
            let r = match (r, record_typed) {
                (Ok(o), Some(id)) => modmake::set_remote_file_id(&m, id).map(|_| o),
                (r, _) => r,
            };
            let _ = tx.send(UpMsg::Done(r.map_err(|e| format!("{e:#}"))));
        });
    }

    fn drain_upload(&mut self) {
        let mut finished = None;
        if let Some(f) = self.upload.as_mut() {
            if let Some(rx) = &f.rx {
                while let Ok(m) = rx.try_recv() {
                    match m {
                        UpMsg::Progress(stage, done, total) => f.stage = Some((stage, done, total)),
                        UpMsg::Done(r) => finished = Some(r),
                    }
                }
            }
            if let Some(r) = finished.take() {
                f.rx = None;
                let line = match &r {
                    Ok(o) => format!("uploaded {} to {}", f.m.name, workshop::item_url(o.id)),
                    Err(e) => format!("could not upload {}: {e}", f.m.name),
                };
                f.result = Some(r);
                self.log.push(line);
                // the descriptors may carry the new item's id now
                if let Ok(g) = &self.game {
                    self.mods = mods::scan(&g.data_dir);
                }
            }
        }
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
            Act::Start(continue_last) => self.start(continue_last),
            Act::SetUsePlugins(on) => {
                self.store.use_plugins = Some(on);
                self.save();
            }
            Act::SetAlternative(i) => {
                self.store.alternative = i;
                self.save();
            }
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
            Act::CreateMod => {
                let (Ok(g), Some(f)) = (&self.game, &mut self.make) else { return };
                let new = modmake::NewMod {
                    name: f.name.trim().to_string(),
                    version: if f.version.trim().is_empty() { "1.0.0".into() } else { f.version.trim().to_string() },
                    supported_version: format!("v{}.*", g.settings.mods_compatibility_version),
                    tags: f.tags.clone(),
                };
                match modmake::create(&g.data_dir, &new) {
                    Ok(m) => {
                        let add = f.add_to_playset;
                        self.make = None;
                        self.say(format!("made the mod {} ({})", m.name, m.id));
                        if add {
                            let active = self.store.active_index();
                            self.store.playsets[active].set_mod(&m.id, true);
                            self.save();
                        }
                        if let Ok(g) = &self.game {
                            self.mods = mods::scan(&g.data_dir);
                        }
                        self.filter = m.name.clone();
                    }
                    Err(e) => f.error = Some(format!("{e:#}")),
                }
            }
            Act::OpenUpload(id) => {
                if let Some(m) = self.mods.iter().find(|m| m.id == id) {
                    let existing = m.remote_file_id.is_some();
                    self.upload = Some(UploadForm { m: m.clone(), visibility: if existing { 0 } else { 1 }, note: String::new(), item_id: String::new(), rx: None, stage: None, result: None });
                    self.pick_upload = false;
                }
            }
            Act::StartUpload => self.start_upload(),
            Act::RescanMods => {
                if let Ok(g) = &self.game {
                    self.mods = mods::scan(&g.data_dir);
                }
                self.mod_times.clear();
                self.mod_covers.clear();
            }
            Act::EditConfig(id) => {
                if let Some(p) = self.plugins.iter().find(|p| p.manifest.id == id) {
                    self.config = Some(ConfigEditor::open(p.clone()));
                }
            }
            Act::RescanPlugins => match plugins::list() {
                Ok((p, problems)) => {
                    self.plugins = p;
                    self.plugin_problems = problems;
                }
                Err(e) => self.plugin_problems = vec![format!("{e:#}")],
            },
            Act::SetModsSort(v) => {
                self.store.mods_sort = Some(v.to_string());
                self.save();
            }
            Act::SetModsView(v) => {
                self.store.mods_view = Some(v.to_string());
                self.save();
            }
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
        // The plan: the picture stays free. The eye starts at the top left (logo, version), and ends at the bottom right on the one light
        // object, the white Play button. Along the bottom, a dark scrim carries the controls (right) and the news (left); both stand on the
        // same baseline, and nothing has a panel of its own.
        let avail = ui.available_rect_before_wrap();
        let screen = ui.ctx().screen_rect();
        let scrim_top = avail.bottom() - 360.0;
        let mut mesh = egui::Mesh::default();
        let (clear, dark) = (Color32::from_black_alpha(0), Color32::from_black_alpha((150.0 * ui.opacity()) as u8));
        mesh.colored_vertex(pos2(screen.left(), scrim_top), clear);
        mesh.colored_vertex(pos2(screen.right(), scrim_top), clear);
        mesh.colored_vertex(screen.right_bottom(), dark);
        mesh.colored_vertex(screen.left_bottom(), dark);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.ctx().layer_painter(egui::LayerId::background()).add(Shape::mesh(mesh));

        let gap = 32.0;
        let controls_w = 296.0f32.min(avail.width() * 0.36);
        let left_rect = Rect::from_min_max(avail.min, pos2(avail.right() - controls_w - gap, avail.bottom()));
        let right_rect = Rect::from_min_max(pos2(avail.right() - controls_w, avail.top()), avail.max);
        let mut brand = ui.new_child(UiBuilder::new().id_salt("play-brand").max_rect(left_rect));
        self.brand(&mut brand);
        let news_rect = Rect::from_min_max(pos2(left_rect.left(), left_rect.bottom() - 168.0), left_rect.max);
        let mut news = ui.new_child(UiBuilder::new().id_salt("play-news").max_rect(news_rect));
        self.news_strip(&mut news);
        // the controls stand on the bottom edge: they are as high as they were laid out last frame (one frame late, then right)
        let height_id = egui::Id::new("play-controls-height");
        let last: f32 = ui.ctx().data(|d| d.get_temp(height_id)).unwrap_or(200.0);
        let controls_rect = Rect::from_min_max(pos2(right_rect.left(), (right_rect.bottom() - last).max(right_rect.top())), right_rect.max);
        let mut controls = ui.new_child(UiBuilder::new().id_salt("play-controls").max_rect(controls_rect));
        self.play_controls(&mut controls);
        let used = controls.min_rect().height();
        if (used - last).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(height_id, used));
            ui.ctx().request_repaint();
        }
        ui.advance_cursor_after_rect(avail);
    }

    /// Logo, and the game version as the largest thing on the page.
    fn brand(&mut self, ui: &mut Ui) {
        let (codename, number, build, date) = match &self.game {
            Ok(g) => {
                let (c, n, b) = version_parts(&g.settings.version);
                (c, n, b, pe::describe(g.exe_timestamp).split(' ').next().unwrap_or("").to_string())
            }
            Err(_) => Default::default(),
        };
        ui.add_space(24.0);
        let logo = self.logo.clone();
        match logo.as_ref().and_then(|p| self.assets.image(p, 700)) {
            Some(t) => {
                let h = 88.0;
                let (r, _) = ui.allocate_exact_size(vec2(h * t.size.x / t.size.y, h), Sense::hover());
                ui.painter().image(t.handle.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                ui.label(RichText::new("Stellaris").size(56.0).family(bold()).color(LABEL));
            }
        }
        ui.add_space(16.0);
        if number.is_empty() {
            return;
        }
        // the code name stands on the baseline of the number: both are painted, the smaller one moved so that the baselines meet
        let baseline = |g: &egui::Galley| g.rows.first().and_then(|r| r.glyphs.first()).map_or(g.size().y, |gl| gl.pos.y);
        let big = ui.painter().layout_no_wrap(format!("v{number}"), egui::FontId::new(48.0, bold()), LABEL);
        let small = ui.painter().layout_no_wrap(codename.clone(), egui::FontId::new(16.0, egui::FontFamily::Proportional), SECONDARY);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), big.size().y), Sense::hover());
        let line = row.top() + baseline(&big);
        let small_at = pos2(row.left() + big.size().x + 10.0, line - baseline(&small));
        ui.painter().galley(row.min, big, LABEL);
        if !codename.is_empty() {
            ui.painter().galley(small_at, small, SECONDARY);
        }
        let mut detail = Vec::new();
        if !build.is_empty() {
            detail.push(format!("build {build}"));
        }
        if !date.is_empty() {
            detail.push(date);
        }
        if !detail.is_empty() {
            ui.label(RichText::new(detail.join("  ·  ")).size(15.0).color(SECONDARY));
        }
    }

    /// The news cards in one row along the bottom, a page at a time (arrows, or the wheel over the row); a tooltip shows one at full size.
    fn news_strip(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let now = ui.input(|i| i.time);
        let acts = Acts::default();
        let loading = self.news.rx.is_some();
        let cards = self.news.cards.clone();
        let height = 128.0;
        let gap = 16.0;
        let width = ui.available_width();

        // the cards in the order shown (the card of the main slot first), each at the row's height
        let mut order: Vec<usize> = Vec::new();
        let mut main_len = 0;
        if !cards.is_empty() {
            let main: Vec<usize> = cards.iter().enumerate().filter(|(_, c)| c.slot == "main").map(|(i, _)| i).collect();
            let main = if main.is_empty() { vec![0] } else { main };
            main_len = main.len();
            if main.len() > 1 && now - self.news.switched > 8.0 {
                self.news.hero = (self.news.hero + 1) % main.len();
                self.news.switched = now;
            }
            let hero_i = main[self.news.hero.min(main.len() - 1)];
            order.push(hero_i);
            order.extend((0..cards.len()).filter(|i| !main.contains(i)));
        }
        let sizes: Vec<Vec2> = order
            .iter()
            .map(|&i| {
                let s = cards[i].image.as_ref().and_then(|p| self.assets.image(p, 1400)).map(|t| t.size).unwrap_or(vec2(246.0, 230.0));
                vec2(s.x * height / s.y, height)
            })
            .collect();
        // the pages: as many cards as fit the width
        let mut pages: Vec<(usize, usize)> = Vec::new();
        let (mut start, mut x) = (0usize, 0.0f32);
        for (k, s) in sizes.iter().enumerate() {
            if k > start && x + s.x > width + 0.5 {
                pages.push((start, k));
                start = k;
                x = 0.0;
            }
            x += s.x + gap;
        }
        if !sizes.is_empty() {
            pages.push((start, sizes.len()));
        }
        let page_count = pages.len().max(1);
        self.news.page = self.news.page.min(page_count - 1);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(tr(lang, "play.news").to_uppercase()).size(12.0).color(SECONDARY));
            if circle_button(ui, Icon::Refresh, theme::white(22), SECONDARY, !loading).clicked() {
                acts.push(Act::RefreshNews);
            }
            if page_count > 1 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if circle_button(ui, Icon::Right, theme::white(22), LABEL, self.news.page + 1 < page_count).clicked() {
                        self.news.page += 1;
                    }
                    ui.label(RichText::new(format!("{}/{}", self.news.page + 1, page_count)).size(12.0).color(SECONDARY));
                    if circle_button(ui, Icon::Left, theme::white(22), LABEL, self.news.page > 0).clicked() {
                        self.news.page -= 1;
                    }
                });
            }
        });
        ui.add_space(8.0);
        if order.is_empty() {
            let text = if loading { "…".to_string() } else { self.news.error.clone().unwrap_or_else(|| tr(lang, "play.news_empty").to_string()) };
            ui.label(RichText::new(text).size(12.5).color(SECONDARY));
        } else {
            let (from, to) = pages[self.news.page];
            let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            if page_count > 1 && ui.rect_contains_pointer(area) {
                let d = ui.input(|i| i.raw_scroll_delta);
                let amount = if d.x.abs() > d.y.abs() { d.x } else { d.y };
                if amount.abs() > 1.0 && now - self.news.wheel > 0.3 {
                    if amount < 0.0 && self.news.page + 1 < page_count {
                        self.news.page += 1;
                    } else if amount > 0.0 && self.news.page > 0 {
                        self.news.page -= 1;
                    }
                    self.news.wheel = now;
                }
            }
            let mut x = 0.0;
            for k in from..to {
                let r = Rect::from_min_size(area.min + vec2(x, 0.0), sizes[k]);
                x += sizes[k].x + gap;
                self.draw_card(ui, &cards[order[k]], r, theme::RADIUS, &format!("card{k}"), &acts);
                if k == 0 && main_len > 1 {
                    let dots = main_len as f32 * 11.0;
                    for d in 0..main_len {
                        let c = pos2(r.center().x - dots / 2.0 + 5.5 + d as f32 * 11.0, r.bottom() - 9.0);
                        if ui.interact(Rect::from_center_size(c, Vec2::splat(11.0)), egui::Id::new(("dot", d)), Sense::click()).clicked() {
                            self.news.hero = d;
                            self.news.switched = now;
                        }
                        ui.painter().circle_filled(c, 2.6, if d == self.news.hero { Color32::WHITE } else { theme::white(110) });
                    }
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
            }
        }
        if self.news.rx.is_some() || self.assets.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
        self.acts.extend(acts.take());
    }

    fn draw_card(&mut self, ui: &mut Ui, card: &Card, rect: Rect, radius: f32, id: &str, acts: &Acts) {
        let resp = ui.interact(rect, egui::Id::new(("news", id)), Sense::click());
        let now = ui.input(|i| i.time);
        // the advertisements are loud: they stay a little dimmed, and come up to full brightness under the pointer
        let lit = ui.ctx().animate_bool_with_time(resp.id.with("lit"), resp.hovered(), 0.18);
        let tint = Color32::from_gray((200.0 + 55.0 * lit) as u8);
        let mut preview = None;
        match card.image.as_ref().and_then(|p| self.assets.image(p, 1400)) {
            Some(tex) => {
                let id = tex.at(now).id();
                theme::cover_image(ui, rect, id, tex.size, radius, tint);
                preview = Some((id, tex.size));
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
            ui.painter().add(Shape::rect_stroke(rect, egui::CornerRadius::same(radius as u8), egui::Stroke::new(1.0, theme::white(90)), egui::StrokeKind::Inside));
        }
        if let Some((tex, size)) = preview {
            resp.clone().on_hover_ui(|ui| {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(tex, size)).corner_radius(12.0));
            });
        }
        if resp.clicked() {
            if let Some(l) = &card.link {
                acts.push(Act::Open(l.clone()));
            }
        }
    }

    /// The controls at the bottom right: the playset, then the two big buttons, Play and Continue, then one line saying what Continue continues
    /// (or that the game is running). The rest of the launch options are in Settings.
    fn play_controls(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let active = self.store.active_index();
        let acts = Acts::default();
        let launching = self.launching.is_some();
        let running = !self.running.is_empty();
        let ready = self.game.is_ok() && !launching && !running;
        {
            let popup_id = egui::Id::new("playset-popup");
            let open = ui.memory(|m| m.is_popup_open(popup_id));
            let trigger = theme::inline_picker(ui, tr(lang, "tab.playsets"), &self.store.playsets[active].name, open);
            if trigger.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup_id));
                self.focus_playset_filter = true;
            }
            // open towards the side with more room (this card sits at the bottom of the window, above the tab bar)
            let screen = ui.ctx().screen_rect();
            let room_below = screen.bottom() - 66.0 - trigger.rect.bottom();
            let room_above = trigger.rect.top() - 42.0;
            let (side, room) = if room_below >= room_above { (egui::AboveOrBelow::Below, room_below) } else { (egui::AboveOrBelow::Above, room_above) };
            let list_h = (room - 90.0).clamp(120.0, 300.0);
            egui::popup::popup_above_or_below_widget(ui, popup_id, &trigger, side, egui::popup::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(trigger.rect.width() - 12.0);
                if self.store.playsets.len() > 6 {
                    let r = theme::search_field(ui, &mut self.playset_filter, tr(lang, "mods.search"), ui.available_width());
                    if std::mem::take(&mut self.focus_playset_filter) {
                        r.request_focus();
                    }
                    ui.add_space(6.0);
                }
                let q = self.playset_filter.to_lowercase();
                let mut shown = 0;
                egui::ScrollArea::vertical().id_salt("playset-popup-scroll").max_height(list_h).auto_shrink([true, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut rows = Rows::new();
                    for (i, p) in self.store.playsets.iter().enumerate() {
                        if !q.is_empty() && !p.name.to_lowercase().contains(&q) {
                            continue;
                        }
                        shown += 1;
                        if i == active {
                            rows.highlight_next();
                        }
                        let mods_on = p.mods.iter().filter(|m| m.enabled).count().to_string();
                        let plugins_on = p.plugins.iter().filter(|x| x.enabled).count().to_string();
                        let r = rows.row(ui, 50.0, 24.0, true, |ui| {
                            stack(ui, 50.0, 38.0, |ui| {
                                ui.add(egui::Label::new(RichText::new(&p.name).size(15.0).family(bold())).truncate());
                                ui.label(RichText::new(tr_args(lang, "ps.counts", &[&mods_on, &plugins_on])).size(12.0).color(SECONDARY));
                            });
                        }, |ui| {
                            if i == active {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                                Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                            }
                        });
                        if r.clicked() {
                            acts.push(Act::SetActive(i));
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                });
                if shown == 0 {
                    ui.add_space(8.0);
                    ui.label(RichText::new("—").color(SECONDARY));
                }
            });
            ui.add_space(16.0);
            let w = ui.available_width();
            let text = if launching { tr(lang, "play.starting") } else if running { tr(lang, "play.running") } else { tr(lang, "play.button") };
            if theme::hero_button(ui, vec2(w, 56.0), text, Icon::PlayFilled, true, ready).clicked() {
                acts.push(Act::Start(false));
            }
            ui.add_space(12.0);
            if theme::hero_button(ui, vec2(w, 48.0), tr(lang, "play.continue"), Icon::Resume, false, ready && self.last_save.is_some()).clicked() {
                acts.push(Act::Start(true));
            }
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if running {
                    let pid = self.running[0].to_string();
                    ui.label(RichText::new(format!("●  {}", tr_args(lang, "play.status_running", &[&pid]))).size(12.5).color(GREEN));
                    if pill_button(ui, tr(lang, "play.close"), ButtonStyle::Plain(RED), true).clicked() {
                        acts.push(Act::CloseGame);
                    }
                } else if let Some(last) = self.log.last().filter(|l| l.contains("could not") || l.contains("failed")) {
                    ui.add(egui::Label::new(RichText::new(last).size(12.0).color(RED)).truncate());
                } else {
                    let line = match &self.last_save {
                        Some(sv) => match saves::local_time(sv.modified) {
                            Some((_, mo, d, h, mi)) => format!("{}  ·  {mo:02}-{d:02} {h:02}:{mi:02}", sv.name),
                            None => sv.name.clone(),
                        },
                        None => tr(lang, "play.no_save").to_string(),
                    };
                    ui.add(egui::Label::new(RichText::new(line).size(12.0).color(SECONDARY)).truncate());
                }
            });
        }
        self.acts.extend(acts.take());
    }

    // ---------------------------------------------------------------- Playsets
    fn page_playsets(&mut self, ui: &mut Ui) {
        // No page title: the tab bar already says where we are. Two cards side by side, their first rows level: the new-playset field on the
        // left, the Mods | DLC | Plugins switch on the right.
        let lang = self.lang;
        let acts = Acts::default();
        ui.add_space(8.0);
        let avail = ui.available_rect_before_wrap();
        let left_w = 300.0f32.min(avail.width() * 0.32);
        let left_rect = Rect::from_min_max(avail.min, pos2(avail.left() + left_w, avail.bottom()));
        let right_rect = Rect::from_min_max(pos2(avail.left() + left_w + 16.0, avail.top()), avail.max);
        let active = self.store.active_index();

        glass_pane(ui, left_rect, theme::RADIUS, 18.0, |ui| {
            // a new playset
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let add_w = ui.painter().layout_no_wrap(tr(lang, "ps.add").to_owned(), egui::FontId::new(14.5, bold()), Color32::WHITE).size().x + 28.0;
                theme::text_field(ui, &mut self.new_name, tr(lang, "ps.new"), ui.available_width() - add_w - 8.0);
                let ok = !self.new_name.trim().is_empty();
                if pill_button(ui, tr(lang, "ps.add"), ButtonStyle::Filled(BLUE), ok).clicked() {
                    acts.push(Act::AddPlayset(self.new_name.clone()));
                }
            });
            theme::divider(ui);
            if self.store.playsets.len() > 8 {
                let w = ui.available_width();
                theme::search_field(ui, &mut self.ps_filter, tr(lang, "mods.search"), w);
                ui.add_space(8.0);
            }
            // the playsets; the import from the official launcher at the foot
            let q = self.ps_filter.to_lowercase();
            let list_h = (ui.available_height() - 44.0).max(60.0);
            egui::ScrollArea::vertical().id_salt("ps-list").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut rows = Rows::new();
                for (i, p) in self.store.playsets.iter().enumerate() {
                    if !q.is_empty() && !p.name.to_lowercase().contains(&q) {
                        continue;
                    }
                    if i == active {
                        rows.highlight_next();
                    }
                    let mods_on = p.mods.iter().filter(|m| m.enabled).count().to_string();
                    let plugins_on = p.plugins.iter().filter(|x| x.enabled).count().to_string();
                    let r = rows.row(ui, 58.0, 0.0, true, |ui| {
                        stack(ui, 58.0, 38.0, |ui| {
                            ui.add(egui::Label::new(RichText::new(&p.name).size(15.5).family(bold())).truncate());
                            ui.label(RichText::new(tr_args(lang, "ps.counts", &[&mods_on, &plugins_on])).size(12.0).color(SECONDARY));
                        });
                    }, |_| {});
                    if r.clicked() {
                        acts.push(Act::SetActive(i));
                    }
                }
            });
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if pill_button(ui, tr(lang, "ps.import"), ButtonStyle::Plain(BLUE), self.game.is_ok()).clicked() {
                    acts.push(Act::Import);
                }
            });
        });

        // the chosen playset
        glass_pane(ui, right_rect, theme::RADIUS, 18.0, |pane| self.playset_detail(pane, &acts));
        ui.advance_cursor_after_rect(avail);
        self.acts.extend(acts.take());
    }

    fn playset_detail(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let active = self.store.active_index();
        let p = &self.store.playsets[active];
        let mods_n = p.mods.len();
        let plugins_n = p.plugins.iter().filter(|x| x.enabled).count();
        let dlc_off = p.disabled_dlcs_or(&self.dlc_current).iter().filter(|id| self.dlcs.iter().any(|d| &d.id == *id)).count();
        let dlc_n = self.dlcs.len().saturating_sub(dlc_off);
        let can_delete = self.store.playsets.len() > 1;
        let labels = [
            format!("{}  {mods_n}", tr(lang, "seg.mods")),
            format!("{}  {dlc_n}", tr(lang, "seg.dlc")),
            format!("{}  {plugins_n}", tr(lang, "seg.plugins")),
        ];
        ui.horizontal(|ui| {
            let width = ui.available_width();
            if let Some(i) = segmented(ui, &labels, self.seg, width.min(420.0)) {
                self.seg = i;
            }
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
        theme::divider(ui);
        let now = ui.input(|i| i.time);
        if self.seg != self.seg_shown {
            self.seg_dir = if self.seg >= self.seg_shown { 1.0 } else { -1.0 };
            self.prev_seg = Some(self.seg_shown);
            self.seg_shown = self.seg;
            self.seg_t0 = now;
        }
        let t = ((now - self.seg_t0) / (0.24 * self.dev_slow)).clamp(0.0, 1.0) as f32;
        let ease = theme::ease_out(t);
        if t < 1.0 {
            ui.ctx().request_repaint();
        } else {
            self.prev_seg = None;
        }
        let area = ui.available_rect_before_wrap();
        if let Some(old) = self.prev_seg {
            let mut out = ui.new_child(UiBuilder::new().id_salt(("playset-body", old)).max_rect(area.translate(vec2(-self.seg_dir * 28.0 * ease, 0.0))));
            out.set_opacity(1.0 - ease);
            let dropped = Acts::default();
            match old {
                0 => self.playset_mods(&mut out, &dropped),
                1 => self.playset_dlc(&mut out, &dropped),
                _ => self.playset_plugins(&mut out, &dropped),
            }
        }
        let mut body = ui.new_child(UiBuilder::new().id_salt(("playset-body", self.seg)).max_rect(area.translate(vec2(self.seg_dir * 28.0 * (1.0 - ease), 0.0))));
        body.set_opacity(ease);
        match self.seg {
            0 => self.playset_mods(&mut body, acts),
            1 => self.playset_dlc(&mut body, acts),
            _ => self.playset_plugins(&mut body, acts),
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
        ui.add_space(8.0);
        let active = self.store.active_index();
        let version = self.game.as_ref().ok().map(|g| g.version().to_string()).unwrap_or_default();
        let sort = self.store.mods_sort.clone().unwrap_or_else(|| "name".into());
        let compact = self.store.mods_view.as_deref() == Some("compact");
        let filter = self.filter.to_lowercase();
        let mut shown: Vec<usize> = (0..self.mods.len()).filter(|&i| filter.is_empty() || self.mods[i].name.to_lowercase().contains(&filter)).collect();
        match sort.as_str() {
            "updated" => {
                for m in &self.mods {
                    self.mod_times.entry(m.id.clone()).or_insert_with(|| {
                        let p = m.path.clone().unwrap_or_else(|| m.file.clone());
                        std::fs::metadata(&p).and_then(|x| x.modified()).ok()
                    });
                }
                shown.sort_by(|&a, &b| self.mod_times.get(&self.mods[b].id).cmp(&self.mod_times.get(&self.mods[a].id)));
            }
            "source" => {
                let rank = |k: Kind| match k {
                    Kind::Local => 0,
                    Kind::Workshop => 1,
                    Kind::ParadoxMods => 2,
                };
                shown.sort_by_key(|&i| rank(self.mods[i].kind));
            }
            _ => {}
        }
        let total = shown.len();
        let rect = ui.available_rect_before_wrap();
        glass_pane(ui, rect, theme::RADIUS, 18.0, |ui| {
            // a row of a fixed height, so that everything in it is centred on one line (a growing row centres the first items too high)
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(RichText::new(tr(lang, "mods.title")).size(17.0).family(bold()));
                theme::count_badge(ui, total);
                ui.add_space(6.0);
                let sorts = [("name", "mods.sort_name"), ("updated", "mods.sort_updated"), ("source", "mods.sort_source")];
                let labels: Vec<String> = sorts.iter().map(|(_, k)| tr(lang, k).to_string()).collect();
                let cur = sorts.iter().position(|(v, _)| *v == sort).unwrap_or(0);
                if let Some(i) = segmented(ui, &labels, cur, 270.0) {
                    acts.push(Act::SetModsSort(sorts[i].0));
                }
                let views = [tr(lang, "mods.view_list").to_string(), tr(lang, "mods.view_compact").to_string()];
                if let Some(i) = segmented(ui, &views, compact as usize, 160.0) {
                    acts.push(Act::SetModsView(if i == 1 { "compact" } else { "list" }));
                }
                if circle_button(ui, Icon::Refresh, theme::white(22), LABEL, true).on_hover_text(tr(lang, "mods.refresh")).clicked() {
                    acts.push(Act::RescanMods);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, tr(lang, "mods.upload_btn"), ButtonStyle::Tinted(Color32::WHITE), self.game.is_ok()).clicked() {
                        self.pick_upload = true;
                    }
                    let w = ui.available_width().min(260.0);
                    theme::search_field(ui, &mut self.filter, tr(lang, "mods.search"), w);
                });
            });
            theme::divider(ui);
            let row_h = if compact { 40.0 } else { 64.0 };
            plain_rows(ui, "mods-library", row_h, total, |ui, range| {
                let mut rows = Rows::starting_at(range.start);
                for k in range {
                    let m = &self.mods[shown[k]];
                    let inside = self.store.playsets[active].mod_pos(&m.id).is_some();
                    let own = m.kind == Kind::Local && m.path.is_some() && m.problem.is_none();
                    let chips = |ui: &mut Ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (t, c) = Self::kind_chip(lang, m.kind);
                        chip(ui, t, c);
                        if let Some(sv) = m.supported_version.as_ref().filter(|sv| !mods::supports(sv, &version)) {
                            chip(ui, &tr_args(lang, "mods.for_version", &[sv]), ORANGE).on_hover_text(tr(lang, "mods.mismatch"));
                        }
                        if let Some(p) = &m.problem {
                            chip(ui, tr(lang, "mods.unusable"), RED).on_hover_text(p);
                        }
                    };
                    let name_color = if m.problem.is_some() { RED } else { LABEL };
                    let cover = if compact { None } else { self.mod_covers.entry(m.id.clone()).or_insert_with(|| mods::thumbnail(m)).clone() };
                    let cover_tex = cover.as_ref().and_then(|p| self.assets.image(p, 160)).map(|t| (t.handle.id(), t.size));
                    rows.row(ui, row_h, 68.0, false, |ui| {
                        if !compact {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(row_h - 14.0), Sense::hover());
                            match cover_tex {
                                Some((id, size)) => theme::cover_image(ui, r, id, size, 10.0, Color32::WHITE),
                                None => {
                                    ui.painter().rect_filled(r, egui::CornerRadius::same(10), theme::white(18));
                                    Icon::Mods.draw(ui.painter(), r.center(), 22.0, theme::TERTIARY, 1.5);
                                }
                            }
                        }
                        if compact {
                            ui.add(egui::Label::new(RichText::new(&m.name).size(14.5).color(name_color)).truncate());
                            chips(ui);
                        } else {
                            stack(ui, row_h, 42.0, |ui| {
                                ui.add(egui::Label::new(RichText::new(&m.name).size(15.0).color(name_color)).truncate());
                                ui.horizontal(|ui| chips(ui));
                            });
                        }
                    }, |ui| {
                        if inside {
                            if circle_button(ui, Icon::Check, GREEN, Color32::WHITE, true).on_hover_text(tr(lang, "mods.remove")).clicked() {
                                acts.push(Act::ModDrop(m.id.clone()));
                            }
                        } else if circle_button(ui, Icon::Plus, BLUE.gamma_multiply(0.35), BLUE, true).on_hover_text(tr(lang, "mods.add")).clicked() {
                            acts.push(Act::ModAdd(m.id.clone()));
                        }
                        if own {
                            ui.add_space(8.0);
                            if circle_button(ui, Icon::Upload, theme::white(26), LABEL, true).on_hover_text(tr(lang, "mods.upload")).clicked() {
                                acts.push(Act::OpenUpload(m.id.clone()));
                            }
                        }
                    });
                }
            });
        });
        ui.advance_cursor_after_rect(rect);
        self.acts.extend(acts.take());
    }

    // ---------------------------------------------------------------- Plugins
    fn page_plugins(&mut self, ui: &mut Ui) {
        let lang = self.lang;
        let acts = Acts::default();
        ui.add_space(8.0);
        let game = self.game.clone().ok();
        let active = self.store.active_index();
        let rect = ui.available_rect_before_wrap();
        glass_pane(ui, rect, theme::RADIUS, 18.0, |ui| {
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(RichText::new(tr(lang, "pl.title")).size(17.0).family(bold())).on_hover_text(tr(lang, "pl.hint"));
                theme::count_badge(ui, self.plugins.len());
                if circle_button(ui, Icon::Refresh, theme::white(22), LABEL, true).on_hover_text(tr(lang, "mods.refresh")).clicked() {
                    acts.push(Act::RescanPlugins);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, tr(lang, "pl.install"), ButtonStyle::Tinted(Color32::WHITE), true).clicked() {
                        acts.push(Act::PluginInstall(false));
                    }
                    if pill_button(ui, tr(lang, "pl.link"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                        acts.push(Act::PluginInstall(true));
                    }
                });
            });
            theme::divider(ui);
            if self.plugins.is_empty() && self.plugin_problems.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(tr(lang, "pl.empty")).size(17.0).color(LABEL));
                    ui.label(RichText::new(tr(lang, "pl.empty_hint")).size(13.0).color(SECONDARY));
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr(lang, "pl.hint")).size(12.5).color(SECONDARY));
                });
                return;
            }
            egui::ScrollArea::vertical().id_salt("plugins-page").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut rows = Rows::new();
                for p in &self.plugins {
                    let on = self.store.playsets[active].plugins.iter().find(|x| x.id == p.manifest.id).map(|x| x.enabled).unwrap_or(false);
                    let (status, color) = plugin_status(lang, game.as_ref(), p);
                    let has_text = !p.manifest.description.is_empty();
                    let h = if has_text { 84.0 } else { 64.0 };
                    let has_config = !p.manifest.config.is_empty() || p.config_dir().is_dir();
                    rows.row(ui, h, 200.0, false, |ui| {
                        stack(ui, h, if has_text { 64.0 } else { 44.0 }, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&p.manifest.name).size(15.5).family(bold()));
                                ui.label(RichText::new(&p.manifest.version).size(12.5).color(SECONDARY));
                            });
                            if has_text {
                                ui.add(egui::Label::new(RichText::new(&p.manifest.description).size(13.0).color(SECONDARY)).truncate());
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
                    }, |ui| {
                        let mut v = on;
                        if switch(ui, &mut v).changed() {
                            acts.push(Act::PluginFlag(p.manifest.id.clone(), v));
                        }
                        ui.add_space(6.0);
                        if pill_button(ui, tr(lang, "pl.remove"), ButtonStyle::Plain(RED), true).clicked() {
                            acts.push(Act::PluginRemove(p.manifest.id.clone()));
                        }
                        if has_config && circle_button(ui, Icon::Settings, theme::white(26), LABEL, true).on_hover_text(tr(lang, "cfg.edit")).clicked() {
                            acts.push(Act::EditConfig(p.manifest.id.clone()));
                        }
                    });
                }
                for pr in &self.plugin_problems {
                    ui.add_space(8.0);
                    ui.label(RichText::new(pr).size(12.5).color(RED));
                }
            });
        });
        ui.advance_cursor_after_rect(rect);
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
            // launch
            theme::section(ui, tr(lang, "set.launch"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                rows.row(ui, 48.0, 46.0, false, |ui| { ui.label(tr(lang, "play.plugins")); }, |ui| {
                    let mut on = self.store.use_plugins != Some(false);
                    if switch(ui, &mut on).changed() {
                        acts.push(Act::SetUsePlugins(on));
                    }
                });
            });
            if let Ok(g) = &self.game {
                if !g.settings.alternative_executables.is_empty() {
                    theme::section(ui, tr(lang, "play.mode"));
                    glass(ui, 16.0, 0.0, |ui| {
                        let mut rows = Rows::new();
                        let cur = self.store.alternative.filter(|&i| i < g.settings.alternative_executables.len());
                        let mut names = vec![(None, tr(lang, "play.standard").to_string())];
                        names.extend(g.settings.alternative_executables.iter().enumerate().map(|(i, a)| (Some(i), alt_label(lang, a, i))));
                        for (which, name) in names {
                            let r = rows.row(ui, 44.0, 24.0, true, |ui| {
                                ui.add(egui::Label::new(RichText::new(&name).color(if cur == which { LABEL } else { SECONDARY })).truncate());
                            }, |ui| {
                                if cur == which {
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                                    Icon::Check.draw(ui.painter(), r.center(), 18.0, BLUE, 2.2);
                                }
                            });
                            if r.clicked() {
                                acts.push(Act::SetAlternative(which));
                            }
                        }
                    });
                }
            }
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
        glass(ui, theme::RADIUS, 18.0, |ui| {
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

/// The name of an alternative executable: the known one in the window's language, else the game's own label.
fn alt_label(lang: Lang, a: &stl_core::game::AlternativeExecutable, i: usize) -> String {
    let en = a.label.get("en").cloned().unwrap_or_default();
    if en.eq_ignore_ascii_case("Cross-Store Multiplayer") {
        return tr(lang, "play.cross_store").to_string();
    }
    let code = match lang {
        Lang::ZhHans => "zh",
        _ => lang.code(),
    };
    a.label.get(code).cloned().filter(|s| !s.is_empty()).unwrap_or(if en.is_empty() { format!("#{}", i + 1) } else { en })
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
        // a minimised window is 0 x 0: egui asserts on a layout with no height, so there is nothing to build until it is back
        let screen = ctx.screen_rect();
        if screen.width() < 64.0 || screen.height() < 64.0 {
            self.assets.poll();
            self.poll_process();
            self.drain_launch();
            self.drain_news();
            return;
        }
        self.assets.poll();
        self.poll_process();
        self.drain_launch();
        self.drain_news();
        self.drain_upload();
        if self.upload.as_ref().is_some_and(|f| f.rx.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.launching.is_some() || !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if std::mem::take(&mut self.dev_popup) {
            ctx.memory_mut(|m| m.open_popup(egui::Id::new("playset-popup")));
        }
        self.assets.set_background(self.background_path());
        self.paint_background(ctx);
        // the page on show changes: it slides in from the side of its tab and fades in
        let now = ctx.input(|i| i.time);
        if let Some(p) = self.dev_then {
            if now > 2.0 {
                self.page = p;
                self.dev_then = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        egui::TopBottomPanel::top("titlebar").exact_height(42.0).show_separator_line(false).frame(egui::Frame::NONE).show(ctx, |ui| self.title_bar(ctx, ui));
        let near_bottom = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.y > ctx.screen_rect().bottom() - 130.0);
        if near_bottom || self.page != self.shown_page || ctx.memory(|m| m.any_popup_open()) {
            self.bar_active = now;
        }
        let sunk = now - self.bar_active > 1.5;
        if !sunk {
            ctx.request_repaint_after(Duration::from_millis(1600));
        }
        let sink = ctx.animate_bool_with_time(egui::Id::new("tab-bar-sink"), sunk, 0.35);
        egui::TopBottomPanel::bottom("tabs").exact_height(76.0).show_separator_line(false).frame(egui::Frame::NONE).show(ctx, |ui| {
            let items: Vec<(Icon, String)> = Page::ALL.iter().map(|p| (p.icon(), tr(self.lang, p.key()).to_string())).collect();
            let current = Page::ALL.iter().position(|p| *p == self.page).unwrap_or(0);
            if let Some(i) = theme::tab_bar(ui, &items, current, sink) {
                self.page = Page::ALL[i];
            }
        });
        if self.page != self.shown_page {
            let index = |p: Page| Page::ALL.iter().position(|x| *x == p).unwrap_or(0);
            self.page_dir = if index(self.page) >= index(self.shown_page) { 1.0 } else { -1.0 };
            self.prev_page = Some(self.shown_page);
            self.shown_page = self.page;
            self.page_t0 = now;
        }
        let t = ((now - self.page_t0) / (0.32 * self.dev_slow)).clamp(0.0, 1.0) as f32;
        let ease = theme::ease_out(t);
        if t < 1.0 {
            ctx.request_repaint();
        } else {
            self.prev_page = None;
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 16 })).show(ctx, |ui| {
            // a cross-fade: the old page slides out the other way while the new one slides in; what the old one asks for is dropped
            if let Some(old) = self.prev_page {
                let rect = ui.max_rect().translate(vec2(-self.page_dir * 44.0 * ease, 0.0));
                let mut out = ui.new_child(UiBuilder::new().id_salt(("page", old as usize)).max_rect(rect));
                out.set_opacity(1.0 - ease);
                let kept = std::mem::take(&mut self.acts);
                self.render_page(&mut out, old);
                self.acts = kept;
            }
            let rect = ui.max_rect().translate(vec2(self.page_dir * 44.0 * (1.0 - ease), 0.0));
            let mut page = ui.new_child(UiBuilder::new().id_salt(("page", self.page as usize)).max_rect(rect));
            page.set_opacity(ease);
            self.render_page(&mut page, self.page);
        });
        self.pick_sheet(ctx);
        self.config_sheet(ctx);
        self.make_sheet(ctx);
        self.upload_sheet(ctx);
        self.window_frame(ctx);
        for act in std::mem::take(&mut self.acts) {
            self.apply(ctx, act);
        }
    }
}

impl App {
    fn sheet_frame() -> egui::Frame {
        egui::Frame::new().fill(Color32::from_rgb(30, 30, 34)).stroke(egui::Stroke::new(1.0, theme::white(30))).corner_radius(theme::RADIUS as u8).inner_margin(egui::Margin::same(24))
    }

    /// A plugin's settings: its files in `config/`, edited as text and saved in place.
    fn config_sheet(&mut self, ctx: &egui::Context) {
        let Some(e) = self.config.as_mut() else { return };
        let lang = self.lang;
        let running = !self.running.is_empty();
        let mut close = false;
        let mut switch_to = None;
        let resp = egui::Modal::new(egui::Id::new("plugin-config")).frame(Self::sheet_frame()).show(ctx, |ui| {
            let w = (ctx.screen_rect().width() - 160.0).clamp(480.0, 860.0);
            ui.set_width(w);
            ui.label(RichText::new(tr_args(lang, "cfg.title", &[&e.plugin.manifest.name])).size(22.0).family(bold()));
            ui.add_space(4.0);
            ui.label(RichText::new(e.plugin.config_dir().display().to_string()).size(12.0).color(SECONDARY));
            ui.add_space(12.0);
            if e.files.is_empty() {
                ui.label(RichText::new(tr(lang, "cfg.none")).size(14.0).color(SECONDARY));
            } else {
                if e.files.len() > 1 {
                    let names: Vec<String> = e.files.iter().map(|f| f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).collect();
                    if let Some(i) = segmented(ui, &names, e.index, (names.len() as f32 * 150.0).min(w)) {
                        if i != e.index {
                            switch_to = Some(i);
                        }
                    }
                    ui.add_space(10.0);
                }
                let h = (ctx.screen_rect().height() - 360.0).clamp(160.0, 520.0);
                egui::Frame::new().fill(Color32::from_black_alpha(90)).corner_radius(12).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("config-text").max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut e.text).code_editor().frame(false).desired_width(f32::INFINITY).desired_rows(12));
                    });
                });
                ui.add_space(8.0);
                if running {
                    ui.label(RichText::new(tr(lang, "cfg.running")).size(12.5).color(ORANGE));
                }
                if let Some((msg, ok)) = &e.status {
                    ui.label(RichText::new(msg).size(12.5).color(if *ok { GREEN } else { RED }));
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if pill_button(ui, tr(lang, "cfg.folder"), ButtonStyle::Plain(BLUE), true).clicked() {
                    let dir = e.plugin.config_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
                let default = e.files.get(e.index).and_then(|f| e.plugin.config_default(f)).filter(|d| d.is_file());
                if let Some(d) = default {
                    if pill_button(ui, tr(lang, "cfg.revert"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                        if let Ok(t) = std::fs::read_to_string(&d) {
                            let substitute = e.plugin.manifest.config.iter().any(|c| c.substitute && e.files.get(e.index).and_then(|f| f.file_name()).is_some_and(|n| n.to_string_lossy() == c.file));
                            e.text = if substitute {
                                t.replace("{plugin_dir}", &e.plugin.dir.to_string_lossy()).replace("{config_dir}", &e.plugin.config_dir().to_string_lossy())
                            } else {
                                t
                            };
                        }
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let dirty = e.text != e.saved;
                    if pill_button(ui, tr(lang, "cfg.save"), ButtonStyle::Filled(BLUE), dirty && !e.files.is_empty()).clicked() {
                        if let Some(f) = e.files.get(e.index) {
                            match std::fs::write(f, &e.text) {
                                Ok(()) => {
                                    e.saved = e.text.clone();
                                    e.status = Some((tr(lang, "cfg.saved").to_string(), true));
                                }
                                Err(err) => e.status = Some((format!("{err}"), false)),
                            }
                        }
                    }
                    let label = if dirty { tr(lang, "cfg.discard") } else { tr(lang, "up.close") };
                    if pill_button(ui, label, ButtonStyle::Plain(SECONDARY), true).clicked() {
                        close = true;
                    }
                });
            });
        });
        if let Some(i) = switch_to {
            e.load(i);
        }
        // a click outside does not throw away unsaved text
        if close || (resp.should_close() && e.text == e.saved) {
            self.config = None;
        }
    }

    /// "Upload mod": the list of your own mods, each saying whether it is on the Workshop already; choosing one opens the upload sheet.
    fn pick_sheet(&mut self, ctx: &egui::Context) {
        if !self.pick_upload {
            return;
        }
        let lang = self.lang;
        let own: Vec<Mod> = self.mods.iter().filter(|m| m.kind == Kind::Local && m.path.is_some() && m.problem.is_none()).cloned().collect();
        let mut close = false;
        let mut chosen = None;
        let mut new = false;
        let resp = egui::Modal::new(egui::Id::new("pick-upload")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new(tr(lang, "mods.upload_btn")).size(22.0).family(bold()));
            ui.add_space(4.0);
            ui.label(RichText::new(tr(lang, "pick.hint")).size(13.0).color(SECONDARY));
            ui.add_space(12.0);
            if own.is_empty() {
                ui.label(RichText::new(tr(lang, "pick.none")).size(14.0).color(SECONDARY));
            }
            egui::ScrollArea::vertical().id_salt("pick-upload-list").max_height(320.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut rows = Rows::new();
                for m in &own {
                    let status = match &m.remote_file_id {
                        Some(id) => tr_args(lang, "pick.on_workshop", &[id]),
                        None => tr(lang, "pick.not_uploaded").to_string(),
                    };
                    let r = rows.row(ui, 54.0, 24.0, true, |ui| {
                        stack(ui, 54.0, 38.0, |ui| {
                            ui.add(egui::Label::new(RichText::new(&m.name).size(15.0).family(bold())).truncate());
                            ui.label(RichText::new(&status).size(12.0).color(if m.remote_file_id.is_some() { GREEN } else { SECONDARY }));
                        });
                    }, |ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                        Icon::Right.draw(ui.painter(), r.center(), 16.0, SECONDARY, 1.8);
                    });
                    if r.clicked() {
                        chosen = Some(m.id.clone());
                    }
                }
            });
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if pill_button(ui, &format!("+  {}", tr(lang, "mods.create")), ButtonStyle::Plain(BLUE), self.game.is_ok()).clicked() {
                    new = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, tr(lang, "common.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                        close = true;
                    }
                });
            });
        });
        if let Some(id) = chosen {
            self.acts.push(Act::OpenUpload(id));
        } else if new {
            self.pick_upload = false;
            self.make = Some(MakeForm { name: String::new(), version: "1.0.0".into(), tags: Vec::new(), add_to_playset: true, error: None });
        } else if close || resp.should_close() {
            self.pick_upload = false;
        }
    }

    /// The "new mod" sheet: name, version, tags; it shows where the mod will be made.
    fn make_sheet(&mut self, ctx: &egui::Context) {
        let Some(f) = self.make.as_mut() else { return };
        let lang = self.lang;
        let compat = self.game.as_ref().map(|g| g.settings.mods_compatibility_version.clone()).unwrap_or_default();
        let mut close = false;
        let mut create = false;
        let resp = egui::Modal::new(egui::Id::new("make-mod")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(tr(lang, "mk.title")).size(22.0).family(bold()));
            ui.add_space(14.0);
            ui.label(RichText::new(tr(lang, "mk.name")).size(12.5).color(SECONDARY));
            let r = theme::text_field(ui, &mut f.name, "My Mod", 460.0);
            if f.name.is_empty() && !r.has_focus() && f.error.is_none() {
                r.request_focus();
            }
            ui.add_space(6.0);
            let folder = modmake::folder_name(&f.name);
            ui.label(RichText::new(tr_args(lang, "mk.where", &[&format!("mod/{folder}")])).size(12.0).color(SECONDARY));
            ui.add_space(12.0);
            ui.label(RichText::new(tr(lang, "mk.version")).size(12.5).color(SECONDARY));
            theme::text_field(ui, &mut f.version, "1.0.0", 160.0);
            ui.label(RichText::new(tr_args(lang, "mk.for", &[&format!("v{compat}.*")])).size(12.0).color(SECONDARY));
            ui.add_space(12.0);
            ui.label(RichText::new(tr(lang, "mk.tags")).size(12.5).color(SECONDARY));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for tag in modmake::TAGS {
                    let on = f.tags.iter().any(|t| t == tag);
                    if theme::toggle_chip(ui, tag, on).clicked() {
                        if on {
                            f.tags.retain(|t| t != tag);
                        } else {
                            f.tags.push(tag.to_string());
                        }
                    }
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                switch(ui, &mut f.add_to_playset);
                ui.label(tr(lang, "mk.add_to_playset"));
            });
            if let Some(e) = &f.error {
                ui.add_space(8.0);
                ui.label(RichText::new(e).size(12.5).color(RED));
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill_button(ui, tr(lang, "mk.create"), ButtonStyle::Filled(BLUE), !f.name.trim().is_empty()).clicked() {
                    create = true;
                }
                if pill_button(ui, tr(lang, "common.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                    close = true;
                }
            });
        });
        if close || resp.should_close() {
            self.make = None;
        } else if create {
            self.acts.push(Act::CreateMod);
        }
    }

    /// The "upload to the Workshop" sheet: what will be sent and where, the visibility, a change note; then the progress and the result.
    fn upload_sheet(&mut self, ctx: &egui::Context) {
        let Some(f) = self.upload.as_mut() else { return };
        let lang = self.lang;
        let running = f.rx.is_some();
        let mut close = false;
        let mut start = false;
        let mut open: Option<String> = None;
        let resp = egui::Modal::new(egui::Id::new("upload-mod")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new(tr(lang, "up.title")).size(22.0).family(bold()));
            ui.add_space(4.0);
            ui.label(RichText::new(&f.m.name).size(15.0).color(LABEL));
            ui.add_space(10.0);
            let existing = f.m.remote_file_id.clone();
            match &existing {
                Some(id) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr_args(lang, "up.update", &[id])).size(13.0).color(SECONDARY));
                        if let Ok(n) = id.parse::<u64>() {
                            if pill_button(ui, tr(lang, "up.open"), ButtonStyle::Plain(BLUE), true).clicked() {
                                open = Some(workshop::item_url(n));
                            }
                        }
                    });
                }
                None => {
                    ui.label(RichText::new(tr(lang, "up.new")).size(13.0).color(SECONDARY));
                }
            }
            if mods::own_thumbnail(&f.m).is_none() {
                if existing.is_some() || !f.item_id.trim().is_empty() {
                    ui.label(RichText::new(tr(lang, "up.keep_preview")).size(12.5).color(SECONDARY));
                } else {
                    ui.label(RichText::new(tr(lang, "up.no_preview")).size(12.5).color(ORANGE));
                }
            }
            ui.add_space(14.0);
            ui.label(RichText::new(tr(lang, "up.visibility")).size(12.5).color(SECONDARY));
            let mut labels = vec![];
            if existing.is_some() {
                labels.push(tr(lang, "up.keep").to_string());
            }
            labels.extend(["up.private", "up.friends", "up.unlisted", "up.public"].iter().map(|k| tr(lang, k).to_string()));
            let offset = if existing.is_some() { 0 } else { 1 };
            if let Some(i) = segmented(ui, &labels, f.visibility - offset, 480.0) {
                if !running {
                    f.visibility = i + offset;
                }
            }
            if existing.is_none() {
                ui.add_space(12.0);
                ui.label(RichText::new(tr(lang, "up.item_id")).size(12.5).color(SECONDARY));
                theme::text_field(ui, &mut f.item_id, "", 240.0);
            }
            ui.add_space(12.0);
            ui.label(RichText::new(tr(lang, "up.note")).size(12.5).color(SECONDARY));
            theme::text_field(ui, &mut f.note, "", 480.0);
            ui.add_space(10.0);
            ui.label(RichText::new(tr(lang, "up.steam_note")).size(12.0).color(SECONDARY));
            // progress and result
            if running {
                ui.add_space(14.0);
                let (text, frac) = match f.stage {
                    Some((workshop::Stage::Creating, _, _)) => (tr(lang, "up.creating"), None),
                    Some((workshop::Stage::Uploading(_), done, total)) if total > 0 => (tr(lang, "up.sending"), Some(done as f32 / total as f32)),
                    Some((workshop::Stage::Uploading(_), _, _)) => (tr(lang, "up.sending"), None),
                    _ => (tr(lang, "up.connecting"), None),
                };
                ui.label(RichText::new(text).size(13.0).color(LABEL));
                theme::progress_bar(ui, frac, ui.input(|i| i.time));
            }
            match &f.result {
                Some(Ok(o)) => {
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("✓  {}", tr(lang, "up.done"))).size(14.0).color(GREEN));
                        if pill_button(ui, tr(lang, "up.open"), ButtonStyle::Plain(BLUE), true).clicked() {
                            open = Some(workshop::item_url(o.id));
                        }
                    });
                    if o.needs_agreement {
                        ui.label(RichText::new(tr(lang, "up.agreement")).size(12.5).color(ORANGE));
                    }
                }
                Some(Err(e)) => {
                    ui.add_space(14.0);
                    ui.label(RichText::new(e).size(12.5).color(RED));
                }
                None => {}
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let finished = matches!(f.result, Some(Ok(_)));
                if !finished && pill_button(ui, tr(lang, "up.upload"), ButtonStyle::Filled(BLUE), !running).clicked() {
                    start = true;
                }
                let label = if finished { tr(lang, "up.close") } else { tr(lang, "common.cancel") };
                if pill_button(ui, label, ButtonStyle::Plain(SECONDARY), !running).clicked() {
                    close = true;
                }
            });
        });
        if let Some(url) = open {
            open_link(&url);
        }
        if (close || resp.should_close()) && !running {
            self.upload = None;
        } else if start {
            self.acts.push(Act::StartUpload);
        }
    }

    fn render_page(&mut self, ui: &mut Ui, page: Page) {
        if self.game.is_err() && page != Page::Settings {
            self.page_missing(ui);
            return;
        }
        match page {
            Page::Play => self.page_play(ui),
            Page::Playsets => self.page_playsets(ui),
            Page::Mods => self.page_mods(ui),
            Page::Plugins => self.page_plugins(ui),
            Page::Settings => self.page_settings(ui),
        }
    }

    /// The window has no system frame: a hairline round it, and the edges and corners resize it (the system does the dragging).
    fn window_frame(&self, ctx: &egui::Context) {
        use egui::{CursorIcon, ResizeDirection, ViewportCommand};
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized {
            return;
        }
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-outline"))).rect_stroke(screen, 0.0, egui::Stroke::new(1.0, theme::white(40)), egui::StrokeKind::Inside);
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 6.0;
        let (l, r, t, b) = (pos.x - screen.left() < m, screen.right() - pos.x < m, pos.y - screen.top() < m, screen.bottom() - pos.y < m);
        let (dir, cursor) = match (l, r, t, b) {
            (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
            (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
            (_, true, true, _) => (ResizeDirection::NorthEast, CursorIcon::ResizeNeSw),
            (true, _, _, true) => (ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
            (true, ..) => (ResizeDirection::West, CursorIcon::ResizeHorizontal),
            (_, true, ..) => (ResizeDirection::East, CursorIcon::ResizeHorizontal),
            (_, _, true, _) => (ResizeDirection::North, CursorIcon::ResizeVertical),
            (_, _, _, true) => (ResizeDirection::South, CursorIcon::ResizeVertical),
            _ => return,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }

    /// The strip at the top, in place of the system's title bar (the window has no frame): the name at the left, minimise / maximise / close at
    /// the right, and the rest of it drags the window (a double click maximises).
    fn title_bar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        use egui::ViewportCommand;
        let rect = ui.max_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let drag = ui.interact(rect, egui::Id::new("titlebar-drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        // the mark: a blue rounded square with the star, as in the window icon
        let mark = Rect::from_center_size(pos2(rect.left() + 34.0 + 10.0, rect.center().y), Vec2::splat(20.0));
        ui.painter().rect_filled(mark, egui::CornerRadius::same(6), BLUE);
        let star: Vec<egui::Pos2> = (0..16)
            .map(|k| {
                let a = k as f32 * std::f32::consts::TAU / 16.0;
                let r = if k % 4 == 0 { 7.0 } else { 2.6 };
                mark.center() + vec2(a.cos() * r, a.sin() * r)
            })
            .collect();
        ui.painter().add(Shape::convex_polygon(star, Color32::WHITE, egui::Stroke::NONE));
        ui.painter().text(pos2(mark.right() + 9.0, rect.center().y), egui::Align2::LEFT_CENTER, "Stellaris Launcher", egui::FontId::new(13.5, bold()), theme::white(200));
        // the buttons
        let mut buttons = ui.new_child(UiBuilder::new().id_salt("window-buttons").max_rect(rect.shrink2(vec2(18.0, 0.0))).layout(Layout::right_to_left(Align::Center)));
        buttons.spacing_mut().item_spacing.x = 8.0;
        if theme::window_button(&mut buttons, Icon::Close, true).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if theme::window_button(&mut buttons, if maximized { Icon::Restore } else { Icon::Maximize }, false).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        if theme::window_button(&mut buttons, Icon::Minimize, false).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }

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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("Stellaris Launcher")
            .with_icon(theme::icon())
            .with_decorations(false)
            .with_maximized(std::env::args().any(|a| a == "--maximized")),
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

#[cfg(test)]
mod tests {
    use super::version_parts;

    #[test]
    fn splits_the_game_version() {
        assert_eq!(version_parts("Cygnus v4.5.1 (358e)"), ("Cygnus".into(), "4.5.1".into(), "358e".into()));
        assert_eq!(version_parts("v3.14.2"), ("".into(), "3.14.2".into(), "".into()));
        assert_eq!(version_parts("Some Name"), ("".into(), "Some Name".into(), "".into()));
        assert_eq!(version_parts(""), ("".into(), "".into(), "".into()));
    }
}
