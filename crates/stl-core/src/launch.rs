//! Starting the game: write what it reads, start it, and load the playset's DLL plugins into it once it is up.

use crate::game::Game;
use crate::plugins::{self, Compat};
use crate::store::Store;
use crate::{bail, dlcload, mods, paths, process, Context, Result};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Options {
    /// a playset name or id prefix; the active one when None
    pub playset: Option<String>,
    pub use_plugins: bool,
    /// the "continue" button: `--continuelastsave`
    pub continue_last: bool,
    /// the alternative executable of `launcher-settings.json` (0 = Cross-Store Multiplayer)
    pub alternative: Option<usize>,
    pub extra_args: Vec<String>,
    /// work out and report, change nothing, start nothing
    pub dry_run: bool,
    pub ready_timeout: Duration,
    /// start even if a game is running
    pub force: bool,
    /// after the plugins, load `stl_notice.dll` (beside the launcher) to say in the game which of them are loaded, in these words
    pub notice: Option<NoticeWords>,
}

/// The words of the in-game notice (`{0}` loaded, `{1}` in all).
#[derive(Debug, Clone)]
pub struct NoticeWords {
    pub title: String,
    pub all: String,
    pub some: String,
    pub missing: String,
}

impl Default for NoticeWords {
    fn default() -> Self {
        NoticeWords {
            title: "Stellaris Launcher".into(),
            all: "{0} DLL plugin(s) loaded in the game".into(),
            some: "{0} of {1} DLL plugins loaded in the game".into(),
            missing: "not found in the game".into(),
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            playset: None,
            use_plugins: true,
            continue_last: false,
            alternative: None,
            extra_args: Vec::new(),
            dry_run: false,
            ready_timeout: Duration::from_secs(180),
            force: false,
            notice: Some(NoticeWords::default()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub pid: Option<u32>,
    pub playset: String,
    pub mods: Vec<String>,
    pub skipped_mods: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub args: Vec<String>,
    pub injected: Vec<String>,
    pub skipped_plugins: Vec<(String, String)>,
    pub failed_plugins: Vec<(String, String)>,
}

/// The enabled mods of a playset that the game can load, in order, and what had to be left out, and warnings.
pub fn resolve_mods(game: &Game, playset: &crate::store::Playset) -> (Vec<String>, Vec<(String, String)>, Vec<String>) {
    let known = mods::scan(&game.data_dir);
    let mut ids = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    for id in playset.enabled_mods() {
        match known.iter().find(|m| m.id == id) {
            None => skipped.push((id, "there is no such descriptor in the mod folder".to_string())),
            Some(m) => {
                if let Some(problem) = &m.problem {
                    skipped.push((id, problem.clone()));
                    continue;
                }
                if let Some(sv) = &m.supported_version {
                    if !mods::supports(sv, game.version()) {
                        warnings.push(format!("{} is made for {} (the game is {}); the game will ask whether to load it", m.name, sv, game.version()));
                    }
                }
                ids.push(id);
            }
        }
    }
    (ids, skipped, warnings)
}

fn game_alive(pid: u32) -> bool {
    process::find_processes("stellaris.exe").contains(&pid)
}

/// Waits until the game has a window (and the plugin's extra delay has passed); false if the game exits or the time is up.
fn wait_ready(pid: u32, spec: &plugins::LoadSpec, deadline: Instant) -> bool {
    if spec.wait != "none" {
        loop {
            if !game_alive(pid) || Instant::now() > deadline {
                return false;
            }
            if process::window_title(pid, "stellaris").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let until = Instant::now() + Duration::from_millis(spec.delay_ms);
    while Instant::now() < until {
        if !game_alive(pid) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    game_alive(pid)
}

pub fn launch(game: &Game, store: &Store, opts: &Options, say: &mut dyn FnMut(&str)) -> Result<Report> {
    let mut report = Report::default();
    if !opts.force && !process::find_processes("stellaris.exe").is_empty() && !opts.dry_run {
        bail!("Stellaris is already running (stl stop, or --force)");
    }
    let index = match &opts.playset {
        Some(name) => store.find(name).with_context(|| format!("no playset called {name} (stl playsets)"))?,
        None => store.active_index(),
    };
    let playset = &store.playsets[index];
    report.playset = playset.name.clone();
    say(&format!("playset: {}", playset.name));

    // mods → dlc_load.json
    let (ids, skipped, warnings) = resolve_mods(game, playset);
    for (id, why) in &skipped {
        say(&format!("  left out {id}: {why}"));
    }
    for w in &warnings {
        say(&format!("  warning: {w}"));
    }
    report.mods = ids.clone();
    report.skipped_mods = skipped;
    report.warnings = warnings;
    if opts.dry_run {
        say(&format!("would write {} mod(s) to {}", ids.len(), game.dlc_load_path().display()));
    } else {
        std::fs::create_dir_all(&game.data_dir)?;
        let changed = dlcload::write(&game.dlc_load_path(), &ids, playset.disabled_dlcs.as_deref())?;
        say(&format!("{} mod(s) in dlc_load.json{}", ids.len(), if changed { "" } else { " (unchanged)" }));
        if let Some(d) = &playset.disabled_dlcs {
            say(&format!("  {} DLC switched off by the playset", d.len()));
        }
    }

    // plugins
    let mut to_load: Vec<plugins::Plugin> = Vec::new();
    if opts.use_plugins {
        let (installed, _) = plugins::list()?;
        for pp in playset.plugins.iter().filter(|p| p.enabled) {
            let Some(plugin) = installed.iter().find(|p| p.manifest.id == pp.id) else {
                report.skipped_plugins.push((pp.id.clone(), "not installed".into()));
                say(&format!("  plugin {}: not installed, skipped", pp.id));
                continue;
            };
            match plugin.compat(game) {
                Compat::Ok | Compat::Unchecked => to_load.push(plugin.clone()),
                Compat::Mismatch { declared, game: g } => {
                    let why = format!(
                        "made for the build {}, the game is {:#010X}",
                        declared.iter().map(|d| format!("{d:#010X}")).collect::<Vec<_>>().join(" / "),
                        g
                    );
                    say(&format!("  plugin {}: {why}; skipped", pp.id));
                    report.skipped_plugins.push((pp.id.clone(), why));
                }
                Compat::MissingDll(p) => {
                    let why = format!("{} is missing", p.display());
                    say(&format!("  plugin {}: {why}; skipped", pp.id));
                    report.skipped_plugins.push((pp.id.clone(), why));
                }
            }
        }
    }

    // command line
    let (exe, mut args) = game.executable(opts.alternative)?;
    if opts.continue_last {
        args.push("--continuelastsave".into());
    }
    args.extend(opts.extra_args.iter().cloned());
    report.args = args.clone();
    if opts.dry_run {
        say(&format!("would start {} {}", exe.display(), args.join(" ")));
        for p in &to_load {
            say(&format!("would load {} ({})", p.manifest.id, p.dll_path().display()));
        }
        return Ok(report);
    }
    for p in &to_load {
        for created in p.ensure_config()?.into_iter().chain(plugins::seed(p, game)?) {
            say(&format!("  {}: created {}", p.manifest.id, created.display()));
        }
    }

    let log = paths::app_data_dir()?.join("logs").join("game-stderr.log");
    let pid = process::spawn(&exe, &args, &game.dir, &[("SteamAppId", "281990")], Some(&log))?;
    report.pid = Some(pid);
    say(&format!("started {} (pid {pid})", exe.file_name().unwrap_or_default().to_string_lossy()));

    let deadline = Instant::now() + opts.ready_timeout;
    for p in &to_load {
        let id = p.manifest.id.clone();
        if !wait_ready(pid, &p.manifest.load, deadline) {
            let why = "the game exited or was not ready in time".to_string();
            say(&format!("  plugin {id}: {why}"));
            report.failed_plugins.push((id, why));
            continue;
        }
        let dll = p.dll_path();
        if process::module_loaded(pid, &dll.to_string_lossy()) || process::module_loaded(pid, &p.manifest.dll) {
            say(&format!("  plugin {id}: already loaded"));
            report.injected.push(id);
            continue;
        }
        match process::inject(pid, &dll) {
            Ok(()) => {
                say(&format!("  plugin {id}: loaded"));
                report.injected.push(id);
            }
            Err(e) => {
                say(&format!("  plugin {id}: {e:#}"));
                report.failed_plugins.push((id, format!("{e:#}")));
            }
        }
    }
    if let Some(words) = &opts.notice {
        if !to_load.is_empty() || !report.skipped_plugins.is_empty() {
            if let Err(e) = notice(game, pid, words, &to_load, &report.skipped_plugins, deadline) {
                say(&format!("  in-game notice: {e:#}"));
            } else {
                say("  in-game notice: shown");
            }
        }
    }
    Ok(report)
}

/// The in-game notice: the plugins to look for and the words go into `%APPDATA%\stellaris-launcher\notice\<pid>.txt`, then `stl_notice.dll`
/// (beside the launcher) is loaded into the game; it checks each plugin from inside, shows the card and unloads itself.
fn notice(game: &Game, pid: u32, words: &NoticeWords, plugins: &[plugins::Plugin], skipped: &[(String, String)], deadline: Instant) -> Result<()> {
    let exe = std::env::current_exe()?;
    let dll = exe.parent().map(|d| d.join("stl_notice.dll")).context("no folder for the launcher")?;
    if !dll.is_file() {
        bail!("stl_notice.dll is not beside the launcher");
    }
    // nothing can show over a game in exclusive full screen
    if crate::gamesettings::read(&game.data_dir).display_mode == "fullscreen" {
        bail!("the game is in exclusive full screen, where nothing can show over it");
    }
    if !wait_ready(pid, &plugins::LoadSpec::default(), deadline) {
        bail!("the game exited or was not ready in time");
    }
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut text = format!("title\t{}\nall\t{}\nsome\t{}\nmissing\t{}\n", clean(&words.title), clean(&words.all), clean(&words.some), clean(&words.missing));
    for p in plugins {
        text += &format!("plugin\t{}\t{}\n", clean(&p.manifest.name), p.dll_path().display());
    }
    for (id, why) in skipped {
        text += &format!("skipped\t{}\t{}\n", clean(id), clean(why));
    }
    let dir = paths::app_data_dir()?.join("notice");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{pid}.txt"));
    std::fs::write(&file, text)?;
    let r = process::inject(pid, &dll);
    if r.is_err() {
        let _ = std::fs::remove_file(&file);
    }
    // it unloads itself once shown, which `inject` may see as already gone
    match r {
        Err(e) if format!("{e:#}").contains("unloaded itself") => Ok(()),
        r => r,
    }
}
