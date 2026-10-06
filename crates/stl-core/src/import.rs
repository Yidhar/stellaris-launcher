//! Copying the official launcher's playsets into ours.

use crate::mods::Mod;
use crate::official;
use crate::store::Store;
use crate::{bail, Result};

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
