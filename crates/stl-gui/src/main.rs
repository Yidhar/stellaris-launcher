//! The window of the Stellaris launcher: pick a playset, switch mods and DLL plugins on and off, press Play.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, UiBuilder, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use stl_core::game::Game;
use stl_core::mods::{self, Kind, Mod};
use stl_core::plugins::{self, Compat, Plugin};
use stl_core::store::Store;
use stl_core::{import, launch, official, pe, process};
use theme::*;

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
    filter: String,
    tab: usize,
    browse: bool,
    game_dir_text: String,
    confirm_delete: bool,
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
            filter: String::new(),
            tab: 0,
            browse: false,
            game_dir_text: String::new(),
            confirm_delete: false,
        };
        app.reload();
        // for screenshots while developing: --tab=1 --browse
        for a in std::env::args().skip(1) {
            if let Some(t) = a.strip_prefix("--tab=") {
                app.tab = t.parse().unwrap_or(0);
            } else if a == "--browse" {
                app.browse = true;
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
        self.tab = 2;
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
        self.tab = 2;
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

    fn kind_chip(kind: Kind) -> (&'static str, Color32) {
        match kind {
            Kind::Workshop => ("Steam", ACCENT),
            Kind::ParadoxMods => ("Paradox", PURPLE),
            Kind::Local => ("Local", OK),
        }
    }

    // ---------------------------------------------------------------- sidebar
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Stellaris").font(egui::FontId::new(24.0, bold())).color(TEXT));
        });
        ui.label(RichText::new("LAUNCHER").size(11.5).color(ACCENT).family(bold()));
        ui.add_space(20.0);
        caption(ui, "Playsets");
        ui.add_space(4.0);

        let active = self.store.active_index();
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("playsets").max_height((ui.available_height() - 210.0).max(80.0)).auto_shrink([false, true]).show(ui, |ui| {
            for (i, p) in self.store.playsets.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 42.0), Sense::click());
                row_background(ui, rect, resp.hovered(), i == active);
                ui.scope_builder(UiBuilder::new().max_rect(rect.shrink2(Vec2::new(12.0, 4.0))).layout(Layout::left_to_right(Align::Center)), |ui| {
                    let on = p.mods.iter().filter(|m| m.enabled).count();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add(egui::Label::new(RichText::new(&p.name).family(if i == active { bold() } else { egui::FontFamily::Proportional }).color(if i == active { TEXT } else { Color32::from_rgb(200, 206, 220) })).truncate());
                        let np = p.plugins.iter().filter(|x| x.enabled).count();
                        ui.label(RichText::new(format!("{on} mod{} · {np} plugin{}", if on == 1 { "" } else { "s" }, if np == 1 { "" } else { "s" })).size(11.5).color(FAINT));
                    });
                });
                if resp.clicked() {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.store.set_active(i);
            self.confirm_delete = false;
            self.save();
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            text_field(ui, &mut self.new_name, "New playset", 130.0);
            let ok = !self.new_name.trim().is_empty();
            if ghost_button_colored(ui, "Add", if ok { ACCENT } else { FAINT }, Color32::from_rgb(140, 175, 255)).clicked() && ok {
                match self.store.add_playset(self.new_name.trim()) {
                    Ok(i) => {
                        self.store.set_active(i);
                        self.new_name.clear();
                        self.save();
                    }
                    Err(e) => self.say(format!("{e:#}")),
                }
            }
        });

        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            if let Ok(g) = &self.game {
                ui.label(RichText::new(format!("build {:#010X} · {}", g.exe_timestamp, pe::describe(g.exe_timestamp).split(' ').next().unwrap_or(""))).size(11.5).color(FAINT));
                ui.label(RichText::new(g.settings.version.clone()).size(12.5).color(MUTED));
            }
            ui.add_space(6.0);
            if nav_button(ui, "Reload").clicked() {
                self.reload();
            }
            if nav_button(ui, "Import from Paradox Launcher").on_hover_text("copy its playsets into ours (its database is only read)").clicked() {
                self.import_official();
            }
        });
    }

    // ---------------------------------------------------------------- mods
    fn mods_tab(&mut self, ui: &mut egui::Ui) {
        let active = self.store.active_index();
        let in_playset = self.store.playsets[active].mods.len();
        let available = self.mods.iter().filter(|m| self.store.playsets[active].mod_pos(&m.id).is_none()).count();
        ui.horizontal(|ui| {
            text_field(ui, &mut self.filter, "Search mods", 260.0);
            ui.add_space(8.0);
            if let Some(i) = tabs(ui, &[format!("In this playset  {in_playset}"), format!("Add mods  {available}")], self.browse as usize) {
                self.browse = i == 1;
            }
        });
        ui.add_space(6.0);
        if !self.browse {
            ui.label(RichText::new("Load order runs top to bottom: a later mod overrides an earlier one.").size(12.0).color(FAINT));
        }
        ui.add_space(4.0);
        let filter = self.filter.to_lowercase();
        let width = ui.available_width();
        let version = self.game.as_ref().ok().map(|g| g.version().to_string()).unwrap_or_default();

        let mut flag: Option<(usize, bool)> = None;
        let mut action: Option<(usize, &'static str)> = None;
        let mut add: Option<String> = None;

        card(ui, |ui| {
            ui.set_width(width - 30.0);
            let list_height = ui.available_height() - 4.0;
            egui::ScrollArea::vertical().id_salt(if self.browse { "browse" } else { "playset-mods" }).max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if !self.browse {
                    let n = self.store.playsets[active].mods.len();
                    if n == 0 {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("No mods in this playset yet.").size(15.0).color(MUTED));
                            ui.label(RichText::new("Open “Add mods” to pick from your mod folder, or import your playsets from the Paradox Launcher.").size(12.5).color(FAINT));
                        });
                    }
                    for (i, m) in self.store.playsets[active].mods.iter().enumerate() {
                        let info = self.mods.iter().find(|x| x.id == m.id);
                        let name = info.map(|x| x.name.clone()).unwrap_or_else(|| m.id.clone());
                        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                            continue;
                        }
                        let problem = match info {
                            Some(x) => x.problem.clone(),
                            None => Some("not in the mod folder".to_string()),
                        };
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
                        row_background(ui, rect, resp.hovered(), false);
                        let inner = rect.shrink2(Vec2::new(8.0, 4.0));
                        let right = egui::Rect::from_min_max(egui::pos2(inner.right() - 92.0, inner.top()), inner.max);
                        let left = egui::Rect::from_min_max(inner.min, egui::pos2(inner.right() - 98.0, inner.bottom()));
                        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
                            let mut on = m.enabled;
                            if switch(ui, &mut on).changed() {
                                flag = Some((i, on));
                            }
                            ui.label(RichText::new(format!("{:>3}", i + 1)).size(12.0).color(FAINT).monospace());
                            let mut chips_w = 60.0;
                            let mismatch = info.and_then(|x| x.supported_version.clone()).filter(|sv| !mods::supports(sv, &version));
                            if mismatch.is_some() {
                                chips_w += 96.0;
                            }
                            if problem.is_some() {
                                chips_w += 90.0;
                            }
                            let name_w = (ui.available_width() - chips_w).max(80.0);
                            let color = if problem.is_some() { DANGER } else if m.enabled { TEXT } else { FAINT };
                            ui.allocate_ui_with_layout(Vec2::new(name_w, 22.0), Layout::left_to_right(Align::Center), |ui| {
                                ui.add(egui::Label::new(RichText::new(&name).color(color)).truncate());
                            });
                            if let Some(x) = info {
                                let (t, c) = Self::kind_chip(x.kind);
                                chip(ui, t, c);
                            }
                            if let Some(sv) = &mismatch {
                                chip(ui, &format!("for {sv}"), WARN).on_hover_text("the game will ask whether to load it");
                            }
                            if let Some(p) = &problem {
                                chip(ui, "left out", DANGER).on_hover_text(format!("{p}"));
                            }
                        });
                        ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            if icon_button(ui, "×", true).on_hover_text("remove from the playset").clicked() {
                                action = Some((i, "remove"));
                            }
                            if icon_button(ui, "↓", i + 1 < n).clicked() {
                                action = Some((i, "down"));
                            }
                            if icon_button(ui, "↑", i > 0).clicked() {
                                action = Some((i, "up"));
                            }
                        });
                    }
                } else {
                    let shown = self.mods.iter().filter(|m| self.store.playsets[active].mod_pos(&m.id).is_none() && (filter.is_empty() || m.name.to_lowercase().contains(&filter)));
                    for m in shown.take(300) {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::hover());
                        row_background(ui, rect, resp.hovered(), false);
                        ui.scope_builder(UiBuilder::new().max_rect(rect.shrink2(Vec2::new(8.0, 4.0))).layout(Layout::left_to_right(Align::Center)), |ui| {
                            if icon_button(ui, "+", true).on_hover_text("add at the end of the playset").clicked() {
                                add = Some(m.id.clone());
                            }
                            let chips_w = 70.0 + if m.problem.is_some() { 90.0 } else { 0.0 };
                            let name_w = (ui.available_width() - chips_w).max(80.0);
                            ui.allocate_ui_with_layout(Vec2::new(name_w, 22.0), Layout::left_to_right(Align::Center), |ui| {
                                ui.add(egui::Label::new(RichText::new(&m.name).color(if m.problem.is_some() { DANGER } else { TEXT })).truncate());
                            });
                            let (t, c) = Self::kind_chip(m.kind);
                            chip(ui, t, c);
                            if let Some(p) = &m.problem {
                                chip(ui, "unusable", DANGER).on_hover_text(p.clone());
                            }
                        });
                    }
                }
            });
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
        if let Some(id) = add {
            self.store.playsets[active].set_mod(&id, true);
            self.save();
        }
    }

    // ---------------------------------------------------------------- plugins
    fn plugins_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Native libraries loaded into the game once its window is up. Each says which game build it was made for.").size(12.5).color(MUTED));
        });
        ui.add_space(8.0);
        let game = self.game.clone().ok();
        let active = self.store.active_index();
        let mut toggles: Vec<(String, bool)> = Vec::new();
        let mut remove: Option<String> = None;
        egui::ScrollArea::vertical().id_salt("plugins").auto_shrink([false, true]).max_height((ui.available_height() - 56.0).max(100.0)).show(ui, |ui| {
            for p in &self.plugins {
                let on = self.store.playsets[active].plugins.iter().find(|x| x.id == p.manifest.id).map(|x| x.enabled).unwrap_or(false);
                let mut v = on;
                let (status, color) = match game.as_ref().map(|g| p.compat(g)) {
                    Some(Compat::Ok) => ("made for this game build".to_string(), OK),
                    Some(Compat::Unchecked) | None => ("build not declared".to_string(), MUTED),
                    Some(Compat::Mismatch { declared, .. }) => (format!("made for {} — will not load", declared.iter().map(|d| format!("{d:#010X}")).collect::<Vec<_>>().join(" / ")), DANGER),
                    Some(Compat::MissingDll(_)) => ("the DLL is missing".to_string(), DANGER),
                };
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&p.manifest.name).size(16.0).family(bold()));
                                ui.label(RichText::new(&p.manifest.version).size(12.5).color(FAINT));
                            });
                            if !p.manifest.description.is_empty() {
                                ui.label(RichText::new(&p.manifest.description).size(13.0).color(MUTED));
                            }
                            ui.horizontal(|ui| {
                                chip(ui, &status, color);
                                if p.linked {
                                    chip(ui, "linked", PURPLE);
                                }
                                ui.label(RichText::new(&p.manifest.id).size(11.5).color(FAINT));
                            });
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if switch(ui, &mut v).changed() {
                                toggles.push((p.manifest.id.clone(), v));
                            }
                            if ghost_button_colored(ui, "Remove", FAINT, DANGER).clicked() {
                                remove = Some(p.manifest.id.clone());
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
            if self.plugins.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No plugins installed.").size(15.0).color(MUTED));
                    ui.label(RichText::new("A plugin is a folder with its DLL and a stl-plugin.json.").size(12.5).color(FAINT));
                });
            }
            for pr in &self.plugin_problems {
                ui.label(RichText::new(pr).size(12.0).color(DANGER));
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ghost_button_colored(ui, "+  Install plugin…", ACCENT, Color32::from_rgb(140, 175, 255)).on_hover_text("choose the folder that holds stl-plugin.json and the DLL").clicked() {
                if let Some(dir) = rfd::FileDialog::new().set_title("Folder of the plugin (stl-plugin.json)").pick_folder() {
                    match plugins::install(&dir, false) {
                        Ok(p) => {
                            self.say(format!("installed {}", p.manifest.id));
                            self.reload();
                        }
                        Err(e) => self.say(format!("{e:#}")),
                    }
                }
            }
            if ghost_button(ui, "Link a plugin under development…").on_hover_text("keep its folder where it is").clicked() {
                if let Some(dir) = rfd::FileDialog::new().set_title("Folder of the plugin (stl-plugin.json)").pick_folder() {
                    match plugins::install(&dir, true) {
                        Ok(p) => {
                            self.say(format!("linked {}", p.manifest.id));
                            self.reload();
                        }
                        Err(e) => self.say(format!("{e:#}")),
                    }
                }
            }
        });
        for (id, v) in toggles {
            self.store.playsets[active].set_plugin(&id, v);
            self.save();
        }
        if let Some(id) = remove {
            match plugins::remove(&id) {
                Ok(()) => {
                    self.say(format!("removed {id}"));
                    self.reload();
                }
                Err(e) => self.say(format!("{e:#}")),
            }
        }
    }

    // ---------------------------------------------------------------- log
    fn log_tab(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        card(ui, |ui| {
            ui.set_width(width - 30.0);
            let h = ui.available_height() - 4.0;
            egui::ScrollArea::vertical().stick_to_bottom(true).max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                if self.log.is_empty() {
                    ui.label(RichText::new("What the launcher does appears here: which mods are written, which plugins are loaded.").color(FAINT));
                }
                for l in &self.log {
                    let color = if l.contains("could not") || l.contains("failed") || l.contains("!!") {
                        DANGER
                    } else if l.contains("left out") || l.contains("warning") || l.contains("skipped") {
                        WARN
                    } else if l.contains("loaded") || l.contains("running") || l.contains("started") {
                        OK
                    } else {
                        MUTED
                    };
                    ui.label(RichText::new(l).monospace().color(color));
                }
            });
        });
    }

    // ---------------------------------------------------------------- play bar
    fn play_bar(&mut self, ui: &mut egui::Ui) {
        let launching = self.launching.is_some();
        let running = !self.running.is_empty();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.horizontal(|ui| {
                switch(ui, &mut self.continue_last);
                ui.label(RichText::new("Continue last save").color(MUTED));
            });
            ui.horizontal(|ui| {
                switch(ui, &mut self.use_plugins);
                ui.label(RichText::new("Load DLL plugins").color(MUTED));
            });
            if let Ok(g) = &self.game {
                if !g.settings.alternative_executables.is_empty() {
                    let label = |i: Option<usize>| match i {
                        None => "Standard".to_string(),
                        Some(i) => g.settings.alternative_executables.get(i).and_then(|a| a.label.get("en")).cloned().unwrap_or_else(|| format!("Alternative {i}")),
                    };
                    egui::ComboBox::from_id_salt("alt").selected_text(label(self.alternative)).width(190.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.alternative, None, label(None));
                        for i in 0..g.settings.alternative_executables.len() {
                            ui.selectable_value(&mut self.alternative, Some(i), label(Some(i)));
                        }
                    });
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can = self.game.is_ok() && !launching && !running;
                let text = if launching {
                    "Starting…"
                } else if running {
                    "Running"
                } else {
                    "▶  Play"
                };
                if primary_button(ui, text, Vec2::new(190.0, 46.0), can).clicked() {
                    self.start();
                }
                if running && ghost_button_colored(ui, "Close game", MUTED, DANGER).clicked() {
                    for pid in self.running.clone() {
                        if let Err(e) = process::terminate(pid) {
                            self.say(format!("{e:#}"));
                        }
                    }
                    self.last_poll = Instant::now() - Duration::from_secs(10);
                }
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_process();
        self.drain_launch();
        if self.launching.is_some() || !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::SidePanel::left("sidebar")
            .exact_width(256.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(egui::Margin::symmetric(16, 18)).stroke(egui::Stroke::new(1.0, LINE)))
            .show(ctx, |ui| self.sidebar(ui));

        egui::TopBottomPanel::bottom("playbar")
            .exact_height(76.0)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(egui::Margin::symmetric(24, 15)).stroke(egui::Stroke::new(1.0, LINE)))
            .show(ctx, |ui| self.play_bar(ui));

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(26, 22))).show(ctx, |ui| {
            if let Err(e) = &self.game {
                let e = e.clone();
                ui.label(RichText::new("Stellaris was not found").font(egui::FontId::new(24.0, bold())));
                ui.label(RichText::new(e).color(MUTED));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    text_field(ui, &mut self.game_dir_text, "Folder of stellaris.exe", 420.0);
                    if ghost_button(ui, "Browse…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.game_dir_text = d.to_string_lossy().to_string();
                        }
                    }
                    if ghost_button_colored(ui, "Use this folder", ACCENT, TEXT).clicked() {
                        self.store.game_dir = Some(self.game_dir_text.trim().to_string());
                        self.save();
                        self.reload();
                    }
                });
                return;
            }
            let active = self.store.active_index();
            let name = self.store.playsets[active].name.clone();
            let total = self.store.playsets[active].mods.len();
            let enabled = self.store.playsets[active].mods.iter().filter(|m| m.enabled).count();
            let plugins_on = self.store.playsets[active].plugins.iter().filter(|p| p.enabled).count();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(&name).font(egui::FontId::new(26.0, bold())));
                    ui.horizontal(|ui| {
                        chip(ui, &format!("{total} mod{}", if total == 1 { "" } else { "s" }), MUTED);
                        chip(ui, &format!("{enabled} enabled"), ACCENT);
                        chip(ui, &format!("{plugins_on} plugin{}", if plugins_on == 1 { "" } else { "s" }), PURPLE);
                        if self.launching.is_some() {
                            chip(ui, "starting the game…", ACCENT);
                        } else if let Some(pid) = self.running.first() {
                            chip(ui, &format!("● Stellaris is running · pid {pid}"), OK);
                        } else if let Some(last) = self.log.last().filter(|l| l.contains("could not")) {
                            chip(ui, &last.chars().take(70).collect::<String>(), DANGER);
                        }
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if self.store.playsets.len() > 1 {
                        if self.confirm_delete {
                            if ghost_button_colored(ui, "Really delete?", DANGER, DANGER).clicked() {
                                let i = self.store.active_index();
                                let n = self.store.playsets[i].name.clone();
                                if self.store.remove_playset(i).is_ok() {
                                    self.save();
                                    self.say(format!("deleted the playset {n}"));
                                }
                                self.confirm_delete = false;
                            }
                            if ghost_button(ui, "Keep").clicked() {
                                self.confirm_delete = false;
                            }
                        } else if ghost_button_colored(ui, "Delete playset", FAINT, DANGER).clicked() {
                            self.confirm_delete = true;
                        }
                    }
                });
            });
            ui.add_space(14.0);
            let labels = ["Mods".to_string(), format!("DLL plugins  {}", self.plugins.len()), "Log".to_string()];
            if let Some(i) = tabs(ui, &labels, self.tab) {
                self.tab = i;
            }
            let rule = ui.max_rect().left_top();
            let _ = rule;
            ui.add_space(8.0);
            match self.tab {
                0 => self.mods_tab(ui),
                1 => self.plugins_tab(ui),
                _ => self.log_tab(ui),
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 780.0]).with_min_inner_size([960.0, 600.0]).with_title("Stellaris Launcher").with_icon(theme::icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Stellaris Launcher",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
