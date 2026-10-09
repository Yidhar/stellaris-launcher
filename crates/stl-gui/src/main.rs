//! The window of the Stellaris launcher, in the manner of iOS: the game's artwork behind frosted-glass cards, a tab bar at the bottom.
//! Play (news, playset, start), Playsets (mods in order, DLC, plugins), Mods (the whole folder), Plugins (install, remove), Settings.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod editor;
mod i18n;
mod theme;
mod upload_i18n;

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
use stl_core::gamesettings::{self, Graphics};
use stl_core::{modmake, uploadcheck, workshop};
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
    /// how the open file is written: its encoding and whether it used CRLF; kept when saving
    encoding: stl_core::textenc::Encoding,
    crlf: bool,
    status: Option<(String, bool)>,
}

impl ConfigEditor {
    fn open(plugin: Plugin) -> ConfigEditor {
        let _ = plugin.ensure_config();
        let files = plugin.config_files();
        let mut e = ConfigEditor { plugin, files, index: 0, text: String::new(), saved: String::new(), encoding: stl_core::textenc::Encoding::Utf8, crlf: false, status: None };
        e.load(0);
        e
    }

    fn load(&mut self, i: usize) {
        self.index = i;
        let t = stl_core::textenc::decode(&self.files.get(i).and_then(|f| std::fs::read(f).ok()).unwrap_or_default());
        self.text = t.text;
        self.encoding = t.encoding;
        self.crlf = t.crlf;
        self.saved = self.text.clone();
        self.status = None;
    }

    fn save(&mut self) -> Result<(), String> {
        let f = self.files.get(self.index).ok_or("no file")?;
        let bytes = stl_core::textenc::encode(&stl_core::textenc::Text { text: self.text.clone(), encoding: self.encoding, crlf: self.crlf }).map_err(|e| format!("{e:#}"))?;
        std::fs::write(f, bytes).map_err(|e| e.to_string())?;
        self.saved = self.text.clone();
        Ok(())
    }
}

/// Update checks of the plugins, and an update being installed.
#[derive(Default)]
struct PluginUpdates {
    /// per plugin id: the newer release, or nothing when up to date, or why the check failed
    found: std::collections::HashMap<String, Result<Option<stl_core::updates::Available>, String>>,
    checking: Option<Receiver<(String, Result<Option<stl_core::updates::Available>, String>)>>,
    installing: Option<(String, Receiver<Result<String, String>>)>,
    message: Option<(String, bool)>,
}

enum UpMsg {
    Progress(workshop::Stage, u64, u64),
    Done(Result<workshop::Outcome, (String, Option<workshop::UploadError>)>),
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
    /// the checks before anything is sent: running, then what they found, for which item
    pre_rx: Option<Receiver<uploadcheck::Preflight>>,
    pre: Option<uploadcheck::Preflight>,
    pre_item: Option<u64>,
    /// upload as soon as the checks pass (the Upload button was pressed before they ran for this item)
    start_after_check: bool,
    /// what went wrong, as the upload reported it
    failure: Option<workshop::UploadError>,
    /// the upload as it was last sent (for the report)
    sent: Option<workshop::Upload>,
    copied_at: f64,
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
    DeletePlayset(usize),
    AskDelete(usize),
    Import,
    ModFlag(usize, bool),
    ModRemove(usize),
    ModMove(usize, i32),
    /// a mod (by its place) goes to a place in the list as it is now (0 = the top, the length = the bottom)
    ModMoveTo(usize, usize),
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
    FoldNews(bool),
    CheckSelfUpdate,
    RunCheck,
    OpenModCheck(usize),
    ApplySort,
    InstallSelfUpdate,
    SetAutoUpdate(bool),
    ChangeGameDir,
    UseGameDir(String),
    Open(String),
    CreateMod,
    RescanMods,
    RescanPlugins,
    EditConfig(String),
    CheckUpdates,
    SetGraphics(Graphics),
    UpdatePlugin(String),
    SetModsSort(&'static str),
    SetModsView(&'static str),
    SetModsFilter(usize),
    OpenUpload(String),
    StartUpload,
    CheckUpload,
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

/// Release notes as GitHub writes them, read plainly: headings in bold, list items with a bullet, `**` and backticks dropped.
fn release_notes(ui: &mut Ui, md: &str) {
    ui.spacing_mut().item_spacing.y = 4.0;
    for line in md.lines() {
        let t = line.trim_end();
        let plain = |s: &str| s.replace("**", "").replace('`', "");
        if t.trim().is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = t.trim_start().strip_prefix('#') {
            ui.label(RichText::new(plain(h.trim_start_matches('#').trim())).size(15.0).family(bold()));
        } else if let Some(item) = t.trim_start().strip_prefix("- ").or_else(|| t.trim_start().strip_prefix("* ")) {
            let indent = (t.len() - t.trim_start().len()) as f32 * 4.0;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label(RichText::new(format!("•  {}", plain(item))).size(13.5));
            });
        } else {
            ui.add(egui::Label::new(RichText::new(plain(t)).size(13.5)).wrap());
        }
    }
}

thread_local! {
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
}

fn set_ctx_of_acts(ctx: &egui::Context) {
    CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
}

/// The window's context, for actions that close or reopen it.
fn ctx_of_acts() -> Option<egui::Context> {
    CTX.with(|c| c.borrow().clone())
}

/// The news cards' height.
fn news_card_height(_ui: &Ui) -> f32 {
    128.0
}

/// The check of a playset's mods (problems, overrides, a better order), run on a worker thread.
#[derive(Default)]
struct CheckState {
    rx: Option<Receiver<CheckResult>>,
    /// 0..1, as f32 bits
    progress: std::sync::Arc<std::sync::atomic::AtomicU32>,
    result: Option<CheckResult>,
    sheet: CheckSheet,
    /// groups of the mod sheet shown in full
    expanded: std::collections::HashSet<usize>,
}

#[derive(Clone)]
struct CheckResult {
    playset: String,
    /// the enabled mods checked, in load order
    ids: Vec<String>,
    report: std::sync::Arc<stl_core::conflicts::Report>,
    plan: stl_core::conflicts::SortPlan,
}

#[derive(Default, Clone)]
enum CheckSheet {
    #[default]
    None,
    All,
    /// a mod (position in the checked order) and what it overlaps with, by the other mod
    Mod(usize, std::sync::Arc<Vec<OverlapGroup>>),
    Sort,
}

/// What one mod and another override of each other.
struct OverlapGroup {
    /// the other source (0: the game)
    other: usize,
    mine: usize,
    theirs: usize,
    /// (what, who wins, how)
    items: Vec<(String, String, &'static str)>,
}

/// The launcher's own update: looked for on start (and every few hours), downloaded in the background, installed on a restart.
#[derive(Default)]
struct SelfUpdate {
    rx: Option<Receiver<SelfUpdateMsg>>,
    phase: SelfPhase,
    /// the sheet with the release notes is open
    sheet: bool,
    /// when it was last looked for
    checked: Option<Instant>,
    /// the user asked (Settings): say so when it is up to date
    manual: bool,
    install_error: Option<String>,
}

#[derive(Default, Clone, PartialEq)]
enum SelfPhase {
    #[default]
    Idle,
    Checking,
    Downloading(String),
    Ready(stl_core::selfupdate::Staged),
    UpToDate,
    Failed(String),
}

enum SelfUpdateMsg {
    Downloading(String),
    Done(SelfPhase),
}

struct News {
    cards: Vec<Card>,
    rx: Option<Receiver<Result<Vec<Card>, String>>>,
    hero: usize,
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
    /// the playset whose delete button was clicked once (the second click deletes)
    confirm_delete: Option<usize>,
    /// the search over the mods of the playset on show
    ps_mod_filter: String,
    /// the mod (its id) being dragged to another place in the playset
    ps_drag: Option<String>,
    /// the search in the playset drop-down (shown when there are many) and in the list of the Playsets page
    playset_filter: String,
    ps_filter: String,
    focus_playset_filter: bool,
    /// development: open the playset drop-down on the first frame
    dev_popup: Option<String>,
    /// development: run the check on start, then open this sheet (`all`, `sort`, or a mod's position)
    dev_check: Option<String>,
    /// development: once the upload checks are done, show a failure with this EResult (for screenshots of the explanation)
    dev_upload_fail: Option<i32>,
    make: Option<MakeForm>,
    config: Option<ConfigEditor>,
    updates: PluginUpdates,
    self_update: SelfUpdate,
    check: CheckState,
    /// the game's graphics settings, and the monitors (read when the Settings page is first shown)
    gfx: Option<Graphics>,
    displays: Vec<gamesettings::Display>,
    /// when the game's settings files were written, as last read; and when that was last looked at
    gfx_stamp: Vec<Option<(std::time::SystemTime, u64)>>,
    gfx_looked: Instant,
    gfx_status: Option<(String, bool)>,
    /// the "Upload mod" picker is open
    pick_upload: bool,
    /// when each mod was last changed (for the sort by date), filled when needed
    mod_times: std::collections::HashMap<String, Option<std::time::SystemTime>>,
    /// where each mod's cover is, looked up once
    mod_covers: std::collections::HashMap<String, Option<PathBuf>>,
    /// per mod: does it change the checksum (no Ironman)? Filled on a worker thread; None = its files could not be read
    ironman: std::collections::HashMap<String, Option<bool>>,
    ironman_rx: Option<Receiver<(String, Option<bool>)>>,
    /// the Mods page's filter: 0 all, 1 Ironman-compatible, 2 not
    mods_filter: usize,
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
            confirm_delete: None,
            ps_mod_filter: String::new(),
            ps_drag: None,
            playset_filter: String::new(),
            ps_filter: String::new(),
            focus_playset_filter: false,
            dev_popup: None,
            dev_check: None,
            dev_upload_fail: None,
            make: None,
            config: None,
            updates: PluginUpdates::default(),
            self_update: SelfUpdate::default(),
            check: CheckState::default(),
            gfx: None,
            displays: Vec::new(),
            gfx_stamp: Vec::new(),
            gfx_looked: Instant::now(),
            gfx_status: None,
            pick_upload: false,
            mod_times: std::collections::HashMap::new(),
            mod_covers: std::collections::HashMap::new(),
            ironman: std::collections::HashMap::new(),
            ironman_rx: None,
            mods_filter: 0,
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
            news: News { cards: Vec::new(), rx: None, hero: 0, error: None, page: 0, wheel: 0.0 },
            acts: Vec::new(),
            logo: None,
            backgrounds: Vec::new(),
        };
        app.reload();
        app.lang = resolve_lang(&app.store, &app.game);
        // for screenshots while developing: --page=1 --seg=1 --lang=ja --popup --many=30 --make --upload=<part of a mod name> --update-sheet --popup=<id>
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
            } else if let Some(v) = a.strip_prefix("--filter=") {
                app.mods_filter = v.parse().unwrap_or(0);
            } else if a == "--pick" {
                app.pick_upload = true;
            } else if a == "--make" {
                app.make = Some(MakeForm { name: "My New Mod".into(), version: "1.0.0".into(), tags: vec!["Gameplay".into()], add_to_playset: true, error: None });
            } else if let Some(v) = a.strip_prefix("--upload-fail=") {
                app.dev_upload_fail = v.parse().ok();
            } else if let Some(v) = a.strip_prefix("--upload=") {
                if let Some(m) = app.mods.iter().find(|m| m.name.contains(v)) {
                    app.acts.push(Act::OpenUpload(m.id.clone()));
                }
            } else if let Some(v) = a.strip_prefix("--playset=") {
                // shown only: not saved unless something else is changed
                if let Some(i) = app.store.find(v) {
                    app.store.active = Some(app.store.playsets[i].id.clone());
                }
            } else if let Some(v) = a.strip_prefix("--ask-delete=") {
                app.confirm_delete = v.parse().ok();
            } else if let Some(v) = a.strip_prefix("--ps-search=") {
                app.ps_mod_filter = v.to_string();
            } else if a == "--check" {
                app.dev_check = Some(String::new());
            } else if let Some(v) = a.strip_prefix("--check-sheet=") {
                app.dev_check = Some(v.to_string());
            } else if a == "--update-sheet" {
                app.self_update.sheet = true;
            } else if a == "--popup" {
                app.dev_popup = Some("playset-popup".into());
            } else if let Some(v) = a.strip_prefix("--popup=") {
                app.dev_popup = Some(v.to_string());
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
            app.refresh_news();
        }
        // a release downloaded earlier and not installed yet shows at once; otherwise look for one
        if let Some(s) = stl_core::selfupdate::staged() {
            app.self_update.phase = SelfPhase::Ready(s);
        } else if app.store.auto_update != Some(false) {
            app.check_self_update(false);
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
        let mut cards = Vec::new();
        if let Ok(g) = &self.game {
            cards = news::load_cached(&g.settings.game_id, &g.settings.dist_platform, code);
            if cards.is_empty() {
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

    /// What the form says to send: the upload, and the item number typed in (None when none was typed).
    fn form_upload(f: &UploadForm) -> Option<(workshop::Upload, Option<String>)> {
        let content = f.m.path.clone()?;
        let visibility = match f.visibility {
            1 => Some(workshop::Visibility::Private),
            2 => Some(workshop::Visibility::FriendsOnly),
            3 => Some(workshop::Visibility::Unlisted),
            4 => Some(workshop::Visibility::Public),
            _ => None,
        };
        let mut up = workshop::Upload {
            title: f.m.name.clone(),
            description: String::new(),
            preview: mods::own_thumbnail(&f.m),
            content,
            tags: f.m.tags.clone(),
            visibility,
            change_note: f.note.trim().to_string(),
            existing: f.m.remote_file_id.as_deref().and_then(|v| v.parse().ok()),
        };
        let typed = f.item_id.trim().to_string();
        if up.existing.is_none() && !typed.is_empty() {
            up.existing = typed.parse().ok();
            return Some((up, Some(typed)));
        }
        Some((up, None))
    }

    /// Runs the checks of the upload form (on the disk, then with Steam) on a worker thread.
    fn check_upload(&mut self) {
        let (Ok(game), Some(f)) = (self.game.clone(), self.upload.as_mut()) else { return };
        if f.pre_rx.is_some() {
            return;
        }
        let Some((up, typed)) = Self::form_upload(f) else {
            f.result = Some(Err("the mod has no content folder (path=)".into()));
            return;
        };
        let m = f.m.clone();
        f.pre_item = up.existing;
        f.pre = None;
        let (tx, rx) = channel();
        f.pre_rx = Some(rx);
        std::thread::spawn(move || {
            let mut p = uploadcheck::local(&m, &up, typed.as_deref());
            uploadcheck::remote(&game.dir, &mut p, up.existing);
            let _ = tx.send(p);
        });
    }

    /// The Upload button: checks first when the checks have not run for this item; uploads when nothing stops it.
    fn start_upload(&mut self) {
        let Some(f) = self.upload.as_mut() else { return };
        if f.rx.is_some() {
            return;
        }
        let item = Self::form_upload(f).and_then(|(u, _)| u.existing);
        if f.pre_rx.is_some() || f.pre.is_none() || f.pre_item != item {
            f.start_after_check = true;
            self.check_upload();
            return;
        }
        if f.pre.as_ref().is_some_and(|p| p.blocked()) {
            return;
        }
        self.upload_now();
    }

    fn upload_now(&mut self) {
        let (Ok(game), Some(f)) = (self.game.clone(), self.upload.as_mut()) else { return };
        let Some((mut up, typed)) = Self::form_upload(f) else { return };
        // a contributor cannot change the cover: it is not sent
        if f.pre.as_ref().is_some_and(|p| p.contributor()) {
            up.preview = None;
        }
        let record_typed = typed.and_then(|t| t.parse::<u64>().ok());
        // what should not be sent (.git, source art…) stays out: a clean copy goes up instead of the folder
        let excluded = f.pre.as_ref().is_some_and(|p| !p.excluded.is_empty());
        let m = f.m.clone();
        let (tx, rx) = channel();
        f.rx = Some(rx);
        f.result = None;
        f.failure = None;
        f.sent = Some(up.clone());
        uploadcheck::journal_start(&m.id, up.existing);
        std::thread::spawn(move || {
            let tx_p = tx.clone();
            let original = up.content.clone();
            let copy = if excluded {
                match uploadcheck::clean_copy(&original) {
                    Ok(c) => {
                        up.content = c.clone();
                        Some(c)
                    }
                    Err(e) => {
                        let _ = tx.send(UpMsg::Done(Err((format!("cannot make a clean copy of the mod to upload: {e:#}"), None))));
                        return;
                    }
                }
            } else {
                None
            };
            let r = workshop::upload(
                &game.dir,
                &up,
                &mut |id| {
                    // remembered first: if anything after this fails, the next try still finds the item
                    uploadcheck::journal_created(&m.id, id);
                    modmake::set_remote_file_id(&m, id)?;
                    // the copy being sent carries the new number too
                    if let Some(c) = &copy {
                        let _ = std::fs::copy(original.join("descriptor.mod"), c.join("descriptor.mod"));
                    }
                    Ok(())
                },
                &mut |stage, done, total| {
                    let _ = tx_p.send(UpMsg::Progress(stage, done, total));
                },
            );
            if let Some(c) = &copy {
                uploadcheck::remove_clean_copy(c);
            }
            let r = match (r, record_typed) {
                (Ok(o), Some(id)) => modmake::set_remote_file_id(&m, id).map(|_| o),
                (r, _) => r,
            };
            if r.is_ok() {
                uploadcheck::journal_finish(&m.id);
            }
            let _ = tx.send(UpMsg::Done(r.map_err(upload_failure)));
        });
    }

    /// The diagnostic report of the upload form, to copy.
    fn upload_report(&self) -> String {
        let Some(f) = &self.upload else { return String::new() };
        let up = f.sent.clone().or_else(|| Self::form_upload(f).map(|(u, _)| u));
        let Some(up) = up else { return String::new() };
        let version = self.game.as_ref().map(|g| g.settings.version.clone()).unwrap_or_default();
        let failure = match &f.result {
            Some(Err(msg)) => Some((msg.as_str(), f.failure.as_ref())),
            _ => None,
        };
        let outcome = match &f.result {
            Some(Ok(o)) => Some(o),
            _ => None,
        };
        uploadcheck::report(&f.m, &up, f.pre.as_ref(), failure, outcome, &version)
    }

    /// Asks GitHub about every plugin that names a repository, on a worker thread.
    fn check_updates(&mut self) {
        if self.updates.checking.is_some() {
            return;
        }
        let list: Vec<Plugin> = self.plugins.iter().filter(|p| p.manifest.update.is_some()).cloned().collect();
        if list.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.updates.checking = Some(rx);
        std::thread::spawn(move || {
            for p in list {
                let r = stl_core::updates::check(&p).map_err(|e| format!("{e:#}"));
                let _ = tx.send((p.manifest.id.clone(), r));
            }
        });
    }

    fn update_plugin(&mut self, id: &str) {
        if self.updates.installing.is_some() {
            return;
        }
        let Some(p) = self.plugins.iter().find(|p| p.manifest.id == id).cloned() else { return };
        let Some(Ok(Some(a))) = self.updates.found.get(id).cloned() else { return };
        let (tx, rx) = channel();
        self.updates.installing = Some((id.to_string(), rx));
        self.updates.message = None;
        std::thread::spawn(move || {
            let r = stl_core::updates::apply(&p, &a).map(|n| n.manifest.version).map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
        });
    }

    fn drain_updates(&mut self) {
        let mut done = Vec::new();
        if let Some(rx) = &self.updates.checking {
            loop {
                match rx.try_recv() {
                    Ok(x) => done.push(x),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.updates.checking = None;
                        break;
                    }
                }
            }
        }
        for (id, r) in done {
            self.updates.found.insert(id, r);
        }
        let mut finished = None;
        if let Some((id, rx)) = &self.updates.installing {
            if let Ok(r) = rx.try_recv() {
                finished = Some((id.clone(), r));
            }
        }
        if let Some((id, r)) = finished {
            self.updates.installing = None;
            match r {
                Ok(v) => {
                    self.updates.found.insert(id.clone(), Ok(None));
                    let mut msg = tr_args(self.lang, "pl.updated", &[&id, &v]);
                    if !self.running.is_empty() {
                        msg = format!("{msg} · {}", tr(self.lang, "pl.next_start"));
                    }
                    self.updates.message = Some((msg, true));
                    self.say(format!("updated {id} to {v}"));
                }
                Err(e) => {
                    self.updates.message = Some((e.clone(), false));
                    self.say(format!("could not update {id}: {e}"));
                }
            }
            if let Ok((p, problems)) = plugins::list() {
                self.plugins = p;
                self.plugin_problems = problems;
            }
        }
    }

    /// Looks at every mod's files for what the game checksums, on a worker thread (a big mod has thousands of files).
    fn start_ironman_scan(&mut self) {
        let Ok(game) = &self.game else { return };
        if self.ironman_rx.is_some() || !self.ironman.is_empty() {
            return;
        }
        let rules = stl_core::ironman::rules(&game.dir);
        let list: Vec<Mod> = self.mods.clone();
        let (tx, rx) = channel();
        self.ironman_rx = Some(rx);
        std::thread::spawn(move || {
            for m in list {
                let r = stl_core::ironman::affects_checksum(&m, &rules);
                if tx.send((m.id.clone(), r)).is_err() {
                    return;
                }
            }
        });
    }

    fn drain_ironman(&mut self) {
        let Some(rx) = &self.ironman_rx else { return };
        loop {
            match rx.try_recv() {
                Ok((id, r)) => {
                    self.ironman.insert(id, r);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ironman_rx = None;
                    break;
                }
            }
        }
    }

    /// The enabled mods of the active playset that are installed, in load order (what a check covers).
    fn checked_ids(&self) -> Vec<String> {
        let p = self.store.active_playset();
        p.mods.iter().filter(|m| m.enabled && self.mods.iter().any(|x| x.id == m.id)).map(|m| m.id.clone()).collect()
    }

    /// The last check, if it is of the active playset as it is now.
    fn current_check(&self) -> Option<&CheckResult> {
        let r = self.check.result.as_ref()?;
        (r.playset == self.store.active_playset().id && r.ids == self.checked_ids()).then_some(r)
    }

    fn run_check(&mut self) {
        let Ok(g) = &self.game else { return };
        if self.check.rx.is_some() {
            return;
        }
        let (game_dir, version) = (g.dir.clone(), g.version().to_string());
        let installed = self.mods.clone();
        let playset = self.store.active_playset().clone();
        let progress = self.check.progress.clone();
        progress.store(0f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = channel();
        self.check.rx = Some(rx);
        std::thread::spawn(move || {
            use stl_core::conflicts as cf;
            let (list, missing) = cf::playset_mods(&playset, &installed);
            let input = cf::Input { game_dir: &game_dir, game_version: &version, mods: list.clone(), installed: &installed, missing };
            let report = cf::analyze(&input, &|p| progress.store(p.to_bits(), std::sync::atomic::Ordering::Relaxed));
            let plan = cf::suggest_order(&list, &report);
            let ids = list.iter().map(|m| m.id.clone()).collect();
            let _ = tx.send(CheckResult { playset: playset.id.clone(), ids, report: std::sync::Arc::new(report), plan });
        });
    }

    fn drain_check(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.check.rx else { return };
        match rx.try_recv() {
            Ok(r) => {
                self.say(format!("checked {}: {} problems, {} file and {} definition overlaps in {} ms", r.playset, r.report.issues.len(), r.report.files.len(), r.report.keys.len(), r.report.millis));
                self.check.result = Some(r);
                self.check.rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.check.rx = None,
        }
    }

    /// What a mod (position in the checked order) overlaps with, grouped by the other mod, the most first.
    fn overlap_groups(r: &CheckResult, m: usize) -> Vec<OverlapGroup> {
        use stl_core::conflicts::Rule;
        let me = m + 1;
        let rep = &r.report;
        let mut by: std::collections::HashMap<usize, OverlapGroup> = std::collections::HashMap::new();
        let name = |s: usize| rep.sources.get(s).cloned().unwrap_or_default();
        for f in rep.files_of(m) {
            let winner = *f.sources.last().unwrap_or(&0);
            for &o in f.sources.iter().filter(|&&o| o != me && o > 0) {
                let g = by.entry(o).or_insert_with(|| OverlapGroup { other: o, mine: 0, theirs: 0, items: Vec::new() });
                if winner == me {
                    g.mine += 1;
                } else if winner == o {
                    g.theirs += 1;
                }
                g.items.push((f.path.clone(), name(winner), if f.intended { "chk.how_patch" } else { "chk.how_file" }));
            }
        }
        for k in rep.keys_of(m) {
            let winner = k.winner.map(|w| k.defs[w].source);
            let how = match k.rule {
                Rule::Lios => "chk.how_lios",
                Rule::Fios => "chk.how_fios",
                Rule::Duplicates => "chk.how_dupl",
                _ => "chk.how_unknown",
            };
            let others: std::collections::HashSet<usize> = k.defs.iter().map(|d| d.source).filter(|&s| s != me).collect();
            for o in others {
                let g = by.entry(o).or_insert_with(|| OverlapGroup { other: o, mine: 0, theirs: 0, items: Vec::new() });
                if winner == Some(me) {
                    g.mine += 1;
                } else if winner == Some(o) {
                    g.theirs += 1;
                }
                g.items.push((format!("{}: {}", k.folder, k.key), winner.map(&name).unwrap_or_else(|| "—".into()), how));
            }
        }
        let mut v: Vec<OverlapGroup> = by.into_values().collect();
        // the game last: overriding it is what mods are for
        v.sort_by_key(|g| (g.other == 0, std::cmp::Reverse(g.items.len())));
        v
    }

    /// Looks for a newer launcher and, when there is one, downloads and unpacks it beside (never over) the running one.
    fn check_self_update(&mut self, manual: bool) {
        if self.self_update.rx.is_some() || matches!(self.self_update.phase, SelfPhase::Ready(_)) {
            return;
        }
        let (tx, rx) = channel();
        self.self_update.rx = Some(rx);
        self.self_update.phase = SelfPhase::Checking;
        self.self_update.checked = Some(Instant::now());
        self.self_update.manual = manual;
        std::thread::spawn(move || {
            use stl_core::selfupdate as su;
            let phase = match su::check() {
                Ok(None) => SelfPhase::UpToDate,
                Err(e) => SelfPhase::Failed(format!("{e:#}")),
                Ok(Some(a)) => {
                    let _ = tx.send(SelfUpdateMsg::Downloading(a.release.version.clone()));
                    match su::stage(&a) {
                        Ok(s) => SelfPhase::Ready(s),
                        Err(e) => SelfPhase::Failed(format!("{e:#}")),
                    }
                }
            };
            let _ = tx.send(SelfUpdateMsg::Done(phase));
        });
    }

    fn drain_self_update(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.self_update.rx {
            loop {
                match rx.try_recv() {
                    Ok(SelfUpdateMsg::Downloading(v)) => self.self_update.phase = SelfPhase::Downloading(v),
                    Ok(SelfUpdateMsg::Done(p)) => {
                        if let SelfPhase::Failed(e) = &p {
                            self.say(format!("launcher update: {e}"));
                        }
                        self.self_update.phase = p;
                        self.self_update.rx = None;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(250));
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.self_update.rx = None;
                        break;
                    }
                }
            }
        }
        // while it runs, look again every six hours
        let due = self.self_update.checked.is_some_and(|t| t.elapsed() > Duration::from_secs(6 * 3600));
        if due && self.store.auto_update != Some(false) && matches!(self.self_update.phase, SelfPhase::UpToDate | SelfPhase::Failed(_)) {
            self.check_self_update(false);
        }
    }

    /// Puts the downloaded release in place, starts it and closes this window.
    fn install_self_update(&mut self, ctx: Option<egui::Context>) {
        let SelfPhase::Ready(s) = self.self_update.phase.clone() else { return };
        self.save();
        match stl_core::selfupdate::install(&s) {
            Ok(exe) => {
                let args: Vec<String> = std::env::args().skip(1).collect();
                match std::process::Command::new(&exe).args(&args).spawn() {
                    Ok(_) => {
                        if let Some(ctx) = ctx {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            std::process::exit(0);
                        }
                    }
                    Err(e) => self.self_update.install_error = Some(format!("{}: {e}", exe.display())),
                }
            }
            Err(e) => self.self_update.install_error = Some(format!("{e:#}")),
        }
    }

    fn drain_upload(&mut self) {
        let mut finished = None;
        let mut go = false;
        let fake = self.dev_upload_fail;
        if let Some(f) = self.upload.as_mut() {
            if let Some(rx) = &f.pre_rx {
                if let Ok(p) = rx.try_recv() {
                    f.pre_rx = None;
                    go = std::mem::take(&mut f.start_after_check) && !p.blocked();
                    if let Some(code) = fake {
                        // a pretend failure, as the upload would hand it over (Steam's log line included)
                        let item = f.pre_item;
                        let err = workshop::UploadError { step: workshop::Step::Submit, result: Some(code), status: Some(3), item, created: false, message: format!("the upload failed: EResult {code}"), steam_log: Some("Timeout uploading manifest (size 425)".into()) };
                        let (msg, err) = upload_failure(anyhow::Error::new(err).context("uploading"));
                        f.failure = err;
                        f.result = Some(Err(msg));
                        go = false;
                    }
                    f.pre = Some(p);
                }
            }
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
                    Err((e, _)) => format!("could not upload {}: {e}", f.m.name),
                };
                f.result = Some(match r {
                    Ok(o) => Ok(o),
                    Err((msg, err)) => {
                        f.failure = err;
                        Err(msg)
                    }
                });
                self.log.push(line);
                // the descriptors may carry the new item's id now
                if let Ok(g) = &self.game {
                    self.mods = mods::scan(&g.data_dir);
                }
            }
        }
        if go {
            self.upload_now();
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
                    self.confirm_delete = None;
                    self.ps_mod_filter.clear();
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
            Act::AskDelete(i) => self.confirm_delete = Some(i),
            Act::DeletePlayset(i) => {
                if i < self.store.playsets.len() {
                    let n = self.store.playsets[i].name.clone();
                    if self.store.remove_playset(i).is_ok() {
                        self.save();
                        self.say(format!("deleted the playset {n}"));
                    }
                }
                self.confirm_delete = None;
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
            Act::ModMoveTo(from, at) => {
                let mods = &mut self.store.playsets[active].mods;
                if from < mods.len() && at <= mods.len() {
                    let to = if at > from { at - 1 } else { at };
                    if to != from {
                        let m = mods.remove(from);
                        mods.insert(to, m);
                        self.save();
                    }
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
                self.gfx = None;
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
            Act::CheckSelfUpdate => self.check_self_update(true),
            Act::RunCheck => self.run_check(),
            Act::OpenModCheck(pos) => {
                if let Some(r) = self.check.result.clone() {
                    self.check.expanded.clear();
                    self.check.sheet = CheckSheet::Mod(pos, std::sync::Arc::new(Self::overlap_groups(&r, pos)));
                }
            }
            Act::ApplySort => {
                if let Some(r) = self.check.result.clone() {
                    let ids: Vec<String> = r.plan.order.iter().filter_map(|&i| r.ids.get(i).cloned()).collect();
                    let active = self.store.active_index();
                    stl_core::conflicts::apply_order(&mut self.store.playsets[active], &ids);
                    self.save();
                    self.check.sheet = CheckSheet::None;
                    self.run_check();
                }
            }
            Act::InstallSelfUpdate => self.install_self_update(ctx_of_acts()),
            Act::SetAutoUpdate(on) => {
                self.store.auto_update = Some(on);
                self.save();
                if on && matches!(self.self_update.phase, SelfPhase::Idle | SelfPhase::Failed(_)) {
                    self.check_self_update(false);
                }
            }
            Act::FoldNews(f) => {
                self.store.news_folded = Some(f);
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
                    // an upload that did not finish had made an item: its number is used, so the next try updates it
                    let remembered = if existing { None } else { uploadcheck::journal_pending(&m.id).and_then(|j| j.item) };
                    self.upload = Some(UploadForm {
                        m: m.clone(),
                        visibility: if existing { 0 } else { 1 },
                        note: String::new(),
                        item_id: remembered.map(|i| i.to_string()).unwrap_or_default(),
                        rx: None,
                        stage: None,
                        result: None,
                        pre_rx: None,
                        pre: None,
                        pre_item: None,
                        start_after_check: false,
                        failure: None,
                        sent: None,
                        copied_at: -10.0,
                    });
                    self.pick_upload = false;
                    self.check_upload();
                }
            }
            Act::StartUpload => self.start_upload(),
            Act::CheckUpload => self.check_upload(),
            Act::RescanMods => {
                if let Ok(g) = &self.game {
                    self.mods = mods::scan(&g.data_dir);
                }
                self.mod_times.clear();
                self.mod_covers.clear();
                self.ironman.clear();
                self.ironman_rx = None;
            }
            Act::CheckUpdates => self.check_updates(),
            Act::SetGraphics(g) => {
                if let Ok(game) = &self.game {
                    match gamesettings::write(&game.data_dir, &g) {
                        Ok(()) => {
                            self.gfx = Some(g);
                            // our own write: not a change to read back
                            self.gfx_stamp = gamesettings::stamp(&game.data_dir);
                            self.gfx_status = Some((tr(self.lang, "gfx.saved").to_string(), true));
                        }
                        Err(e) => self.gfx_status = Some((format!("{e:#}"), false)),
                    }
                }
            }
            Act::UpdatePlugin(id) => self.update_plugin(&id),
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
            Act::SetModsFilter(f) => self.mods_filter = f,
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
        let news_rect = Rect::from_min_max(pos2(left_rect.left(), left_rect.bottom() - news_card_height(ui) - 40.0), left_rect.max);
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
        let height = news_card_height(ui);
        let gap = 16.0;
        // the fold handle stands at the end of the row
        let handle_w = 26.0;
        let width = ui.available_width() - handle_w - gap;

        // every card once (a slot lists some several times to show them more often), the main slot first; a page at a time
        let mut seen = std::collections::HashSet::new();
        let order: Vec<usize> = (0..cards.len()).filter(|&i| seen.insert((cards[i].image_url.clone(), cards[i].link.clone()))).collect();
        let folded = self.store.news_folded == Some(true);
        // 1 = open, 0 = folded away to the left
        let open_t = ui.ctx().animate_bool_with_time(egui::Id::new("news-open"), !folded, 0.32);
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
            // folded, the title row keeps its place (unseen), so that the cards do not jump
            if folded && open_t < 0.01 {
                ui.set_invisible();
            }
            ui.set_opacity(open_t);
            ui.label(RichText::new(tr(lang, "play.news").to_uppercase()).size(12.0).color(SECONDARY));
            {
                ui.add_enabled_ui(open_t > 0.5, |ui| {
                    if circle_button(ui, Icon::Refresh, theme::white(22), SECONDARY, !loading).clicked() {
                        acts.push(Act::RefreshNews);
                    }
                });
            }
            if page_count > 1 && !folded {
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
            let (area, _) = ui.allocate_exact_size(vec2(width + gap + handle_w, height), Sense::hover());
            if page_count > 1 && open_t > 0.99 && ui.rect_contains_pointer(area) {
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
            // folding slides the row behind the window's left edge, leaving the end of its last card in view (a hint that there is more)
            let row_w: f32 = sizes[from..to].iter().map(|s| s.x).sum::<f32>() + gap * (to - from).saturating_sub(1) as f32;
            let peek = 30.0;
            let ease = { let t = open_t; if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 } };
            let shift = (1.0 - ease) * (peek - row_w);
            let screen = ui.ctx().screen_rect();
            let mut row = ui.new_child(UiBuilder::new().id_salt("news-row").max_rect(area));
            row.set_clip_rect(Rect::from_min_max(pos2(screen.left(), area.top() - 4.0), pos2(area.right() + 4.0, area.bottom() + 4.0)));
            row.set_opacity(0.55 + 0.45 * ease);
            let mut x = 0.0;
            for k in from..to {
                let r = Rect::from_min_size(area.min + vec2(x + shift, 0.0), sizes[k]);
                x += sizes[k].x + gap;
                self.draw_card(&mut row, &cards[order[k]], r, theme::RADIUS, &format!("card{k}"), &acts);
            }
            // the handle: a tall click area right after the cards, its chevron pointing where the row will go
            let hx = area.left() + row_w + shift + gap * (0.5 + 0.5 * ease);
            let hh = (height * 0.62).max(64.0);
            let handle = Rect::from_min_size(pos2(hx, area.center().y - hh / 2.0), vec2(handle_w, hh));
            let resp = ui.interact(handle, egui::Id::new("news-fold"), Sense::click());
            // folded, the peeking card also opens the row again
            let peek_rect = Rect::from_min_max(pos2(screen.left(), area.top()), pos2(handle.left(), area.bottom()));
            let peek_resp = if folded { Some(ui.interact(peek_rect, egui::Id::new("news-peek"), Sense::click())) } else { None };
            let hot = resp.hovered() || peek_resp.as_ref().is_some_and(|r| r.hovered());
            let lit = ui.ctx().animate_bool_with_time(resp.id.with("lit"), hot, 0.15);
            // only the chevron: quiet until the pointer comes near, then a faint fill shows where to click
            ui.painter().rect_filled(handle, egui::CornerRadius::same((handle_w / 2.0) as u8), theme::white((14.0 * lit) as u8));
            let icon = if folded { Icon::Right } else { Icon::Left };
            icon.draw(ui.painter(), handle.center(), 18.0, if hot { LABEL } else { SECONDARY }, 2.0);
            let hint = if folded { tr(lang, "play.news_show") } else { tr(lang, "play.news_hide") };
            let clicked = resp.on_hover_text(hint).clicked() || peek_resp.is_some_and(|r| r.on_hover_cursor(CursorIcon::PointingHand).clicked());
            if clicked {
                acts.push(Act::FoldNews(!folded));
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
            let (above, room) = if room_below >= room_above { (false, room_below) } else { (true, room_above) };
            let list_h = (room - 90.0).clamp(120.0, 300.0);
            theme::menu_on(ui, popup_id, &trigger, trigger.rect.width(), above, |ui| {
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
            let can_delete = self.store.playsets.len() > 1;
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.confirm_delete = None;
            }
            let confirming = self.confirm_delete;
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
                    let sure = confirming == Some(i);
                    let r = rows.row(ui, 58.0, if can_delete { if sure { 64.0 } else { 30.0 } } else { 0.0 }, true, |ui| {
                        stack(ui, 58.0, 38.0, |ui| {
                            ui.add(egui::Label::new(RichText::new(&p.name).size(15.5).family(bold())).truncate());
                            ui.label(RichText::new(tr_args(lang, "ps.counts", &[&mods_on, &plugins_on])).size(12.0).color(SECONDARY));
                        });
                    }, |ui| {
                        // quiet until the pointer is on it; the first click asks, the second deletes
                        let hint = if sure { tr(lang, "ps.delete_sure_hint") } else { tr(lang, "ps.delete") };
                        if theme::delete_button(ui, sure, tr(lang, "ps.delete_short")).on_hover_text(hint).clicked() {
                            if sure {
                                acts.push(Act::DeletePlayset(i));
                            } else {
                                acts.push(Act::AskDelete(i));
                            }
                        }
                    });
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
            // the search over this playset's mods (on the Mods segment)
            if self.seg == 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let w = ui.available_width().clamp(140.0, 260.0);
                    theme::search_field(ui, &mut self.ps_mod_filter, tr(lang, "ps.search_mods"), w);
                });
            }
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
        // the check: a button, its progress, then what it found (counts that open the list, and the order it suggests)
        let current = self.current_check().cloned();
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if self.check.rx.is_some() {
                let p = f32::from_bits(self.check.progress.load(std::sync::atomic::Ordering::Relaxed));
                ui.label(RichText::new(tr_args(lang, "chk.running", &[&format!("{:.0}", p * 100.0)])).size(12.5).color(SECONDARY));
                let time = ui.input(|i| i.time);
                ui.allocate_ui(vec2(140.0, 8.0), |ui| theme::progress_bar(ui, Some(p), time));
            } else if let Some(r) = &current {
                use stl_core::conflicts::Severity;
                let errors = r.report.issues.iter().filter(|i| i.severity == Severity::Error).count();
                let warnings = r.report.issues.iter().filter(|i| i.severity == Severity::Warning).count();
                if errors + warnings == 0 {
                    chip(ui, tr(lang, "chk.ok"), GREEN);
                }
                if errors > 0 && theme::chip_button(ui, &tr_args(lang, "chk.errors", &[&errors.to_string()]), RED).clicked() {
                    self.check.sheet = CheckSheet::All;
                }
                if warnings > 0 && theme::chip_button(ui, &tr_args(lang, "chk.warnings", &[&warnings.to_string()]), ORANGE).clicked() {
                    self.check.sheet = CheckSheet::All;
                }
                let overlaps = r.report.files.iter().filter(|f| !f.intended).count() + r.report.keys.iter().filter(|k| k.severity > Severity::Info).count();
                chip(ui, &tr_args(lang, "chk.overlaps", &[&overlaps.to_string()]), SECONDARY).on_hover_text(tr(lang, "chk.overlaps_hint"));
                // the same size as the counts beside it: an action, but a quiet one (blue)
                if r.plan.changes() && theme::chip_button(ui, tr(lang, "chk.sort"), BLUE).clicked() {
                    self.check.sheet = CheckSheet::Sort;
                }
            } else {
                ui.label(RichText::new(tr(lang, "ps.order_hint")).size(12.5).color(SECONDARY));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.check.result.is_some() && current.is_none() { tr(lang, "chk.stale") } else { tr(lang, "chk.run") };
                if pill_button(ui, label, ButtonStyle::Tinted(BLUE), self.check.rx.is_none() && self.game.is_ok()).on_hover_text(tr(lang, "chk.run_hint")).clicked() {
                    acts.push(Act::RunCheck);
                }
            });
        });
        ui.add_space(6.0);
        let position: std::collections::HashMap<String, usize> = current.as_ref().map(|r| r.ids.iter().enumerate().map(|(i, id)| (id.clone(), i)).collect()).unwrap_or_default();
        let q = self.ps_mod_filter.trim().to_lowercase();
        let shown: Vec<usize> = (0..total)
            .filter(|&i| {
                if q.is_empty() {
                    return true;
                }
                let m = &self.store.playsets[active].mods[i];
                let name = self.mods.iter().find(|x| x.id == m.id).map(|x| x.name.to_lowercase()).unwrap_or_default();
                name.contains(&q) || m.id.to_lowercase().contains(&q)
            })
            .collect();
        if shown.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr(lang, "ps.search_none")).size(14.0).color(SECONDARY));
            });
            return;
        }
        // dragging a row: where it would go (a place in the whole list), and the drag that starts now
        let pointer = ui.ctx().pointer_latest_pos();
        let dragging = self.ps_drag.clone();
        let shift = ui.input(|i| i.modifiers.shift);
        let mut drop_at: Option<usize> = None;
        let mut drag_start: Option<String> = None;
        plain_rows(ui, "ps-mods", 58.0, shown.len(), |ui, range| {
            let mut rows = Rows::starting_at(range.start);
            let drop_layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("ps-drop"))).with_clip_rect(ui.clip_rect());
            let mut last: Option<(usize, Rect)> = None;
            for i in range.map(|k| shown[k]) {
                let m = &self.store.playsets[active].mods[i];
                let info = self.mods.iter().find(|x| x.id == m.id);
                let name = info.map(|x| x.name.clone()).unwrap_or_else(|| m.id.clone());
                let problem = match info {
                    Some(x) => x.problem.clone(),
                    None => Some(tr(lang, "mods.unusable").to_string()),
                };
                let enabled = m.enabled;
                // what the check found about this mod: its worst problem and how much it overrides / is overridden
                let found = current.as_ref().zip(position.get(&m.id)).map(|(r, &pos)| {
                    use stl_core::conflicts::Severity;
                    let mine: Vec<&stl_core::conflicts::Issue> = r.report.issues.iter().filter(|x| x.mod_index == Some(pos) && x.severity > Severity::Info).collect();
                    let worst = mine.iter().map(|x| x.severity).max();
                    let s = &r.report.per_mod[pos];
                    (pos, mine.len(), worst == Some(Severity::Error), s.wins, s.loses)
                });
                let row = rows.row_with(ui, 58.0, 146.0, Some(egui::Id::new(("ps-row", &m.id))), Sense::click_and_drag(), |ui| {
                    let mut on = enabled;
                    if theme::switch_keyed(ui, ("ps-mod", &m.id), &mut on).changed() {
                        acts.push(Act::ModFlag(i, on));
                    }
                    ui.label(RichText::new(format!("{:>3}", i + 1)).size(12.0).color(theme::TERTIARY).monospace());
                    stack(ui, 58.0, 42.0, |ui| {
                        let color = if problem.is_some() { RED } else if enabled { LABEL } else { SECONDARY };
                        ui.add(egui::Label::new(RichText::new(&name).size(15.0).color(color)).truncate());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            self.mod_sub_line(ui, info, problem.as_deref(), &version);
                            if let Some((_, n, error, wins, loses)) = found {
                                if n > 0 {
                                    chip(ui, &tr_args(lang, "chk.mod_issues", &[&n.to_string()]), if error { RED } else { ORANGE });
                                }
                                if wins + loses > 0 {
                                    chip(ui, &tr_args(lang, "chk.wins_loses", &[&wins.to_string(), &loses.to_string()]), SECONDARY);
                                }
                            }
                        });
                    });
                }, |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some((pos, n, _, wins, loses)) = found {
                        if n + wins + loses > 0 && circle_button(ui, Icon::Info, theme::white(24), LABEL, true).on_hover_text(tr(lang, "chk.details")).clicked() {
                            acts.push(Act::OpenModCheck(pos));
                        }
                    }
                    if circle_button(ui, Icon::Close, theme::white(24), SECONDARY, true).on_hover_text(tr(lang, "mods.remove")).clicked() {
                        acts.push(Act::ModRemove(i));
                    }
                    // Shift: all the way
                    if circle_button(ui, Icon::Down, theme::white(24), LABEL, i + 1 < total).on_hover_text(tr(lang, "ps.down_hint")).clicked() {
                        acts.push(if shift { Act::ModMoveTo(i, total) } else { Act::ModMove(i, 1) });
                    }
                    if circle_button(ui, Icon::Up, theme::white(24), LABEL, i > 0).on_hover_text(tr(lang, "ps.up_hint")).clicked() {
                        acts.push(if shift { Act::ModMoveTo(i, 0) } else { Act::ModMove(i, -1) });
                    }
                });
                // the row is dragged anywhere outside its buttons; right-click offers the top and the bottom
                if row.drag_started() {
                    drag_start = Some(m.id.clone());
                }
                if dragging.is_none() && row.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab);
                }
                row.context_menu(|ui| {
                    ui.set_width(180.0);
                    if theme::menu_item(ui, tr(lang, "ps.to_top"), false).clicked() {
                        acts.push(Act::ModMoveTo(i, 0));
                        ui.close_menu();
                    }
                    if theme::menu_item(ui, tr(lang, "ps.to_bottom"), false).clicked() {
                        acts.push(Act::ModMoveTo(i, total));
                        ui.close_menu();
                    }
                    if theme::menu_item(ui, tr(lang, "mods.remove"), false).clicked() {
                        acts.push(Act::ModRemove(i));
                        ui.close_menu();
                    }
                });
                if let (Some(id), Some(p)) = (&dragging, pointer) {
                    if *id == m.id {
                        ui.painter().rect_filled(row.rect.shrink2(vec2(6.0, 2.0)), egui::CornerRadius::same(10), BLUE.gamma_multiply(0.22));
                    }
                    if row.rect.y_range().contains(p.y) {
                        let before = p.y < row.rect.center().y;
                        drop_at = Some(if before { i } else { i + 1 });
                        let y = if before { row.rect.top() } else { row.rect.bottom() };
                        drop_layer.hline(egui::Rangef::new(row.rect.left() + 10.0, row.rect.right() - 10.0), y, egui::Stroke::new(2.5, BLUE));
                    }
                }
                last = Some((i, row.rect));
            }
            if let (Some(_), Some(p)) = (&dragging, pointer) {
                // under the last row: to the end
                if let Some((i, r)) = last {
                    if drop_at.is_none() && p.y > r.bottom() {
                        drop_at = Some(i + 1);
                        drop_layer.hline(egui::Rangef::new(r.left() + 10.0, r.right() - 10.0), r.bottom(), egui::Stroke::new(2.5, BLUE));
                    }
                }
                // near the top or the bottom of the list: it scrolls
                let clip = ui.clip_rect();
                let edge = 40.0;
                if p.y < clip.top() + edge {
                    ui.scroll_with_delta(vec2(0.0, 12.0 * (1.0 - (p.y - clip.top()).max(0.0) / edge)));
                } else if p.y > clip.bottom() - edge {
                    ui.scroll_with_delta(vec2(0.0, -12.0 * (1.0 - (clip.bottom() - p.y).max(0.0) / edge)));
                }
                ui.ctx().request_repaint();
            }
        });
        if drag_start.is_some() {
            self.ps_drag = drag_start;
        }
        if let Some(id) = self.ps_drag.clone() {
            let (released, escape) = ui.input(|i| (i.pointer.any_released() || !i.pointer.any_down(), i.key_pressed(egui::Key::Escape)));
            if escape {
                self.ps_drag = None;
            } else if released {
                self.ps_drag = None;
                let from = self.store.playsets[active].mods.iter().position(|x| x.id == id);
                if let (Some(from), Some(at)) = (from, drop_at) {
                    acts.push(Act::ModMoveTo(from, at));
                }
            } else if let Some(p) = pointer {
                // the mod follows the pointer
                let name = self.mods.iter().find(|x| x.id == id).map(|x| x.name.clone()).unwrap_or(id);
                let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("ps-drag-ghost")));
                let galley = painter.layout_no_wrap(name, egui::FontId::new(14.0, egui::FontFamily::Proportional), LABEL);
                let rect = Rect::from_min_size(p + vec2(16.0, -galley.size().y / 2.0 - 9.0), galley.size() + vec2(28.0, 18.0));
                painter.add(theme::lift(rect, 10.0));
                painter.rect_filled(rect, egui::CornerRadius::same(10), Color32::from_rgb(52, 52, 56));
                painter.rect_stroke(rect, egui::CornerRadius::same(10), egui::Stroke::new(1.0, BLUE), egui::StrokeKind::Inside);
                painter.galley(rect.min + vec2(14.0, 9.0), galley, LABEL);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
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
                    if theme::switch_keyed(ui, ("ps-dlc", &d.id), &mut v).changed() {
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
                    if theme::switch_keyed(ui, ("ps-plugin", &p.manifest.id), &mut v).changed() {
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
        self.start_ironman_scan();
        let filter = self.filter.to_lowercase();
        let want = self.mods_filter;
        let mut shown: Vec<usize> = (0..self.mods.len())
            .filter(|&i| filter.is_empty() || self.mods[i].name.to_lowercase().contains(&filter))
            .filter(|&i| match want {
                1 => self.ironman.get(&self.mods[i].id) == Some(&Some(false)),
                2 => self.ironman.get(&self.mods[i].id) == Some(&Some(true)),
                _ => true,
            })
            .collect();
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
                if let Some(i) = segmented(ui, &labels, cur, 240.0) {
                    acts.push(Act::SetModsSort(sorts[i].0));
                }
                // the Ironman filter: a quiet drop-down (text and a chevron, no frame)
                let filters = ["mods.filter_all", "mods.filter_ironman", "mods.filter_not"];
                let popup = egui::Id::new("mods-ironman-filter");
                let open = ui.memory(|m| m.is_popup_open(popup));
                let r = theme::text_dropdown(ui, tr(lang, filters[want.min(2)]), open);
                if r.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup));
                }
                theme::menu(ui, popup, &r, 180.0, |ui| {
                    for (i, k) in filters.iter().enumerate() {
                        if theme::menu_item(ui, tr(lang, k), i == want).clicked() {
                            acts.push(Act::SetModsFilter(i));
                        }
                    }
                });
                // at the right: Upload, the search, then the small tools (refresh, and the two views as icons)
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if pill_button(ui, tr(lang, "mods.upload_btn"), ButtonStyle::Tinted(Color32::WHITE), self.game.is_ok()).clicked() {
                        self.pick_upload = true;
                    }
                    let w = (ui.available_width() - 120.0).clamp(140.0, 260.0);
                    theme::search_field(ui, &mut self.filter, tr(lang, "mods.search"), w);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if theme::icon_toggle(ui, Icon::Refresh, false).on_hover_text(tr(lang, "mods.refresh")).clicked() {
                        acts.push(Act::RescanMods);
                    }
                    ui.add_space(6.0);
                    if theme::icon_toggle(ui, Icon::ViewCompact, compact).on_hover_text(tr(lang, "mods.view_compact")).clicked() {
                        acts.push(Act::SetModsView("compact"));
                    }
                    if theme::icon_toggle(ui, Icon::ViewList, !compact).on_hover_text(tr(lang, "mods.view_list")).clicked() {
                        acts.push(Act::SetModsView("list"));
                    }
                });
            });
            theme::divider(ui);
            if self.ironman_rx.is_some() && want != 0 {
                let done = self.ironman.len().to_string();
                let all = self.mods.len().to_string();
                ui.label(RichText::new(tr_args(lang, "mods.checking", &[&done, &all])).size(12.5).color(SECONDARY));
                ui.add_space(6.0);
            }
            let row_h = if compact { 40.0 } else { 64.0 };
            plain_rows(ui, "mods-library", row_h, total, |ui, range| {
                let mut rows = Rows::starting_at(range.start);
                for k in range {
                    let m = &self.mods[shown[k]];
                    let inside = self.store.playsets[active].mod_pos(&m.id).is_some();
                    let own = m.kind == Kind::Local && m.path.is_some() && m.problem.is_none();
                    let iron = self.ironman.get(&m.id).cloned();
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
                        match iron {
                            Some(Some(false)) => {
                                chip(ui, tr(lang, "mods.ironman_ok"), theme::TEAL_TEXT).on_hover_text(tr(lang, "mods.ironman_ok_hint"));
                            }
                            Some(Some(true)) => {
                                chip(ui, tr(lang, "mods.ironman_no"), SECONDARY).on_hover_text(tr(lang, "mods.ironman_no_hint"));
                            }
                            _ => {}
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
                let checking = self.updates.checking.is_some();
                if pill_button(ui, if checking { tr(lang, "pl.checking") } else { tr(lang, "pl.check") }, ButtonStyle::Plain(BLUE), !checking).clicked() {
                    self.updates.found.clear();
                    acts.push(Act::CheckUpdates);
                }
                if let Some((msg, ok)) = &self.updates.message {
                    ui.label(RichText::new(msg).size(12.5).color(if *ok { GREEN } else { RED }));
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
                    let page = p.manifest.homepage.clone().filter(|h| h.starts_with("http")).or_else(|| p.manifest.update.as_ref().and_then(|u| stl_core::updates::repo_of(&u.github)).map(|r| format!("https://github.com/{r}")));
                    let update_ready = matches!(self.updates.found.get(&p.manifest.id), Some(Ok(Some(_)))) && !p.linked;
                    let installing = self.updates.installing.as_ref().is_some_and(|(i, _)| i == &p.manifest.id);
                    rows.row(ui, h, 300.0, false, |ui| {
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
                                match self.updates.found.get(&p.manifest.id) {
                                    Some(Ok(Some(a))) => {
                                        chip(ui, &tr_args(lang, "pl.update_available", &[&a.release.version]), BLUE);
                                    }
                                    Some(Ok(None)) => {
                                        chip(ui, tr(lang, "pl.up_to_date"), SECONDARY);
                                    }
                                    Some(Err(e)) => {
                                        chip(ui, tr(lang, "pl.check_failed"), ORANGE).on_hover_text(e);
                                    }
                                    None => {}
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
                        if let Some(url) = &page {
                            if circle_button(ui, Icon::Globe, theme::white(26), LABEL, true).on_hover_text(url).clicked() {
                                acts.push(Act::Open(url.clone()));
                            }
                        }
                        if update_ready || installing {
                            ui.add_space(4.0);
                            let label = if installing { tr(lang, "pl.updating") } else { tr(lang, "pl.update") };
                            if pill_button(ui, label, ButtonStyle::Filled(BLUE), !installing).clicked() {
                                acts.push(Act::UpdatePlugin(p.manifest.id.clone()));
                            }
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
        ui.add_space(8.0);
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
            // graphics
            self.graphics_section(ui, &acts);
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
                // read the game, the mods, the plugins and the news again (the button the page title used to carry)
                rows.row(ui, 48.0, 140.0, false, |ui| { ui.label(RichText::new(tr(lang, "set.reload_hint")).color(SECONDARY)); }, |ui| {
                    if pill_button(ui, tr(lang, "set.reload"), ButtonStyle::Tinted(BLUE), true).clicked() {
                        acts.push(Act::Reload);
                    }
                });
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
            // the launcher's own updates
            theme::section(ui, tr(lang, "upd.section"));
            glass(ui, 16.0, 0.0, |ui| {
                let mut rows = Rows::new();
                rows.row(ui, 48.0, 46.0, false, |ui| { ui.label(tr(lang, "upd.auto")); }, |ui| {
                    let mut on = self.store.auto_update != Some(false);
                    if switch(ui, &mut on).changed() {
                        acts.push(Act::SetAutoUpdate(on));
                    }
                });
                let status = match &self.self_update.phase {
                    SelfPhase::Idle => tr_args(lang, "upd.version", &[stl_core::selfupdate::current_version()]),
                    SelfPhase::Checking => tr(lang, "upd.checking").to_string(),
                    SelfPhase::Downloading(v) => tr_args(lang, "upd.downloading", &[v]),
                    SelfPhase::Ready(s) => tr_args(lang, "upd.ready", &[&s.version]),
                    SelfPhase::UpToDate => tr_args(lang, "upd.up_to_date", &[stl_core::selfupdate::current_version()]),
                    SelfPhase::Failed(e) => format!("{} {e}", tr(lang, "upd.failed")),
                };
                let failed = matches!(self.self_update.phase, SelfPhase::Failed(_));
                let ready = matches!(self.self_update.phase, SelfPhase::Ready(_));
                let busy = self.self_update.rx.is_some();
                rows.row(ui, 52.0, 150.0, false, |ui| {
                    ui.add(egui::Label::new(RichText::new(status).size(13.5).color(if failed { RED } else { SECONDARY })).wrap());
                }, |ui| {
                    if ready {
                        if pill_button(ui, tr(lang, "upd.install"), ButtonStyle::Filled(BLUE), true).clicked() {
                            self.self_update.install_error = None;
                            self.self_update.sheet = true;
                        }
                    } else if pill_button(ui, tr(lang, "upd.check"), ButtonStyle::Tinted(BLUE), !busy).clicked() {
                        acts.push(Act::CheckSelfUpdate);
                    }
                });
            });
            theme::footnote(ui, &tr_args(lang, "upd.note", &[&stl_core::selfupdate::repo()]));
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

struct SlowFrame(Instant, Page);

impl Drop for SlowFrame {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_secs_f64() * 1000.0;
        if ms > 30.0 && std::env::var_os("STL_FRAME_LOG").is_some() {
            eprintln!("slow frame: {ms:.0} ms on {:?}", self.1 as usize);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        set_ctx_of_acts(ctx);
        // development: STL_FRAME_LOG=1 prints the frames that took long (with the page on show)
        let frame_start = Instant::now();
        let _slow = SlowFrame(frame_start, self.page);
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
        self.drain_self_update(ctx);
        self.drain_check(ctx);
        self.drain_upload();
        self.drain_updates();
        self.drain_ironman();
        if self.ironman_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if self.updates.checking.is_some() || self.updates.installing.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // the plugins are checked for updates once, the first time their page is shown
        if self.page == Page::Plugins && self.updates.found.is_empty() && self.updates.checking.is_none() && self.store.news_online != Some(false) {
            self.check_updates();
        }
        if self.upload.as_ref().is_some_and(|f| f.rx.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if self.launching.is_some() || !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if let Some(want) = self.dev_check.clone() {
            if self.check.result.is_none() && self.check.rx.is_none() {
                self.run_check();
            } else if self.check.result.is_some() {
                self.dev_check = None;
                match want.as_str() {
                    "all" => self.check.sheet = CheckSheet::All,
                    "sort" => self.check.sheet = CheckSheet::Sort,
                    n => {
                        if let Ok(pos) = n.parse::<usize>() {
                            self.acts.push(Act::OpenModCheck(pos));
                        }
                    }
                }
            }
        }
        if let Some(id) = self.dev_popup.take() {
            ctx.memory_mut(|m| m.open_popup(egui::Id::new(id.as_str())));
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
        self.self_update_sheet(ctx);
        self.check_sheet(ctx);
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
                let name = e.files.get(e.index).and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let syntax = editor::syntax_of(&name);
                let issues = editor::lint(&e.text, syntax, &editor::LintWords {
                    json: tr(lang, "cfg.lint_json").into(),
                    section: tr(lang, "cfg.lint_section").into(),
                    no_equals: tr(lang, "cfg.lint_no_equals").into(),
                    no_key: tr(lang, "cfg.lint_no_key").into(),
                    duplicate: tr(lang, "cfg.lint_duplicate").into(),
                });
                let h = (ctx.screen_rect().height() - 420.0).clamp(160.0, 500.0);
                let font = egui::FontId::monospace(13.5);
                let mut layouter = |ui: &Ui, text: &str, wrap: f32| {
                    let job = editor::highlight(text, syntax, &font, wrap);
                    ui.fonts(|f| f.layout_job(job))
                };
                egui::Frame::new().fill(Color32::from_black_alpha(90)).corner_radius(12).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("config-text").max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut e.text).code_editor().frame(false).desired_width(f32::INFINITY).desired_rows(12).layouter(&mut layouter));
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let kind = match syntax {
                        editor::Syntax::Ini => "INI",
                        editor::Syntax::Json => "JSON",
                        editor::Syntax::Plain => "Text",
                    };
                    let lines = e.text.lines().count().to_string();
                    ui.label(RichText::new(format!("{kind}  ·  {}  ·  {}  ·  {}", e.encoding.label(), if e.crlf { "CRLF" } else { "LF" }, tr_args(lang, "cfg.lines", &[&lines]))).size(12.0).color(SECONDARY));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if syntax != editor::Syntax::Plain {
                            if issues.is_empty() {
                                ui.label(RichText::new(format!("✓  {}", tr(lang, "cfg.lint_ok"))).size(12.0).color(GREEN));
                            } else {
                                ui.label(RichText::new(tr_args(lang, "cfg.lint_count", &[&issues.len().to_string()])).size(12.0).color(ORANGE));
                            }
                        }
                    });
                });
                for (line, msg) in issues.iter().take(4) {
                    ui.label(RichText::new(format!("{}  {msg}", tr_args(lang, "cfg.line", &[&line.to_string()]))).size(12.0).color(ORANGE));
                }
                ui.add_space(4.0);
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
                        if let Ok(t) = std::fs::read(&d).map(|b| stl_core::textenc::decode(&b).text) {
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
                        match e.save() {
                            Ok(()) => e.status = Some((tr(lang, "cfg.saved").to_string(), true)),
                            Err(err) => e.status = Some((err, false)),
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

    /// One problem in the window's words.
    fn issue_text(lang: Lang, i: &stl_core::conflicts::Issue) -> String {
        use stl_core::conflicts::IssueKind as K;
        let key = match i.kind {
            K::MissingMod => "iss.missing_mod",
            K::Unloadable => "iss.unloadable",
            K::MissingDependency => "iss.missing_dep",
            K::DisabledDependency => "iss.disabled_dep",
            K::DependencyAfter => "iss.dep_after",
            K::Duplicate => "iss.duplicate",
            K::OutdatedReplacesVanilla => "iss.outdated",
            K::BomFirstKey => "iss.bom",
            K::IneffectiveOverride => "iss.ineffective",
            K::DuplicateDefinitions => "iss.dupl_defs",
            K::PatchBeforeTarget => "iss.patch_before",
        };
        let args: Vec<&str> = i.args.iter().map(|s| s.as_str()).collect();
        tr_args(lang, key, &args)
    }

    /// The check's sheets: every problem; one mod's overlaps; the suggested order.
    fn check_sheet(&mut self, ctx: &egui::Context) {
        let sheet = self.check.sheet.clone();
        if matches!(sheet, CheckSheet::None) {
            return;
        }
        let Some(r) = self.check.result.clone() else {
            self.check.sheet = CheckSheet::None;
            return;
        };
        let lang = self.lang;
        let mut close = false;
        let mut next: Option<CheckSheet> = None;
        let resp = egui::Modal::new(egui::Id::new("check-sheet")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(640.0);
            let name = |s: usize| r.report.sources.get(s).cloned().unwrap_or_default();
            match &sheet {
                CheckSheet::All => {
                    ui.label(RichText::new(tr(lang, "chk.all_title")).size(22.0).family(bold()));
                    let secs = format!("{:.1}", r.report.millis as f64 / 1000.0);
                    ui.label(RichText::new(tr_args(lang, "chk.done", &[&r.report.files_scanned.to_string(), &r.report.definitions_read.to_string(), &secs])).size(12.5).color(SECONDARY));
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut rows = Rows::new();
                        for i in &r.report.issues {
                            use stl_core::conflicts::Severity;
                            let color = match i.severity { Severity::Error => RED, Severity::Warning => ORANGE, Severity::Info => SECONDARY };
                            let who = i.mod_index.map(|m| name(m + 1)).unwrap_or_else(|| tr(lang, "chk.playset").to_string());
                            let resp = rows.row(ui, 50.0, 30.0, i.mod_index.is_some(), |ui| {
                                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, color);
                                stack(ui, 50.0, 38.0, |ui| {
                                    ui.add(egui::Label::new(RichText::new(&who).size(14.0).family(bold())).truncate());
                                    ui.add(egui::Label::new(RichText::new(Self::issue_text(lang, i)).size(12.5).color(SECONDARY)).truncate());
                                });
                            }, |_| {});
                            if let (true, Some(m)) = (resp.clicked(), i.mod_index) {
                                next = Some(CheckSheet::Mod(m, std::sync::Arc::new(Self::overlap_groups(&r, m))));
                            }
                        }
                    });
                }
                CheckSheet::Mod(m, groups) => {
                    let s = &r.report.per_mod[*m];
                    ui.label(RichText::new(name(m + 1)).size(22.0).family(bold()));
                    ui.label(RichText::new(tr_args(lang, "chk.mod_sub", &[&s.wins.to_string(), &s.loses.to_string(), &s.replaces_vanilla_files.to_string(), &s.overrides_vanilla_keys.to_string()])).size(12.5).color(SECONDARY));
                    if s.fallbacks > 0 {
                        ui.label(RichText::new(tr_args(lang, "chk.fallbacks", &[&s.fallbacks.to_string()])).size(12.5).color(SECONDARY));
                    }
                    if !s.patches.is_empty() {
                        let names: Vec<String> = s.patches.iter().map(|&t| name(t + 1)).collect();
                        ui.label(RichText::new(tr_args(lang, "chk.patches", &[&names.join(", ")])).size(12.5).color(theme::TEAL_TEXT));
                    }
                    ui.add_space(8.0);
                    for i in r.report.issues.iter().filter(|i| i.mod_index == Some(*m)) {
                        use stl_core::conflicts::Severity;
                        let color = if i.severity == Severity::Error { RED } else { ORANGE };
                        ui.label(RichText::new(format!("●  {}", Self::issue_text(lang, i))).size(13.0).color(color));
                    }
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                        for (gi, g) in groups.iter().enumerate() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(tr_args(lang, "chk.with", &[&name(g.other)])).size(14.5).family(bold()));
                                ui.label(RichText::new(tr_args(lang, "chk.group", &[&g.mine.to_string(), &g.theirs.to_string()])).size(12.5).color(SECONDARY));
                            });
                            let all = self.check.expanded.contains(&gi);
                            let shown = if all { g.items.len() } else { g.items.len().min(6) };
                            for (what, wins, how) in &g.items[..shown] {
                                ui.horizontal(|ui| {
                                    ui.add_space(10.0);
                                    ui.add(egui::Label::new(RichText::new(what).size(12.5).monospace()).truncate());
                                });
                                ui.horizontal(|ui| {
                                    ui.add_space(24.0);
                                    ui.label(RichText::new(format!("{} · {}", tr_args(lang, "chk.wins", &[wins]), tr(lang, how))).size(11.5).color(SECONDARY));
                                });
                            }
                            if g.items.len() > shown && ui.add(egui::Label::new(RichText::new(tr_args(lang, "chk.show_all", &[&g.items.len().to_string()])).size(12.5).color(BLUE)).sense(Sense::click())).clicked() {
                                self.check.expanded.insert(gi);
                            }
                        }
                        if groups.is_empty() {
                            ui.label(RichText::new(tr(lang, "chk.no_overlaps")).size(13.0).color(SECONDARY));
                        }
                    });
                }
                CheckSheet::Sort => {
                    ui.label(RichText::new(tr(lang, "chk.sort_title")).size(22.0).family(bold()));
                    ui.label(RichText::new(tr(lang, "chk.sort_hint")).size(12.5).color(SECONDARY));
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                        for (to, &from) in r.plan.order.iter().enumerate() {
                            if to == from {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{:>3} → {:>3}", from + 1, to + 1)).size(12.5).monospace().color(SECONDARY));
                                ui.add(egui::Label::new(RichText::new(name(from + 1)).size(14.0)).truncate());
                            });
                        }
                        ui.add_space(8.0);
                        // why: only the requirements of the mods that move
                        let moved: std::collections::HashSet<usize> = r.plan.order.iter().enumerate().filter(|(to, from)| *to != **from).map(|(_, &from)| from).collect();
                        for (a, b, why) in r.plan.edges.iter().filter(|(a, b, _)| moved.contains(a) || moved.contains(b)) {
                            let key = match why {
                                stl_core::conflicts::Reason::Dependency => "chk.reason_dep",
                                stl_core::conflicts::Reason::Patch => "chk.reason_patch",
                            };
                            ui.label(RichText::new(tr_args(lang, key, &[&name(a + 1), &name(b + 1)])).size(12.0).color(SECONDARY));
                        }
                        if !r.plan.cycle.is_empty() {
                            let names: Vec<String> = r.plan.cycle.iter().map(|&i| name(i + 1)).collect();
                            ui.label(RichText::new(tr_args(lang, "chk.cycle", &[&names.join(", ")])).size(12.5).color(ORANGE));
                        }
                    });
                }
                CheckSheet::None => {}
            }
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if matches!(sheet, CheckSheet::Sort) {
                    if pill_button(ui, tr(lang, "chk.sort_apply"), ButtonStyle::Filled(BLUE), true).clicked() {
                        self.acts.push(Act::ApplySort);
                    }
                    if pill_button(ui, tr(lang, "common.cancel"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                        close = true;
                    }
                } else {
                    if pill_button(ui, tr(lang, "common.close"), ButtonStyle::Tinted(Color32::WHITE), true).clicked() {
                        close = true;
                    }
                    if matches!(sheet, CheckSheet::Mod(..)) && pill_button(ui, tr(lang, "chk.all_title"), ButtonStyle::Plain(BLUE), true).clicked() {
                        next = Some(CheckSheet::All);
                    }
                }
            });
        });
        if let Some(n) = next {
            self.check.expanded.clear();
            self.check.sheet = n;
        } else if close || resp.should_close() {
            self.check.sheet = CheckSheet::None;
        }
    }

    /// The new launcher's version and notes; restart into it now, or let the next start do it.
    fn self_update_sheet(&mut self, ctx: &egui::Context) {
        if !self.self_update.sheet {
            return;
        }
        let SelfPhase::Ready(s) = self.self_update.phase.clone() else {
            self.self_update.sheet = false;
            return;
        };
        let lang = self.lang;
        let mut close = false;
        let mut install = false;
        let resp = egui::Modal::new(egui::Id::new("self-update")).frame(Self::sheet_frame()).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(RichText::new(tr_args(lang, "upd.title", &[&s.version])).size(22.0).family(bold()));
            ui.label(RichText::new(tr_args(lang, "upd.current", &[stl_core::selfupdate::current_version()])).size(12.5).color(SECONDARY));
            ui.add_space(12.0);
            let notes = if s.notes.trim().is_empty() { tr(lang, "upd.no_notes").to_string() } else { s.notes.replace("\r\n", "\n") };
            glass(ui, 12.0, 12.0, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    release_notes(ui, &notes);
                });
            });
            if !s.page.is_empty() {
                ui.add_space(6.0);
                if ui.add(egui::Label::new(RichText::new(tr(lang, "upd.page")).size(12.5).color(BLUE)).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    open_link(&s.page);
                }
            }
            ui.add_space(10.0);
            ui.label(RichText::new(tr(lang, "upd.later_note")).size(12.5).color(SECONDARY));
            if let Some(e) = &self.self_update.install_error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).size(12.5).color(RED));
            }
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill_button(ui, tr(lang, "upd.restart"), ButtonStyle::Filled(BLUE), self.running.is_empty()).clicked() {
                    install = true;
                }
                if pill_button(ui, tr(lang, "upd.later"), ButtonStyle::Plain(SECONDARY), true).clicked() {
                    close = true;
                }
                if !self.running.is_empty() {
                    ui.label(RichText::new(tr(lang, "upd.game_running")).size(12.0).color(SECONDARY));
                }
            });
        });
        if close || resp.should_close() {
            self.self_update.sheet = false;
        } else if install {
            self.acts.push(Act::InstallSelfUpdate);
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
        let report = self.upload_report();
        let Some(f) = self.upload.as_mut() else { return };
        let lang = self.lang;
        let running = f.rx.is_some();
        let checking = f.pre_rx.is_some();
        let mut close = false;
        let mut start = false;
        let mut recheck = false;
        let mut open: Option<String> = None;
        let now = ctx.input(|i| i.time);
        if checking {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // the sheet fits the window: what is above the checks and the buttons below take about 560 points; the checks and an explanation of
        // a failure share the rest, each scrolling on its own
        let room = (ctx.screen_rect().height() - 560.0).clamp(140.0, 560.0);
        let failed_now = matches!(f.result, Some(Err(_)));
        let checks_h = if failed_now { (room * 0.3).max(70.0) } else { room };
        let explain_h = (room - checks_h).max(120.0);
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
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(RichText::new(tr_args(lang, "up.update", &[id])).size(13.0).color(SECONDARY));
                        if let Ok(n) = id.parse::<u64>() {
                            if theme::text_link(ui, tr(lang, "up.open"), 13.0).clicked() {
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
                    ui.label(RichText::new(tr(lang, "up.no_preview")).size(12.5).color(SECONDARY));
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
            // what the checks found: red stops the upload, orange is worth a look, grey is for information
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr(lang, "up.checks")).size(12.5).color(SECONDARY));
                if checking {
                    ui.label(RichText::new(tr(lang, "up.checking")).size(12.5).color(SECONDARY));
                } else if !running && theme::text_link(ui, tr(lang, "up.recheck"), 12.5).clicked() {
                    recheck = true;
                }
            });
            if let Some(p) = &f.pre {
                let mut found: Vec<&uploadcheck::Finding> = p.findings.iter().collect();
                found.sort_by(|a, b| b.level.cmp(&a.level));
                egui::ScrollArea::vertical().id_salt("upload-checks").max_height(checks_h).show(ui, |ui| {
                    for x in found {
                        let color = match x.level {
                            uploadcheck::Level::Error => RED,
                            uploadcheck::Level::Warning => ORANGE,
                            uploadcheck::Level::Info => SECONDARY,
                        };
                        theme::bullet(ui, color, &upload_i18n::text(lang, x.key, &x.args), 12.5, if x.level == uploadcheck::Level::Info { SECONDARY } else { LABEL });
                    }
                });
                if p.blocked() {
                    ui.label(RichText::new(tr(lang, "up.blocked")).size(12.5).color(RED));
                }
            }
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
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(RichText::new(format!("✓  {}", tr(lang, "up.done"))).size(14.0).color(GREEN));
                        if theme::text_link(ui, tr(lang, "up.open"), 14.0).clicked() {
                            open = Some(workshop::item_url(o.id));
                        }
                    });
                    ui.add_space(4.0);
                    // what Steam has now, against what was sent
                    if let Some(sent) = &f.sent {
                        for x in uploadcheck::verify(sent, o) {
                            let color = if x.level == uploadcheck::Level::Info { SECONDARY } else { ORANGE };
                            theme::bullet(ui, color, &upload_i18n::text(lang, x.key, &x.args), 12.5, if x.level == uploadcheck::Level::Info { SECONDARY } else { LABEL });
                        }
                    }
                    if o.needs_agreement && theme::text_link(ui, tr(lang, "up.open_agreement"), 12.5).clicked() {
                        open = Some("https://steamcommunity.com/sharedfiles/workshoplegalagreement".into());
                    }
                }
                Some(Err(e)) => {
                    ui.add_space(14.0);
                    match &f.failure {
                        Some(err) => egui::ScrollArea::vertical().id_salt("upload-explain").max_height(explain_h).show(ui, |ui| {
                            // what happened, why it probably happened here, and what to do
                            let x = uploadcheck::explain(err, f.pre.as_ref());
                            let t = |l: &uploadcheck::Line| upload_i18n::text(lang, l.key, &l.args);
                            ui.add(egui::Label::new(RichText::new(t(&x.headline)).size(14.0).family(bold()).color(RED)).wrap());
                            ui.add(egui::Label::new(RichText::new(t(&x.meaning)).size(12.5).color(LABEL)).wrap());
                            if !x.causes.is_empty() {
                                ui.add_space(6.0);
                                ui.label(RichText::new(tr(lang, "up.causes")).size(12.5).color(SECONDARY));
                                for c in &x.causes {
                                    theme::bullet(ui, ORANGE, &t(c), 12.5, LABEL);
                                }
                            }
                            if !x.fixes.is_empty() {
                                ui.add_space(6.0);
                                ui.label(RichText::new(tr(lang, "up.todo")).size(12.5).color(SECONDARY));
                                for c in &x.fixes {
                                    theme::bullet(ui, BLUE, &t(c), 12.5, LABEL);
                                }
                            }
                            if let Some(url) = &x.link {
                                ui.add_space(4.0);
                                if theme::text_link(ui, tr(lang, "up.open_agreement"), 12.5).clicked() {
                                    open = Some(url.clone());
                                }
                            }
                        }).inner,
                        None => {
                            ui.add(egui::Label::new(RichText::new(e).size(12.5).color(RED)).wrap());
                        }
                    }
                }
                None => {}
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let finished = matches!(f.result, Some(Ok(_)));
                let failed = matches!(f.result, Some(Err(_)));
                let blocked = f.pre.as_ref().is_some_and(|p| p.blocked()) && !checking;
                let label = if failed { tr(lang, "up.retry") } else { tr(lang, "up.upload") };
                if !finished && pill_button(ui, label, ButtonStyle::Filled(BLUE), !running && !blocked).clicked() {
                    start = true;
                }
                let label = if finished { tr(lang, "up.close") } else { tr(lang, "common.cancel") };
                if pill_button(ui, label, ButtonStyle::Plain(SECONDARY), !running).clicked() {
                    close = true;
                }
                // everything about this upload, to send to whoever helps
                let copy_label = if now - f.copied_at < 2.0 { tr(lang, "up.copied") } else { tr(lang, "up.copy_report") };
                if pill_button(ui, copy_label, ButtonStyle::Plain(BLUE), !report.is_empty()).clicked() {
                    ui.ctx().copy_text(report.clone());
                    f.copied_at = now;
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
        } else if recheck {
            self.acts.push(Act::CheckUpload);
        }
    }

    /// The game's graphics settings: display mode, monitor, resolution, refresh rate, interface scale, vsync and anti-aliasing.
    fn graphics_section(&mut self, ui: &mut Ui, acts: &Acts) {
        let lang = self.lang;
        let Ok(game) = &self.game else { return };
        // the game writes its settings when it starts and when they change in it: what is shown follows the files
        if self.gfx.is_some() && self.gfx_looked.elapsed() > Duration::from_secs(1) {
            self.gfx_looked = Instant::now();
            if gamesettings::stamp(&game.data_dir) != self.gfx_stamp {
                self.gfx = None;
            }
        }
        if self.gfx.is_none() {
            self.gfx_stamp = gamesettings::stamp(&game.data_dir);
            self.gfx_looked = Instant::now();
            self.gfx = Some(gamesettings::read(&game.data_dir));
            self.displays = gamesettings::displays();
        }
        let Some(cur) = self.gfx.clone() else { return };
        let mut g = cur.clone();
        let mut commit = false;
        let running = !self.running.is_empty();
        theme::section(ui, tr(lang, "set.graphics"));
        glass(ui, 16.0, 0.0, |ui| {
            let mut rows = Rows::new();
            // display mode
            let modes = [("fullscreen", "gfx.fullscreen"), ("borderless_fullscreen", "gfx.borderless"), ("windowed", "gfx.windowed")];
            let labels: Vec<String> = modes.iter().map(|(_, k)| tr(lang, k).to_string()).collect();
            let at = modes.iter().position(|(v, _)| *v == g.display_mode).unwrap_or(1);
            rows.row(ui, 52.0, 420.0, false, |ui| { ui.label(tr(lang, "gfx.mode")); }, |ui| {
                if let Some(i) = segmented(ui, &labels, at, 420.0) {
                    g.display_mode = modes[i].0.to_string();
                    commit = true;
                }
            });
            // monitor
            if self.displays.len() > 1 {
                let names: Vec<String> = self.displays.iter().enumerate().map(|(i, d)| format!("{} · {}×{}", i + 1, d.current.0, d.current.1)).collect();
                let at = (g.display_index as usize).min(names.len() - 1);
                rows.row(ui, 52.0, 420.0, false, |ui| { ui.label(tr(lang, "gfx.display")); }, |ui| {
                    if let Some(i) = segmented(ui, &names, at, 420.0) {
                        g.display_index = i as u32;
                        commit = true;
                    }
                });
            }
            let display = self.displays.get(g.display_index as usize).or(self.displays.first());
            // resolution: the one of the current mode
            let windowed = g.display_mode == "windowed";
            let res = if windowed { g.windowed_resolution } else { g.fullscreen_resolution };
            let mut sizes: Vec<(u32, u32)> = Vec::new();
            if let Some(d) = display {
                for (w, h, _) in &d.modes {
                    if !sizes.contains(&(*w, *h)) {
                        sizes.push((*w, *h));
                    }
                }
            }
            if !sizes.contains(&res) {
                sizes.insert(0, res);
            }
            rows.row(ui, 52.0, 200.0, false, |ui| { ui.label(tr(lang, "gfx.resolution")); }, |ui| {
                let popup = egui::Id::new("gfx-resolution");
                let open = ui.memory(|m| m.is_popup_open(popup));
                let r = pill_button(ui, &format!("{} × {}", res.0, res.1), ButtonStyle::Tinted(Color32::WHITE), true);
                if r.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup));
                }
                let _ = open;
                theme::menu(ui, popup, &r, 180.0, |ui| {
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for (w, h) in &sizes {
                            let chosen = (*w, *h) == res;
                            if theme::menu_item(ui, &format!("{w} × {h}"), chosen).clicked() {
                                if windowed {
                                    g.windowed_resolution = (*w, *h);
                                } else {
                                    g.fullscreen_resolution = (*w, *h);
                                }
                                commit = true;
                            }
                        }
                    });
                });
            });
            // refresh rate (full screen only)
            if !windowed {
                let mut rates: Vec<u32> = display.map(|d| d.modes.iter().filter(|m| (m.0, m.1) == res).map(|m| m.2).collect()).unwrap_or_default();
                rates.sort_unstable_by(|a, b| b.cmp(a));
                rates.dedup();
                rates.truncate(5);
                if !rates.contains(&g.refresh_rate) && !rates.is_empty() {
                    // the stored rate is not offered at this size: show the best one as the choice
                    g.refresh_rate = rates[0];
                }
                if !rates.is_empty() {
                    let labels: Vec<String> = rates.iter().map(|r| format!("{r} Hz")).collect();
                    let at = rates.iter().position(|r| *r == g.refresh_rate).unwrap_or(0);
                    let w = (labels.len() as f32 * 84.0).min(420.0);
                    rows.row(ui, 52.0, w, false, |ui| { ui.label(tr(lang, "gfx.refresh")); }, |ui| {
                        if let Some(i) = segmented(ui, &labels, at, w) {
                            g.refresh_rate = rates[i];
                            commit = true;
                        }
                    });
                }
            }
            // interface scale
            let recommended = ((res.1 as f32 / 1080.0) * 4.0).round() / 4.0;
            let recommended = recommended.clamp(0.5, 2.0);
            rows.row(ui, 64.0, 420.0, false, |ui| {
                stack(ui, 64.0, 38.0, |ui| {
                    ui.label(tr(lang, "gfx.scale"));
                    ui.label(RichText::new(tr_args(lang, "gfx.scale_hint", &[&format!("{:.0}%", recommended * 100.0), &format!("{}", res.1)])).size(12.0).color(SECONDARY));
                });
            }, |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for v in [2.0f32, 1.5, 1.25, 1.0] {
                    let on = (g.gui_scale - v).abs() < 0.001;
                    if theme::toggle_chip(ui, &format!("{:.0}%", v * 100.0), on).clicked() {
                        g.gui_scale = v;
                        commit = true;
                    }
                }
                ui.label(RichText::new(format!("{:.0}%", g.gui_scale * 100.0)).size(15.0).family(bold()));
                ui.spacing_mut().slider_width = 120.0;
                let r = ui.add(egui::Slider::new(&mut g.gui_scale, 0.5..=2.0).step_by(0.05).show_value(false));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    commit = true;
                }
            });
            // vsync
            rows.row(ui, 48.0, 46.0, false, |ui| { ui.label(tr(lang, "gfx.vsync")); }, |ui| {
                if switch(ui, &mut g.vsync).changed() {
                    commit = true;
                }
            });
            // anti-aliasing
            let levels = [0u32, 2, 4, 8];
            let labels: Vec<String> = levels.iter().map(|l| if *l == 0 { tr(lang, "gfx.off").to_string() } else { format!("{l}×") }).collect();
            let at = levels.iter().position(|l| *l == g.multi_sampling).unwrap_or(2);
            rows.row(ui, 52.0, 300.0, false, |ui| { ui.label(tr(lang, "gfx.msaa")); }, |ui| {
                if let Some(i) = segmented(ui, &labels, at, 300.0) {
                    g.multi_sampling = levels[i];
                    commit = true;
                }
            });
        });
        if running {
            theme::footnote(ui, tr(lang, "gfx.running"));
        } else {
            theme::footnote(ui, tr(lang, "gfx.note"));
        }
        if let Some((msg, ok)) = &self.gfx_status {
            if !ok {
                theme::footnote(ui, msg);
            }
        }
        // the slider moves the value while dragging; it is written once let go
        if g != cur {
            if commit {
                acts.push(Act::SetGraphics(g));
            } else {
                self.gfx = Some(g);
            }
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
        // the mark: the star alone, in white like the title (no coloured tile, which stood out against the artwork)
        let mark = Rect::from_center_size(pos2(rect.left() + 34.0 + 10.0, rect.center().y), Vec2::splat(20.0));
        // painted as a convex shape on purpose: that fills the concave star into a ray (theme::in_mark), which the icons copy
        ui.painter().add(Shape::convex_polygon(theme::mark_points(mark.center(), 9.0), theme::white(225), egui::Stroke::NONE));
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
        // the launcher's own update: quiet while it downloads, a small capsule once it can be installed
        buttons.add_space(10.0);
        match &self.self_update.phase {
            SelfPhase::Ready(s) => {
                let text = tr_args(self.lang, "upd.badge", &[&s.version]);
                if theme::update_badge(&mut buttons, &text).on_hover_text(tr(self.lang, "upd.badge_hint")).clicked() {
                    self.self_update.install_error = None;
                    self.self_update.sheet = true;
                }
            }
            SelfPhase::Downloading(v) => {
                buttons.label(RichText::new(tr_args(self.lang, "upd.downloading", &[v])).size(12.0).color(SECONDARY));
            }
            _ => {}
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

/// `stellaris-launcher.exe --uninstall`, what "Apps & features" runs for a copy unpacked from the zip: asks, removes the registry entries
/// and the files a release brings, optionally the playsets and settings. A copy the setup installed hands over to the setup's uninstaller.
fn uninstall() -> eframe::Result<()> {
    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
    let store = Store::load().ok();
    let lang = store.as_ref().map(|s| resolve_lang(s, &Game::open(s.game_dir.as_deref().map(std::path::Path::new)).map_err(|e| e.to_string()))).unwrap_or(Lang::En);
    if stl_core::install::installed_by_setup() {
        if let Some(u) = stl_core::install::setup_uninstaller() {
            let _ = std::process::Command::new(u).spawn();
        }
        return Ok(());
    }
    // --quiet (Windows' QuietUninstallString, scripts): no questions, the playsets and settings are kept
    if std::env::args().any(|a| a == "--quiet") {
        let _ = stl_core::install::uninstall_portable(false);
        return Ok(());
    }
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.display().to_string())).unwrap_or_default();
    let ask = |text: String| MessageDialog::new().set_level(MessageLevel::Warning).set_title("Stellaris Launcher").set_description(text).set_buttons(MessageButtons::YesNo).show() == MessageDialogResult::Yes;
    if !ask(tr_args(lang, "uninst.confirm", &[&dir])) {
        return Ok(());
    }
    let data = stl_core::paths::app_data_dir().map(|d| d.display().to_string()).unwrap_or_default();
    let with_data = ask(tr_args(lang, "uninst.data", &[&data]));
    let text = match stl_core::install::uninstall_portable(with_data) {
        Ok(()) => tr(lang, "uninst.done").to_string(),
        Err(e) => format!("{}\n{e:#}", tr(lang, "uninst.failed")),
    };
    MessageDialog::new().set_level(MessageLevel::Info).set_title("Stellaris Launcher").set_description(text).set_buttons(MessageButtons::Ok).show();
    Ok(())
}

fn main() -> eframe::Result<()> {
    if std::env::args().any(|a| a == "--uninstall") {
        return uninstall();
    }
    // after an update: the replaced files; and a release downloaded last time is installed now, before anything is shown
    stl_core::selfupdate::cleanup();
    // "Apps & features" and App Paths: written on the first start of a copy from the zip, refreshed when its folder or version changes
    let _ = stl_core::install::register(stl_core::selfupdate::current_version());
    // (--update-sheet, for screenshots while developing, shows the sheet instead)
    let auto = Store::load().map(|s| s.auto_update != Some(false)).unwrap_or(true) && !std::env::args().any(|a| a == "--update-sheet");
    if auto {
        if let Some(staged) = stl_core::selfupdate::staged() {
            if let Ok(exe) = stl_core::selfupdate::install(&staged) {
                if std::process::Command::new(&exe).args(std::env::args().skip(1)).spawn().is_ok() {
                    return Ok(());
                }
            }
        }
    }
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
            if std::env::args().any(|a| a == "--other-theme") {
                // as when Windows reports the other app theme after the start (a check that the look does not depend on it)
                let t = cc.egui_ctx.theme();
                cc.egui_ctx.set_theme(if t == egui::Theme::Dark { egui::Theme::Light } else { egui::Theme::Dark });
            }
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

/// An upload's error as the sheet keeps it: the text, and what went wrong (for the explanation) when the upload said.
fn upload_failure(e: anyhow::Error) -> (String, Option<workshop::UploadError>) {
    let err = e.chain().find_map(|c| c.downcast_ref::<workshop::UploadError>()).cloned();
    (format!("{e:#}"), err)
}
