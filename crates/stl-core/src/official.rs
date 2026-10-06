//! The official launcher's playsets, read from its sqlite database (`launcher-v2*.sqlite` in the game data folder). Only ever read, and from a
//! copy: the launcher may be running, and its database is not ours to write (see docs/FINDINGS.md).

use crate::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PlaysetMod {
    /// `mod/ugc_727000451.mod`
    pub registry_id: String,
    pub enabled: bool,
    pub name: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct Playset {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub mods: Vec<PlaysetMod>,
}

/// The newest `launcher-v2*.sqlite` that is not a backup.
pub fn find_database(data_dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(data_dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("launcher-v2") && name.ends_with(".sqlite") && !name.contains("backup") {
            if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                found.push((t, e.path()));
            }
        }
    }
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// Copies the database (and a write-ahead log next to it) to a temporary folder and opens the copy read-only.
fn open_copy(db: &Path) -> Result<(Connection, PathBuf)> {
    let tmp = std::env::temp_dir().join(format!("stl-official-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let name = db.file_name().context("no file name")?;
    let copy = tmp.join(name);
    std::fs::copy(db, &copy).with_context(|| format!("cannot copy {}", db.display()))?;
    for suffix in ["-wal", "-shm"] {
        let mut src = db.as_os_str().to_owned();
        src.push(suffix);
        let src = PathBuf::from(src);
        if src.is_file() {
            let mut dst = copy.as_os_str().to_owned();
            dst.push(suffix);
            let _ = std::fs::copy(&src, PathBuf::from(dst));
        }
    }
    let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY).context("cannot open the copy of the launcher's database")?;
    Ok((conn, tmp))
}

pub fn read_playsets(db: &Path) -> Result<Vec<Playset>> {
    let (conn, tmp) = open_copy(db)?;
    let result = (|| -> Result<Vec<Playset>> {
        let mut playsets = Vec::new();
        let mut stmt = conn.prepare("select id, name, coalesce(isActive, 0) from playsets where coalesce(isRemoved, 0) = 0 order by coalesce(lastUsedAt, ''), name")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0)))?;
        for row in rows {
            let (id, name, is_active) = row?;
            let mut mods = Vec::new();
            let mut q = conn.prepare(
                "select m.gameRegistryId, coalesce(pm.enabled, 1), coalesce(m.displayName, m.name), m.status \
                 from playsets_mods pm join mods m on m.id = pm.modId where pm.playsetId = ?1 order by pm.position",
            )?;
            let mrows = q.query_map([&id], |r| {
                Ok(PlaysetMod { registry_id: r.get::<_, Option<String>>(0)?.unwrap_or_default(), enabled: r.get::<_, i64>(1)? != 0, name: r.get(2)?, status: r.get::<_, Option<String>>(3)?.unwrap_or_default() })
            })?;
            for m in mrows {
                let m = m?;
                if !m.registry_id.is_empty() {
                    mods.push(m);
                }
            }
            playsets.push(Playset { id, name, is_active, mods });
        }
        Ok(playsets)
    })();
    drop(conn);
    let _ = std::fs::remove_dir_all(tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_playsets_in_load_order() {
        let dir = std::env::temp_dir().join(format!("stl-test-official-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("launcher-v2_test.sqlite");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(
                "create table playsets (id text, name text, isActive boolean, isRemoved boolean default 0, lastUsedAt text);
                 create table mods (id text, gameRegistryId text, name text, displayName text, status text);
                 create table playsets_mods (playsetId text, modId text, enabled boolean, position integer);
                 insert into playsets values ('p1', 'Main', 1, 0, null), ('p2', 'Gone', 0, 1, null);
                 insert into mods values ('m1', 'mod/ugc_1.mod', 'One', null, 'ready_to_play'), ('m2', 'mod/ugc_2.mod', 'Two', 'Two shown', 'ready_to_play'), ('m3', null, 'NoRegistry', null, 'x');
                 insert into playsets_mods values ('p1', 'm2', 1, 1), ('p1', 'm1', 0, 0), ('p1', 'm3', 1, 2);",
            )
            .unwrap();
        }
        let sets = read_playsets(&db).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "Main");
        assert!(sets[0].is_active);
        let ids: Vec<_> = sets[0].mods.iter().map(|m| m.registry_id.as_str()).collect();
        assert_eq!(ids, vec!["mod/ugc_1.mod", "mod/ugc_2.mod"]);
        assert!(!sets[0].mods[0].enabled);
        assert_eq!(sets[0].mods[1].name.as_deref(), Some("Two shown"));
        assert_eq!(find_database(&dir), Some(db));
        let _ = std::fs::remove_dir_all(dir);
    }
}
