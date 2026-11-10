//! Copying the official launcher's playsets into ours, and reading the JSON files it shares playsets in.

use crate::mods::Mod;
use crate::official;
use crate::store::{Playset, PlaysetMod, Store};
use crate::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Outcome {
    /// the name we gave it (a second playset of the same name gets a number)
    pub name: String,
    pub mods: usize,
    pub enabled: usize,
    /// listed in the official playset but with no descriptor in the mod folder
    pub missing: usize,
    /// we already had a playset of that name and `replace` was off
    pub skipped: bool,
    pub was_active: bool,
}

/// Imports playsets (all, or the one named `only`). The official launcher allows two playsets with one name (this machine has two "Initial
/// playset"); the later ones are numbered.
pub fn import_official(store: &mut Store, known: &[Mod], sets: &[official::Playset], only: Option<&str>, replace: bool) -> Result<Vec<Outcome>> {
    let mut made_now: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for s in sets.iter().filter(|s| only.map_or(true, |n| s.name.eq_ignore_ascii_case(n))) {
        let mut name = s.name.clone();
        let mut n = 2;
        while made_now.iter().any(|m| m.eq_ignore_ascii_case(&name)) {
            name = format!("{} ({n})", s.name);
            n += 1;
        }
        let existing = store.find(&name);
        let outcome = |skipped: bool, missing: usize| Outcome {
            name: name.clone(),
            mods: s.mods.len(),
            enabled: s.mods.iter().filter(|m| m.enabled).count(),
            missing,
            skipped,
            was_active: s.is_active,
        };
        if existing.is_some() && !replace {
            out.push(outcome(true, 0));
            continue;
        }
        let i = match existing {
            Some(i) => {
                store.playsets[i].mods.clear();
                i
            }
            None => store.add_playset(&name)?,
        };
        made_now.push(name.clone());
        let mut missing = 0;
        for m in &s.mods {
            if !known.iter().any(|k| k.id == m.registry_id) {
                missing += 1;
            }
            store.playsets[i].set_mod(&m.registry_id, m.enabled);
        }
        out.push(outcome(false, missing));
    }
    if out.is_empty() {
        if let Some(n) = only {
            bail!("no playset called {n} in the official launcher");
        }
    }
    Ok(out)
}

/// A mod of a playset file. The official launcher's schema asks for `steamId` or `pdxId`; a pinned `version` of a Paradox mod is not followed.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SharedMod {
    pub display_name: String,
    #[serde(default)]
    pub steam_id: Option<String>,
    #[serde(default)]
    pub pdx_id: Option<String>,
    pub position: f64,
    pub enabled: bool,
    /// a mod that is neither (a playset of stellarismod.com: a local mod or a package the site hosts), by its descriptor id
    #[serde(skip)]
    pub local_id: Option<String>,
}

impl SharedMod {
    /// What the playset lists it as. The official launcher names the descriptor after the item, whichever machine it is on.
    pub fn descriptor(&self) -> String {
        match (&self.steam_id, &self.pdx_id, &self.local_id) {
            (Some(id), _, _) => format!("mod/ugc_{id}.mod"),
            (None, Some(id), _) => format!("mod/pdx_{id}.mod"),
            (None, None, Some(local)) => local.clone(),
            (None, None, None) => "mod/pdx_.mod".to_string(),
        }
    }
}

/// A playset file of the official launcher ("Share playset"), its mods in load order.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Shared {
    pub game: String,
    pub name: String,
    pub mods: Vec<SharedMod>,
}

/// Reads a playset file, with the checks the official launcher makes: the right game, and no mod twice. The ids become file names, so they
/// must be numbers.
pub fn parse_shared(text: &str, game_id: &str) -> Result<Shared> {
    let mut s: Shared = serde_json::from_str(text.trim_start_matches('\u{feff}')).context("this is not a playset file")?;
    if s.game != game_id {
        bail!("this playset is for {}, not {game_id}", s.game);
    }
    s.name = s.name.trim().to_string();
    if s.name.is_empty() {
        bail!("this playset has no name");
    }
    let number = |x: &Option<String>| x.as_deref().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let mut seen = std::collections::HashSet::new();
    for m in &mut s.mods {
        m.steam_id = number(&m.steam_id);
        m.pdx_id = number(&m.pdx_id);
        let Some(id) = m.steam_id.as_ref().or(m.pdx_id.as_ref()) else { bail!("{} has neither a steamId nor a pdxId", m.display_name) };
        if !id.bytes().all(|b| b.is_ascii_digit()) || id.parse::<u64>().is_err() {
            bail!("{} has the id {id}, which is not a number", m.display_name);
        }
        if !seen.insert(m.descriptor()) {
            bail!("{} is in this playset twice", m.display_name);
        }
    }
    s.mods.sort_by(|a, b| a.position.total_cmp(&b.position));
    Ok(s)
}

fn usable(known: &[Mod], id: &str) -> bool {
    known.iter().any(|k| k.id == id && k.problem.is_none())
}

impl Shared {
    /// Makes the playset's mods the file's, in the file's order, with the file's switches.
    pub fn apply(&self, playset: &mut Playset) {
        playset.mods = self.mods.iter().map(|m| PlaysetMod { id: m.descriptor(), enabled: m.enabled }).collect();
    }

    /// The Workshop items of the file that are not usable here.
    pub fn missing_steam(&self, known: &[Mod]) -> Vec<u64> {
        self.mods.iter().filter(|m| !usable(known, &m.descriptor())).filter_map(|m| m.steam_id.as_ref()?.parse().ok()).collect()
    }

    /// The Paradox Mods of the file that are not here. Getting those takes a Paradox account, which this launcher does not sign in to.
    pub fn missing_pdx(&self, known: &[Mod]) -> Vec<&SharedMod> {
        self.mods.iter().filter(|m| m.steam_id.is_none() && m.pdx_id.is_some() && !usable(known, &m.descriptor())).collect()
    }

    /// The mods named by a descriptor id only (`local_id`) that are not here.
    pub fn missing_local(&self, known: &[Mod]) -> Vec<&SharedMod> {
        self.mods.iter().filter(|m| m.local_id.is_some() && m.steam_id.is_none() && !usable(known, &m.descriptor())).collect()
    }

    pub fn name_of(&self, steam_id: u64) -> Option<&str> {
        let id = steam_id.to_string();
        self.mods.iter().find(|m| m.steam_id.as_deref() == Some(&id)).map(|m| m.display_name.as_str())
    }
}

/// A playset as the file the official launcher shares playsets in, and the names of the local mods left out of it: nobody else has those.
pub fn export_shared(playset: &crate::store::Playset, known: &[Mod], game_id: &str) -> (String, Vec<String>) {
    let remote = |id: &str, prefix: &str| -> Option<String> {
        let n = id.strip_prefix(prefix)?.strip_suffix(".mod")?;
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
    };
    let mut mods = Vec::new();
    let mut local = Vec::new();
    for m in &playset.mods {
        let k = known.iter().find(|k| k.id == m.id);
        let steam = remote(&m.id, "mod/ugc_").or_else(|| k.filter(|k| matches!(k.kind, crate::mods::Kind::Workshop)).and_then(|k| k.remote_file_id.clone()));
        let pdx = remote(&m.id, "mod/pdx_");
        let name = k.map(|k| k.name.clone()).or_else(|| steam.clone()).or_else(|| pdx.clone()).unwrap_or_else(|| m.id.clone());
        let mut entry = serde_json::json!({ "displayName": name, "enabled": m.enabled, "position": mods.len() });
        match (steam, pdx) {
            (Some(id), _) => entry["steamId"] = id.into(),
            (None, Some(id)) => entry["pdxId"] = id.into(),
            (None, None) => {
                local.push(name);
                continue;
            }
        }
        mods.push(entry);
    }
    let file = serde_json::json!({ "game": game_id, "name": playset.name, "mods": mods });
    (serde_json::to_string_pretty(&file).unwrap_or_default(), local)
}

/// A new playset from a file, whose index is returned. A second playset of that name gets a number, as in `import_official`.
pub fn add_shared(store: &mut Store, shared: &Shared) -> Result<usize> {
    let mut name = shared.name.clone();
    let mut n = 2;
    while store.find(&name).is_some() {
        name = format!("{} ({n})", shared.name);
        n += 1;
    }
    let i = store.add_playset(&name)?;
    shared.apply(&mut store.playsets[i]);
    Ok(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::official::{Playset, PlaysetMod};

    fn set(name: &str, ids: &[(&str, bool)]) -> Playset {
        Playset {
            id: name.into(),
            name: name.into(),
            is_active: false,
            mods: ids.iter().map(|(i, e)| PlaysetMod { registry_id: i.to_string(), enabled: *e, name: None, status: String::new() }).collect(),
        }
    }

    const FILE: &str = r#"{"game":"stellaris","name":" Shared ","mods":[
        {"displayName":"Third","steamId":"30","position":2,"enabled":true},
        {"displayName":"First","steamId":"10","position":0,"enabled":false,"future":1},
        {"displayName":"Second","pdxId":"20","position":1,"enabled":true,"version":"1.0"}]}"#;

    fn known(id: &str, problem: Option<&str>) -> Mod {
        Mod {
            id: id.into(),
            file: Default::default(),
            name: id.into(),
            version: None,
            supported_version: None,
            path: None,
            archive: None,
            remote_file_id: None,
            picture: None,
            tags: Vec::new(),
            dependencies: Vec::new(),
            replace_paths: Vec::new(),
            kind: crate::mods::Kind::Workshop,
            problem: problem.map(String::from),
        }
    }

    #[test]
    fn a_shared_playset_keeps_the_order_of_its_file() {
        let s = parse_shared(&format!("\u{feff}{FILE}"), "stellaris").unwrap();
        assert_eq!(s.name, "Shared");
        let dir = std::env::temp_dir().join(format!("stl-test-shared-{}", std::process::id()));
        let mut store = Store::load_from(&dir.join("playsets.json")).unwrap();
        let a = add_shared(&mut store, &s).unwrap();
        let b = add_shared(&mut store, &s).unwrap();
        assert_eq!(store.playsets[b].name, "Shared (2)");
        let listed = |p: &crate::store::Playset| p.mods.iter().map(|m| (m.id.clone(), m.enabled)).collect::<Vec<_>>();
        let want = vec![("mod/ugc_10.mod".to_string(), false), ("mod/pdx_20.mod".to_string(), true), ("mod/ugc_30.mod".to_string(), true)];
        assert_eq!(listed(&store.playsets[a]), want);
        // a list changed meanwhile goes back to the file's
        store.playsets[a].mods.reverse();
        s.apply(&mut store.playsets[a]);
        assert_eq!(listed(&store.playsets[a]), want);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_exported_playset_reads_back_without_its_local_mods() {
        let dir = std::env::temp_dir().join(format!("stl-test-export-{}", std::process::id()));
        let mut store = Store::load_from(&dir.join("playsets.json")).unwrap();
        let i = store.add_playset("Mine").unwrap();
        let p = &mut store.playsets[i];
        p.set_mod("mod/ugc_30.mod", true);
        p.set_mod("mod/mine.mod", true);
        p.set_mod("mod/pdx_20.mod", false);
        p.set_mod("mod/ugc_10.mod", true);
        let mut named = known("mod/ugc_30.mod", None);
        named.name = "Thirty".into();
        let mut mine = known("mod/mine.mod", None);
        mine.name = "My mod".into();
        mine.kind = crate::mods::Kind::Local;
        let (text, local) = export_shared(&store.playsets[i], &[named, mine], "stellaris");
        assert_eq!(local, vec!["My mod"]);
        let back = parse_shared(&text, "stellaris").unwrap();
        assert_eq!(back.name, "Mine");
        let got: Vec<_> = back.mods.iter().map(|m| (m.display_name.as_str(), m.descriptor(), m.enabled)).collect();
        let want = vec![("Thirty", "mod/ugc_30.mod".to_string(), true), ("20", "mod/pdx_20.mod".to_string(), false), ("10", "mod/ugc_10.mod".to_string(), true)];
        assert_eq!(got, want);
        // the positions have no gap where the local mod was, and are whole numbers as the official launcher writes them
        assert!(text.contains("\"position\": 2"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_shared_playset_says_what_is_missing() {
        let s = parse_shared(FILE, "stellaris").unwrap();
        // 10 is here; 30 has a descriptor whose folder has gone; 20 is a Paradox mod
        let have = [known("mod/ugc_10.mod", None), known("mod/ugc_30.mod", Some("gone"))];
        assert_eq!(s.missing_steam(&have), vec![30]);
        assert_eq!(s.missing_pdx(&have).iter().map(|m| m.display_name.as_str()).collect::<Vec<_>>(), vec!["Second"]);
        assert_eq!(s.name_of(30), Some("Third"));
    }

    #[test]
    fn a_shared_playset_is_refused_as_the_official_launcher_does() {
        assert!(parse_shared(FILE, "hoi4").is_err(), "another game");
        assert!(parse_shared("[]", "stellaris").is_err(), "not a playset");
        let one = |m: &str| format!(r#"{{"game":"stellaris","name":"x","mods":[{m}]}}"#);
        let bad = |m: &str| parse_shared(&one(m), "stellaris").is_err();
        assert!(bad(r#"{"displayName":"a","position":0,"enabled":true}"#), "no id");
        assert!(bad(r#"{"displayName":"a","steamId":"..\\x","position":0,"enabled":true}"#), "an id that is a path");
        let twice = r#"{"displayName":"a","steamId":"1","position":0,"enabled":true},{"displayName":"b","steamId":"1","position":1,"enabled":true}"#;
        assert!(bad(twice), "twice");
    }

    #[test]
    fn numbers_duplicates_and_keeps_order() {
        let dir = std::env::temp_dir().join(format!("stl-test-import-{}", std::process::id()));
        let mut store = Store::load_from(&dir.join("playsets.json")).unwrap();
        let sets = vec![set("Initial", &[("mod/a.mod", true), ("mod/b.mod", false)]), set("Initial", &[("mod/c.mod", true)])];
        let r = import_official(&mut store, &[], &sets, None, false).unwrap();
        assert_eq!(r.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), vec!["Initial", "Initial (2)"]);
        assert_eq!((r[0].mods, r[0].enabled, r[0].missing), (2, 1, 2), "nothing is known, so everything is 'missing'");
        let i = store.find("initial").unwrap();
        assert_eq!(store.playsets[i].mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["mod/a.mod", "mod/b.mod"]);
        // again: both skipped; with replace both rewritten
        let again = import_official(&mut store, &[], &sets, None, false).unwrap();
        assert!(again.iter().all(|o| o.skipped));
        let replaced = import_official(&mut store, &[], &sets, None, true).unwrap();
        assert!(replaced.iter().all(|o| !o.skipped));
        assert!(import_official(&mut store, &[], &sets, Some("nope"), false).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
