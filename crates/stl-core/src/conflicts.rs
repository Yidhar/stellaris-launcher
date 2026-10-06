//! What the mods of a playset do to each other and to the game: problems that stop them working, the files and definitions they override,
//! and a load order that respects what they need.
//!
//! How the game decides, measured on 4.5.2 with two test mods loaded in both orders (docs/CONFLICTS.md):
//! - **The same file** (same path) in several mods: only the one loaded **last** is read; vanilla's is replaced too.
//! - **The same definition** (same key) in different files of one folder: the files are read in **file-name order** whatever the mod order,
//!   and the folder's rule decides: the last one read wins (LIOS, e.g. technology, strategic_resources), the first one (FIOS, e.g. events,
//!   scripted_variables), every copy is kept (traits: two definitions of one trait both exist), or the definitions merge (on_actions).
//!   The rules per folder come from the CWTools Stellaris config (`config/override_modes.cwt`, MIT, github.com/Aa728848/cwtools-stellaris-config),
//!   corrected where the measurement disagreed.
//! - A descriptor's `replace_path` hides that folder of everything loaded before the mod.
//!
//! So only whole-file overrides depend on the load order; the order fixes problems only there (a patch must come after what it patches).

use crate::mods::{self, Mod};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// the definition read last is used
    Lios,
    /// the definition read first is used; later ones are dropped
    Fios,
    /// definitions are merged (not a conflict)
    Merge,
    /// every definition is kept: two of the same name break things
    Duplicates,
    /// not known: reported as possible only
    Unknown,
}

/// (folder, rule). Longest matching folder wins. Measured on 4.5.2: technology, scripted_triggers, scripted_effects, agreement_term_values
/// (LIOS; the CWT config says FIOS for the last), strategic_resources (LIOS, the CWT config says DUPL), component_templates (FIOS),
/// traits (both kept, the CWT config says "cannot override"), events, scripted_variables and section_templates (FIOS; the game logs the
/// later copy as a duplicate). The rest is the CWT config's.
const RULES: &[(&str, Rule)] = &[
    ("common/anomalies", Rule::Lios), ("common/armies", Rule::Lios), ("common/artifact_actions", Rule::Lios), ("common/ascension_perks", Rule::Lios),
    ("common/ascension_perk_categories", Rule::Lios), ("common/attitudes", Rule::Lios), ("common/bombardment_stances", Rule::Lios),
    ("common/buildings", Rule::Lios), ("common/button_effects", Rule::Lios), ("common/bypass", Rule::Lios), ("common/casus_belli", Rule::Lios),
    ("common/colony_automation_categories", Rule::Lios), ("common/colony_types", Rule::Lios), ("common/council_agendas", Rule::Lios),
    ("common/country_focus", Rule::Lios), ("common/country_limits", Rule::Lios), ("common/country_types", Rule::Lios),
    ("common/crisis_levels", Rule::Lios), ("common/crisis_objectives", Rule::Lios), ("common/decisions", Rule::Lios), ("common/deposits", Rule::Lios),
    ("common/diplomatic_actions", Rule::Lios), ("common/diplomatic_economy", Rule::Lios), ("common/districts", Rule::Lios),
    ("common/economic_categories", Rule::Lios), ("common/edicts", Rule::Lios), ("common/espionage_assets", Rule::Lios),
    ("common/espionage_operation_types", Rule::Lios), ("common/ethics", Rule::Lios), ("common/game_rules", Rule::Lios), ("common/governments", Rule::Lios),
    ("common/governments/civics", Rule::Lios), ("common/leader_classes", Rule::Lios), ("common/mandates", Rule::Lios), ("common/map_modes", Rule::Lios),
    ("common/megastructures", Rule::Lios), ("common/personalities", Rule::Lios), ("common/planet_modifiers", Rule::Lios), ("common/policies", Rule::Lios),
    ("common/pop_categories", Rule::Lios), ("common/pop_faction_types", Rule::Lios), ("common/pop_jobs", Rule::Lios), ("common/relics", Rule::Lios),
    ("common/resolution_groups", Rule::Lios), ("common/resolutions", Rule::Lios), ("common/script_values", Rule::Lios),
    ("common/scripted_effects", Rule::Lios), ("common/scripted_modifiers", Rule::Lios), ("common/scripted_triggers", Rule::Lios),
    ("common/sector_focuses", Rule::Lios), ("common/ship_sizes", Rule::Lios), ("common/situations", Rule::Lios),
    ("common/specialist_subject_perks", Rule::Lios), ("common/specialist_subject_types", Rule::Lios), ("common/species_archetypes", Rule::Lios),
    ("common/species_classes", Rule::Lios), ("common/species_rights", Rule::Lios), ("common/specimens", Rule::Lios), ("common/star_classes", Rule::Lios),
    ("common/starbase_buildings", Rule::Lios), ("common/starbase_levels", Rule::Lios), ("common/starbase_modules", Rule::Lios),
    ("common/starbase_types", Rule::Lios), ("common/static_modifiers", Rule::Lios), ("common/subjects", Rule::Lios), ("common/system_types", Rule::Lios),
    ("common/technology", Rule::Lios), ("common/trade_conversions", Rule::Lios), ("common/tradition_categories", Rule::Lios),
    ("common/traditions", Rule::Lios), ("common/war_goals", Rule::Lios), ("common/agreement_presets", Rule::Lios), ("common/agreement_term_values", Rule::Lios), ("common/agreement_resources", Rule::Lios),
    ("common/agreement_terms", Rule::Lios), ("common/ai_budget", Rule::Lios), ("common/ai_espionage", Rule::Lios), ("common/colony_automation", Rule::Lios),
    ("common/colony_automation_exceptions", Rule::Lios), ("common/country_container", Rule::Lios), ("common/country_customization", Rule::Lios),
    ("common/observation_station_missions", Rule::Lios), ("common/opinion_modifiers", Rule::Lios), ("common/planet_classes", Rule::Lios),
    ("common/ship_categories", Rule::Lios), ("common/ship_sets", Rule::Lios), ("common/storm_types", Rule::Lios), ("common/zone_slots", Rule::Lios),
    ("common/zones", Rule::Lios), ("common/economic_plans", Rule::Lios), ("common/strategic_resources", Rule::Lios),
    ("common/component_sets", Rule::Fios), ("common/component_templates", Rule::Fios),
    ("common/event_chains", Rule::Fios), ("common/global_ship_designs", Rule::Fios), ("common/governments/authorities", Rule::Fios),
    ("common/scripted_variables", Rule::Fios), ("common/ship_behaviors", Rule::Fios),
    ("common/solar_system_initializers", Rule::Fios), ("common/special_projects", Rule::Fios), ("common/start_screen_messages", Rule::Fios),
    ("events", Rule::Fios),
    ("common/name_lists", Rule::Duplicates), ("common/terraform", Rule::Duplicates), ("common/traits", Rule::Duplicates),
    ("common/achievements", Rule::Duplicates), ("common/section_templates", Rule::Fios),
    ("common/on_actions", Rule::Merge), ("common/country_limits/ownership_limits", Rule::Merge), ("common/job_tags", Rule::Merge),
    ("common/component_tags", Rule::Merge), ("common/trait_tags", Rule::Merge), ("common/defines", Rule::Merge),
    // not measured (the game logs nothing about it): which duplicate is used is not known, so only shown as possible. scripted_loc:
    // the CWTools config says FIOS, Irony does not list it
    ("localisation", Rule::Unknown), ("common/scripted_loc", Rule::Unknown),
];

/// The rule of a folder (`common/technology/category` falls back to `common/technology` when it has none of its own).
pub fn rule_for(folder: &str) -> Rule {
    let f = folder.to_lowercase();
    RULES
        .iter()
        .filter(|(d, _)| f == *d || f.starts_with(&format!("{d}/")))
        .max_by_key(|(d, _)| d.len())
        .map(|(_, r)| *r)
        .unwrap_or(Rule::Unknown)
}

// ------------------------------------------------------------------ reading definitions

#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// `key = { … }` at the top
    Blocks,
    /// the `id = …` of each event
    Events,
    /// `@name = value` at the top
    Variables,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Eq,
    Open,
    Close,
}

fn tokens(text: &[u8]) -> Vec<(Tok, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    if text.starts_with(b"\xEF\xBB\xBF") {
        i = 3;
    }
    while i < text.len() {
        let c = text[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                while i < text.len() && text[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                out.push((Tok::Open, line));
                i += 1;
            }
            b'}' => {
                out.push((Tok::Close, line));
                i += 1;
            }
            b'=' => {
                out.push((Tok::Eq, line));
                i += 1;
                if text.get(i) == Some(&b'=') {
                    i += 1;
                }
            }
            b'<' | b'>' | b'!' => {
                // comparisons: not assignments
                out.push((Tok::Word(String::new()), line));
                i += 1;
                if text.get(i) == Some(&b'=') {
                    i += 1;
                }
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < text.len() && text[i] != b'"' {
                    if text[i] == b'\\' {
                        i += 1;
                    } else if text[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                let s = String::from_utf8_lossy(&text[start.min(text.len())..i.min(text.len())]).to_string();
                out.push((Tok::Word(s), line));
                i += 1;
            }
            _ => {
                let start = i;
                while i < text.len() && !matches!(text[i], b' ' | b'\t' | b'\r' | b'\n' | b'{' | b'}' | b'=' | b'#' | b'"' | b'<' | b'>') {
                    i += 1;
                }
                out.push((Tok::Word(String::from_utf8_lossy(&text[start..i]).to_string()), line));
            }
        }
    }
    out
}

/// Top-level names that are a type, not a name: every entry of the folder is called so (`ship_section_template = { key = "x" … }`).
/// Such an entry is known by its inner `key` (or `name`), as the game does; a name used for several blocks of one file is treated the same.
const GENERIC: &[&str] = &[
    "ship_section_template", "ship_behavior", "special_project", "defined_text", "ship_design", "component_set", "part",
    "weapon_component_template", "utility_component_template", "strike_craft_component_template", "component_template",
];

fn keys(text: &[u8], mode: Mode) -> Vec<Key> {
    let t = tokens(text);
    let mut out = Vec::new();
    // Blocks: (name, line, the inner key/name)
    let mut blocks: Vec<(String, u32, Option<String>)> = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < t.len() {
        match &t[i].0 {
            Tok::Open => depth += 1,
            Tok::Close => depth = (depth - 1).max(0),
            Tok::Word(w) => {
                let assigned = matches!(t.get(i + 1), Some((Tok::Eq, _)));
                let block = assigned && matches!(t.get(i + 2), Some((Tok::Open, _)));
                match mode {
                    Mode::Blocks if depth == 0 && block && !w.starts_with('@') && w != "namespace" && !w.is_empty() => {
                        blocks.push((w.clone(), t[i].1, None))
                    }
                    Mode::Blocks if depth == 1 && assigned && (w == "key" || w == "name") => {
                        if let (Some(b), Some((Tok::Word(v), _))) = (blocks.last_mut(), t.get(i + 2)) {
                            if b.2.is_none() && !v.is_empty() {
                                b.2 = Some(v.clone());
                            }
                        }
                    }
                    Mode::Variables if depth == 0 && assigned && w.starts_with('@') => out.push(Key { name: w.clone(), line: t[i].1 }),
                    Mode::Events if depth == 1 && assigned && w.eq_ignore_ascii_case("id") => {
                        if let Some((Tok::Word(id), line)) = t.get(i + 2) {
                            out.push(Key { name: id.clone(), line: *line });
                        }
                    }
                    _ => {}
                }
            }
            Tok::Eq => {}
        }
        i += 1;
    }
    if mode == Mode::Blocks {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for b in &blocks {
            *count.entry(b.0.as_str()).or_default() += 1;
        }
        for (name, line, inner) in &blocks {
            if GENERIC.contains(&name.as_str()) || count[name.as_str()] > 1 {
                if let Some(inner) = inner {
                    out.push(Key { name: format!("{name}:{inner}"), line: *line });
                }
            } else {
                out.push(Key { name: name.clone(), line: *line });
            }
        }
    }
    out
}

/// The keys of a localisation file (`l_english:` then ` KEY:0 "text"`), each prefixed with its language.
fn loc_keys(text: &[u8]) -> Vec<Key> {
    let s = String::from_utf8_lossy(text);
    let mut lang = String::new();
    let mut out = Vec::new();
    for (n, raw) in s.lines().enumerate() {
        let l = raw.trim_start_matches('\u{feff}').trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some(colon) = l.find(':') else { continue };
        let key = &l[..colon];
        let rest = l[colon + 1..].trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
        if lang.is_empty() && key.starts_with("l_") && rest.is_empty() {
            lang = key.to_string();
            continue;
        }
        if rest.starts_with('"') && !key.is_empty() && !key.contains(' ') {
            out.push(Key { name: format!("{lang}:{key}"), line: n as u32 + 1 });
        }
    }
    out
}

/// A script file that starts with a byte-order mark directly followed by a definition: the game takes the mark as part of that first
/// name (measured), so that definition gets another name and overrides nothing. A comment after the mark is harmless (vanilla has those).
pub fn bom_breaks_first_key(text: &[u8]) -> bool {
    let Some(rest) = text.strip_prefix(b"\xEF\xBB\xBF") else { return false };
    match rest.iter().position(|b| !b" \t\r\n".contains(b)) {
        Some(i) => rest[i] != b'#',
        None => false,
    }
}

// ------------------------------------------------------------------ the files of each source

#[derive(Debug, Clone)]
enum Root {
    Dir(PathBuf),
    Zip(PathBuf),
    None,
}

fn root_of(m: &Mod) -> Root {
    match (&m.path, &m.archive) {
        (Some(p), _) if p.is_dir() => Root::Dir(p.clone()),
        (_, Some(a)) if a.is_file() => Root::Zip(a.clone()),
        _ => Root::None,
    }
}

/// (lower-case relative path, relative path as on disk) of every file below the top folder.
fn list(root: &Root) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match root {
        Root::Dir(d) => {
            fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
                let Ok(rd) = std::fs::read_dir(dir) else { return };
                for e in rd.flatten() {
                    let p = e.path();
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        walk(base, &p, out);
                    } else if let Ok(rel) = p.strip_prefix(base) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if rel.contains('/') {
                            out.push((rel.to_lowercase(), rel));
                        }
                    }
                }
            }
            walk(d, d, &mut out);
        }
        Root::Zip(z) => {
            if let Ok(f) = std::fs::File::open(z) {
                if let Ok(a) = zip::ZipArchive::new(std::io::BufReader::new(f)) {
                    for n in a.file_names() {
                        let rel = n.replace('\\', "/");
                        if rel.contains('/') && !rel.ends_with('/') {
                            out.push((rel.to_lowercase(), rel));
                        }
                    }
                }
            }
        }
        Root::None => {}
    }
    out
}

fn read_many(root: &Root, rels: &[String]) -> HashMap<String, Vec<u8>> {
    let mut out = HashMap::new();
    match root {
        Root::Dir(d) => {
            for r in rels {
                if let Ok(b) = std::fs::read(d.join(r)) {
                    out.insert(r.clone(), b);
                }
            }
        }
        Root::Zip(z) => {
            use std::io::Read;
            if let Ok(f) = std::fs::File::open(z) {
                if let Ok(mut a) = zip::ZipArchive::new(std::io::BufReader::new(f)) {
                    for r in rels {
                        if let Ok(mut e) = a.by_name(r) {
                            let mut b = Vec::new();
                            if e.read_to_end(&mut b).is_ok() {
                                out.insert(r.clone(), b);
                            }
                        }
                    }
                }
            }
        }
        Root::None => {}
    }
    out
}

/// Which files are read for their definitions, and how.
fn keyed(rel_lower: &str) -> Option<Mode> {
    let (folder, name) = rel_lower.rsplit_once('/')?;
    if rel_lower.starts_with("localisation/") && name.ends_with(".yml") {
        return None; // handled by loc_keys
    }
    if !name.ends_with(".txt") {
        return None;
    }
    if folder == "events" {
        return Some(Mode::Events);
    }
    if folder == "common/scripted_variables" {
        return Some(Mode::Variables);
    }
    if folder.starts_with("common/") && !folder.starts_with("common/inline_scripts") && rule_for(folder) != Rule::Merge {
        return Some(Mode::Blocks);
    }
    None
}

// ------------------------------------------------------------------ the report

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IssueKind {
    /// the playset names a mod that is not installed (args: its id)
    MissingMod,
    /// installed but its files are gone (args: why)
    Unloadable,
    /// needs a mod that is not installed (args: its name)
    MissingDependency,
    /// needs a mod that is installed but not on (args: its name)
    DisabledDependency,
    /// needs a mod that is loaded after it (args: its name)
    DependencyAfter,
    /// on twice (args: the other's name)
    Duplicate,
    /// made for another game version and replaces game files whole (args: its version, number of files)
    OutdatedReplacesVanilla,
    /// files whose first definition is renamed by a byte-order mark (args: count, first file)
    BomFirstKey,
    /// its definitions that the game's own win over (args: count, an example "folder: key", the reason)
    IneffectiveOverride,
    /// definitions kept twice, which breaks them (args: count, an example "folder: key")
    DuplicateDefinitions,
    /// loaded before the mod it patches, so its files lose (args: the other's name)
    PatchBeforeTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    /// position in the load order (None: the playset itself)
    pub mod_index: Option<usize>,
    pub kind: IssueKind,
    pub args: Vec<String>,
}

/// One file several mods have (the source numbers: 0 is the game, i + 1 the i-th mod in load order).
#[derive(Debug, Clone, PartialEq)]
pub struct FileOverlap {
    pub path: String,
    /// in load order; the last one is read
    pub sources: Vec<usize>,
    /// the winner patches the others (by its dependencies, or by overriding their own files)
    pub intended: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    pub source: usize,
    pub file: String,
    pub line: u32,
}

/// One definition several mods (or a mod and the game, in a folder that keeps duplicates) have.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyOverlap {
    pub folder: String,
    pub key: String,
    pub rule: Rule,
    /// in the order the game reads them
    pub defs: Vec<Def>,
    /// the definition used (None: all kept, or not known)
    pub winner: Option<usize>,
    pub severity: Severity,
    pub intended: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModSummary {
    /// files or definitions of other mods it overrides
    pub wins: usize,
    /// its files or definitions another mod overrides
    pub loses: usize,
    pub replaces_vanilla_files: usize,
    pub overrides_vanilla_keys: usize,
    /// its definitions the game's own beat: (folder, key, its file, the game's file)
    pub ineffective: Vec<(String, String, String, String)>,
    /// its definitions read before the game's in a folder where the last one wins: fallbacks the game's own replace (placeholders for
    /// content of DLC the player may not own, `!!!ph_…`, `000_…_dummy`), counted, not a problem
    pub fallbacks: usize,
    /// the mods it patches (positions in the load order)
    pub patches: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    /// 0: the game; i + 1: the i-th enabled mod in load order
    pub sources: Vec<String>,
    pub files: Vec<FileOverlap>,
    pub keys: Vec<KeyOverlap>,
    pub issues: Vec<Issue>,
    pub per_mod: Vec<ModSummary>,
    pub files_scanned: usize,
    pub definitions_read: usize,
    pub millis: u128,
}

impl Report {
    /// The overlaps a mod (position in the load order) takes part in.
    pub fn files_of(&self, m: usize) -> impl Iterator<Item = &FileOverlap> {
        self.files.iter().filter(move |f| f.sources.contains(&(m + 1)))
    }
    pub fn keys_of(&self, m: usize) -> impl Iterator<Item = &KeyOverlap> {
        self.keys.iter().filter(move |k| k.defs.iter().any(|d| d.source == m + 1))
    }
}

pub struct Input<'a> {
    pub game_dir: &'a Path,
    /// `4.5.2`
    pub game_version: &'a str,
    /// the enabled mods, in load order
    pub mods: Vec<&'a Mod>,
    /// every installed mod (to tell "not installed" from "not on")
    pub installed: &'a [Mod],
    /// ids the playset lists that are not installed
    pub missing: Vec<String>,
}

/// `v4.5.1` and `4.5.2`: the same major and minor version (a patch release seldom changes the files a mod replaces).
fn same_minor(supported: &str, game: &str) -> bool {
    let two = |v: &str| -> Vec<String> { v.trim().trim_start_matches(['v', 'V']).split('.').take(2).map(str::to_string).collect() };
    let (a, b) = (two(supported), two(game));
    a.len() == 2 && a == b
}

/// The enabled mods of a playset in load order, and the ids it lists that are not installed.
pub fn playset_mods<'a>(playset: &crate::store::Playset, installed: &'a [Mod]) -> (Vec<&'a Mod>, Vec<String>) {
    let mut mods = Vec::new();
    let mut missing = Vec::new();
    for pm in playset.mods.iter().filter(|m| m.enabled) {
        match installed.iter().find(|m| m.id == pm.id) {
            Some(m) => mods.push(m),
            None => missing.push(pm.id.clone()),
        }
    }
    (mods, missing)
}

fn name_key(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Reads everything and builds the report. `progress` gets 0..1.
pub fn analyze(input: &Input, progress: &(dyn Fn(f32) + Sync)) -> Report {
    let started = std::time::Instant::now();
    let n = input.mods.len();
    let mut report = Report { sources: vec!["Stellaris".into()], per_mod: vec![ModSummary::default(); n], ..Default::default() };
    report.sources.extend(input.mods.iter().map(|m| m.name.clone()));
    let roots: Vec<Root> = std::iter::once(Root::Dir(input.game_dir.to_path_buf())).chain(input.mods.iter().map(|m| root_of(m))).collect();

    // 1. the files of every source
    let lists: Vec<Vec<(String, String)>> = {
        let done = std::sync::atomic::AtomicUsize::new(0);
        let total = roots.len();
        std::thread::scope(|s| {
            let handles: Vec<_> = roots
                .iter()
                .map(|r| {
                    let done = &done;
                    s.spawn(move || {
                        let l = list(r);
                        let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        progress(0.3 * k as f32 / total as f32);
                        l
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    // a mod's file counts only in a folder the game has (not `.git/`, notes, archives of old versions, …)
    let game_folders: HashSet<String> = lists[0].iter().filter_map(|(l, _)| l.split('/').next().filter(|t| !t.starts_with('.')).map(str::to_string)).collect();
    let lists: Vec<Vec<(String, String)>> = lists
        .into_iter()
        .enumerate()
        .map(|(_, l)| {
            l.into_iter()
                .filter(|(lower, _)| lower.split('/').next().is_some_and(|top| game_folders.contains(top)) && !lower.split('/').any(|seg| seg.starts_with('.')))
                .collect()
        })
        .collect();
    let vanilla: HashSet<&str> = lists[0].iter().map(|(l, _)| l.as_str()).collect();
    report.files_scanned = lists.iter().map(|l| l.len()).sum();

    // 2. who provides each path (load order; replace_path hides the folder of everything before)
    let mut owners: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut original: HashMap<(usize, &str), &str> = HashMap::new();
    for (src, l) in lists.iter().enumerate() {
        if src > 0 {
            for rp in &input.mods[src - 1].replace_paths {
                let prefix = format!("{rp}/");
                for (path, v) in owners.iter_mut() {
                    if path.starts_with(&prefix) {
                        v.clear();
                    }
                }
            }
        }
        for (lower, orig) in l {
            owners.entry(lower.as_str()).or_default().push(src);
            original.insert((src, lower.as_str()), orig.as_str());
        }
    }
    owners.retain(|_, v| !v.is_empty());

    // which mod patches which: it declares the dependency, or it overrides files only that mod brings (not the game's)
    let by_name: HashMap<String, usize> = input.mods.iter().enumerate().map(|(i, m)| (name_key(&m.name), i)).collect();
    let mut patches: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (i, m) in input.mods.iter().enumerate() {
        for d in &m.dependencies {
            if let Some(&j) = by_name.get(&name_key(d)) {
                if j != i {
                    patches[i].insert(j);
                }
            }
        }
    }
    // by the files, whatever the order: a mod whose own files (not the game's) are mostly files another mod brings too is a patch of it
    // (a compatibility patch is little more than the other mod's files, changed); two mods sharing a few files are not
    let mut own_files = vec![0usize; n];
    let mut shared: HashMap<(usize, usize), usize> = HashMap::new();
    for (path, src) in &owners {
        if vanilla.contains(path) || path.ends_with("descriptor.mod") {
            continue;
        }
        let mods_here: Vec<usize> = src.iter().filter(|&&s| s > 0).map(|s| s - 1).collect();
        for &m in &mods_here {
            own_files[m] += 1;
        }
        if mods_here.len() == 2 {
            let (a, b) = (mods_here[0].min(mods_here[1]), mods_here[0].max(mods_here[1]));
            *shared.entry((a, b)).or_default() += 1;
        }
    }
    for (&(a, b), &k) in &shared {
        let declared = patches[a].contains(&b) || patches[b].contains(&a);
        if declared {
            continue;
        }
        let (ra, rb) = (k as f32 / own_files[a].max(1) as f32, k as f32 / own_files[b].max(1) as f32);
        if ra >= 0.5 && ra > 2.0 * rb {
            patches[a].insert(b);
        } else if rb >= 0.5 && rb > 2.0 * ra {
            patches[b].insert(a);
        }
    }
    // a patch relation both ways (by dependency) is ignored
    for i in 0..n {
        for j in patches[i].clone() {
            if patches[j].contains(&i) {
                patches[i].remove(&j);
                patches[j].remove(&i);
            }
        }
    }
    for (i, p) in patches.iter().enumerate() {
        let mut v: Vec<usize> = p.iter().copied().collect();
        v.sort();
        report.per_mod[i].patches = v;
    }

    // 3. whole files
    let mut file_list: Vec<(&str, &Vec<usize>)> = owners.iter().map(|(k, v)| (*k, v)).collect();
    file_list.sort_by(|a, b| a.0.cmp(b.0));
    for (path, src) in &file_list {
        let name = path.rsplit('/').next().unwrap_or("");
        if name == "descriptor.mod" || name.starts_with("thumbnail.") {
            continue;
        }
        for &s in src.iter().filter(|&&s| s > 0) {
            if src[0] == 0 {
                report.per_mod[s - 1].replaces_vanilla_files += 1;
            }
        }
        let mods_here: Vec<usize> = src.iter().copied().filter(|&s| s > 0).collect();
        if mods_here.len() < 2 {
            continue;
        }
        let winner = *mods_here.last().unwrap();
        let intended = mods_here[..mods_here.len() - 1].iter().all(|&o| patches[winner - 1].contains(&(o - 1)));
        report.per_mod[winner - 1].wins += 1;
        for &o in &mods_here[..mods_here.len() - 1] {
            report.per_mod[o - 1].loses += 1;
        }
        report.files.push(FileOverlap { path: original.get(&(winner, *path)).map(|s| s.to_string()).unwrap_or_else(|| path.to_string()), sources: src.to_vec(), intended });
    }
    progress(0.4);

    // 4. definitions: the files that are read (the winner of each path), per folder, in file-name order
    let mut per_folder: HashMap<String, Vec<(String, usize, Option<Mode>)>> = HashMap::new();
    for (path, src) in &owners {
        let winner = *src.last().unwrap();
        let loc = path.starts_with("localisation/") && path.ends_with(".yml");
        let mode = keyed(path);
        if mode.is_none() && !loc {
            continue;
        }
        // the game's definitions only matter where a mod defines something too
        let folder = path.rsplit_once('/').map(|x| x.0).unwrap_or("").to_string();
        per_folder.entry(folder).or_default().push((path.to_string(), winner, mode));
    }
    let mod_folders: HashSet<String> = owners
        .iter()
        .filter(|(_, s)| s.iter().any(|&x| x > 0))
        .filter_map(|(p, _)| p.rsplit_once('/').map(|x| x.0.to_string()))
        .collect();
    per_folder.retain(|f, _| mod_folders.contains(f));
    // read and parse, a source at a time (a zip is opened once), on all cores
    let mut wanted: HashMap<usize, Vec<String>> = HashMap::new();
    for files in per_folder.values() {
        for (p, src, _) in files {
            wanted.entry(*src).or_default().push(original.get(&(*src, p.as_str())).map(|s| s.to_string()).unwrap_or_else(|| p.clone()));
        }
    }
    let wanted: Vec<(usize, Vec<String>)> = wanted.into_iter().collect();
    let parsed: HashMap<(usize, String), (Vec<Key>, bool)> = {
        let done = std::sync::atomic::AtomicUsize::new(0);
        let total = wanted.len().max(1);
        let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(4).min(16);
        let chunks: Vec<Vec<&(usize, Vec<String>)>> = (0..threads).map(|t| wanted.iter().skip(t).step_by(threads).collect()).collect();
        std::thread::scope(|s| {
            let handles: Vec<_> = chunks
                .into_iter()
                .map(|chunk| {
                    let (done, roots) = (&done, &roots);
                    s.spawn(move || {
                        let mut out = Vec::new();
                        for (src, rels) in chunk {
                            for (rel, bytes) in read_many(&roots[*src], rels) {
                                let lower = rel.to_lowercase();
                                let ks = match keyed(&lower) {
                                    Some(mode) => keys(&bytes, mode),
                                    None => loc_keys(&bytes),
                                };
                                let bom = *src > 0 && keyed(&lower).is_some() && bom_breaks_first_key(&bytes);
                                out.push(((*src, lower), (ks, bom)));
                            }
                            let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            progress(0.4 + 0.5 * k as f32 / total as f32);
                        }
                        out
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    report.definitions_read = parsed.values().map(|v| v.0.len()).sum();

    let mut ineffective: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut duplicates: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut boms: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut folders: Vec<&String> = per_folder.keys().collect();
    folders.sort();
    for folder in folders {
        let mut files = per_folder[folder].clone();
        // the game reads a folder's files in name order; localisation/replace after the rest, so it wins
        // the game reads a folder's files in file-name order, byte by byte (upper case before lower case: measured); localisation/replace
        // after the rest, so it wins
        let disk_name = |path: &str, src: usize| -> String {
            let orig = original.get(&(src, path)).copied().unwrap_or(path);
            orig.rsplit('/').next().unwrap_or(orig).to_string()
        };
        files.sort_by(|a, b| {
            let ra = a.0.contains("/replace/");
            let rb = b.0.contains("/replace/");
            ra.cmp(&rb).then_with(|| disk_name(&a.0, a.1).as_bytes().cmp(disk_name(&b.0, b.1).as_bytes()))
        });
        let rule = rule_for(folder);
        let mut defs: HashMap<&str, Vec<Def>> = HashMap::new();
        let mut order: Vec<&str> = Vec::new();
        for (path, src, _) in &files {
            let Some((ks, bom)) = parsed.get(&(*src, path.clone())) else { continue };
            if *bom && *src > 0 {
                boms[src - 1].push(original.get(&(*src, path.as_str())).map(|s| s.to_string()).unwrap_or_else(|| path.clone()));
            }
            for k in ks {
                let e = defs.entry(k.name.as_str()).or_insert_with(|| {
                    order.push(k.name.as_str());
                    Vec::new()
                });
                e.push(Def { source: *src, file: original.get(&(*src, path.as_str())).map(|s| s.to_string()).unwrap_or_else(|| path.clone()), line: k.line });
            }
        }
        for key in order {
            let d = &defs[key];
            let sources: HashSet<usize> = d.iter().map(|x| x.source).collect();
            if sources.len() < 2 || !sources.iter().any(|&s| s > 0) {
                continue;
            }
            let winner = match rule {
                Rule::Lios => Some(d.len() - 1),
                Rule::Fios => Some(0),
                _ => None,
            };
            let mods_in: Vec<usize> = sources.iter().copied().filter(|&s| s > 0).collect();
            let with_game = sources.contains(&0);
            // a mod's definition the game's own beats: the override does nothing
            if let Some(w) = winner {
                if d[w].source == 0 {
                    for x in d.iter().filter(|x| x.source > 0) {
                        if rule == Rule::Lios {
                            report.per_mod[x.source - 1].fallbacks += 1;
                            continue;
                        }
                        ineffective[x.source - 1].push(format!("{folder}: {key}"));
                        report.per_mod[x.source - 1].ineffective.push((folder.clone(), key.to_string(), x.file.clone(), d[w].file.clone()));
                    }
                } else if with_game {
                    report.per_mod[d[w].source - 1].overrides_vanilla_keys += 1;
                }
            }
            if rule == Rule::Duplicates {
                for &m in &mods_in {
                    duplicates[m - 1].push(format!("{folder}: {key}"));
                }
            }
            if mods_in.len() < 2 && rule != Rule::Duplicates {
                continue;
            }
            let (severity, intended) = match (rule, winner) {
                (Rule::Duplicates, _) => (Severity::Error, false),
                (Rule::Unknown, _) => (Severity::Info, false),
                (_, Some(w)) => {
                    let ws = d[w].source;
                    let intended = ws > 0 && mods_in.iter().filter(|&&m| m != ws).all(|&m| patches[ws - 1].contains(&(m - 1)));
                    (if intended { Severity::Info } else { Severity::Warning }, intended)
                }
                _ => (Severity::Info, false),
            };
            if let Some(w) = winner {
                let ws = d[w].source;
                if ws > 0 {
                    report.per_mod[ws - 1].wins += 1;
                }
                for &m in mods_in.iter().filter(|&&m| m != ws) {
                    report.per_mod[m - 1].loses += 1;
                }
            }
            report.keys.push(KeyOverlap { folder: folder.clone(), key: key.to_string(), rule, defs: d.clone(), winner, severity, intended });
        }
    }
    progress(0.95);

    // 5. problems
    for id in &input.missing {
        report.issues.push(Issue { severity: Severity::Error, mod_index: None, kind: IssueKind::MissingMod, args: vec![id.clone()] });
    }
    let installed: HashMap<String, &Mod> = input.installed.iter().map(|m| (name_key(&m.name), m)).collect();
    let mut seen_names: HashMap<String, usize> = HashMap::new();
    let mut seen_remote: HashMap<String, usize> = HashMap::new();
    for (i, m) in input.mods.iter().enumerate() {
        let push = |r: &mut Report, severity, kind, args: Vec<String>| r.issues.push(Issue { severity, mod_index: Some(i), kind, args });
        if let Some(p) = &m.problem {
            push(&mut report, Severity::Error, IssueKind::Unloadable, vec![p.clone()]);
        }
        for d in &m.dependencies {
            let k = name_key(d);
            if k == "stellaris" {
                continue;
            }
            match by_name.get(&k) {
                Some(&j) if j > i => push(&mut report, Severity::Warning, IssueKind::DependencyAfter, vec![input.mods[j].name.clone()]),
                Some(_) => {}
                None if installed.contains_key(&k) => push(&mut report, Severity::Warning, IssueKind::DisabledDependency, vec![d.clone()]),
                None => push(&mut report, Severity::Error, IssueKind::MissingDependency, vec![d.clone()]),
            }
        }
        if let Some(&j) = seen_names.get(&name_key(&m.name)) {
            push(&mut report, Severity::Warning, IssueKind::Duplicate, vec![input.mods[j].name.clone()]);
        } else if let Some(&j) = m.remote_file_id.as_ref().and_then(|r| seen_remote.get(r)) {
            push(&mut report, Severity::Warning, IssueKind::Duplicate, vec![input.mods[j].name.clone()]);
        }
        seen_names.entry(name_key(&m.name)).or_insert(i);
        if let Some(r) = &m.remote_file_id {
            seen_remote.entry(r.clone()).or_insert(i);
        }
        let replaced = report.per_mod[i].replaces_vanilla_files;
        if let Some(sv) = &m.supported_version {
            if replaced > 0 && !input.game_version.is_empty() && !mods::supports(sv, input.game_version) && !same_minor(sv, input.game_version) {
                push(&mut report, Severity::Warning, IssueKind::OutdatedReplacesVanilla, vec![sv.clone(), replaced.to_string()]);
            }
        }
        if !boms[i].is_empty() {
            push(&mut report, Severity::Warning, IssueKind::BomFirstKey, vec![boms[i].len().to_string(), boms[i][0].clone()]);
        }
        if !ineffective[i].is_empty() {
            push(&mut report, Severity::Warning, IssueKind::IneffectiveOverride, vec![ineffective[i].len().to_string(), ineffective[i][0].clone()]);
        }
        if !duplicates[i].is_empty() {
            push(&mut report, Severity::Error, IssueKind::DuplicateDefinitions, vec![duplicates[i].len().to_string(), duplicates[i][0].clone()]);
        }
        for &t in &report.per_mod[i].patches.clone() {
            if t > i {
                push(&mut report, Severity::Error, IssueKind::PatchBeforeTarget, vec![input.mods[t].name.clone()]);
            }
        }
    }
    report.issues.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.mod_index.cmp(&b.mod_index)));
    report.keys.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.folder.cmp(&b.folder)).then_with(|| a.key.cmp(&b.key)));
    report.millis = started.elapsed().as_millis();
    progress(1.0);
    report
}

// ------------------------------------------------------------------ the load order

#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// the second needs the first
    Dependency,
    /// the second overrides files the first brings
    Patch,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SortPlan {
    /// positions in the current order, in the suggested order
    pub order: Vec<usize>,
    /// (must come first, must come after, why)
    pub edges: Vec<(usize, usize, Reason)>,
    /// mods in a loop of requirements, left where they were
    pub cycle: Vec<usize>,
}

impl SortPlan {
    pub fn changes(&self) -> bool {
        self.order.iter().enumerate().any(|(i, &x)| i != x)
    }
}

/// The current order changed as little as possible so that every mod comes after the mods it needs and the mods it patches.
pub fn suggest_order(mods: &[&Mod], report: &Report) -> SortPlan {
    let n = mods.len();
    let by_name: HashMap<String, usize> = mods.iter().enumerate().map(|(i, m)| (name_key(&m.name), i)).collect();
    let mut edges: Vec<(usize, usize, Reason)> = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        for d in &m.dependencies {
            if let Some(&j) = by_name.get(&name_key(d)) {
                if j != i {
                    edges.push((j, i, Reason::Dependency));
                }
            }
        }
        if let Some(s) = report.per_mod.get(i) {
            for &t in &s.patches {
                if !edges.iter().any(|e| e.0 == t && e.1 == i) {
                    edges.push((t, i, Reason::Patch));
                }
            }
        }
    }
    let mut indeg = vec![0usize; n];
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, b, _) in &edges {
        indeg[*b] += 1;
        after[*a].push(*b);
    }
    // Kahn's algorithm, always taking the earliest mod (in the current order) that is free: the order moves only where it must
    let mut ready: BinaryHeap<std::cmp::Reverse<usize>> = (0..n).filter(|&i| indeg[i] == 0).map(std::cmp::Reverse).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(std::cmp::Reverse(i)) = ready.pop() {
        order.push(i);
        for &b in &after[i] {
            indeg[b] -= 1;
            if indeg[b] == 0 {
                ready.push(std::cmp::Reverse(b));
            }
        }
    }
    let placed: HashSet<usize> = order.iter().copied().collect();
    let cycle: Vec<usize> = (0..n).filter(|i| !placed.contains(i)).collect();
    order.extend(cycle.iter().copied());
    SortPlan { order, edges, cycle }
}

/// Puts a playset's enabled mods in a new order (ids in that order); switched-off mods keep their places.
pub fn apply_order(playset: &mut crate::store::Playset, ids: &[String]) {
    let slots: Vec<usize> = playset.mods.iter().enumerate().filter(|(_, m)| m.enabled && ids.contains(&m.id)).map(|(i, _)| i).collect();
    let old = playset.mods.clone();
    let mut next = ids.iter().filter(|id| old.iter().any(|m| m.enabled && &m.id == *id));
    for slot in slots {
        if let Some(id) = next.next() {
            if let Some(m) = old.iter().find(|m| &m.id == id) {
                playset.mods[slot] = m.clone();
            }
        }
    }
}

/// One line of English for a problem (the window has its own words for each).
pub fn describe(issue: &Issue) -> String {
    let a = |i: usize| issue.args.get(i).cloned().unwrap_or_default();
    match issue.kind {
        IssueKind::MissingMod => format!("the playset lists {} but it is not installed (unsubscribed?)", a(0)),
        IssueKind::Unloadable => format!("cannot be loaded: {}", a(0)),
        IssueKind::MissingDependency => format!("needs \"{}\", which is not installed", a(0)),
        IssueKind::DisabledDependency => format!("needs \"{}\", which is not on in this playset", a(0)),
        IssueKind::DependencyAfter => format!("needs \"{}\", which is loaded after it", a(0)),
        IssueKind::Duplicate => format!("is on twice (also as \"{}\")", a(0)),
        IssueKind::OutdatedReplacesVanilla => format!("made for {} and replaces {} game files whole: they may be out of date", a(0), a(1)),
        IssueKind::BomFirstKey => format!("{} file(s) start with a byte-order mark that renames their first definition, e.g. {}", a(0), a(1)),
        IssueKind::IneffectiveOverride => format!("{} definition(s) do nothing: the game's own file is read first in a folder where the first one wins, e.g. {}", a(0), a(1)),
        IssueKind::DuplicateDefinitions => format!("{} definition(s) exist twice in a folder that keeps both, e.g. {}", a(0), a(1)),
        IssueKind::PatchBeforeTarget => format!("patches \"{}\" but is loaded before it, so its files lose", a(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorders_only_the_enabled_mods() {
        use crate::store::{Playset, PlaysetMod};
        let m = |id: &str, enabled| PlaysetMod { id: id.into(), enabled };
        let mut p = Playset { id: "p".into(), name: "p".into(), mods: vec![m("a", true), m("x", false), m("b", true), m("c", true)], plugins: vec![], disabled_dlcs: None };
        apply_order(&mut p, &["c".into(), "a".into(), "b".into()]);
        assert_eq!(p.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["c", "x", "a", "b"]);
    }

    #[test]
    fn rules_by_folder() {
        assert_eq!(rule_for("common/technology"), Rule::Lios);
        assert_eq!(rule_for("common/technology/category"), Rule::Lios);
        assert_eq!(rule_for("common/governments/authorities"), Rule::Fios);
        assert_eq!(rule_for("events"), Rule::Fios);
        assert_eq!(rule_for("common/traits"), Rule::Duplicates);
        assert_eq!(rule_for("common/on_actions"), Rule::Merge);
        assert_eq!(rule_for("common/strategic_resources"), Rule::Lios);
        assert_eq!(rule_for("common/something_new"), Rule::Unknown);
        assert_eq!(rule_for("localisation/english"), Rule::Unknown);
        assert_eq!(rule_for("common/agreement_term_values"), Rule::Lios);
        assert_eq!(rule_for("common/component_templates"), Rule::Fios);
        assert_eq!(rule_for("common/scripted_loc"), Rule::Unknown);
        assert!(same_minor("v4.5.1", "4.5.2"));
        assert!(!same_minor("v4.4.*", "4.5.2"));
    }

    #[test]
    fn reads_definitions() {
        let t = b"\xEF\xBB\xBF# comment\n@cost = 5\nnamespace = x\ntech_a = {\n\tcost = @cost\n\tpotential = { has_x = yes }\n}\n\"tech_b\" = { a = \"}\" }\nvalue = 3\ntech_c={}\n";
        let k: Vec<String> = keys(t, Mode::Blocks).into_iter().map(|k| k.name).collect();
        assert_eq!(k, vec!["tech_a", "tech_b", "tech_c"]);
        assert_eq!(keys(t, Mode::Blocks)[0].line, 4);
        let g: Vec<String> = keys(b"ship_section_template = { key = \"a\" x = { key = no } }\nship_section_template = { key = \"b\" }\nship_section_template = { }\n", Mode::Blocks).into_iter().map(|k| k.name).collect();
        assert_eq!(g, vec!["ship_section_template:a", "ship_section_template:b"]);
        let v: Vec<String> = keys(b"@a = 1\n@b = { }\nx = { @c = 2 }\n", Mode::Variables).into_iter().map(|k| k.name).collect();
        assert_eq!(v, vec!["@a", "@b"]);
        let e = keys(b"namespace = ns\ncountry_event = {\n\tid = ns.1\n\toption = { id = no }\n}\nevent = { id = \"ns.2\" }\n", Mode::Events);
        assert_eq!(e.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), vec!["ns.1", "ns.2"]);
        let l = loc_keys("\u{feff}l_english:\n KEY_A:0 \"A\"\n # no\n KEY_B: \"B\"\n".as_bytes());
        assert_eq!(l.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), vec!["l_english:KEY_A", "l_english:KEY_B"]);
        assert!(bom_breaks_first_key(b"\xEF\xBB\xBFtech = {}"));
        assert!(!bom_breaks_first_key(b"\xEF\xBB\xBF# x\ntech = {}"));
        assert!(!bom_breaks_first_key(b"tech = {}"));
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn a_mod(dir: &Path, name: &str, extra: &str) -> Mod {
        let path = dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        mods::parse_descriptor(&dir.join(format!("{name}.mod")), &format!("name=\"{name}\"\npath=\"{}\"\nsupported_version=\"v4.5.*\"\n{extra}", path.to_string_lossy().replace('\\', "/")), dir)
    }

    #[test]
    fn finds_what_overrides_what() {
        let base = std::env::temp_dir().join(format!("stl-test-conflicts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let game = base.join("game");
        write(&game, "common/technology/00_tech.txt", "tech_a = { }\ntech_b = { }\n");
        write(&game, "events/00_events.txt", "namespace = v\ncountry_event = { id = v.1 }\n");
        write(&game, "common/traits/00_traits.txt", "trait_x = { }\n");
        write(&game, "events/crisis_events.txt", "namespace = c\ncountry_event = { id = c.1 }\n");
        write(&game, "common/scripted_triggers/02_triggers.txt", "dlc_trigger = { always = yes }\n");
        write(&game, "interface/main.gui", "x");
        let a = a_mod(&base, "A", "");
        write(a.path.as_ref().unwrap(), "common/technology/zzz_a.txt", "tech_a = { }\nmy_tech = { }\n");
        write(a.path.as_ref().unwrap(), "common/technology/a_own.txt", "a_only = { }\n");
        write(a.path.as_ref().unwrap(), "interface/main.gui", "a");
        write(a.path.as_ref().unwrap(), "events/zz_a.txt", "namespace = v\ncountry_event = { id = v.1 }\n");
        write(a.path.as_ref().unwrap(), "common/traits/a_traits.txt", "trait_x = { }\n");
        // FOX… sorts before crisis… byte by byte (upper case first): A's event is read first and wins
        write(a.path.as_ref().unwrap(), "events/FOXCrisis.txt", "namespace = c\ncountry_event = { id = c.1 }\n");
        // a placeholder read before the game's own trigger: a fallback, not a problem
        write(a.path.as_ref().unwrap(), "common/scripted_triggers/!!!ph_triggers.txt", "dlc_trigger = { always = no }\n");
        let b = a_mod(&base, "B", "dependencies = { \"A\" \"Not Here\" }\n");
        write(b.path.as_ref().unwrap(), "common/technology/aaa_b.txt", "my_tech = { }\n");
        write(b.path.as_ref().unwrap(), "common/technology/a_own.txt", "a_only = { }\n");
        write(b.path.as_ref().unwrap(), "interface/main.gui", "b");
        write(b.path.as_ref().unwrap(), "common/technology/bom.txt", "\u{feff}bom_tech = { }\n");
        let installed = vec![a.clone(), b.clone()];
        // B is loaded first although it needs and patches A
        let input = Input { game_dir: &game, game_version: "4.5.2", mods: vec![&b, &a], installed: &installed, missing: vec!["mod/gone.mod".into()] };
        let r = analyze(&input, &|_| {});
        // the same gui file in both: A is loaded last, so A's is read, over B which patches A: not intended
        let gui = r.files.iter().find(|f| f.path == "interface/main.gui").unwrap();
        assert_eq!(gui.sources, vec![0, 1, 2]);
        assert!(!gui.intended);
        // my_tech: zzz_a.txt is read after aaa_b.txt, technology is LIOS: A wins whatever the load order
        let k = r.keys.iter().find(|k| k.key == "my_tech").unwrap();
        assert_eq!(k.defs[k.winner.unwrap()].source, 2);
        // the event A redefines: FIOS, the game's file is read first, so A's does nothing
        assert!(r.issues.iter().any(|i| i.mod_index == Some(1) && i.kind == IssueKind::IneffectiveOverride));
        assert!(!r.per_mod[1].ineffective.iter().any(|x| x.0 == "events" && x.1 == "c.1"), "FOXCrisis.txt is read before crisis_events.txt");
        assert!(!r.per_mod[1].ineffective.iter().any(|x| x.1 == "dlc_trigger"));
        assert_eq!(r.per_mod[1].fallbacks, 1);
        // a trait of the game's name: both kept
        assert!(r.issues.iter().any(|i| i.mod_index == Some(1) && i.kind == IssueKind::DuplicateDefinitions));
        let kinds = |m: usize| r.issues.iter().filter(|i| i.mod_index == Some(m)).map(|i| i.kind.clone()).collect::<Vec<_>>();
        assert!(kinds(0).contains(&IssueKind::DependencyAfter));
        assert!(kinds(0).contains(&IssueKind::MissingDependency));
        assert!(kinds(0).contains(&IssueKind::BomFirstKey));
        assert!(kinds(0).contains(&IssueKind::PatchBeforeTarget));
        assert!(r.issues.iter().any(|i| i.kind == IssueKind::MissingMod));
        assert_eq!(r.per_mod[1].replaces_vanilla_files, 1);
        // sorting puts A first; then the gui file of B (the patch) wins and is intended
        let plan = suggest_order(&[&b, &a], &r);
        assert_eq!(plan.order, vec![1, 0]);
        assert!(plan.changes());
        let input2 = Input { game_dir: &game, game_version: "4.5.2", mods: vec![&a, &b], installed: &installed, missing: vec![] };
        let r2 = analyze(&input2, &|_| {});
        assert!(r2.files.iter().find(|f| f.path == "interface/main.gui").unwrap().intended);
        assert!(!suggest_order(&[&a, &b], &r2).changes());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sorting_keeps_the_order_where_it_can_and_reports_loops() {
        let base = std::env::temp_dir().join(format!("stl-test-sort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let x = a_mod(&base, "X", "dependencies = { \"Y\" }\n");
        let y = a_mod(&base, "Y", "dependencies = { \"X\" }\n");
        let z = a_mod(&base, "Z", "");
        let r = Report { per_mod: vec![ModSummary::default(); 3], ..Default::default() };
        let plan = suggest_order(&[&z, &x, &y], &r);
        assert_eq!(plan.order, vec![0, 1, 2]);
        assert_eq!(plan.cycle, vec![1, 2]);
        let _ = std::fs::remove_dir_all(base);
    }
}
