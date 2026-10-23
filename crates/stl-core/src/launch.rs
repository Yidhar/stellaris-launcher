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

/// The words of the in-game notice (`{0}` loaded, `{1}` in all) and of the plugin list on the title screen.
#[derive(Debug, Clone)]
pub struct NoticeWords {
    pub all: String,
    pub some: String,
    pub missing: String,
    /// the corner button of the title screen
    pub button: String,
    /// the list's heading, and what it says under the list
    pub heading: String,
    pub footer: String,
    pub loaded: String,
    /// a plugin switched off in the playset
    pub off: String,
}

impl NoticeWords {
    /// In a language: the launcher's code (`zh-Hans`, …) or the game's (`l_simp_chinese`, …); English for any other.
    pub fn for_language(code: &str) -> NoticeWords {
        let code = match code {
            "l_english" => "en",
            "l_simp_chinese" => "zh-Hans",
            "l_traditional_chinese" => "zh-Hant",
            "l_japanese" => "ja",
            "l_korean" => "ko",
            "l_german" => "de",
            "l_french" => "fr",
            "l_spanish" => "es",
            "l_russian" => "ru",
            other => other,
        };
        let w: [&str; 8] = match code {
            "en" => ["{0} DLL plugin(s) loaded in the game", "{0} of {1} DLL plugins loaded in the game", "not found in the game", "Plugins", "DLL plugins", "Changes take effect the next time the game starts", "loaded", "off"],
            "zh-Hans" => ["{0} 个 DLL 插件已在游戏内加载", "{0} / {1} 个 DLL 插件已在游戏内加载", "没有在游戏中找到", "插件", "DLL 插件", "更改在下次启动游戏时生效", "已加载", "已关闭"],
            "zh-Hant" => ["{0} 個 DLL 外掛已在遊戲內載入", "{0} / {1} 個 DLL 外掛已在遊戲內載入", "沒有在遊戲中找到", "外掛", "DLL 外掛", "變更在下次啟動遊戲時生效", "已載入", "已關閉"],
            "ja" => ["{0} 個の DLL プラグインをゲーム内で読み込みました", "{1} 個中 {0} 個の DLL プラグインをゲーム内で読み込みました", "ゲーム内に見つかりません", "プラグイン", "DLL プラグイン", "変更は次回ゲーム起動時に反映されます", "読み込み済み", "オフ"],
            "ko" => ["DLL 플러그인 {0}개가 게임에 로드됨", "DLL 플러그인 {1}개 중 {0}개가 게임에 로드됨", "게임에서 찾을 수 없음", "플러그인", "DLL 플러그인", "변경 사항은 다음에 게임을 시작할 때 적용됩니다", "로드됨", "꺼짐"],
            "de" => ["{0} DLL-Plugin(s) im Spiel geladen", "{0} von {1} DLL-Plugins im Spiel geladen", "im Spiel nicht gefunden", "Plugins", "DLL-Plugins", "Änderungen gelten beim nächsten Spielstart", "geladen", "aus"],
            "fr" => ["{0} plugin(s) DLL chargé(s) dans le jeu", "{0} plugins DLL sur {1} chargés dans le jeu", "introuvable dans le jeu", "Plugins", "Plugins DLL", "Les changements prennent effet au prochain lancement du jeu", "chargé", "désactivé"],
            "es" => ["{0} plugin(s) DLL cargado(s) en el juego", "{0} de {1} plugins DLL cargados en el juego", "no está en el juego", "Plugins", "Plugins DLL", "Los cambios se aplican la próxima vez que inicies el juego", "cargado", "desactivado"],
            "ru" => ["DLL-плагинов загружено в игру: {0}", "Загружено DLL-плагинов: {0} из {1}", "не найден в игре", "Плагины", "DLL-плагины", "Изменения вступят в силу при следующем запуске игры", "загружен", "выключен"],
            _ => return NoticeWords::default(),
        };
        let [all, some, missing, button, heading, footer, loaded, off] = w.map(String::from);
        NoticeWords { all, some, missing, button, heading, footer, loaded, off }
    }

    /// In the language of the launcher's setting, else the game's own (`pdx_settings.txt`).
    pub fn for_launch(store: &Store, game: &Game) -> NoticeWords {
        let code = store.language.clone().or_else(|| crate::gamesettings::language(&game.data_dir)).unwrap_or_default();
        NoticeWords::for_language(&code)
    }
}

impl Default for NoticeWords {
    fn default() -> Self {
        NoticeWords {
            all: "{0} DLL plugin(s) loaded in the game".into(),
            some: "{0} of {1} DLL plugins loaded in the game".into(),
            missing: "not found in the game".into(),
            button: "Plugins".into(),
            heading: "DLL plugins".into(),
            footer: "Changes take effect the next time the game starts".into(),
            loaded: "loaded".into(),
            off: "off".into(),
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
    // what the official launcher would have written for the Workshop items is there, up to date
    mods::sync_workshop(&game.data_dir);
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
        // with any plugin installed: the title-screen list can switch them on even when none is now
        let installed = plugins::list().map(|(i, _)| !i.is_empty()).unwrap_or(false);
        if opts.use_plugins && (installed || !report.skipped_plugins.is_empty()) {
            if let Err(e) = notice(game, pid, words, playset, &report.skipped_plugins, deadline) {
                say(&format!("  in-game notice: {e:#}"));
            } else {
                say("  in-game notice: shown");
            }
        }
    }
    Ok(report)
}

/// The in-game notice: the plugins to look for and the words go into `%APPDATA%\stellaris-launcher\notice\<pid>.txt`, then `stl_notice.dll`
/// (beside the launcher) is loaded into the game. It checks each plugin from inside and says how many are loaded; on the title screen a corner
/// of the game opens the list of them; once a game is loaded (the game's `time.log` says so) it unloads itself.
fn notice(game: &Game, pid: u32, words: &NoticeWords, playset: &crate::store::Playset, skipped: &[(String, String)], deadline: Instant) -> Result<()> {
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
    let mut text = String::new();
    for (k, v) in [
        ("all", &words.all),
        ("some", &words.some),
        ("missing", &words.missing),
        ("button", &words.button),
        ("heading", &words.heading),
        ("footer", &words.footer),
        ("loaded", &words.loaded),
        ("off", &words.off),
    ] {
        text += &format!("{k}\t{}\n", clean(v));
    }
    // a game being played writes this into the game's time.log (and leaving it for the title screen writes nothing)
    text += &format!("gamelog\t{}\n", game.data_dir.join("logs").join("time.log").display());
    // where switches changed in the game go, for the launcher to take in
    text += &format!("toggles\t{}\n", crate::store::Store::pending_toggles_path()?.display());
    text += &format!("playset\t{}\n", playset.id);
    // every installed plugin, with its switch in this playset; the reason when the launcher left one out
    let installed = plugins::list().map(|(i, _)| i).unwrap_or_default();
    let why = |id: &str| skipped.iter().find(|(s, _)| s == id).map(|(_, w)| w.clone()).unwrap_or_default();
    for p in &installed {
        let on = playset.plugins.iter().any(|x| x.id == p.manifest.id && x.enabled);
        text += &format!(
            "plugin\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clean(&p.manifest.id),
            clean(&p.manifest.name),
            p.dll_path().display(),
            clean(&p.manifest.version),
            clean(&p.manifest.description),
            on as u8,
            clean(&why(&p.manifest.id))
        );
    }
    // switched on in the playset but not installed
    for x in playset.plugins.iter().filter(|x| x.enabled && !installed.iter().any(|p| p.manifest.id == x.id)) {
        text += &format!("plugin\t{0}\t{0}\t\t\t\t1\t{1}\n", clean(&x.id), clean(&why(&x.id)));
    }
    let dir = paths::app_data_dir()?.join("notice");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{pid}.txt"));
    std::fs::write(&file, text)?;
    let r = process::inject(pid, &dll);
    if r.is_err() {
        let _ = std::fs::remove_file(&file);
    }
    // it unloads itself once a game is loaded, which `inject` may see as already gone
    match r {
        Err(e) if format!("{e:#}").contains("unloaded itself") => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::NoticeWords;

    #[test]
    fn notice_words_follow_the_launcher_or_the_game() {
        assert_eq!(NoticeWords::for_language("zh-Hans").button, "插件");
        assert_eq!(NoticeWords::for_language("l_simp_chinese").off, "已关闭");
        assert_eq!(NoticeWords::for_language("l_russian").button, "Плагины");
        assert_eq!(NoticeWords::for_language("l_polish").button, NoticeWords::default().button);
    }
}
