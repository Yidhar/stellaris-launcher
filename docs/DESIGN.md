# Design

A launcher for Stellaris that replaces the Paradox Launcher for single-player and for everyone who wants playsets, mods and **DLL plugins** in
one place, started without three programs and a mandatory click. Written in Rust (a small native exe, no runtime), Windows only. What the
official launcher does and where it falls short is in FINDINGS.md and PROBLEMS.md.

## What it is, and is not

- **Is:** playsets (ordered mod lists), the mod folder and the Steam workshop mods in it, DLL plugins per playset, a launch button that writes
  what the game reads, starts `stellaris.exe` the way the official launcher does, and loads the plugins once the game is up.
- **Is not:** an account client (no Paradox login, so no Paradox Mods browsing and no session token for the game), a mod downloader (Steam
  keeps downloading workshop mods), a DLC manager (DLC state and `dlc_signature` stay as they are), a multiplayer matchmaker.
- **Does not touch** the official launcher's database, `userSettings.json` or `session.js`. It only *reads* the database (a copy of it) to import
  playsets. Both launchers can coexist; whichever last wrote `dlc_load.json` wins, and our `launch` writes it every time.

## Pieces

```
crates/stl-core    the library: everything below
crates/stl-cli     `stl`, the command line over it
crates/stl-gui     the window (egui) over it                                  (planned)
```

| Module | Does |
|---|---|
| `paths` | Documents (known-folder API), `%APPDATA%\stellaris-launcher`, the Steam libraries (registry + `libraryfolders.vdf`), the game folder |
| `game` | `launcher-settings.json` of the game: exe, arguments, alternative executables (Cross-Store Multiplayer), data folder, mods compatibility; the exe's PE timestamp |
| `script`, `mods` | Paradox script subset; the `*.mod` descriptors → mods (workshop `ugc_`, Paradox Mods `pdx_`, local), their content folder, problems, `supported_version` |
| `official` | the official database, read from a copy: playsets, their mods in order, enabled flags |
| `store` | our playsets: `playsets.json` (ordered mods with flags, plugins with flags, the active one) |
| `dlcload` | `dlc_load.json`: writes `enabled_mods`, keeps `disabled_dlcs` and any other key, backs the original up once |
| `plugins` | manifests (`stl-plugin.json`), install / link / remove, build compatibility, seed files — see PLUGINS.md |
| `process` | find the game, window and modules, spawn, `CreateRemoteThread` + `LoadLibraryW`, terminate |
| `launch` | the sequence: playset → mods → plugin checks → seed files → spawn → wait for the window → load plugins |

## Decisions

1. **Our own playsets, imported once.** The official database is the source of truth only for the official launcher; reading it live would tie us to
   its schema (36 migrations and counting) and to its locks. `stl import-official` copies what is there into `playsets.json`; after that the two
   drift apart on purpose. A playset names descriptors (`mod/ugc_123.mod`), the same ids `dlc_load.json` uses, so nothing is translated at launch.
2. **`dlc_load.json` is written at every launch**, from the playset, and only the `enabled_mods` part: DLC choice stays whatever it is (it is bound to
   `dlc_signature`, which the game itself rewrites at start). Mods the playset names that the game cannot load (descriptor missing, content
   folder gone) are left out and reported, not silently written.
3. **Start the game directly**: `child_process`-style detached spawn in the game folder with `SteamAppId=281990`, the arguments from the game's
   own `launcher-settings.json` (`-gdpr-compliant`), `--continuelastsave` for continue, stderr to a log. Steam must be running (the game
   talks to it); no account token is passed, which only affects Paradox online features (to be measured, FINDINGS.md open question 2).
4. **Plugins are loaded by injection from outside**, not by files dropped into the game folder: nothing to clean up after a game update or a
   "verify files", nothing to forget. The cost is that the launcher must be the one starting the game (or `stl inject` run by hand). A
   proxy-DLL host mode, for starting from Steam directly, can be added later without changing the manifest.
5. **A plugin states which game builds it is for** (`exe_timestamps`), and the launcher refuses to load it into another. The plugins of this
   project already refuse for themselves; the launcher says why *before* the game is running, and says it in one place for all of them.
6. **Rust, `windows-sys`, `rusqlite` (bundled), `serde`**: one static exe of a few MB. The same crate serves the CLI and the window.

## Roadmap

1. **Done:** the core, the CLI, plugin loading, playsets and import; tests of every module; a live run against the real game.
2. The window: playset picker, mod list with check boxes and drag order, plugins with switches, Play, a log line.
3. Mod conflict report (files two mods both provide, with the load-order winner) — the feature the official launcher hides behind a flag.
4. Release builds in CI, a zip with `stl.exe` and the window.
5. Proxy-DLL host (launch from Steam), per-plugin settings, plugin dependencies and update checks.
