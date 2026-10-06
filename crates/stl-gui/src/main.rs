//! The window of the Stellaris launcher: pick a playset, switch mods and DLL plugins on and off, press Play.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use stl_core::game::Game;
use stl_core::mods::{self, Kind, Mod};
use stl_core::plugins::{self, Compat, Plugin};
use stl_core::store::Store;
use stl_core::{import, launch, official, pe, process};

enum Msg {
    Line(String),
    Done(Result<launch::Report, String>),
}

struct App {
    store: Store,
    game: Result<Game, String>,
    mods: Vec<Mod>,
    plugins: Vec<Plugin>,
    plugin_problems: Vec<String>,
    log: Vec<String>,
    running: Vec<u32>,
    last_poll: Instant,
    launching: Option<Receiver<Msg>>,
    continue_last: bool,
    use_plugins: bool,
    alternative: Option<usize>,
    new_name: String,
    mod_filter: String,
    add_filter: String,
    game_dir_text: String,
    link_plugin: bool,
}

fn setup_fonts(ctx: &egui::Context) {
    // egui's built-in fonts have no CJK glyphs and mod names are often Chinese: use a system font as the fallback
    let mut fonts = egui::FontDefinitions::default();
    for (name, path) in [("yahei", r"C:\Windows\Fonts\msyh.ttc"), ("simhei", r"C:\Windows\Fonts\simhei.ttf"), ("simsun", r"C:\Windows\Fonts\simsun.ttc")] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.to_string(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.to_string());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

impl App {
    fn new() -> App {
        let store = Store::load().unwrap_or_else(|e| {
            eprintln!("playsets: {e:#}");
            Store::load_from(&std::env::temp_dir().join("stl-playsets.json")).expect("a store in the temp folder")
        });
        let mut app = App {
            store,
            game: Err("not looked for yet".into()),
            mods: Vec::new(),
            plugins: Vec::new(),
            plugin_problems: Vec::new(),
            log: Vec::new(),
            running: Vec::new(),
            last_poll: Instant::now() - Duration::from_secs(10),
            launching: None,
            continue_last: false,
            use_plugins: true,
            alternative: None,
            new_name: String::new(),
            mod_filter: String::new(),
            add_filter: String::new(),
            game_dir_text: String::new(),
            link_plugin: false,
        };
        app.reload();
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

    /// Looks at the disk again: the game, the mod folder, the plugins.
    fn reload(&mut self) {
        let explicit = self.store.game_dir.clone().map(PathBuf::from);
        self.game = Game::open(explicit.as_deref()).map_err(|e| format!("{e:#}"));
        self.mods = self.game.as_ref().map(|g| mods::scan(&g.data_dir)).unwrap_or_default();
        match plugins::list() {
            Ok((p, problems)) => {
                self.plugins = p;
                self.plugin_problems = problems;
            }
            Err(e) => self.plugin_problems = vec![format!("{e:#}")],
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
        self.say("---");
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

    fn mod_name(&self, id: &str) -> (String, Option<String>, Option<&Mod>) {
        match self.mods.iter().find(|m| m.id == id) {
            Some(m) => (m.name.clone(), m.problem.clone(), Some(m)),
            None => (id.to_string(), Some("not in the mod folder".to_string()), None),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_process();
        self.drain_launch();
        if self.launching.is_some() || !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let launching = self.launching.is_some();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Stellaris Launcher");
                ui.separator();
                ui.label("Playset");
                let active = self.store.active_index();
                let mut chosen = active;
                egui::ComboBox::from_id_salt("playset").width(260.0).selected_text(self.store.playsets[active].name.clone()).show_ui(ui, |ui| {
                    for (i, p) in self.store.playsets.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, format!("{}  ({} mods)", p.name, p.mods.iter().filter(|m| m.enabled).count()));
                    }
                });
                if chosen != active {
                    self.store.set_active(chosen);
                    self.save();
                }
                ui.add(egui::TextEdit::singleline(&mut self.new_name).hint_text("new playset").desired_width(140.0));
                if ui.add_enabled(!self.new_name.trim().is_empty(), egui::Button::new("New")).clicked() {
                    match self.store.add_playset(self.new_name.trim()) {
                        Ok(i) => {
                            self.store.set_active(i);
                            self.new_name.clear();
                            self.save();
                        }
                        Err(e) => self.say(format!("{e:#}")),
                    }
                }
                if ui.add_enabled(self.store.playsets.len() > 1, egui::Button::new("Delete")).on_hover_text("delete the active playset").clicked() {
                    let i = self.store.active_index();
                    let n = self.store.playsets[i].name.clone();
                    match self.store.remove_playset(i) {
                        Ok(()) => {
                            self.save();
                            self.say(format!("deleted the playset {n}"));
                        }
                        Err(e) => self.say(format!("{e:#}")),
                    }
                }
                if ui.button("Import official").on_hover_text("copy the playsets of the Paradox Launcher into ours (its database is only read)").clicked() {
                    self.import_official();
                }
                if ui.button("Reload").clicked() {
                    self.reload();
                }
            });
            if let Ok(g) = &self.game {
                ui.small(format!("{}   build {:#010X} ({})   {}", g.settings.version, g.exe_timestamp, pe::describe(g.exe_timestamp), g.dir.display()));
            }
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("bottom").resizable(true).min_height(150.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can = self.game.is_ok() && !launching;
                let running = !self.running.is_empty();
                let label = if launching {
                    "Starting…"
                } else if running {
                    "Running"
                } else {
                    "▶  Play"
                };
                if ui.add_enabled(can && !running, egui::Button::new(egui::RichText::new(label).size(22.0)).min_size(egui::vec2(180.0, 44.0))).clicked() {
                    self.start();
                }
                ui.vertical(|ui| {
                    ui.checkbox(&mut self.continue_last, "Continue the last save");
                    ui.checkbox(&mut self.use_plugins, "Load DLL plugins");
                });
                if let Ok(g) = &self.game {
                    if !g.settings.alternative_executables.is_empty() {
                        let text = match self.alternative {
                            None => "Normal".to_string(),
                            Some(i) => g.settings.alternative_executables.get(i).and_then(|a| a.label.get("en")).cloned().unwrap_or_else(|| format!("Alternative {i}")),
                        };
                        egui::ComboBox::from_id_salt("alt").selected_text(text).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.alternative, None, "Normal");
                            for (i, a) in g.settings.alternative_executables.iter().enumerate() {
                                ui.selectable_value(&mut self.alternative, Some(i), a.label.get("en").cloned().unwrap_or_else(|| format!("Alternative {i}")));
                            }
                        });
                    }
                }
                if !self.running.is_empty() {
                    ui.label(format!("Stellaris is running (pid {})", self.running.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")));
                    if ui.button("Close the game").clicked() {
                        for pid in self.running.clone() {
                            if let Err(e) = process::terminate(pid) {
                                self.say(format!("{e:#}"));
                            }
                        }
                        self.last_poll = Instant::now() - Duration::from_secs(10);
                    }
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for l in &self.log {
                    ui.monospace(l);
                }
            });
        });

        egui::SidePanel::right("plugins").default_width(340.0).min_width(260.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("DLL plugins");
            ui.small("loaded into the game when it is up, for this playset");
            ui.separator();
            let game = self.game.clone().ok();
            let active = self.store.active_index();
            let mut toggles: Vec<(String, bool)> = Vec::new();
            for p in &self.plugins {
                let on = self.store.playsets[active].plugins.iter().find(|x| x.id == p.manifest.id).map(|x| x.enabled).unwrap_or(false);
                let mut v = on;
                let (status, bad) = match game.as_ref().map(|g| p.compat(g)) {
                    Some(Compat::Ok) => ("made for this game build".to_string(), false),
                    Some(Compat::Unchecked) | None => ("build not declared".to_string(), false),
                    Some(Compat::Mismatch { declared, .. }) => (format!("made for {} — will not be loaded", declared.iter().map(|d| format!("{d:#010X}")).collect::<Vec<_>>().join(" / ")), true),
                    Some(Compat::MissingDll(_)) => ("the DLL is missing".to_string(), true),
                };
                ui.horizontal(|ui| {
                    ui.checkbox(&mut v, "");
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(format!("{}  {}", p.manifest.name, p.manifest.version)).strong());
                        ui.small(&p.manifest.description);
                        let text = egui::RichText::new(format!("{}{}", status, if p.linked { " · linked" } else { "" })).small();
                        ui.label(if bad { text.color(egui::Color32::from_rgb(230, 120, 90)) } else { text.weak() });
                    });
                });
                if v != on {
                    toggles.push((p.manifest.id.clone(), v));
                }
                ui.separator();
            }
            if self.plugins.is_empty() {
                ui.label("No plugins installed.");
            }
            for (id, v) in toggles {
                self.store.playsets[active].set_plugin(&id, v);
                self.save();
            }
            for pr in &self.plugin_problems {
                ui.colored_label(egui::Color32::from_rgb(230, 120, 90), pr);
            }
            ui.horizontal(|ui| {
                if ui.button("Install plugin…").on_hover_text("a folder holding stl-plugin.json and the DLL").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Folder of the plugin (stl-plugin.json)").pick_folder() {
                        match plugins::install(&dir, self.link_plugin) {
                            Ok(p) => {
                                let id = p.manifest.id.clone();
                                self.say(format!("{} {}", if self.link_plugin { "linked" } else { "installed" }, id));
                                self.reload();
                            }
                            Err(e) => self.say(format!("{e:#}")),
                        }
                    }
                }
                ui.checkbox(&mut self.link_plugin, "link").on_hover_text("keep the folder where it is (a plugin under development)");
            });
            if !self.plugins.is_empty() {
                ui.small("Remove or link plugins with `stl plugin`.");
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Err(e) = &self.game {
                ui.heading("Stellaris was not found");
                ui.label(e);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.game_dir_text).hint_text("folder of stellaris.exe").desired_width(420.0));
                    if ui.button("Browse…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.game_dir_text = d.to_string_lossy().to_string();
                        }
                    }
                    if ui.button("Use").clicked() {
                        self.store.game_dir = Some(self.game_dir_text.trim().to_string());
                        self.save();
                        self.reload();
                    }
                });
                return;
            }
            let active = self.store.active_index();
            let enabled = self.store.playsets[active].mods.iter().filter(|m| m.enabled).count();
            ui.horizontal(|ui| {
                ui.heading("Mods");
                ui.label(format!("{} in the playset, {} enabled", self.store.playsets[active].mods.len(), enabled));
                ui.add(egui::TextEdit::singleline(&mut self.mod_filter).hint_text("filter").desired_width(160.0));
            });
            ui.small("Load order is top to bottom: a later mod overrides an earlier one.");
            ui.separator();

            let filter = self.mod_filter.to_lowercase();
            let mut action: Option<(usize, &'static str)> = None;
            let mut flag: Option<(usize, bool)> = None;
            let height = (ui.available_height() * 0.55).max(160.0);
            egui::ScrollArea::vertical().id_salt("playset-mods").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                let n = self.store.playsets[active].mods.len();
                for (i, m) in self.store.playsets[active].mods.iter().enumerate() {
                    let (name, problem, info) = self.mod_name(&m.id);
                    if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let mut on = m.enabled;
                        if ui.checkbox(&mut on, "").changed() {
                            flag = Some((i, on));
                        }
                        ui.label(egui::RichText::new(format!("{:>3}", i + 1)).weak().monospace());
                        let kind = match info.map(|x| x.kind) {
                            Some(Kind::Workshop) => "Steam",
                            Some(Kind::ParadoxMods) => "PDX",
                            Some(Kind::Local) => "local",
                            None => "?",
                        };
                        let mut text = egui::RichText::new(&name);
                        if problem.is_some() {
                            text = text.color(egui::Color32::from_rgb(230, 120, 90));
                        } else if !m.enabled {
                            text = text.weak();
                        }
                        let r = ui.label(text);
                        if let Some(p) = &problem {
                            r.on_hover_text(format!("will be left out: {p}"));
                        }
                        ui.small(kind);
                        if let (Some(x), Some(g)) = (info, self.game.as_ref().ok()) {
                            if let Some(sv) = &x.supported_version {
                                if !mods::supports(sv, g.version()) {
                                    ui.small(egui::RichText::new(format!("for {sv}")).color(egui::Color32::from_rgb(220, 180, 80)));
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("remove from the playset").clicked() {
                                action = Some((i, "remove"));
                            }
                            if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                                action = Some((i, "down"));
                            }
                            if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                                action = Some((i, "up"));
                            }
                        });
                    });
                }
            });
            if let Some((i, on)) = flag {
                self.store.playsets[active].mods[i].enabled = on;
                self.save();
            }
            if let Some((i, what)) = action {
                let p = &mut self.store.playsets[active];
                match what {
                    "remove" => {
                        p.mods.remove(i);
                    }
                    "up" => p.mods.swap(i, i - 1),
                    _ => p.mods.swap(i, i + 1),
                }
                self.save();
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("Add mods");
                ui.add(egui::TextEdit::singleline(&mut self.add_filter).hint_text("search the mod folder").desired_width(220.0));
            });
            let f = self.add_filter.to_lowercase();
            let mut add: Option<String> = None;
            egui::ScrollArea::vertical().id_salt("available-mods").auto_shrink([false, false]).show(ui, |ui| {
                let shown = self.mods.iter().filter(|m| self.store.playsets[active].mod_pos(&m.id).is_none() && (f.is_empty() || m.name.to_lowercase().contains(&f)));
                for m in shown.take(200) {
                    ui.horizontal(|ui| {
                        if ui.small_button("+").on_hover_text("add at the end of the playset").clicked() {
                            add = Some(m.id.clone());
                        }
                        let mut t = egui::RichText::new(&m.name);
                        if m.problem.is_some() {
                            t = t.color(egui::Color32::from_rgb(230, 120, 90));
                        }
                        let r = ui.label(t);
                        if let Some(p) = &m.problem {
                            r.on_hover_text(p);
                        }
                    });
                }
            });
            if let Some(id) = add {
                self.store.playsets[active].set_mod(&id, true);
                self.save();
            }
        });
    }
}

impl App {
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
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 760.0]).with_min_inner_size([820.0, 520.0]).with_title("Stellaris Launcher"),
        ..Default::default()
    };
    eframe::run_native(
        "Stellaris Launcher",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
