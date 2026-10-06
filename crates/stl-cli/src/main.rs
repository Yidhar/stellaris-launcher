//! `stl`: the command line of the Stellaris launcher.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use stl_core::game::Game;
use stl_core::mods::{self, Mod};
use stl_core::store::{Playset, Store};
use stl_core::{launch, official, pe, plugins, process};

#[derive(Parser)]
#[command(name = "stl", version, about = "Stellaris launcher: playsets, mods and DLL plugins, and the game started without the Paradox Launcher")]
struct Cli {
    /// the folder of stellaris.exe (found in the Steam libraries by default)
    #[arg(long, global = true)]
    game: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// the game, the active playset, the plugins
    Status,
    /// the mods the game can load (the descriptors in the mod folder)
    Mods {
        /// also list the ones with a problem
        #[arg(long)]
        all: bool,
    },
    /// list the playsets
    Playsets,
    /// work with a playset
    Playset {
        #[command(subcommand)]
        command: PlaysetCmd,
    },
    /// copy the playsets of the official Paradox Launcher into ours (its database is only read)
    ImportOfficial {
        /// only this playset (by name)
        #[arg(long)]
        name: Option<String>,
        /// replace a playset of ours that has the same name
        #[arg(long)]
        replace: bool,
    },
    /// DLL plugins
    Plugins,
    Plugin {
        #[command(subcommand)]
        command: PluginCmd,
    },
    /// write dlc_load.json from a playset, start the game and load its plugins
    Launch {
        /// the playset (name or start of its id); the active one by default
        #[arg(long, short)]
        playset: Option<String>,
        /// do not load DLL plugins
        #[arg(long)]
        no_plugins: bool,
        /// continue the last save (the launcher's "continue")
        #[arg(long = "continue")]
        continue_last: bool,
        /// the alternative executable of the game (0 = Cross-Store Multiplayer)
        #[arg(long)]
        alt: Option<usize>,
        /// say what would happen, change nothing
        #[arg(long)]
        dry_run: bool,
        /// start even if the game is running
        #[arg(long)]
        force: bool,
        /// seconds to wait for the game's window before giving up on plugins
        #[arg(long, default_value_t = 180)]
        timeout: u64,
        /// more arguments for the game (after --)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// close the game
    Stop,
    /// load a DLL into the running game by hand
    Inject {
        dll: PathBuf,
        #[arg(long)]
        pid: Option<u32>,
    },
}

#[derive(Subcommand)]
enum PlaysetCmd {
    /// show a playset (the active one by default)
    Show { name: Option<String> },
    /// make a new, empty playset
    New { name: String },
    /// make a playset the active one
    Use { name: String },
    /// delete a playset
    Remove { name: String },
    /// add mods to the end of the active playset (by name or descriptor)
    Add { mods: Vec<String> },
    /// take mods out of the active playset
    Rm { mods: Vec<String> },
    Enable { mods: Vec<String> },
    Disable { mods: Vec<String> },
    /// move a mod to a position (1 = loaded first)
    Move { r#mod: String, position: usize },
}

#[derive(Subcommand)]
enum PluginCmd {
    /// install from a folder with stl-plugin.json (copied; --link keeps it where it is)
    Install {
        dir: PathBuf,
        #[arg(long)]
        link: bool,
    },
    Remove { id: String },
    /// details of an installed plugin
    Info { id: String },
    /// load the plugin in the active playset
    Enable { id: String },
    Disable { id: String },
}

fn open_game(cli: &Cli, store: &Store) -> Result<Game> {
    let explicit = cli.game.clone().or_else(|| store.game_dir.as_ref().map(PathBuf::from));
    Game::open(explicit.as_deref())
}

/// A mod of the mod folder by exact descriptor id, or by a name that is found in only one.
fn pick_mod<'a>(known: &'a [Mod], what: &str) -> Result<&'a Mod> {
    if let Some(m) = known.iter().find(|m| m.id.eq_ignore_ascii_case(what) || m.id.eq_ignore_ascii_case(&format!("mod/{what}"))) {
        return Ok(m);
    }
    let w = what.to_lowercase();
    let exact: Vec<&Mod> = known.iter().filter(|m| m.name.to_lowercase() == w).collect();
    let hits: Vec<&Mod> = if exact.len() == 1 { exact } else { known.iter().filter(|m| m.name.to_lowercase().contains(&w)).collect() };
    match hits.len() {
        1 => Ok(hits[0]),
        0 => bail!("no mod matches {what:?} (stl mods)"),
        n => bail!("{n} mods match {what:?}: {}", hits.iter().take(6).map(|m| format!("{} [{}]", m.name, m.id)).collect::<Vec<_>>().join("; ")),
    }
}

fn mod_label(known: &[Mod], id: &str) -> String {
    match known.iter().find(|m| m.id == id) {
        Some(m) => {
            let mut s = m.name.clone();
            if let Some(p) = &m.problem {
                s.push_str(&format!("  !! {p}"));
            }
            s
        }
        None => format!("{id}  !! not in the mod folder"),
    }
}

fn print_playset(known: &[Mod], p: &Playset, active: bool) {
    println!("{}{}  ({} mods, {} enabled; id {})", p.name, if active { "  [active]" } else { "" }, p.mods.len(), p.mods.iter().filter(|m| m.enabled).count(), p.id);
    for (i, m) in p.mods.iter().enumerate() {
        println!("  {:>3} [{}] {}", i + 1, if m.enabled { 'x' } else { ' ' }, mod_label(known, &m.id));
    }
    for pl in &p.plugins {
        println!("  plugin [{}] {}", if pl.enabled { 'x' } else { ' ' }, pl.id);
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("stl: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut store = Store::load()?;
    match &cli.command {
        Cmd::Status => {
            let game = open_game(&cli, &store)?;
            println!("game        {} ({})", game.settings.version, game.dir.display());
            println!("exe build   {:#010X} ({})", game.exe_timestamp, pe::describe(game.exe_timestamp));
            println!("data folder {}", game.data_dir.display());
            let running = process::find_processes("stellaris.exe");
            println!("running     {}", if running.is_empty() { "no".to_string() } else { format!("yes (pid {})", running.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")) });
            match official::find_database(&game.data_dir) {
                Some(db) => println!("official    {}", db.file_name().unwrap().to_string_lossy()),
                None => println!("official    no launcher database found"),
            }
            let known = mods::scan(&game.data_dir);
            let p = store.active_playset();
            let (ids, skipped, _) = launch::resolve_mods(&game, p);
            println!("mods        {} descriptors in the mod folder, {} with a problem", known.len(), known.iter().filter(|m| m.problem.is_some()).count());
            println!("playset     {} ({} mods enabled, {} would load, {} left out)", p.name, p.enabled_mods().len(), ids.len(), skipped.len());
            let (installed, problems) = plugins::list()?;
            println!("plugins     {} installed", installed.len());
            for pl in &installed {
                let state = p.plugins.iter().find(|x| x.id == pl.manifest.id).map(|x| if x.enabled { "on" } else { "off" }).unwrap_or("off");
                let compat = match pl.compat(&game) {
                    plugins::Compat::Ok => "build ok".to_string(),
                    plugins::Compat::Unchecked => "build not declared".to_string(),
                    plugins::Compat::Mismatch { declared, .. } => format!("made for {}", declared.iter().map(|d| format!("{d:#010X}")).collect::<Vec<_>>().join("/")),
                    plugins::Compat::MissingDll(_) => "DLL missing".to_string(),
                };
                println!("  {:<24} {:<4} {:<8} {}{}", pl.manifest.id, state, pl.manifest.version, compat, if pl.linked { "  (linked)" } else { "" });
            }
            for pr in problems {
                println!("  ! {pr}");
            }
        }
        Cmd::Mods { all } => {
            let game = open_game(&cli, &store)?;
            let known = mods::scan(&game.data_dir);
            let active = store.active_playset();
            let mut shown = 0;
            for m in &known {
                if m.problem.is_some() && !*all {
                    continue;
                }
                shown += 1;
                let flag = active.mods.iter().find(|x| x.id == m.id).map(|x| if x.enabled { '*' } else { '-' }).unwrap_or(' ');
                let kind = match m.kind {
                    mods::Kind::Workshop => 'W',
                    mods::Kind::ParadoxMods => 'P',
                    mods::Kind::Local => 'L',
                };
                let compat = match &m.supported_version {
                    Some(sv) if mods::supports(sv, game.version()) => "",
                    Some(sv) => &format!("  (for {sv})"),
                    None => "",
                };
                println!("{flag} {kind} {:<48} {:<10}{}{}", m.name, m.version.as_deref().unwrap_or(""), compat, m.problem.as_ref().map(|p| format!("  !! {p}")).unwrap_or_default());
            }
            println!("{shown} shown of {} ({} have a problem{}); * enabled in the active playset, - listed but off", known.len(), known.iter().filter(|m| m.problem.is_some()).count(), if *all { "" } else { ", hidden: --all" });
        }
        Cmd::Playsets => {
            let game = open_game(&cli, &store).ok();
            let known = game.map(|g| mods::scan(&g.data_dir)).unwrap_or_default();
            let _ = known;
            for (i, p) in store.playsets.iter().enumerate() {
                println!("{} {:<28} {:>3} mods ({} on), {} plugin(s)  id {}", if i == store.active_index() { "*" } else { " " }, p.name, p.mods.len(), p.mods.iter().filter(|m| m.enabled).count(), p.plugins.iter().filter(|x| x.enabled).count(), p.id);
            }
        }
        Cmd::Playset { command } => {
            let game = open_game(&cli, &store);
            let known = game.as_ref().map(|g| mods::scan(&g.data_dir)).unwrap_or_default();
            match command {
                PlaysetCmd::Show { name } => {
                    let i = match name {
                        Some(n) => store.find(n).with_context(|| format!("no playset called {n}"))?,
                        None => store.active_index(),
                    };
                    print_playset(&known, &store.playsets[i], i == store.active_index());
                }
                PlaysetCmd::New { name } => {
                    let i = store.add_playset(name)?;
                    store.save()?;
                    println!("created {} (id {}); `stl playset use {name}` makes it active", name, store.playsets[i].id);
                }
                PlaysetCmd::Use { name } => {
                    let i = store.find(name).with_context(|| format!("no playset called {name}"))?;
                    store.set_active(i);
                    store.save()?;
                    println!("active playset: {}", store.playsets[i].name);
                }
                PlaysetCmd::Remove { name } => {
                    let i = store.find(name).with_context(|| format!("no playset called {name}"))?;
                    let n = store.playsets[i].name.clone();
                    store.remove_playset(i)?;
                    store.save()?;
                    println!("removed {n}");
                }
                PlaysetCmd::Add { mods: names } | PlaysetCmd::Rm { mods: names } | PlaysetCmd::Enable { mods: names } | PlaysetCmd::Disable { mods: names } => {
                    let idx = store.active_index();
                    for n in names {
                        let m = pick_mod(&known, n)?.clone();
                        let p = &mut store.playsets[idx];
                        match command {
                            PlaysetCmd::Add { .. } => {
                                if p.mod_pos(&m.id).is_none() {
                                    p.set_mod(&m.id, true);
                                    println!("added {}", m.name);
                                } else {
                                    println!("{} is already in the playset", m.name);
                                }
                            }
                            PlaysetCmd::Rm { .. } => {
                                println!("{} {}", if p.remove_mod(&m.id) { "removed" } else { "was not in the playset:" }, m.name);
                            }
                            PlaysetCmd::Enable { .. } => {
                                p.set_mod(&m.id, true);
                                println!("enabled {}", m.name);
                            }
                            _ => {
                                p.set_mod(&m.id, false);
                                println!("disabled {}", m.name);
                            }
                        }
                    }
                    store.save()?;
                }
                PlaysetCmd::Move { r#mod: what, position } => {
                    let m = pick_mod(&known, what)?.clone();
                    let idx = store.active_index();
                    if !store.playsets[idx].move_mod(&m.id, position.saturating_sub(1)) {
                        bail!("{} is not in the playset", m.name);
                    }
                    store.save()?;
                    println!("{} is now number {}", m.name, store.playsets[idx].mod_pos(&m.id).unwrap() + 1);
                }
            }
        }
        Cmd::ImportOfficial { name, replace } => {
            let game = open_game(&cli, &store)?;
            let db = official::find_database(&game.data_dir).context("no launcher-v2*.sqlite in the game data folder: the official launcher has not been used here")?;
            let sets = official::read_playsets(&db)?;
            let known = mods::scan(&game.data_dir);
            for o in stl_core::import::import_official(&mut store, &known, &sets, name.as_deref(), *replace)? {
                if o.skipped {
                    println!("skipped {} (we have one with that name; --replace)", o.name);
                } else {
                    println!(
                        "imported {} ({} mods, {} enabled, {} without a descriptor in the mod folder){}",
                        o.name,
                        o.mods,
                        o.enabled,
                        o.missing,
                        if o.was_active { "  [was active in the official launcher]" } else { "" }
                    );
                }
            }
            store.save()?;
        }
        Cmd::Plugins => {
            let game = open_game(&cli, &store).ok();
            let (installed, problems) = plugins::list()?;
            if installed.is_empty() {
                println!("no plugins installed (stl plugin install <folder with stl-plugin.json>)");
            }
            for pl in &installed {
                let compat = match game.as_ref().map(|g| pl.compat(g)) {
                    Some(plugins::Compat::Ok) => "build ok",
                    Some(plugins::Compat::Unchecked) | None => "build not declared",
                    Some(plugins::Compat::Mismatch { .. }) => "WRONG BUILD",
                    Some(plugins::Compat::MissingDll(_)) => "DLL MISSING",
                };
                println!("{:<24} {:<8} {:<20} {}{}", pl.manifest.id, pl.manifest.version, compat, pl.manifest.name, if pl.linked { "  (linked)" } else { "" });
            }
            for pr in problems {
                println!("! {pr}");
            }
        }
        Cmd::Plugin { command } => match command {
            PluginCmd::Install { dir, link } => {
                let p = plugins::install(dir, *link)?;
                println!("{} {} {} ({})", if *link { "linked" } else { "installed" }, p.manifest.id, p.manifest.version, p.dir.display());
            }
            PluginCmd::Remove { id } => {
                plugins::remove(id)?;
                println!("removed {id}");
            }
            PluginCmd::Info { id } => {
                let p = plugins::find(id)?;
                let m = &p.manifest;
                println!("{} {}  {}", m.id, m.version, m.name);
                if !m.description.is_empty() {
                    println!("  {}", m.description);
                }
                println!("  folder   {}{}", p.dir.display(), if p.linked { "  (linked)" } else { "" });
                println!("  dll      {}  sha256 {}", m.dll, p.sha256().unwrap_or_else(|e| format!("({e})")));
                println!("  builds   {}", if m.game.exe_timestamps.is_empty() { "not declared".to_string() } else { m.game.exe_timestamps.join(", ") });
                println!("  loaded   {} after {} ms", if m.load.wait == "none" { "when the process starts" } else { "when the game window exists" }, m.load.delay_ms);
                for s in &m.seed_files {
                    println!("  seeds    {} -> game folder/{}", s.from, s.to);
                }
            }
            PluginCmd::Enable { id } | PluginCmd::Disable { id } => {
                let p = plugins::find(id)?;
                let on = matches!(command, PluginCmd::Enable { .. });
                let idx = store.active_index();
                store.playsets[idx].set_plugin(&p.manifest.id, on);
                store.save()?;
                println!("{} {} in the playset {}", if on { "enabled" } else { "disabled" }, p.manifest.id, store.playsets[idx].name);
            }
        },
        Cmd::Launch { playset, no_plugins, continue_last, alt, dry_run, force, timeout, args } => {
            let game = open_game(&cli, &store)?;
            let opts = launch::Options {
                playset: playset.clone(),
                use_plugins: !*no_plugins,
                continue_last: *continue_last,
                alternative: *alt,
                extra_args: args.clone(),
                dry_run: *dry_run,
                ready_timeout: std::time::Duration::from_secs(*timeout),
                force: *force,
            };
            let report = launch::launch(&game, &store, &opts, &mut |line| println!("{line}"))?;
            if !report.failed_plugins.is_empty() {
                bail!("{} plugin(s) could not be loaded", report.failed_plugins.len());
            }
        }
        Cmd::Stop => {
            let pids = process::find_processes("stellaris.exe");
            if pids.is_empty() {
                println!("Stellaris is not running");
            }
            for pid in pids {
                process::terminate(pid)?;
                println!("closed pid {pid}");
            }
        }
        Cmd::Inject { dll, pid } => {
            let pid = match pid {
                Some(p) => *p,
                None => *process::find_processes("stellaris.exe").first().context("Stellaris is not running")?,
            };
            process::inject(pid, dll)?;
            println!("loaded {} into pid {pid}", dll.display());
        }
    }
    Ok(())
}
